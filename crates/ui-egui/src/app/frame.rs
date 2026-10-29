// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
use super::*;

impl PdfCraftApp {
    /// Draw the running job's progress card and pass a Cancel click on to the job.
    pub(super) fn show_progress(&mut self, ctx: &egui::Context) {
        if widgets::progress_notice(self, ctx) {
            self.cancel_optimize();
        }
    }

    pub(super) fn sync_theme(&mut self, ctx: &egui::Context) {
        #[cfg_attr(not(target_os = "linux"), expect(unused_mut))]
        let mut system = ctx.system_theme().map(|theme| theme == egui::Theme::Dark);
        #[cfg(target_os = "linux")]
        {
            let active = self.theme_preference == ThemePreference::System;
            if self.desktop_theme_enabled && active && self.desktop_theme.is_none() {
                let ctx = ctx.clone();
                self.desktop_theme = Some(pdfcraft_platform::desktop_theme::DesktopTheme::start(move || ctx.request_repaint()));
            }
            if let Some(desktop) = &self.desktop_theme {
                desktop.set_active(active);
                // The standard portal supplies Linux desktop appearance. No preference
                // must clear an earlier answer rather than retaining stale dark chrome.
                system = desktop.get().map(|appearance| appearance == pdfcraft_platform::desktop_theme::DesktopAppearance::Dark);
            }
        }
        let kind = self.theme_preference.resolve(system, ThemeKind::Light);
        if kind != self.theme {
            self.theme = kind;
            theme::apply(ctx, kind);
        }
    }
}

/// Root-level panel salts whose 0.36 persisted state key changed in 0.37.
///
/// The shared rail and side panes now own their state. The legacy `rail` and
/// `read_right_panel` entries are inert; the former `rail-nav` footer is no longer migrated.
const PANEL_SALTS: [&str; 4] = ["tab_strip", "mode_bar", "rail", "read_right_panel"];

/// Copy persisted 0.36 panel sizes to 0.37's parent-scope key, once, before the first panel.
///
/// 0.36 stored [`egui::PanelState`] under the panel's own `Id::unique(salt)`; 0.37 derives it
/// from the parent `Ui`'s scope plus an `IdSalt`. A missing copy would reset a width the user
/// chose. The old entry is left untouched, so rolling back to 0.36 still finds it.
pub(super) fn migrate_panel_state(ui: &egui::Ui) {
    if ui.ctx().viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    for salt in PANEL_SALTS {
        migrate_panel_state_from(ui, salt);
    }
}

/// Copy one 0.36 panel's state to the key derived from this exact parent `Ui`.
///
/// The parent must be the `Ui` the panel is shown in (for a nested panel, the panel's content
/// `Ui`), because 0.37's key includes that `Ui`'s scope.
pub(crate) fn migrate_panel_state_from(ui: &egui::Ui, salt: &str) {
    let old = egui::Id::unique(salt);
    let new = ui.scope_id().with_salt(egui::IdSalt::new(salt));
    if old == new || egui::PanelState::load(ui.ctx(), new).is_some() {
        return;
    }
    if let Some(state) = egui::PanelState::load(ui.ctx(), old) {
        ui.ctx().data_mut(|data| data.insert_persisted(new, state));
        log::debug!("migrated panel {salt} state {:?}", state.size());
    }
}

impl PdfCraftApp {
    /// The parts of [`eframe::App::raw_input_hook`] that must run before egui's input hooks:
    /// publish this pass's input-target generation to the control plugin, and hand the wheel
    /// input to the reduced-motion reader.
    ///
    /// A test host that drives the app without eframe's raw hook can call this first, so its
    /// frame ordering matches production instead of hiding focus-change bugs.
    #[doc(hidden)]
    pub fn prepare_raw_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // The control plugin's input hook runs next: give it this pass's input generation.
        if let Some(control) = &self.control {
            control.set_epoch(self.input_epoch());
        }
        // Before egui reads this frame's wheel input.
        self.scroll_speed.apply(ctx, raw);
        // The native menu's key equivalents.
        if let Some(native) = self.native_menu.as_mut() {
            native.raw_input(raw);
        }
    }
}

impl PdfCraftApp {
    /// Rulers and guides of the documents with a tab: remembered ones applied when a document
    /// opens, pages and saves followed, closed ones let go of (this fork, OV11).
    pub(crate) fn sync_doc_prefs(&mut self) {
        let views = &self.views;
        let docs = self.session.docs().iter().filter(|d| views.iter().any(|v| v.id == d.id));
        self.doc_prefs.sync(docs, self.units);
    }

    pub(crate) fn finish_render_frame(&mut self, ctx: &egui::Context) {
        self.finish_document_copy(ctx);
        // The Combine grid's thumbnails, whichever tab shows (renders under way still finish).
        self.combine_thumbs_frame(ctx);
        // Retire hidden documents before admitting the visible document's textures, so the
        // three cache limits apply to the application, regardless of how many tabs are open.
        // Side by side: the comparison's old file renders alongside the active one. In Tile View
        // every tile on screen renders, and the active view too (OV7.2).
        let tiled = self.tiled();
        let displayed: Vec<usize> = if tiled {
            let mut shown = self.tile_frame.clone();
            shown.extend(self.active);
            shown.sort_unstable();
            shown.dedup();
            shown
        } else {
            let partner = self.active.and_then(|i| compare_ui::side_by_side(self, i)).map(|(old, _)| old);
            self.active.into_iter().chain(partner).collect()
        };
        let shown: Vec<DocId> = displayed.iter().filter_map(|&i| self.views.get(i).map(|v| v.id)).collect();
        // A hidden tab of a document another tab shows (Duplicate Tab) must leave the shared
        // render pool alone: suspending would clear the shown tab's queue and drain its results.
        let mut requeue: Vec<DocId> = Vec::new();
        for (i, view) in self.views.iter_mut().enumerate() {
            if !displayed.contains(&i)
                && let Some(doc) = self.session.get(view.id)
            {
                if !shown.contains(&view.id) {
                    view.suspend_rendering(&doc.renderer);
                } else if view.release_textures() {
                    // Its requests are still in the pool: the shown tab replaces them with its own.
                    requeue.push(view.id);
                }
            }
        }
        // In Tile View the views on screen share one prefetch allowance between them.
        let share = if tiled { displayed.len().max(1) } else { 1 };
        let mut done: Vec<DocId> = Vec::new();
        for id in shown {
            if done.contains(&id) {
                continue;
            }
            done.push(id);
            let Some(doc) = self.session.get(id) else { continue };
            // A document in several tiles hands its render pool one queue for all of them.
            let mut group: Vec<&mut DocView> =
                self.views.iter_mut().enumerate().filter(|(j, v)| v.id == id && displayed.contains(j)).map(|(_, v)| v).collect();
            for view in group.iter_mut() {
                view.budget_share = share;
                view.requeue |= requeue.contains(&id);
            }
            match group.as_mut_slice() {
                [view] => view.finish_render_frame(ctx, &doc.info, &doc.renderer),
                views => canvas::finish_shared_render_frame(views, ctx, &doc.info, &doc.renderer),
            }
        }
    }
}

#[cfg(test)]
mod panel_migration_tests {
    use super::*;

    fn state(width: f32) -> egui::PanelState {
        egui::PanelState { outer_rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 900.0)) }
    }

    /// 0.36 stored a panel's persisted size under `Id::unique(salt)`. 0.37 derives the key from
    /// the root `Ui`'s scope; the upgrade must carry the chosen width over, not reset it.
    #[test]
    fn upgrade_from_036_panel_memory_keeps_width() {
        let ctx = egui::Context::default();
        let old = egui::Id::unique("rail");
        ctx.data_mut(|d| d.insert_persisted(old, state(287.0)));
        let mut migrated = None;
        let mut new_id = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            migrate_panel_state(ui);
            new_id = Some(ui.scope_id().with_salt(egui::IdSalt::new("rail")));
            migrated = egui::PanelState::load(ui.ctx(), new_id.expect("an id"));
        });
        output.drop_without_applying_deltas();
        let migrated = migrated.expect("the 0.36 panel width migrates to the 0.37 key");
        assert!((migrated.size().x - 287.0).abs() <= 1.0, "width survives: {:?}", migrated.size());
        assert_eq!(new_id.unwrap(), ctx.viewport_id().root_ui_id().with_salt(egui::IdSalt::new("rail")));
        assert!(egui::PanelState::load(&ctx, old).is_some(), "the old key stays for rollback");
    }

    /// A synthetic nested panel is keyed from its own parent `Ui`, not the root.
    /// Migrating it at the root would silently miss its 0.36 state; the fixture uses
    /// legacy rail names but does not model the current shared rail implementation.
    #[test]
    fn nested_panel_state_migrates_from_its_actual_parent_ui() {
        let ctx = egui::Context::default();
        let old = egui::Id::unique("rail-nav");
        ctx.data_mut(|d| d.insert_persisted(old, state(140.0)));
        let mut nested_id = None;
        let mut migrated = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Panel::right("rail").resizable(false).exact_size(48.0).show(ui, |ui| {
                migrate_panel_state_from(ui, "rail-nav");
                // The key egui will use for the nested panel shown in this same Ui.
                let key = ui.scope_id().with_salt(egui::IdSalt::new("rail-nav"));
                nested_id = Some(key);
                migrated = egui::PanelState::load(ui.ctx(), key);
                egui::Panel::bottom("rail-nav").resizable(false).show(ui, |_ui| {});
            });
        });
        output.drop_without_applying_deltas();
        let migrated = migrated.expect("the nested 0.36 state migrates to its parent-scoped key");
        assert_eq!(migrated.size().x, 140.0);
        assert_ne!(nested_id.unwrap(), ctx.viewport_id().root_ui_id().with_salt(egui::IdSalt::new("rail-nav")), "the nested key is not the root key");
    }

    /// The one-time copy must never overwrite state a 0.37 run has already stored.
    #[test]
    fn migration_does_not_replace_a_037_panel_state() {
        let ctx = egui::Context::default();
        let old = egui::Id::unique("rail");
        let new = ctx.viewport_id().root_ui_id().with_salt(egui::IdSalt::new("rail"));
        ctx.data_mut(|d| {
            d.insert_persisted(old, state(287.0));
            d.insert_persisted(new, state(412.0));
        });
        let output = ctx.run_ui(egui::RawInput::default(), |ui| migrate_panel_state(ui));
        output.drop_without_applying_deltas();
        let kept = egui::PanelState::load(&ctx, new).expect("the 0.37 state");
        assert_eq!(kept.size().x, 412.0);
    }
}

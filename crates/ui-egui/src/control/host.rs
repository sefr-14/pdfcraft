// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
use super::{Host, Reply, Value, json};

impl Host for crate::PdfCraftApp {
    /// Cancel any in-progress pointer gesture in every open view.
    fn cancel_pointer_gestures(&mut self) {
        for view in &mut self.win.views {
            view.cancel_pointer_gestures();
        }
    }

    /// The input target: active document, open dialog/prompt, palette and find state.
    ///
    /// Synthetic input admitted for one generation is never replayed into another document or
    /// dialog. Only these identities are hashed, never text or document content.
    fn input_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.capture_epoch().hash(&mut hasher);
        // The focused widget is part of the input target: keys typed for one field must not land
        // in another after focus moves. Only the id value is hashed, never text.
        let focused = self.ctx.as_ref().and_then(|ctx| ctx.memory(|m| m.focused())).map(|id| id.value());
        focused.hash(&mut hasher);
        hasher.finish()
    }

    /// The captured content: active document and open dialog/prompt, never keyboard focus.
    fn capture_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let doc = self.win.active.and_then(|i| self.win.views.get(i)).map(|v| v.id.0);
        doc.hash(&mut hasher);
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}",
            self.dialog,
            self.password_prompt.is_some(),
            self.win.palette_open,
            self.close_request.is_some(),
            self.win.active.and_then(|i| self.win.views.get(i)).and_then(|v| v.find.as_ref().map(|_| true)),
        )
        .hash(&mut hasher);
        hasher.finish()
    }

    /// Control requests wait for the first UI pass that installs fonts; a font-gated early
    /// return cannot consume injected input or issue a screenshot.
    fn accepts_input(&self) -> bool {
        self.fonts_ready
    }

    fn panel(&mut self, reset: bool, params: &Value) -> Reply {
        crate::side_panels::command(self, reset, params)
    }

    fn quit(&mut self, ctx: &egui::Context) -> bool {
        self.request_quit(ctx)
    }

    fn window_count(&self) -> usize {
        crate::PdfCraftApp::window_count(self)
    }

    fn windows(&self) -> Value {
        let unavailable = self.windows_unavailable();
        json!({
            "windows": self.windows_state(),
            // Whether tabs can move to other windows here; when not, why.
            "available": unavailable.is_none(),
            "unavailable_reason": unavailable,
        })
    }

    fn window_action(&mut self, method: &str, p: &Value) -> Reply {
        self.control_window_action(method, p)
    }

    fn window_page_render(&mut self, index: usize, dpi: f64) -> Reply {
        let windows = self.window_uis();
        let count = windows.len();
        let Some((_, ui)) = windows.get(index) else {
            return Err(format!("there is no window {}; there are {count} (1 is the first window)", index.saturating_add(1)));
        };
        let view = ui.active.and_then(|i| ui.views.get(i)).ok_or_else(|| format!("window {} shows no document", index + 1))?;
        let (id, page) = (view.id, view.current);
        let doc = self.session.get(id).ok_or_else(|| format!("window {}'s document is no longer open", index + 1))?;
        let png = pdfcraft_engine::export::Exporter::new(doc).png(page, dpi)?;
        let mut reply = super::screenshot::encoded_png(&png)?;
        reply["window"] = json!(index + 1);
        reply["doc"] = json!(id.0);
        reply["page"] = json!(page + 1);
        reply["dpi"] = json!(dpi);
        // Never to be mistaken for a capture of the window (owner decision Q2).
        reply["source"] = json!("engine-render");
        reply["native_window_capture"] = json!(false);
        reply["note"] = json!(format!(
            "Not a screenshot: the control channel captures window 1 only. This is the engine's render of page {} of the document window {} shows, without the window's interface.",
            page + 1,
            index + 1
        ));
        Ok(reply)
    }

    fn state(&self) -> Value {
        let active = self.win.active.and_then(|i| self.win.views.get(i));
        let docs: Vec<Value> = self
            .win.views
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let d = self.session.get(v.id)?;
                Some(json!({ "tab": i, "doc": v.id.0, "name": d.name, "path": d.path, "pages": d.info.pages.len(), "dirty": d.dirty, "active": self.win.active == Some(i) }))
            })
            .collect();
        #[cfg(not(target_arch = "wasm32"))]
        let pending_opens: Vec<Value> = self
            .pending_opens
            .iter()
            .map(|pending| json!({ "token": pending.token, "name": pending.name, "path": pending.path }))
            .chain(self.deferred_opens.iter().map(|deferred| {
                let name = std::path::Path::new(&deferred.path).file_name().map(|n| n.to_string_lossy().into_owned());
                json!({ "token": null, "name": name, "path": deferred.path })
            }))
            .collect();
        #[cfg(target_arch = "wasm32")]
        let pending_opens: Vec<Value> = Vec::new();
        let mut state = json!({
            "side_panels": crate::side_panels::state(self),
            "documents": docs,
            "pending_opens": pending_opens,
            "active": active.map(|v| json!({
                "doc": v.id.0,
                "page": v.current + 1,
                "zoom_percent": (v.zoom * 100.0).round(),
                "fit": format!("{:?}", v.fit),
                "layout": format!("{:?}", v.layout),
                "continuous": v.continuous(),
                "page_gaps": v.show_page_gaps,
                "organize": v.organize,
                "grid_zoom": v.grid_zoom(),
                "thumbnail_zoom": v.thumbnail_zoom(),
                // Pages picked in the organize grid or the Pages panel (empty: the current page).
                "selected_pages": v.selected.iter().map(|p| p + 1).collect::<Vec<_>>(),
                "auto_scrolling": v.auto_scrolling(),
                "raster_memory": v.raster_memory(),
                "viewport": [v.viewport_rect().min.x, v.viewport_rect().min.y, v.viewport_rect().max.x, v.viewport_rect().max.y],
                "find_open": v.find.is_some(),
                "page_errors": v.page_errors().iter().map(|(p, e)| json!({ "page": p + 1, "error": e })).collect::<Vec<_>>(),
                // Pages cut at the renderer's limits (#623), 1-based.
                "cut_pages": v.cut_pages().iter().map(|p| p + 1).collect::<Vec<_>>(),
                // Where pages are on screen (points), to aim ui.click / ui.drag at page content.
                "pages_on_screen": v.visible_page_rects().iter().map(|(p, r)| json!({ "page": p + 1, "rect": [r.min.x, r.min.y, r.max.x, r.max.y] })).collect::<Vec<_>>(),
                "selected_comment": v.comments.selected.map(|(p, i)| json!({ "page": p + 1, "index": i + 1 })),
                // Every selected comment (⌘-click adds), the one with the handles first.
                "selected_comments": v.comments.selection().iter().map(|(p, i)| json!({ "page": p + 1, "index": i + 1 })).collect::<Vec<_>>(),
                "comment_composer_open": v.comments.composer.is_some(),
            })),
            // The Combine files tab: how its files show, and each grid thumbnail's state.
            "combine": {
                "showing": self.combine_showing(),
                "view": self.combine_view.as_str(),
                "zoom": self.combine_zoom,
                "files": self.combine_draft.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
                "selected": self.combine_selection(),
                "thumbnails": self.combine_thumbnails().iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(),
                "expanded": self.combine_expanded().iter().filter_map(|i| self.combine_draft.get(*i).map(|f| f.name.clone())).collect::<Vec<_>>(),
                "preview": self.combine_preview().map(|(file, page, render)| json!({ "file": file, "page": page + 1, "render": format!("{render:?}") })),
            },
            "quick_tool": match self.win.quick_tool {
                crate::QuickTool::Measure(t) => format!("measure-{}", t.name()),
                crate::QuickTool::Select => "select".to_string(),
                crate::QuickTool::Hand => "hand".to_string(),
                crate::QuickTool::Crop => "crop".to_string(),
                crate::QuickTool::EnhanceCamera => "enhance-camera".to_string(),
                crate::QuickTool::Redact => "redact".to_string(),
                crate::QuickTool::AddText => "add-text".to_string(),
                crate::QuickTool::EditText => "edit-text".to_string(),
                crate::QuickTool::Link => "link".to_string(),
                crate::QuickTool::SignArea { certify: false } => "sign".to_string(),
                crate::QuickTool::SignArea { certify: true } => "certify".to_string(),
                crate::QuickTool::MarqueeZoom => "marquee-zoom".to_string(),
                crate::QuickTool::Snapshot => "snapshot".to_string(),
                crate::QuickTool::ColumnSelect => "column-select".to_string(),
                crate::QuickTool::Stamp(k) => format!("stamp-{}", k.name().trim_start_matches("PC").to_ascii_lowercase()),
                crate::QuickTool::CustomStamp(i) => format!("custom-stamp-{i}"),
                crate::QuickTool::Fill(f) => format!("fill-{}", f.command().trim_start_matches("sign.fill.")),
                crate::QuickTool::Field(f) => format!("field-{}", f.command().trim_start_matches("form.add.")),
                crate::QuickTool::Comment(t) => t.command().trim_start_matches("comment.").to_string(),
            },
            "home": self.win.active.is_none(),
            "mode": format!("{:?}", self.win.mode),
            "default_mode": self.default_mode,
            "hotbar": self.win.hotbar,
            "comment_tool_pinned": self.comment_prefs.pinned,
            // Home's pinned folders, in order, and whether each is folded down to its heading.
            "pinned_folders": self.pinned.folders.iter().map(|f| json!({ "folder": f, "collapsed": self.pinned.collapsed.contains(f) })).collect::<Vec<_>>(),
            "left_panel": if self.win.left_open { json!(format!("{:?}", self.win.left)) } else { Value::Null },
            "tools_side": if self.win.side_panels.tools_on_right() { "right" } else { "left" },
            "rail_side": if self.win.side_panels.rail_on_right() { "right" } else { "left" },
            "right_panel": self.win.right.map(|r| format!("{r:?}")),
            "hidden_interface": self.hidden_interface().to_json(),
            "dialog": self.dialog.map(|d| format!("{d:?}")),
            "palette_open": self.win.palette_open,
            "theme": format!("{:?}", self.theme),
            "theme_preference": self.theme_preference,
            "show_page_edge_shadows": self.show_page_edge_shadows,
            "language": self.language,
            "notice": self.win.toast.as_ref().map(|t| t.0.clone()),
            "progress": self.progress_notice.as_ref().map(|p| json!({ "label": p.label, "fraction": p.fraction })),
            "password_prompt": self.password_prompt.is_some(),
            "close_prompt": self.close_request.is_some(),
        });
        // This fork (OV11): the active PDF's rulers and guides (pages 1-based, user space).
        if let (Some(view), Some(out)) = (active, state.get_mut("active").and_then(Value::as_object_mut))
            && let Some(marks) = self.doc_prefs.state(view.id)
        {
            out.insert("rulers".into(), json!({ "shown": marks.prefs.rulers, "unit": marks.prefs.unit.label() }));
            out.insert("guides_shown".into(), json!(marks.prefs.guides_shown));
            let guides: Vec<Value> = marks
                .pages_with_guides()
                .flat_map(|(page, g)| {
                    let x = g.x.iter().map(move |at| json!({ "page": page + 1, "axis": "x", "at": at }));
                    x.chain(g.y.iter().map(move |at| json!({ "page": page + 1, "axis": "y", "at": at })))
                })
                .collect();
            out.insert("guides".into(), Value::Array(guides));
        }
        state["windows"] = self.windows_state();
        // The last frame's CPU time in milliseconds, every window's passes included.
        state["frame_cpu_ms"] = json!(self.last_frame_cpu.map(|s| (f64::from(s) * 1000.0 * 100.0).round() / 100.0));
        // The window the last file dialog belongs to (1 is the first window), and whether it was
        // parented to that native window (TAB.WINDOW.1). The web build has no native pickers.
        #[cfg(not(target_arch = "wasm32"))]
        let last_window = self.pickers.last_window;
        #[cfg(target_arch = "wasm32")]
        let last_window: Option<(egui::ViewportId, bool)> = None;
        state["last_file_dialog"] = match last_window {
            Some((viewport, parented)) => {
                let window = self
                    .window_uis()
                    .iter()
                    .position(|(key, _)| key.map_or(egui::ViewportId::ROOT, crate::multi_window::WindowId::viewport) == viewport);
                json!({ "window": window.map(|n| n + 1), "parented": parented })
            }
            None => Value::Null,
        };
        state
    }

    fn command(&mut self, id: &str) -> Reply {
        let spec = pdfcraft_engine::commands::command(id).ok_or_else(|| format!("unknown command {id:?} (see ui.commands)"))?;
        if !self.command_enabled(spec) {
            return Err(format!("{id} is disabled right now"));
        }
        self.execute(id);
        Ok(json!({ "ran": id }))
    }

    fn commands(&self) -> Value {
        let mut list: Vec<Value> = pdfcraft_engine::commands::COMMANDS
            .iter()
            .map(|c| json!({ "id": c.id, "label": c.label, "menu": c.menu, "enabled": self.command_enabled(c), "shortcut": c.shortcut.map(|s| s.label(cfg!(target_os = "macos"))) }))
            .collect();
        list.push(json!({"id": "ui.panel", "label": "Side panel", "enabled": true, "params": "panel: tools or side; open: boolean; width: number (clamped to the panel's limits)"}));
        json!({ "commands": list })
    }

    fn set(&mut self, key: &str, value: &str) -> Reply {
        self.set_option(key, value).map(|()| json!({ "set": key, "value": value }))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open(&mut self, path: &str) -> Reply {
        match self.open_path(path) {
            crate::OpenOutcome::Opened => Ok(json!({ "opened": path, "password_prompt": self.password_prompt.is_some() })),
            crate::OpenOutcome::Focused => Ok(json!({ "opened": path, "focused": true })),
            crate::OpenOutcome::Pending => Ok(json!({ "accepted": path, "pending": true })),
            crate::OpenOutcome::Failed => Err(self.win.toast.as_ref().map(|t| t.0.clone()).unwrap_or_else(|| format!("couldn't open {path}"))),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn open(&mut self, _path: &str) -> Reply {
        Err("ui.open needs a file system; on the web, drop the file on the page".into())
    }
}

impl crate::PdfCraftApp {
    /// Every window, the first one first (TAB.WINDOW.1): its tabs and active page, where a
    /// dialog shows, and that only window 1 is captured natively by `ui.screenshot`.
    fn windows_state(&self) -> Value {
        let dialog = self.modal_open();
        let at = self.dialog_window();
        let windows: Vec<Value> = self
            .window_uis()
            .iter()
            .enumerate()
            .map(|(n, (key, ui))| {
                let tabs: Vec<Value> = ui
                    .views
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let doc = self.session.get(v.id);
                        json!({ "tab": i, "doc": v.id.0, "name": doc.map(|d| d.name.clone()), "dirty": doc.map(|d| d.dirty), "active": ui.active == Some(i) })
                    })
                    .collect();
                let active = ui.active.and_then(|i| ui.views.get(i));
                json!({
                    "window": n + 1,
                    "first": key.is_none(),
                    "tabs": tabs,
                    "active_doc": active.map(|v| v.id.0),
                    "active_page": active.map(|v| v.current + 1),
                    "zoom_percent": active.map(|v| (v.zoom * 100.0).round()),
                    "rotation": active.map(|v| v.rotation),
                    "dialog": dialog && *key == at,
                    "screenshot": if n == 0 { "native" } else { "engine-render" },
                })
            })
            .collect();
        Value::Array(windows)
    }
}

impl crate::PdfCraftApp {
    /// `ui.window.new {window?, tab}`, `ui.window.move {window?, tab, to}`, `ui.window.merge`,
    /// `ui.window.close {window}` and `ui.window.focus {window}`: the Window menu's commands for
    /// any window (1 is the first, as `ui.windows` numbers them) and tab (0-based, as `ui.state`
    /// numbers them). Checked here; done between this pass and the next, as the menu's are.
    fn control_window_action(&mut self, method: &str, p: &Value) -> Reply {
        use crate::multi_window::WindowOp;
        let count = self.window_count();
        let window_param = |key: &str, default: Option<u64>| -> Result<usize, String> {
            let n = match p.get(key) {
                None | Some(Value::Null) => default.ok_or_else(|| format!("{method}: missing {key} (a window number from 1 to {count})"))?,
                Some(v) => v.as_u64().ok_or_else(|| format!("{method}: {key} must be a window number from 1 to {count}"))?,
            };
            match usize::try_from(n) {
                Ok(n) if (1..=count).contains(&n) => Ok(n - 1),
                _ => Err(format!("{method}: there is no window {n}; windows are numbered 1 to {count} (ui.windows lists them)")),
            }
        };
        let key_of = |app: &Self, index: usize| app.window_at(index).ok_or_else(|| format!("{method}: that window closed meanwhile"));
        match method {
            "ui.window.new" | "ui.window.move" => {
                let from_index = window_param("window", Some(1))?;
                let from = key_of(self, from_index)?;
                let tabs = self.window_uis().get(from_index).map_or(0, |(_, ui)| ui.views.len());
                let tab = p.get("tab").and_then(Value::as_u64).and_then(|t| usize::try_from(t).ok());
                let tab =
                    tab.ok_or_else(|| format!("{method}: tab must be a tab number of window {} (0 to {})", from_index + 1, tabs.saturating_sub(1)))?;
                if tab >= tabs {
                    return Err(format!("{method}: window {} has no tab {tab}; its tabs are 0 to {}", from_index + 1, tabs.saturating_sub(1)));
                }
                let to = if method == "ui.window.move" { Some(key_of(self, window_param("to", None)?)?) } else { None };
                if let Some(reason) = self.move_unavailable_from(from, tabs, tab, to) {
                    let hint =
                        if self.windows_unavailable().is_some() { " (ui.set key=keep_one_window value=false allows several windows)" } else { "" };
                    return Err(format!("{method}: {reason}{hint}"));
                }
                let view = self.window_uis().get(from_index).and_then(|(_, ui)| ui.views.get(tab)).map(|v| crate::canvas::ViewKey(v.view_key()));
                let view = view.ok_or_else(|| format!("{method}: that tab closed meanwhile"))?;
                match to {
                    None => {
                        let geometry = self.new_window_geometry();
                        self.request_window_op(WindowOp::MoveToNew { from, view, geometry });
                    }
                    Some(to) => self.request_window_op(WindowOp::MoveTo { from, view, to, at: None }),
                }
                Ok(json!({ "requested": method, "window": from_index + 1, "tab": tab }))
            }
            "ui.window.merge" => {
                if count < 2 {
                    return Err(format!("{method}: there is only one window"));
                }
                self.request_window_op(WindowOp::MergeAll);
                Ok(json!({ "requested": method }))
            }
            "ui.window.close" => {
                let index = window_param("window", None)?;
                let key = key_of(self, index)?;
                self.request_window_op(WindowOp::Close(key));
                // A document only this window shows with unsaved work asks first (ui.state's
                // close_prompt); closing the last window quits, asking the same way.
                Ok(json!({ "requested": method, "window": index + 1 }))
            }
            "ui.window.focus" => {
                let index = window_param("window", None)?;
                let key = key_of(self, index)?;
                self.request_window_op(WindowOp::Focus(key));
                Ok(json!({ "requested": method, "window": index + 1 }))
            }
            other => {
                Err(format!("unknown window method {other:?} (ui.window.new, ui.window.move, ui.window.merge, ui.window.close, ui.window.focus)"))
            }
        }
    }
}

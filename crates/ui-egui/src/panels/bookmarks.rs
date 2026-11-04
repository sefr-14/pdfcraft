// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
use super::{DocInfo, Nav, OutlineItem, Rect, Sense, Tokens, icons, pos2, theme, vec2};

/// What the user asked to do with a bookmark (applied after the panel is drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum BmAction {
    /// New "Untitled" bookmark for the current page, after the selected one (⌘B).
    New,
    StartRename(Vec<usize>),
    Rename(Vec<usize>, String),
    SetToCurrentPage(Vec<usize>),
    /// Make the title bold or italic or not (`None` keeps that one).
    SetStyle(Vec<usize>, Option<bool>, Option<bool>),
    /// Colour the title; black is the default.
    SetColor(Vec<usize>, [f64; 3]),
    Delete(Vec<usize>),
    MoveUp(Vec<usize>),
    MoveDown(Vec<usize>),
    /// Make it the last child of the bookmark above it.
    Indent(Vec<usize>),
    /// Move it out to follow its parent.
    Outdent(Vec<usize>),
    /// This fork (BOOKMARK.1): indent the selected bookmarks together.
    IndentSelected,
    /// This fork (BOOKMARK.1): outdent the selected bookmarks together.
    OutdentSelected,
    /// Bookmarks from the tagged headings, under a new first "Untitled" bookmark.
    FromStructure,
    /// Open New bookmarks from text styles, to choose a style per bookmark level (#1693).
    FromTextStyles,
}

/// This fork (BOOKMARK.1): the bookmarks selected in a view's Bookmarks panel, by path.
///
/// A click selects one bookmark, Ctrl+Click (⌘ on macOS) adds or removes one, and Shift+Click
/// selects the rows shown from the anchor to the one clicked. Paths name rows only while the
/// document stays as it was: the selection carries the document version it was made in
/// ([`crate::content_version`]) and is dropped as soon as that changes (an edit, undo or redo, a
/// save, another tab's change, a reload). A row that stops being shown (its parent folds, a search
/// leaves it out) leaves the selection.
#[derive(Clone, Debug, Default)]
pub struct BookmarkSelection {
    selected: std::collections::BTreeSet<Vec<usize>>,
    anchor: Option<Vec<usize>>,
    /// The rows drawn on the last frame, in order: what a Shift+Click range runs through.
    shown: Vec<Vec<usize>>,
    /// The document version the selection was made in.
    seen: Option<(u64, usize)>,
    /// Bookmarks whose rows open on the next frame (the new parents after a group move).
    reveal: std::collections::HashSet<Vec<usize>>,
}

impl BookmarkSelection {
    /// The selected paths, in outline order (the order of paths is outline order).
    pub fn paths(&self) -> Vec<Vec<usize>> {
        self.selected.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub(crate) fn contains(&self, path: &[usize]) -> bool {
        self.selected.contains(path)
    }

    /// The document version the selection was made in.
    pub(crate) fn seen(&self) -> Option<(u64, usize)> {
        self.seen
    }

    pub(crate) fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.seen = None;
    }

    /// Drop the selection when the document is no longer the version it was made in.
    pub(crate) fn expire(&mut self, now: (u64, usize)) {
        if self.seen.is_some_and(|seen| seen != now) {
            self.clear();
        }
    }

    /// A plain click: only `path`, which becomes the anchor.
    pub(crate) fn only(&mut self, path: &[usize], now: (u64, usize)) {
        self.selected = std::iter::once(path.to_vec()).collect();
        self.anchor = Some(path.to_vec());
        self.seen = Some(now);
    }

    /// Ctrl+Click: add or remove `path`, which becomes the anchor.
    pub(crate) fn toggle(&mut self, path: &[usize], now: (u64, usize)) {
        self.expire(now);
        if !self.selected.remove(path) {
            self.selected.insert(path.to_vec());
        }
        self.anchor = Some(path.to_vec());
        self.seen = Some(now);
    }

    /// Shift+Click: the rows shown from the anchor to `path`, in place of the selection; the
    /// anchor stays. Without a shown anchor it is a plain click's selection.
    pub(crate) fn range_to(&mut self, path: &[usize], now: (u64, usize)) {
        self.expire(now);
        let at = |p: &[usize]| self.shown.iter().position(|s| s.as_slice() == p);
        match (self.anchor.as_deref().and_then(at), at(path)) {
            (Some(a), Some(b)) => {
                let (from, to) = (a.min(b), a.max(b));
                self.selected = self.shown.get(from..=to).map(|rows| rows.iter().cloned().collect()).unwrap_or_default();
                self.seen = Some(now);
            }
            _ => self.only(path, now),
        }
    }

    /// After a group move: select the moved bookmarks at their new `paths`, and open their new
    /// parents on the next frame so they still show.
    pub(crate) fn select_moved(&mut self, paths: Vec<Vec<usize>>, now: (u64, usize)) {
        self.reveal = paths.iter().flat_map(|p| (1..p.len()).filter_map(|n| p.get(..n).map(<[usize]>::to_vec))).collect();
        self.anchor = paths.first().cloned();
        self.selected = paths.into_iter().collect();
        self.seen = Some(now);
    }

    /// Whether the row at `path` opens on this frame.
    fn reveals(&self, path: &[usize]) -> bool {
        self.reveal.contains(path)
    }

    /// The panel drew `shown`, in order: what is not shown leaves the selection, and an anchor no
    /// longer shown becomes its nearest shown ancestor.
    pub(crate) fn finish_frame(&mut self, shown: Vec<Vec<usize>>) {
        let visible: std::collections::HashSet<&[usize]> = shown.iter().map(Vec::as_slice).collect();
        self.selected.retain(|p| visible.contains(p.as_slice()));
        if let Some(anchor) = self.anchor.take() {
            self.anchor = (1..=anchor.len()).rev().filter_map(|n| anchor.get(..n)).find(|p| visible.contains(p)).map(<[usize]>::to_vec);
        }
        if self.selected.is_empty() && self.anchor.is_none() {
            self.seen = None;
        }
        self.reveal.clear();
        self.shown = shown;
    }
}

/// Why a group Indent or Outdent is not offered.
fn shift_refusal(e: &pdfcraft_engine::OutlineError) -> &'static str {
    use pdfcraft_engine::OutlineError as E;
    match e {
        E::CannotIndent(_) => tl!("The first bookmark at its level has nothing above it to go under"),
        E::CannotOutdent(_) => tl!("A top-level bookmark can't move out a level"),
        E::TooDeep(_) => tl!("Too deeply nested to show in the panel"),
        _ => tl!("These bookmarks can't be moved together"),
    }
}

/// Matching titles plus their ancestors, keeping document paths rather than filtered indexes.
/// `query` is folded with [`pdfcraft_render::text::fold_case`], as titles are here.
pub(super) fn outline_matches(items: &[OutlineItem], query: &str) -> std::collections::HashSet<Vec<usize>> {
    let mut matches = std::collections::HashSet::new();
    if query.is_empty() {
        return matches;
    }
    let mut pending: Vec<_> = items.iter().enumerate().map(|(i, item)| (vec![i], item)).collect();
    while let Some((path, item)) = pending.pop() {
        if pdfcraft_render::text::fold_case(&item.title).contains(query) {
            let mut ancestor = path.clone();
            while !ancestor.is_empty() {
                matches.insert(ancestor.clone());
                ancestor.pop();
            }
        }
        for (i, child) in item.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(i);
            pending.push((child_path, child));
        }
    }
    matches
}

pub(super) struct OutlineCtx<'a> {
    pub(super) nav: &'a mut Option<Nav>,
    pub(super) action: &'a mut Option<BmAction>,
    pub(super) rename: &'a mut Option<(Vec<usize>, String)>,
    pub(super) editable: bool,
    pub(super) current: usize,
    /// Expand all (`Some(usize::MAX)`), collapse all (`Some(0)`) or expand to a depth, this frame.
    pub(super) expand: Option<usize>,
    pub(super) matches: &'a std::collections::HashSet<Vec<usize>>,
    pub(super) searching: bool,
    /// This fork (BOOKMARK.1): the view's selection, the rows drawn so far this frame, and the
    /// document version a click selects in.
    pub(super) sel: &'a mut BookmarkSelection,
    pub(super) shown: &'a mut Vec<Vec<usize>>,
    pub(super) stamp: (u64, usize),
    /// Whether the viewer's outline leaves bookmarks out (the group preview can't tell then).
    pub(super) partial: bool,
}

/// The destination page label gets its own right-aligned column in a bookmark row; drawing
/// tools put whole section titles in /PageLabels, so the column is bounded (#124).
const LABEL_COLUMN_MAX: f32 = 72.0;

/// More characters than ever fit [`LABEL_COLUMN_MAX`] at the label size.
const LABEL_MEASURE_CHARS: usize = 64;

/// The longest head of `text` that `fits`, plus an ellipsis when the whole string doesn't.
/// Whole characters are dropped from the end, so the string is only ever cut at a char
/// boundary, and the loop always ends (at the ellipsis alone). A label is document text: only
/// its first [`LABEL_MEASURE_CHARS`] characters are measured, so a huge one can't make each
/// frame lay out thousands of candidates.
pub(crate) fn ellipsized_prefix(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    let mut s: String = text.chars().take(LABEL_MEASURE_CHARS).collect();
    if s.len() == text.len() && fits(text) {
        return text.to_owned();
    }
    loop {
        s.pop();
        let candidate = format!("{s}\u{2026}");
        if s.is_empty() || fits(&candidate) {
            return candidate;
        }
    }
}

/// How a bookmark's title is drawn: in its own style and colour (ISO 32000-2 §12.3.3, Tables 151
/// and 152), bold in the heavier face and italic slanted, its colour kept readable on the panel.
/// Top-level bookmarks without a style keep the medium face.
fn title_format(item: &OutlineItem, depth: usize, t: &Tokens) -> egui::TextFormat {
    let font_id = match (item.bold, depth) {
        (true, _) => theme::semibold(13.0),
        (false, 0) => theme::medium(13.0),
        (false, _) => theme::regular(13.0),
    };
    let color = item.color.map_or(t.text, |rgb| {
        // Each component is within 0–1, so the rounded value fits a byte.
        let [r, g, b] = rgb.map(|c| (c * 255.0).round() as u8);
        theme::readable(egui::Color32::from_rgb(r, g, b), t.panel, t.text)
    });
    egui::TextFormat { font_id, color, italics: item.italic, ..Default::default() }
}

/// Draw one bookmark row with expansion, inline rename and the editable context menu.
pub(super) fn outline_item(
    ui: &mut egui::Ui,
    t: &Tokens,
    info: &DocInfo,
    item: &OutlineItem,
    path: &[usize],
    siblings: usize,
    cx: &mut OutlineCtx<'_>,
) {
    if cx.searching && !cx.matches.contains(path) {
        return;
    }
    let depth = path.len().saturating_sub(1);
    let indent = depth as f32 * 16.0;
    let id = ui.scope_id().with(("outline", path));
    if let Some(levels) = cx.expand {
        ui.data_mut(|d| d.insert_temp(id, depth < levels));
    }
    // This fork (BOOKMARK.1): the new parent of a group just moved opens, so the group still shows.
    if cx.sel.reveals(path) {
        ui.data_mut(|d| d.insert_temp(id, true));
    }
    cx.shown.push(path.to_vec());
    // A bookmark opens as the file says at every level: a positive /Count is open, a negative
    // one (or none) closed (ISO 32000-2 §12.3.3, Table 151). The reader's own toggles and a
    // search override it.
    let mut open = cx.searching || ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(item.open);
    let x_text = indent + 20.0;
    if let Some((rpath, text)) = cx.rename.as_mut()
        && rpath.as_slice() == path
    {
        // Inline rename: Enter commits, Escape cancels.
        let mut commit = None;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add_space(x_text);
            crate::widgets::begin_dialog(ui);
            let edit = crate::widgets::line(text).desired_width(ui.available_width() - 8.0).id(id.with("rename"));
            let resp = ui.add(edit);
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if resp.lost_focus() {
                commit = Some(text.trim().to_string());
            }
        });
        if cancel {
            *cx.rename = None;
        } else if let Some(title) = commit {
            *cx.rename = None;
            if !title.is_empty() && title != item.title {
                *cx.action = Some(BmAction::Rename(path.to_vec(), title));
            }
        }
    } else {
        // The page label sits right-aligned in its own bounded column, and the title wraps to
        // what is really left of it — a long label painted over a long title was the overlap
        // of #124.
        let label_font = theme::regular(11.0);
        let label_galley = item.page.and_then(|p| info.pages.get(p)).map(|page| {
            ui.fonts_mut(|f| {
                let text =
                    ellipsized_prefix(&page.label, |s| f.layout_no_wrap(s.to_owned(), label_font.clone(), t.text_faint).size().x <= LABEL_COLUMN_MAX);
                f.layout_no_wrap(text, label_font, t.text_faint)
            })
        });
        let label_w = label_galley.as_ref().map_or(0.0, |g| g.size().x);
        let wrap_w = (ui.available_width() - indent - 20.0 - label_w - 12.0).max(60.0);
        let mut job = egui::text::LayoutJob::single_section(item.title.clone(), title_format(item, depth, t));
        job.wrap.max_width = wrap_w;
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let h = (galley.size().y + 12.0).max(28.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &item.title));
        theme::Press::track(ui, &resp).wash(ui, rect, 6, cx.sel.contains(path));
        let x0 = rect.left() + indent;
        if !item.children.is_empty() {
            let tri = Rect::from_min_size(pos2(x0, rect.top() + 6.0), vec2(16.0, 16.0));
            icons::paint(ui, tri, if open { "chevron-down" } else { "chevron-right" }, 14.0, t.text_muted);
            if !cx.searching && ui.interact(tri, id.with("tri"), Sense::click()).clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
        }
        ui.painter().galley(pos2(x0 + 20.0, rect.top() + 6.0), galley, t.text);
        if let Some(g) = label_galley {
            // Right-aligned, vertically centred at the first line, inside the column the title
            // wrapped around.
            let pos = pos2(rect.right() - 6.0 - g.size().x, rect.top() + 14.0 - g.size().y / 2.0);
            ui.painter().galley(pos, g, t.text_faint);
        }
        // This fork (BOOKMARK.1): Ctrl+Click and Shift+Click select without going anywhere; a
        // plain click selects the row and goes to its destination, as before.
        if theme::hand(resp.clone()).clicked() {
            let m = ui.input(|i| i.modifiers);
            if m.command {
                cx.sel.toggle(path, cx.stamp);
            } else if m.shift {
                cx.sel.range_to(path, cx.stamp);
            } else {
                cx.sel.only(path, cx.stamp);
                if let Some(p) = item.page {
                    *cx.nav = Some(Nav::Dest(p, item.view));
                }
            }
        }
        // A right-click on a row outside the selection makes it the selection, without going there.
        if resp.secondary_clicked() && !cx.sel.contains(path) {
            cx.sel.only(path, cx.stamp);
        }
        if resp.double_clicked() && cx.editable {
            *cx.action = Some(BmAction::StartRename(path.to_vec()));
        }
        if cx.editable
            && let Some(&i) = path.last()
        {
            let current = cx.current;
            resp.context_menu(|ui| {
                // This fork (BOOKMARK.1): on one of several selected rows, the menu acts on them all.
                if cx.sel.len() >= 2 && cx.sel.contains(path) {
                    let paths = cx.sel.paths();
                    for label in ["Rename", "Style", "Colour"] {
                        ui.add_enabled(false, egui::Button::new(tl!(label)));
                    }
                    ui.add_enabled(
                        false,
                        egui::Button::new(crate::i18n::fmt(tl!("Set to current page ({n})"), &[("n", &(current + 1).to_string())])),
                    );
                    ui.separator();
                    ui.add_enabled(false, egui::Button::new(tl!("Move up")));
                    ui.add_enabled(false, egui::Button::new(tl!("Move down")));
                    for (label, outdent, action) in [("Indent", false, BmAction::IndentSelected), ("Outdent", true, BmAction::OutdentSelected)] {
                        let refusal = if cx.partial { None } else { pdfcraft_engine::preview_shift(&info.outline, &paths, outdent).err() };
                        let button = ui.add_enabled(refusal.is_none(), egui::Button::new(tl!(label)));
                        let button = match &refusal {
                            Some(e) => button.on_disabled_hover_text(shift_refusal(e)),
                            None => button,
                        };
                        if button.clicked() {
                            *cx.action = Some(action);
                            ui.close();
                        }
                    }
                    ui.separator();
                    ui.add_enabled(false, egui::Button::new(tl!("Delete")));
                    return;
                }
                let mut pick = |ui: &mut egui::Ui, label: &str, enabled: bool, a: BmAction| {
                    if ui.add_enabled(enabled, egui::Button::new(tl!(label))).clicked() {
                        *cx.action = Some(a);
                        ui.close();
                    }
                };
                pick(ui, tl!("Rename"), true, BmAction::StartRename(path.to_vec()));
                let set_page = crate::i18n::fmt(tl!("Set to current page ({n})"), &[("n", &(current + 1).to_string())]);
                pick(ui, &set_page, true, BmAction::SetToCurrentPage(path.to_vec()));
                // The title's style and colour (ISO 32000-2 §12.3.3, Tables 151 and 152).
                let mut styled = None;
                ui.menu_button(tl!("Style"), |ui| {
                    let (mut bold, mut italic) = (item.bold, item.italic);
                    if ui.add(crate::widgets::checkbox(&mut bold, tl!("Bold"))).clicked() {
                        styled = Some(BmAction::SetStyle(path.to_vec(), Some(bold), None));
                    }
                    if ui.add(crate::widgets::checkbox(&mut italic, tl!("Italic"))).clicked() {
                        styled = Some(BmAction::SetStyle(path.to_vec(), None, Some(italic)));
                    }
                });
                ui.menu_button(tl!("Colour"), |ui| {
                    let current = item.color.map_or([0.0; 3], |c| c.map(f64::from));
                    if let Some(c) = crate::comments::swatch_grid(ui, Some(current)) {
                        styled = Some(BmAction::SetColor(path.to_vec(), c));
                    }
                });
                if styled.is_some() {
                    ui.close();
                }
                ui.separator();
                pick(ui, tl!("Move up"), i > 0, BmAction::MoveUp(path.to_vec()));
                pick(ui, tl!("Move down"), i + 1 < siblings, BmAction::MoveDown(path.to_vec()));
                pick(ui, tl!("Indent"), i > 0, BmAction::Indent(path.to_vec()));
                pick(ui, tl!("Outdent"), depth > 0, BmAction::Outdent(path.to_vec()));
                ui.separator();
                pick(ui, tl!("Delete"), true, BmAction::Delete(path.to_vec()));
                if let Some(a) = styled {
                    *cx.action = Some(a);
                }
            });
        }
    }
    if open {
        for (ci, c) in item.children.iter().enumerate() {
            let mut p = path.to_vec();
            p.push(ci);
            outline_item(ui, t, info, c, &p, item.children.len(), cx);
        }
    }
}

#[cfg(test)]
mod label_column {
    use super::ellipsized_prefix;

    /// A short page label (the usual "3", "iv", "1-1") is shown unchanged.
    #[test]
    fn short_labels_are_not_touched() {
        for label in ["3", "iv", "1-1", "A-201"] {
            assert_eq!(ellipsized_prefix(label, |s| s.len() <= 10), label);
        }
    }

    /// A label wider than its column (#124: whole section titles in /PageLabels) keeps only
    /// the head that fits, with an ellipsis, and that head is a prefix of the real label.
    #[test]
    fn long_labels_are_cut_to_their_column_with_an_ellipsis() {
        let label = "Floor Plans (001 Floor Plans): GROUND FLOOR PLAN";
        let shown = ellipsized_prefix(label, |s| s.len() <= 20);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 20, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    /// CJK labels (three bytes per character) are cut on character boundaries too.
    #[test]
    fn multibyte_labels_are_cut_on_char_boundaries() {
        let label = "図面（一階平面図）：配置図";
        let shown = ellipsized_prefix(label, |s| s.len() <= 12);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 12, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    /// The pathological floor: nothing fits, the loop still ends with the lone ellipsis.
    #[test]
    fn the_loop_ends_when_nothing_fits() {
        assert_eq!(ellipsized_prefix("Floor Plans", |_| false), "\u{2026}");
    }

    /// A huge label from a hostile file is measured a bounded number of times per frame.
    #[test]
    fn a_huge_label_is_measured_a_bounded_number_of_times() {
        let label = "x".repeat(1_000_000);
        let mut calls = 0;
        let shown = ellipsized_prefix(&label, |s| {
            calls += 1;
            s.len() <= 12
        });
        assert!(calls <= super::LABEL_MEASURE_CHARS + 1, "{calls} measurements");
        assert!(shown.ends_with('\u{2026}') && label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }
}

#[cfg(test)]
mod bookmark_filter {
    use super::{outline_matches, title_format};
    use crate::theme::{self, Tokens};
    use pdfcraft_render::OutlineItem;

    /// #3193: a bookmark's title is drawn in its own style and colour.
    #[test]
    fn bookmark_titles_take_their_style_and_colour() {
        let t = Tokens::for_kind(crate::theme::ThemeKind::Light);
        let plain = item("Plain", Vec::new());
        assert_eq!(title_format(&plain, 0, &t).font_id, theme::medium(13.0));
        assert_eq!(title_format(&plain, 1, &t).font_id, theme::regular(13.0));
        assert_eq!(title_format(&plain, 1, &t).color, t.text);
        assert!(!title_format(&plain, 1, &t).italics);
        let styled = OutlineItem { color: Some([1.0, 0.0, 0.0]), italic: true, bold: true, ..item("Styled", Vec::new()) };
        let f = title_format(&styled, 1, &t);
        assert_eq!(f.font_id, theme::semibold(13.0));
        assert!(f.italics);
        assert_eq!(f.color, egui::Color32::from_rgb(255, 0, 0));
        let half = OutlineItem { color: Some([0.0, 0.5, 0.0]), ..item("Green", Vec::new()) };
        assert_eq!(title_format(&half, 1, &t).color, egui::Color32::from_rgb(0, 128, 0));
    }

    fn item(title: &str, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem {
            title: title.into(),
            page: Some(0),
            view: pdfcraft_render::DestView::Top,
            children,
            open: true,
            color: None,
            italic: false,
            bold: false,
        }
    }

    /// Turkish dotted and dotless i match in any case, as in Find (#892).
    #[test]
    fn titles_match_in_any_case_with_turkish_i() {
        let outline = [item("İSTANBUL ŞUBESİ", vec![item("Kış dönemi", vec![])]), item("Ankara", vec![])];
        let fold = pdfcraft_render::text::fold_case;
        assert_eq!(outline_matches(&outline, &fold("istanbul")).len(), 1);
        assert_eq!(outline_matches(&outline, &fold("şubesi")).len(), 1);
        // A child match keeps its parent: [0] and [0, 0].
        assert_eq!(outline_matches(&outline, &fold("KIŞ")).len(), 2);
        assert!(outline_matches(&outline, &fold("izmir")).is_empty());
    }
}

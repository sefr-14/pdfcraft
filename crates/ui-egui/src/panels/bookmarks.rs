// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
use super::{DocInfo, Nav, OutlineItem, Rect, Sense, Stroke, Tokens, icons, pos2, theme, vec2};

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
    /// Copy the bookmark and everything under it to the bookmark clipboard.
    Copy(Vec<usize>),
    /// Copy it, then delete it (one undo step: only the delete changes the document).
    Cut(Vec<usize>),
    /// Paste the bookmark clipboard under `parent` at `index` (past the end appends).
    Paste {
        parent: Vec<usize>,
        index: usize,
    },
    /// This fork (BOOKMARK.DND.1): a drag was dropped. Move the bookmarks at `paths` together to
    /// child `index` of `parent` (counted in that list as it is; past the end appends), if the
    /// document is still the version (`stamp`) the drag began in.
    MoveTo {
        paths: Vec<Vec<usize>>,
        parent: Vec<usize>,
        index: usize,
        stamp: (u64, usize),
    },
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
    /// BOOKMARK.DND.1: the bookmarks being dragged, if any.
    drag: Option<BookmarkDrag>,
}

/// This fork (BOOKMARK.DND.1): bookmarks being dragged in the panel.
#[derive(Clone, Debug)]
struct BookmarkDrag {
    /// What moves: the selection when the drag began on a selected row, else that row alone.
    paths: Vec<Vec<usize>>,
    /// The document version the drag began in; the paths mean nothing in another.
    stamp: (u64, usize),
    /// The closed bookmark the pointer rests on, and since when (egui time): it opens after
    /// [`HOVER_OPENS_AFTER`].
    hover: Option<(Vec<usize>, f64)>,
}

/// How long a dragged bookmark rests on a closed one before it opens, in seconds.
const HOVER_OPENS_AFTER: f64 = 0.7;

/// The part of a row, from each edge, that means "before" or "after" it; the middle means "into".
const EDGE_BAND: f32 = 0.25;

/// How near the panel's top or bottom edge a drag scrolls it, and by how much a frame.
const DRAG_SCROLL_EDGE: f32 = 40.0;
const DRAG_SCROLL_STEP: f32 = 10.0;

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

    /// BOOKMARK.DND.1: whether bookmarks are being dragged.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// BOOKMARK.DND.1: give up a drag; nothing moves.
    pub(crate) fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// BOOKMARK.DND.1: a drag begins on the row at `path`. A selected row takes the whole
    /// selection with it; any other row goes alone and becomes the selection.
    fn start_drag(&mut self, path: &[usize], now: (u64, usize)) {
        self.expire(now);
        if !self.contains(path) {
            self.only(path, now);
        }
        self.drag = Some(BookmarkDrag { paths: self.paths(), stamp: now, hover: None });
    }

    /// Drop the selection when the document is no longer the version it was made in.
    pub(crate) fn expire(&mut self, now: (u64, usize)) {
        if self.seen.is_some_and(|seen| seen != now) {
            self.clear();
        }
        // A drag's paths are of the version it began in, too.
        if self.drag.as_ref().is_some_and(|d| d.stamp != now) {
            self.drag = None;
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
        E::GroupTooDeep(_) => tl!("Too deeply nested to show in the panel"),
        _ => tl!("These bookmarks can't be moved together"),
    }
}

/// This fork (BOOKMARK.DND.1): why a drag can't be dropped where the pointer is.
fn move_refusal(e: &pdfcraft_engine::OutlineError) -> &'static str {
    use pdfcraft_engine::OutlineError as E;
    match e {
        E::IntoItself => tl!("A bookmark can't go inside itself"),
        E::GroupTooDeep(_) => tl!("Too deeply nested to show in the panel"),
        _ => tl!("These bookmarks can't be moved together"),
    }
}

/// This fork (BOOKMARK.DND.1): a bookmark row as drawn this frame, for a drag to aim at.
pub(super) struct DragRow {
    pub(super) path: Vec<usize>,
    pub(super) rect: Rect,
    /// The id its open state is kept under.
    id: egui::Id,
    /// It has bookmarks under it that are not shown.
    closed: bool,
}

/// Which part of a row the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Band {
    /// The top quarter: before the row, at its level.
    Before,
    /// The middle half: under the row, as its last child.
    Onto,
    /// The bottom quarter: after the row and everything under it, at the row's level.
    After,
}

/// The row a drag at height `y` aims at, and which part of it. `rows` are the rows drawn, top to
/// bottom, each with its path. Above the first row is before it; below the last is after the
/// top-level bookmark it belongs to, so at the end of the outline. A gap between rows belongs to
/// the row above it.
pub(super) fn drop_at(rows: &[(&[usize], Rect)], y: f32) -> Option<(usize, Band)> {
    let (_, first) = rows.first()?;
    let (_, last) = rows.last()?;
    if !y.is_finite() {
        return None;
    }
    if y < first.top() {
        return Some((0, Band::Before));
    }
    if y >= last.bottom() {
        // The last row's top-level ancestor: the last row whose path is one long.
        let top = rows.iter().rposition(|(path, _)| path.len() == 1)?;
        return Some((top, Band::After));
    }
    // The last row that starts at or above `y`.
    let at = rows.iter().rposition(|(_, r)| r.top() <= y)?;
    let (_, rect) = rows.get(at)?;
    let part = if rect.height() > 0.0 { (y - rect.top()) / rect.height() } else { 0.5 };
    let band = if part < EDGE_BAND {
        Band::Before
    } else if part >= 1.0 - EDGE_BAND {
        Band::After
    } else {
        Band::Onto
    };
    Some((at, band))
}

/// Where a drop on `band` of the row at `path` puts bookmarks: the parent, and the position in
/// its list as it is now. After a row is after everything under it, whether or not that shows:
/// the next position at the row's own level.
pub(super) fn drop_target(path: &[usize], band: Band) -> Option<(Vec<usize>, usize)> {
    let (&i, parent) = path.split_last()?;
    Some(match band {
        Band::Before => (parent.to_vec(), i),
        Band::After => (parent.to_vec(), i.saturating_add(1)),
        Band::Onto => (path.to_vec(), usize::MAX),
    })
}

/// The last row drawn under row `at` (itself when nothing under it shows).
fn subtree_end(rows: &[(&[usize], Rect)], at: usize) -> usize {
    let Some((path, _)) = rows.get(at) else { return at };
    let under = rows.iter().skip(at.saturating_add(1)).take_while(|(p, _)| p.len() > path.len() && p.starts_with(path)).count();
    at.saturating_add(under)
}

/// How many of `paths` move on their own: those not under another of them.
fn roots(paths: &[Vec<usize>]) -> usize {
    paths.iter().filter(|p| !paths.iter().any(|q| q.len() < p.len() && p.starts_with(q))).count()
}

/// The x a row at `depth` starts its title at, in a row whose left edge is `left`.
fn title_x(left: f32, depth: usize) -> f32 {
    left + depth as f32 * 16.0 + 20.0
}

/// This fork (BOOKMARK.DND.1): while bookmarks are dragged, show where they would go, open a
/// closed bookmark the pointer rests on, scroll near the panel's edges, and on release ask for the
/// move. `rows` are the rows drawn this frame, top to bottom.
///
/// The top quarter of a row is before it, the bottom quarter after it and everything under it,
/// and the middle under it as its last child. A place the bookmarks can't go, or already are,
/// takes no drop.
pub(super) fn drag_frame(
    ui: &mut egui::Ui,
    t: &Tokens,
    info: &DocInfo,
    sel: &mut BookmarkSelection,
    rows: &[DragRow],
    stamp: (u64, usize),
    action: &mut Option<BmAction>,
) {
    let Some(drag) = sel.drag.as_mut() else { return };
    let (released, down) = ui.input(|i| (i.pointer.primary_released(), i.pointer.primary_down()));
    // The document changed under the drag, or the button was let go somewhere this panel didn't
    // see: there is nothing to drop.
    if drag.stamp != stamp || (!released && !down) {
        sel.drag = None;
        return;
    }
    let clip = ui.clip_rect();
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let spans: Vec<(&[usize], Rect)> = rows.iter().map(|r| (r.path.as_slice(), r.rect)).collect();
    // Only inside the list: a release anywhere else drops nothing.
    let aim = pointer.filter(|p| clip.contains(*p)).and_then(|p| drop_at(&spans, p.y));
    let target = aim.and_then(|(at, band)| {
        let row = rows.get(at)?;
        let (parent, index) = drop_target(&row.path, band)?;
        Some((at, band, parent, index))
    });
    // What the move would do, as the viewer's outline tells it; a window that leaves bookmarks
    // out can't tell, and the move itself decides.
    let verdict = target.as_ref().map(|(_, _, parent, index)| {
        if info.outline_more { Ok(true) } else { pdfcraft_engine::preview_move(&info.outline, &drag.paths, parent, *index).map(|m| m.changed) }
    });
    let mut cursor = egui::CursorIcon::Grabbing;
    if let (Some((at, band, _, _)), Some(verdict), Some(row)) = (target.as_ref(), verdict.as_ref(), target.as_ref().and_then(|x| rows.get(x.0))) {
        let depth = row.path.len().saturating_sub(1);
        let x = title_x(row.rect.left(), depth) - 4.0;
        let stroke = Stroke::new(2.0, t.accent);
        match verdict {
            Ok(true) => match band {
                Band::Onto => {
                    ui.painter().rect_stroke(row.rect.shrink(1.0), 6, stroke, egui::StrokeKind::Inside);
                }
                Band::Before => {
                    drop_line(ui, x, row.rect.right(), row.rect.top(), stroke);
                }
                Band::After => {
                    // After everything shown under the row, at the row's own indent: a line
                    // shorter-indented than the rows above it, joined to its row.
                    let end = rows.get(subtree_end(&spans, *at)).map_or(row.rect, |r| r.rect);
                    if end.bottom() > row.rect.bottom() {
                        ui.painter().line_segment([pos2(x, row.rect.bottom()), pos2(x, end.bottom())], stroke);
                    }
                    drop_line(ui, x, row.rect.right(), end.bottom(), stroke);
                }
            },
            // Already there: nothing to show, nothing to drop.
            Ok(false) => {}
            Err(e) => {
                cursor = egui::CursorIcon::NotAllowed;
                if let Some(p) = pointer {
                    drag_note(ui, t, p + vec2(14.0, 34.0), move_refusal(e), theme::regular(12.0), t.text);
                }
            }
        }
    }
    ui.ctx().set_cursor_icon(cursor);
    if let Some(p) = pointer {
        let n = roots(&drag.paths);
        if n > 1 {
            let what = crate::i18n::fmt(tl!("{n} bookmarks"), &[("n", &n.to_string())]);
            drag_note(ui, t, p + vec2(14.0, 12.0), &what, theme::medium(12.0), t.accent_text);
        }
        if p.y < clip.top() + DRAG_SCROLL_EDGE && clip.x_range().contains(p.x) {
            ui.scroll_with_delta(vec2(0.0, DRAG_SCROLL_STEP));
        } else if p.y > clip.bottom() - DRAG_SCROLL_EDGE && clip.x_range().contains(p.x) {
            ui.scroll_with_delta(vec2(0.0, -DRAG_SCROLL_STEP));
        }
        ui.ctx().request_repaint();
    }
    // A closed bookmark the pointer rests on opens, so the drag can go under what it holds. Only
    // the panel's own fold state: nothing is written to the document.
    let resting = aim.and_then(|(at, _)| rows.get(at)).filter(|r| r.closed && !drag.paths.iter().any(|p| r.path.starts_with(p)));
    let now = ui.input(|i| i.time);
    match (resting, drag.hover.as_ref()) {
        (Some(row), Some((path, since))) if *path == row.path => {
            if now - since >= HOVER_OPENS_AFTER {
                ui.data_mut(|d| d.insert_temp(row.id, true));
                drag.hover = None;
            }
        }
        (Some(row), _) => drag.hover = Some((row.path.clone(), now)),
        (None, _) => drag.hover = None,
    }
    if !released {
        return;
    }
    let paths = std::mem::take(&mut drag.paths);
    sel.drag = None;
    if let (Some((_, _, parent, index)), Some(Ok(true))) = (target, verdict) {
        *action = Some(BmAction::MoveTo { paths, parent, index, stamp });
    }
}

/// A short note beside the pointer during a drag, on its own plate above the rows so it reads
/// whatever it is over.
fn drag_note(ui: &egui::Ui, t: &Tokens, at: egui::Pos2, text: &str, font: egui::FontId, color: egui::Color32) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("bookmark-drag-note")));
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let plate = Rect::from_min_size(at, galley.size()).expand2(vec2(6.0, 3.0));
    painter.rect(plate, 4, t.panel, Stroke::new(1.0, t.divider), egui::StrokeKind::Outside);
    painter.galley(at, galley, color);
}

/// The line a drop before or after a row shows: from `x`, the level the bookmarks would be at,
/// to the row's right edge, with a dot at its start.
fn drop_line(ui: &egui::Ui, x: f32, right: f32, y: f32, stroke: Stroke) {
    ui.painter().line_segment([pos2(x, y), pos2(right - 4.0, y)], stroke);
    ui.painter().circle_filled(pos2(x, y), 3.0, stroke.color);
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
    /// The bookmark clipboard holds something to paste.
    pub(super) can_paste: bool,
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
    /// BOOKMARK.DND.1: the rows drawn so far this frame, for a drag to aim at.
    pub(super) rows: &'a mut Vec<DragRow>,
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

/// The inline rename field (#5529). On the frame it takes focus it selects the whole title, so
/// typing replaces the old text at once; later frames leave the user's own caret and selection
/// alone. Same pattern as the zoom box in `chrome.rs`.
fn rename_field(ui: &mut egui::Ui, text: &mut String, id: egui::Id) -> egui::Response {
    let first_frame = !ui.memory(|m| m.has_focus(id));
    let resp = ui.add(crate::widgets::line(text).desired_width(ui.available_width() - 8.0).id(id));
    if !resp.has_focus() && !resp.lost_focus() {
        if first_frame {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), id);
        }
        resp.request_focus();
    }
    resp
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
            let resp = rename_field(ui, text, id.with("rename"));
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
        // This fork (BOOKMARK.DND.1): a row can be dragged where bookmarks can be edited.
        let draggable = cx.editable && cx.rename.is_none();
        let sense = if draggable { Sense::click_and_drag() } else { Sense::click() };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), sense);
        cx.rows.push(DragRow { path: path.to_vec(), rect, id, closed: !item.children.is_empty() && !open });
        if draggable && cx.searching {
            // A search hides the bookmarks between the ones it shows, so a place among these
            // rows isn't a place in the outline: nothing is dragged, and the row says why.
            if resp.dragged_by(egui::PointerButton::Primary) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
                resp.show_tooltip_text(tl!("Clear the search to rearrange bookmarks"));
            }
        } else if draggable && resp.drag_started_by(egui::PointerButton::Primary) {
            cx.sel.start_drag(path, cx.stamp);
        }
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
                match (item.page, &item.action) {
                    (Some(p), _) => *cx.nav = Some(Nav::Dest(p, item.view)),
                    // A web address, another PDF or a layer change, as a link runs it.
                    (None, Some(action)) => *cx.nav = Some(Nav::Action(action.clone())),
                    (None, None) => {}
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
                    // SYNC.16, SYNC.17 (D-A3): upstream's Cut, Copy and Paste take one bookmark and what is
                    // under it; not offered for several.
                    ui.separator();
                    for label in ["Cut", "Copy", "Paste"] {
                        ui.add_enabled(false, egui::Button::new(tl!(label)))
                            .on_disabled_hover_text(tl!("Cut, Copy and Paste take one bookmark at a time"));
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
                pick(ui, tl!("Cut"), true, BmAction::Cut(path.to_vec()));
                pick(ui, tl!("Copy"), true, BmAction::Copy(path.to_vec()));
                let parent = path.get(..depth).unwrap_or_default().to_vec();
                pick(ui, tl!("Paste"), cx.can_paste, BmAction::Paste { parent, index: i.saturating_add(1) });
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
            action: None,
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

#[cfg(test)]
mod rename_field {
    use super::rename_field as field;

    fn cursor_range(ctx: &egui::Context, id: egui::Id) -> Option<(usize, usize)> {
        egui::TextEdit::load_state(ctx, id)
            .and_then(|s| s.cursor.char_range())
            .map(|r| (r.primary.index.0.min(r.secondary.index.0), r.primary.index.0.max(r.secondary.index.0)))
    }

    /// #5529: the rename field opens with the whole title selected, so typing replaces it.
    #[test]
    fn first_focus_selects_the_whole_title() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("bm-rename-test");
        let mut text = String::from("Untitled");
        for _ in 0..2 {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                field(ui, &mut text, id);
            });
            output.drop_without_applying_deltas();
        }
        assert_eq!(cursor_range(&ctx, id), Some((0, 8)));
        assert!(ctx.memory(|m| m.has_focus(id)), "the field takes focus");
    }

    /// A Unicode title is counted in characters, not bytes.
    #[test]
    fn a_multibyte_title_is_selected_in_characters() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("bm-rename-unicode");
        let mut text = String::from("図面（一階平面図）");
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            field(ui, &mut text, id);
        });
        output.drop_without_applying_deltas();
        assert_eq!(cursor_range(&ctx, id), Some((0, 9)));
    }

    /// A later frame must not reset the selection the user made; a caret stays a caret.
    #[test]
    fn a_later_frame_keeps_the_users_selection() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("bm-rename-keep");
        let mut text = String::from("Chapter");
        for _ in 0..2 {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                field(ui, &mut text, id);
            });
            output.drop_without_applying_deltas();
        }
        // The user moves the caret to 3 with no selection.
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(3))));
        state.store(&ctx, id);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            field(ui, &mut text, id);
        });
        output.drop_without_applying_deltas();
        assert_eq!(cursor_range(&ctx, id), Some((3, 3)), "the user's caret is kept");
    }
}

/// This fork (BOOKMARK.DND.1): where a drag aims.
#[cfg(test)]
mod drop_places {
    use super::{Band, Rect, drop_at, drop_target, pos2, roots, subtree_end};

    /// Rows 30 high from y = 100, with these paths.
    fn rows(paths: &[&'static [usize]]) -> Vec<(&'static [usize], Rect)> {
        paths
            .iter()
            .enumerate()
            .map(|(i, p)| (*p, Rect::from_min_max(pos2(0.0, 100.0 + 30.0 * i as f32), pos2(200.0, 130.0 + 30.0 * i as f32))))
            .collect()
    }

    /// A[a1,a2],B as drawn with A open.
    const OPEN: &[&[usize]] = &[&[0], &[0, 0], &[0, 1], &[1]];

    #[test]
    fn a_row_has_a_quarter_before_a_half_onto_and_a_quarter_after() {
        let r = rows(OPEN);
        // Row 1 (a1) spans 130..160: quarters at 137.5 and 152.5.
        assert_eq!(drop_at(&r, 130.0), Some((1, Band::Before)));
        assert_eq!(drop_at(&r, 137.0), Some((1, Band::Before)));
        assert_eq!(drop_at(&r, 138.0), Some((1, Band::Onto)));
        assert_eq!(drop_at(&r, 152.0), Some((1, Band::Onto)));
        assert_eq!(drop_at(&r, 153.0), Some((1, Band::After)));
        assert_eq!(drop_at(&r, 159.9), Some((1, Band::After)));
        assert_eq!(drop_at(&r, 160.0), Some((2, Band::Before)));
    }

    #[test]
    fn above_the_list_is_before_the_first_row_and_below_it_is_the_end_of_the_outline() {
        let r = rows(OPEN);
        assert_eq!(drop_at(&r, 0.0), Some((0, Band::Before)));
        // Below the last row: after the last top-level bookmark.
        assert_eq!(drop_at(&r, 500.0), Some((3, Band::After)));
        // When the last row is a child, that is still after its top-level bookmark, not after it.
        let nested = rows(&[&[0], &[1], &[1, 0], &[1, 0, 0]]);
        assert_eq!(drop_at(&nested, 500.0), Some((1, Band::After)));
        assert_eq!(drop_at(&[], 120.0), None);
        assert_eq!(drop_at(&r, f32::NAN), None);
    }

    /// A gap between two rows belongs to the row above it, as that row's last part.
    #[test]
    fn a_gap_between_rows_belongs_to_the_row_above() {
        let mut r = rows(&[&[0], &[1]]);
        r[1].1 = r[1].1.translate(egui::vec2(0.0, 6.0));
        assert_eq!(drop_at(&r, 133.0), Some((0, Band::After)));
    }

    #[test]
    fn the_target_is_a_parent_and_a_place_in_its_list_as_it_is() {
        assert_eq!(drop_target(&[2], Band::Before), Some((vec![], 2)));
        assert_eq!(drop_target(&[2], Band::After), Some((vec![], 3)));
        assert_eq!(drop_target(&[2], Band::Onto), Some((vec![2], usize::MAX)));
        assert_eq!(drop_target(&[0, 1], Band::Before), Some((vec![0], 1)));
        // After a bookmark is at its own level whatever is under it.
        assert_eq!(drop_target(&[0], Band::After), Some((vec![], 1)));
        assert_eq!(drop_target(&[], Band::Onto), None);
        assert_eq!(drop_target(&[usize::MAX], Band::After), Some((vec![], usize::MAX)));
    }

    /// The line for "after" goes under everything shown beneath the row.
    #[test]
    fn the_end_of_a_rows_subtree_is_its_last_shown_descendant() {
        let r = rows(&[&[0], &[0, 0], &[0, 0, 0], &[0, 1], &[1], &[1, 0]]);
        assert_eq!(subtree_end(&r, 0), 3);
        assert_eq!(subtree_end(&r, 1), 2);
        assert_eq!(subtree_end(&r, 3), 3);
        assert_eq!(subtree_end(&r, 4), 5);
        assert_eq!(subtree_end(&r, 99), 99);
    }

    #[test]
    fn a_bookmark_under_another_dragged_one_isnt_counted_twice() {
        assert_eq!(roots(&[vec![0], vec![0, 1], vec![2]]), 2);
        assert_eq!(roots(&[vec![1, 0], vec![1, 1]]), 2);
        assert_eq!(roots(&[]), 0);
    }
}

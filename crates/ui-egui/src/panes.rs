// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Split view (`view.split-window`): the main window's page area in two panes, and which pane
//! shows each tab.
//!
//! Every tab is a [`DocView`](crate::DocView) in `PdfCraftApp::win.views`, and its [`Place`] says where
//! it shows. The window's tab strip holds the [`Place::Main`] tabs; split view adds a second pane,
//! to the right or below, with a small strip of its own ([`Place::Second`]). Each pane shows the
//! tab of it that was active last, and `PdfCraftApp::win.active` is the tab of the pane last clicked:
//! menus, keyboard shortcuts and the side panels act on it.
//!
//! One document can show in both panes at once (page 2 beside page 40): two views with one
//! `DocId`. The document, its edits and its undo history are the session's, so an edit made in one
//! view is the other's too (`PdfCraftApp::sync_shared_views`, this fork's one refresh for Duplicate
//! Tab and split view, refreshes the other view), while each view keeps its own scroll, zoom and
//! selection. Closing one of them closes only that view. This fork: a view split view would close
//! (ending split view, or a tab moved to a pane that shows its document) is kept, as a tab of its
//! own, while it holds typing not in the document yet, so nothing typed is lost.
//!
//! Nothing here is kept across restarts: the second pane's tabs are ordinary tabs, so the last
//! session reopens them in the one strip, unsplit.

use egui::{Pos2, Rect, Sense, Stroke, vec2};

use crate::theme::Tokens;
use crate::{CloseRequest, PdfCraftApp};

/// Where a tab is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Place {
    /// The window's tab strip and page area (the left or top pane when split).
    #[default]
    Main,
    /// The second pane of split view (right or bottom).
    Second,
}

/// A tab's place among the panes (`DocView::pane`). This fork (OV7, SYNC.17): a view's key is
/// `DocView::key`, which Duplicate Tab's views have too (`DocView::view_key`), so a pane keeps
/// none of its own.
#[derive(Clone, Debug, Default)]
pub(crate) struct ViewPane {
    /// Where the tab shows.
    pub(crate) place: Place,
    /// Whether `place` was chosen yet (a new tab opens in the pane the user was working in).
    pub(crate) placed: bool,
    /// When this tab was last the active one (each pane shows its latest).
    pub(crate) shown_seq: u64,
    /// The document's render queue was last set for both of its views at once.
    pub(crate) shared: bool,
}

/// The page area in two panes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    /// The second pane is below the first (Split down) rather than beside it (Split right).
    pub down: bool,
    /// The first pane's share of the page area.
    pub fraction: f32,
}

impl Split {
    fn new(down: bool) -> Self {
        Self { down, fraction: 0.5 }
    }
}

/// How far the divider between the panes may go, as a share of the page area.
const FRACTION: std::ops::RangeInclusive<f32> = 0.15..=0.85;
/// The divider's width, and the height of the second pane's tab strip.
const DIVIDER: f32 = 1.0;
const PANE_STRIP: f32 = 30.0;

/// Where a dragged tab goes if it is let go now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TabDrop {
    /// Its own strip: it moves along it (the strip does that).
    Strip,
    /// The other pane.
    Pane(Place),
    /// The right half of a page area that isn't split: split right with it.
    SplitRight,
}

/// Split view's state: see the module docs.
#[derive(Default)]
pub struct Panes {
    /// Split view, while it is on.
    pub split: Option<Split>,
    /// The latest [`ViewPane::shown_seq`] handed out.
    top_seq: u64,
    /// The pane of the tab active last: a new tab opens there.
    focus: Place,
    /// The main tab strip and the page area, last frame.
    pub(crate) main_strip: Option<Rect>,
    pub(crate) page_rect: Option<Rect>,
    /// The pane being drawn now (for what belongs to one pane only).
    pub(crate) drawing: Place,
    /// Where a dragged tab would land, painted over the page area this frame.
    pub(crate) drop_hint: Option<Rect>,
}

impl Panes {
    /// An egui id for something drawn once per pane (the floating tools bar): the main pane keeps
    /// the plain id.
    pub(crate) fn area_id(&self, name: &'static str) -> egui::Id {
        match self.drawing {
            Place::Main => egui::Id::unique(name),
            place => egui::Id::unique((name, place)),
        }
    }
}

/// The two panes and the divider between them, in `area`.
fn layout(area: Rect, split: Split) -> (Rect, Rect, Rect) {
    let f = if split.fraction.is_finite() { split.fraction.clamp(*FRACTION.start(), *FRACTION.end()) } else { 0.5 };
    if split.down {
        let y = (area.top() + area.height() * f).round();
        (
            Rect::from_min_max(area.min, egui::pos2(area.right(), y)),
            Rect::from_min_max(egui::pos2(area.left(), y), egui::pos2(area.right(), y + DIVIDER)),
            Rect::from_min_max(egui::pos2(area.left(), y + DIVIDER), area.max),
        )
    } else {
        let x = (area.left() + area.width() * f).round();
        (
            Rect::from_min_max(area.min, egui::pos2(x, area.bottom())),
            Rect::from_min_max(egui::pos2(x, area.top()), egui::pos2(x + DIVIDER, area.bottom())),
            Rect::from_min_max(egui::pos2(x + DIVIDER, area.top()), area.max),
        )
    }
}

impl PdfCraftApp {
    /// The tabs shown in `place`, in their strip's order (indexes into `views`).
    pub fn tabs_in(&self, place: Place) -> Vec<usize> {
        self.win.views.iter().enumerate().filter(|(_, v)| v.pane.place == place).map(|(i, _)| i).collect()
    }

    /// The tab `place` shows: the active tab when it is there, else the one active there last.
    /// `None` for the main pane while Home (or Combine files) shows.
    pub fn pane_front(&self, place: Place) -> Option<usize> {
        if let Some(a) = self.win.active
            && self.win.views.get(a).is_some_and(|v| v.pane.place == place)
        {
            return Some(a);
        }
        if place == Place::Main && self.win.active.is_none() {
            return None;
        }
        self.latest_in(place)
    }

    /// The tab of `place` active last (the first one when none has been).
    fn latest_in(&self, place: Place) -> Option<usize> {
        self.win
            .views
            .iter()
            .enumerate()
            .filter(|(_, v)| v.pane.place == place)
            .max_by_key(|(i, v)| (v.pane.shown_seq, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
    }

    /// Where tab `index` shows.
    pub fn place_of(&self, index: usize) -> Option<Place> {
        self.win.views.get(index).map(|v| v.pane.place)
    }

    /// Whether another tab shows the same document as tab `index`.
    pub(crate) fn has_twin(&self, index: usize) -> bool {
        let Some(id) = self.win.views.get(index).map(|v| v.id) else { return false };
        self.win.views.iter().enumerate().any(|(k, v)| k != index && v.id == id)
    }

    /// The tab with view key `key` ([`crate::DocView::view_key`]).
    pub(crate) fn tab_with_key(&self, key: u64) -> Option<usize> {
        self.win.views.iter().position(|v| v.view_key() == key)
    }

    /// Bring forward the tab `place` shows (a click in that pane).
    pub fn activate_place(&mut self, place: Place) {
        if self.win.active.and_then(|i| self.win.views.get(i)).is_some_and(|v| v.pane.place == place) {
            return;
        }
        if let Some(i) = self.latest_in(place) {
            self.win.active = Some(i);
            self.combine_tab.focused = false;
        }
    }

    /// Keep the panes consistent, once a frame before anything is drawn and before every change
    /// here: new tabs go to the pane the user was working in, a pane left without tabs ends split
    /// view, the views of a document changed through another view catch up, and the active tab
    /// becomes its pane's latest.
    pub(crate) fn settle_panes(&mut self) {
        let split = self.win.panes.split.is_some();
        let focus = if split { self.win.panes.focus } else { Place::Main };
        for v in &mut self.win.views {
            if !v.pane.placed {
                v.pane.place = focus;
                v.pane.placed = true;
            }
            if v.pane.place == Place::Second && !split {
                v.pane.place = Place::Main;
            }
        }
        if split {
            let has = |p: Place| self.win.views.iter().any(|v| v.pane.place == p);
            if !has(Place::Main) || !has(Place::Second) {
                self.unsplit();
            }
        }
        // Tabs that show a document changed through another of its tabs catch up here, where
        // upstream's split view refreshed them: after `begin_render_frame`, so a view caught up
        // this frame drops the demand it collects for it (#3149). This fork's `sync_shared_views`
        // (Duplicate Tab) keeps each view's place, zoom and history: one refresh for both features.
        self.sync_shared_views();
        // Then a comment selection taken in another version of its document goes (SYNC.16h,
        // SYNC.17): one this view's own edits haven't kept current.
        self.expire_comment_selections();
        if let Some(v) = self.win.active.and_then(|i| self.win.views.get_mut(i)) {
            self.win.panes.focus = v.pane.place;
            if v.pane.shown_seq != self.win.panes.top_seq || self.win.panes.top_seq == 0 {
                self.win.panes.top_seq = self.win.panes.top_seq.saturating_add(1);
                v.pane.shown_seq = self.win.panes.top_seq;
            }
        }
    }

    /// Move tab `from` along its strip: `to` is where it lands among the strip's tabs (`slots`, the
    /// strip's tabs in order, from first to last; `chrome::drop_index`). In split view a strip
    /// holds only some of the tabs, so this goes by the tab it lands next to rather than by index
    /// in `views`.
    pub(crate) fn move_tab_in_strip(&mut self, from: usize, slots: &[(usize, Rect)], to: usize) {
        let others: Vec<usize> = slots.iter().map(|(i, _)| *i).filter(|&i| i != from).collect();
        let (anchor, before) = match others.get(to) {
            Some(&a) => (a, true),
            None => match others.last() {
                Some(&a) => (a, false),
                None => return,
            },
        };
        if from >= self.win.views.len() || slots.iter().position(|(i, _)| *i == from) == Some(to) {
            return;
        }
        let Some(anchor) = self.win.views.get(anchor).map(|v| v.view_key()) else { return };
        let active = self.active_key().map(|k| k.0);
        let view = self.win.views.remove(from);
        let Some(a) = self.tab_with_key(anchor) else {
            self.win.views.insert(from.min(self.win.views.len()), view);
            return;
        };
        let at = if before { a } else { a.saturating_add(1) };
        self.win.views.insert(at.min(self.win.views.len()), view);
        self.win.active = active.and_then(|k| self.tab_with_key(k));
    }

    /// Remove tab `index` but not its document, which another tab still shows. When it was the
    /// active tab, that other tab of the document becomes active.
    fn drop_view(&mut self, index: usize) {
        let Some((key, id)) = self.win.views.get(index).map(|v| (v.view_key(), v.id)) else { return };
        let active = self.active_key().map(|k| k.0);
        self.win.views.remove(index);
        self.win.active =
            if active == Some(key) { self.win.views.iter().position(|v| v.id == id) } else { active.and_then(|k| self.tab_with_key(k)) };
    }

    /// Tab `index`'s right-click Split right (or Split down). The tab the main pane shows gets a
    /// second view in the second pane (the same document, twice); another tab moves there (two
    /// documents side by side). A tab of the second pane only turns the split.
    pub fn split_tab(&mut self, index: usize, down: bool) {
        self.settle_panes();
        let Some(place) = self.place_of(index) else { return };
        match place {
            Place::Second => {
                if let Some(s) = self.win.panes.split.as_mut() {
                    s.down = down;
                }
                self.win.active = Some(index);
            }
            Place::Main => {
                let alone = self.tabs_in(Place::Main).len() <= 1;
                let shown = self.pane_front(Place::Main) == Some(index) || (self.win.active.is_none() && self.latest_in(Place::Main) == Some(index));
                match self.win.panes.split.as_mut() {
                    Some(s) => s.down = down,
                    None => self.win.panes.split = Some(Split::new(down)),
                }
                if shown || alone {
                    self.show_twin(index, Place::Second);
                } else {
                    self.move_tab_to(index, Place::Second);
                }
            }
        }
    }

    /// Show tab `index`'s document in `place` too (a second view of it), or bring forward the
    /// view of it already there.
    fn show_twin(&mut self, index: usize, place: Place) {
        let Some(view) = self.win.views.get(index) else { return };
        let id = view.id;
        if let Some(there) = self.win.views.iter().position(|v| v.id == id && v.pane.place == place) {
            self.win.active = Some(there);
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let mut twin = view.twin(&doc.info);
        // This fork (SYNC.16m, SYNC.17): its comment selection starts in this version of the document.
        twin.comments.stamp(crate::content_version(doc));
        twin.pane.place = place;
        twin.pane.placed = true;
        self.win.views.push(twin);
        self.win.active = Some(self.win.views.len() - 1);
    }

    /// View ▸ Split (⌘\): split the current document into two panes, or end split view.
    pub fn toggle_split(&mut self) {
        self.settle_panes();
        if self.win.panes.split.is_some() {
            self.unsplit();
        } else if let Some(i) = self.win.active {
            self.split_tab(i, false);
        }
    }

    /// End split view: the second pane's tabs join the main strip, except second views of
    /// documents the main pane has open already, which close, unless they hold typing of their own
    /// (this fork): those join the strip too, as a duplicate tab would.
    pub fn unsplit(&mut self) {
        self.win.panes.split = None;
        for i in (0..self.win.views.len()).rev() {
            let Some((id, place)) = self.win.views.get(i).map(|v| (v.id, v.pane.place)) else { continue };
            if place != Place::Second {
                continue;
            }
            if self.win.views.iter().any(|v| v.id == id && v.pane.place == Place::Main) && !self.has_unsaved_typing(i) {
                self.drop_view(i);
            } else if let Some(v) = self.win.views.get_mut(i) {
                v.pane.place = Place::Main;
            }
        }
    }

    /// Move tab `index` to `place` and make it active. A pane that shows its document already
    /// keeps the view it has, and the moving one closes, unless it holds typing of its own (this
    /// fork): then it moves too. Returns the tab's index afterwards.
    pub fn move_tab_to(&mut self, index: usize, place: Place) -> Option<usize> {
        let (id, key, from) = self.win.views.get(index).map(|v| (v.id, v.view_key(), v.pane.place))?;
        if from == place {
            self.win.active = Some(index);
            return Some(index);
        }
        if place == Place::Second && self.win.panes.split.is_none() {
            self.win.panes.split = Some(Split::new(false));
        }
        if let Some(there) = self.win.views.iter().position(|v| v.id == id && v.pane.place == place && v.view_key() != key)
            && !self.has_unsaved_typing(index)
        {
            let keep = self.win.views.get(there).map(|v| v.view_key());
            self.drop_view(index);
            self.win.active = keep.and_then(|k| self.tab_with_key(k));
            return self.win.active;
        }
        if let Some(v) = self.win.views.get_mut(index) {
            v.pane.place = place;
            v.pane.placed = true;
        }
        self.win.active = Some(index);
        self.combine_tab.focused = false;
        Some(index)
    }

    /// Quitting or File ▸ Close all with unsaved documents: one tab per document first, so each
    /// document is asked about once (a tab with typing of its own stays, to be asked about).
    fn one_tab_per_document(&mut self) {
        for i in (0..self.win.views.len()).rev() {
            let Some(id) = self.win.views.get(i).map(|v| v.id) else { continue };
            if self.win.views.iter().take(i).any(|v| v.id == id) && !self.has_unsaved_typing(i) {
                self.drop_view(i);
            }
        }
    }

    /// The tab that comes forward when the active tab closes (it was at `index`, in `place`, and
    /// is gone from `views`): the one that took its place in the same strip, else the strip's last,
    /// else the tab the other pane shows.
    pub(crate) fn after_close(&self, place: Place, index: usize) -> Option<usize> {
        let tabs = self.tabs_in(place);
        tabs.iter().copied().find(|&i| i >= index).or_else(|| tabs.last().copied()).or_else(|| match place {
            Place::Main if self.win.panes.split.is_some() => self.latest_in(Place::Second),
            Place::Main => None,
            Place::Second => self.latest_in(Place::Main),
        })
    }

    /// Where a tab dragged from `from`'s strip lands if let go at `pos`: the other pane, the right
    /// half of a page area that isn't split, or else its own strip (as before split view, the
    /// tab moves along the strip wherever it is let go).
    pub(crate) fn drop_target(&self, pos: Pos2, from: Place) -> TabDrop {
        if from != Place::Main && self.win.panes.main_strip.is_some_and(|r| r.contains(pos)) {
            return TabDrop::Pane(Place::Main);
        }
        let Some(page) = self.win.panes.page_rect.filter(|r| r.contains(pos)) else { return TabDrop::Strip };
        match self.win.panes.split {
            Some(split) => {
                let (first, _, second) = layout(page, split);
                if second.contains(pos) && from != Place::Second {
                    TabDrop::Pane(Place::Second)
                } else if first.contains(pos) && from != Place::Main {
                    TabDrop::Pane(Place::Main)
                } else {
                    TabDrop::Strip
                }
            }
            None if pos.x >= page.center().x => TabDrop::SplitRight,
            None => TabDrop::Strip,
        }
    }

    /// The area a [`TabDrop`] would fill, for the hint drawn while dragging.
    pub(crate) fn drop_hint_rect(&self, drop: TabDrop) -> Option<Rect> {
        let page = self.win.panes.page_rect?;
        match drop {
            TabDrop::Pane(p) => {
                let split = self.win.panes.split?;
                let (first, _, second) = layout(page, split);
                Some(if p == Place::Second { second } else { first })
            }
            TabDrop::SplitRight => Some(Rect::from_min_max(egui::pos2(page.center().x, page.top()), page.max)),
            TabDrop::Strip => None,
        }
    }

    /// Let tab `index` go at `drop`.
    pub(crate) fn drop_tab(&mut self, index: usize, drop: TabDrop) {
        match drop {
            TabDrop::Strip => {}
            TabDrop::Pane(place) => {
                self.move_tab_to(index, place);
            }
            TabDrop::SplitRight => self.split_tab(index, false),
        }
    }

    /// The tabs on screen now: what each pane shows (and a comparison's other half).
    pub(crate) fn views_on_screen(&self) -> Vec<usize> {
        let mut shown = Vec::new();
        let mut add = |i: Option<usize>| {
            if let Some(i) = i
                && !shown.contains(&i)
            {
                shown.push(i);
            }
        };
        let main = if self.win.full_screen { self.win.active } else { self.pane_front(Place::Main) };
        add(main);
        add(main.and_then(|i| crate::compare_ui::side_by_side(self, i)).map(|(old, _)| old));
        if !self.win.full_screen && self.win.panes.split.is_some() {
            add(self.pane_front(Place::Second));
        }
        shown
    }
}

/// Once a frame, before anything is drawn: the panes settle (and a quit or Close all asks about
/// each document once).
pub(crate) fn frame(app: &mut PdfCraftApp) {
    if matches!(app.close_request, Some(CloseRequest::Quit | CloseRequest::All)) {
        app.one_tab_per_document();
    }
    app.settle_panes();
}

/// The page area: the main pane's tab (Home or Combine files while it has none, a comparison's
/// two halves), or split view's two panes.
pub(crate) fn page_area(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    app.win.panes.page_rect = Some(ui.max_rect());
    app.win.panes.drawing = Place::Main;
    if let Some(hint) = app.win.panes.drop_hint.take() {
        let t = Tokens::get(ui.ctx());
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("pane-drop-hint")));
        painter.rect(hint.shrink(4.0), 6.0, t.accent.gamma_multiply(0.12), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    }
    let main = app.pane_front(Place::Main);
    match app.win.panes.split.and_then(|s| Some((s, app.pane_front(Place::Second)?))) {
        Some((split, second)) => split_area(app, split, main, second, ui),
        None => main_pane(app, main, ui),
    }
}

/// What the main pane shows: tab `main`, or Home or Combine files while it has none.
fn main_pane(app: &mut PdfCraftApp, main: Option<usize>, ui: &mut egui::Ui) {
    match main {
        None if app.combine_showing() => crate::combine_ui::page(app, ui),
        None => crate::home::show(app, ui),
        Some(index) => match crate::compare_ui::side_by_side(app, index) {
            Some((old, new)) => crate::compare_ui::compare_area(app, old, new, ui),
            None => crate::canvas::document_area(app, index, ui),
        },
    }
}

/// Whether a press this frame began in `rect` on the page area itself, or on one of the floating
/// bars of the view drawn there (not on a dialog, menu or panel over it).
fn pressed_in(ui: &egui::Ui, rect: Rect, place: Place, view: Option<&crate::DocView>) -> bool {
    let Some(pos) = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None }) else { return false };
    if !rect.contains(pos) {
        return false;
    }
    match ui.ctx().layer_id_at(pos) {
        None => true,
        Some(layer) => {
            let quick = if place == Place::Main { egui::Id::unique("quick-bar") } else { egui::Id::unique(("quick-bar", place)) };
            layer == ui.layer_id()
                || [quick, egui::Id::unique("camera-bar")].contains(&layer.id)
                || view.is_some_and(|v| egui::Id::unique(("find-bar", v.view_key())) == layer.id)
        }
    }
}

/// Split view: the main pane, the divider and the second pane with its strip. A press in a pane
/// makes it the active one before it is drawn, so the click acts there.
fn split_area(app: &mut PdfCraftApp, split: Split, main: Option<usize>, second: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let area = ui.max_rect();
    let (first, divider, other) = layout(area, split);
    if !app.modal_open() && !app.win.palette_open {
        // (While Home shows in the main pane, a click there stays on Home.)
        if main.is_some() && pressed_in(ui, first, Place::Main, main.and_then(|i| app.win.views.get(i))) {
            app.activate_place(Place::Main);
        } else if pressed_in(ui, other, Place::Second, app.win.views.get(second)) {
            app.activate_place(Place::Second);
        }
    }
    // The divider, dragged to share the area differently.
    let grab = if split.down { divider.expand2(vec2(0.0, 3.0)) } else { divider.expand2(vec2(3.0, 0.0)) };
    let response = ui.interact(grab, ui.scope_id().with("pane-divider"), Sense::drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if split.down { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
    }
    if response.dragged()
        && let Some(p) = ui.ctx().pointer_latest_pos()
        && let Some(s) = app.win.panes.split.as_mut()
    {
        let f = if split.down { (p.y - area.top()) / area.height().max(1.0) } else { (p.x - area.left()) / area.width().max(1.0) };
        if f.is_finite() {
            s.fraction = f.clamp(*FRACTION.start(), *FRACTION.end());
        }
    }
    ui.painter().rect_filled(divider, 0.0, if response.dragged() || response.hovered() { t.accent } else { t.divider });
    let active = app.win.active.and_then(|i| app.win.views.get(i)).map(|v| v.pane.place);
    pane(app, ui, first, Place::Main, main, active == Some(Place::Main));
    pane(app, ui, other, Place::Second, Some(second), active == Some(Place::Second));
}

/// One pane of split view: the second has its own tab strip above its page area.
fn pane(app: &mut PdfCraftApp, ui: &mut egui::Ui, rect: Rect, place: Place, front: Option<usize>, active: bool) {
    let t = Tokens::get(ui.ctx());
    let salt = if place == Place::Second { "second-pane" } else { "main-pane" };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(salt));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    let body = if place == Place::Second {
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), PANE_STRIP));
        crate::chrome::pane_tab_strip(app, &mut child, strip, place);
        Rect::from_min_max(egui::pos2(rect.left(), strip.bottom()), rect.max)
    } else {
        rect
    };
    // The strip may have closed or moved a tab just now: look the pane's tab up again (a pane
    // left empty ends split view next frame).
    let front = if place == Place::Second { app.pane_front(place) } else { front.filter(|&i| i < app.win.views.len()) };
    let mut body_ui = child.new_child(egui::UiBuilder::new().max_rect(body).id_salt("body"));
    body_ui.set_clip_rect(body.intersect(ui.clip_rect()));
    app.win.panes.drawing = place;
    match (place, front) {
        (Place::Main, _) => main_pane(app, front, &mut body_ui),
        (Place::Second, Some(index)) => crate::canvas::document_area(app, index, &mut body_ui),
        (Place::Second, None) => {}
    }
    app.win.panes.drawing = Place::Main;
    // The active pane is marked along its top edge.
    if active {
        let edge = Rect::from_min_size(rect.min, vec2(rect.width(), 2.0));
        ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(("active-pane", salt)))).rect_filled(edge, 0.0, t.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panes_share_the_area_and_the_divider_stays_inside() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let (a, d, b) = layout(area, Split { down: false, fraction: 0.5 });
        assert_eq!(a.right(), d.left());
        assert_eq!(d.right(), b.left());
        assert_eq!(b.right(), 1000.0);
        let (a, _, b) = layout(area, Split { down: true, fraction: 2.0 });
        assert!(a.height() <= 600.0 * 0.85 + 1.0 && b.height() > 0.0, "a fraction past the end is held back");
        let (a, _, _) = layout(area, Split { down: false, fraction: f32::NAN });
        assert_eq!(a.width(), 500.0, "a fraction that isn't a number shares evenly");
    }
}

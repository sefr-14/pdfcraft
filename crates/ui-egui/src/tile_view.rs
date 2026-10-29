//! Tab View and Tile View (OV7.2, OV7.3). In Tile View every open view sits in a column of its
//! own, side by side, each navigated on its own. The wheel, zoom and middle-button panning go to
//! the tile under the pointer without a click first; a click in a tile makes it the active one,
//! which tools, editing commands and the keyboard act on.

use egui::{Align2, PointerButton, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::PdfCraftApp;
use crate::canvas::{Interaction, ViewKey};
use crate::theme::{self, Tokens};

/// How the document area shows the open views (the tabs' right-click menu, View ▸ Tab view /
/// Tile view). Tab View at every start: the choice isn't kept across launches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewLayout {
    /// The active view fills the document area; the others wait behind their tabs.
    #[default]
    Tabs,
    /// Every view in a column of its own, side by side.
    Tiles,
}

/// Narrower than this a tile stops being useful: the tiles keep this width and the row scrolls
/// sideways by its scrollbar instead.
pub(crate) const MIN_TILE: f32 = 360.0;
/// The strip above each tile naming its document.
const CAPTION: f32 = 24.0;
/// The line between two tiles.
const GAP: f32 = 1.0;

impl PdfCraftApp {
    /// Whether the document area shows the views as tiles now: Tile View, a document tab
    /// selected (Home and Combine files show as usual), more than one view, not full screen.
    pub fn tiled(&self) -> bool {
        self.layout_mode == ViewLayout::Tiles && self.active.is_some() && self.views.len() > 1 && !self.full_screen
    }

    /// Switch between Tab View and Tile View. Only the arrangement changes: no view is closed,
    /// reloaded or copied into another, and each keeps its own page, zoom and scroll position.
    pub fn set_view_layout(&mut self, layout: ViewLayout) {
        self.layout_mode = layout;
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

/// The width of each of `n` tiles in a row `width` wide, and the row's whole width (wider than
/// `width` when the tiles would be narrower than [`MIN_TILE`]).
pub(crate) fn column_width(width: f32, n: usize) -> (f32, f32) {
    let n = n.max(1) as f32;
    let gaps = GAP * (n - 1.0);
    let tile = ((width - gaps) / n).max(MIN_TILE);
    (tile, tile * n + gaps)
}

/// The document area in Tile View: every view in its own column.
pub(crate) fn area(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let n = app.views.len();
    if n == 0 {
        return;
    }
    // The eased tail of a touchpad swipe over one tile must not scroll another, whichever of
    // them is drawn first.
    let now = ui.input(|i| i.time);
    crate::canvas::share_wheel_suppression(&mut app.views, now);
    let rect = ui.available_rect_before_wrap();
    let (tile_w, row_w) = column_width(rect.width(), n);
    // Bring the active tile into view when it changes (a tab click, Ctrl+Tab, a duplicate).
    let active = app.active_key();
    let reveal = ui.data_mut(|d| {
        let id = egui::Id::new("tile-strip-active");
        let changed = d.get_temp::<Option<ViewKey>>(id) != Some(active);
        d.insert_temp(id, active);
        changed
    });
    if row_w <= rect.width() + 0.5 {
        let clip = ui.clip_rect().intersect(rect);
        tiles(app, ui, rect, tile_w, clip, false, &t);
        return;
    }
    // Too many to fit: the row scrolls sideways by its scrollbar only. The wheel belongs to the
    // tile under the pointer, never to the row.
    let bar = ui.spacing().scroll.bar_width + 6.0;
    let height = (rect.height() - bar).max(1.0);
    egui::ScrollArea::horizontal()
        .id_salt("tile-strip")
        .auto_shrink([false, false])
        .scroll_source(egui::scroll_area::ScrollSource { scroll_bar: true, drag: egui::scroll_area::DragScroll::Never, mouse_wheel: false })
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            let (row, _) = ui.allocate_exact_size(vec2(row_w, height), egui::Sense::hover());
            let clip = ui.clip_rect().intersect(row);
            tiles(app, ui, row, tile_w, clip, reveal, &t);
        });
}

/// Draw the tiles of `row`, those in `clip` only.
fn tiles(app: &mut PdfCraftApp, ui: &mut egui::Ui, row: Rect, tile_w: f32, clip: Rect, reveal: bool, t: &Tokens) {
    let n = app.views.len();
    let rects: Vec<Rect> =
        (0..n).map(|i| Rect::from_min_size(pos2(row.min.x + i as f32 * (tile_w + GAP), row.min.y), vec2(tile_w, row.height()))).collect();
    if reveal && let Some(r) = app.active.and_then(|i| rects.get(i)) {
        ui.scroll_to_rect(*r, Some(egui::Align::Center));
    }
    activate(app, ui, &rects, clip);
    for (i, r) in rects.iter().enumerate() {
        if !r.intersects(clip) {
            continue;
        }
        let Some(key) = app.views.get(i).map(|v| v.key) else { break };
        let active = app.active == Some(i);
        let caption = Rect::from_min_size(r.min, vec2(r.width(), CAPTION));
        draw_caption(app, ui, i, key, caption, active, t);
        // Never inverted, however short the window.
        let body = Rect::from_min_max(pos2(r.min.x, (r.min.y + CAPTION).min(r.max.y)), r.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("tile", key)));
        child.set_clip_rect(body.intersect(clip));
        let interaction = if active { Interaction::Full } else { Interaction::NavigateOnly };
        let views = app.views.len();
        crate::canvas::document_area_with(app, i, &mut child, interaction);
        app.tile_frame.push(i);
        if active {
            ui.painter().with_clip_rect(clip).rect_stroke(r.shrink(1.0), 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
        }
        if i + 1 < n {
            ui.painter().with_clip_rect(clip).vline(r.max.x + GAP / 2.0, r.y_range(), Stroke::new(GAP, t.divider));
        }
        // A tile closed itself (a snapshot's Close): the others have moved, so draw them next frame.
        if app.views.len() != views {
            ui.ctx().request_repaint();
            break;
        }
    }
}

/// A press in a tile that isn't the active one makes it active (D1), before any tile is drawn,
/// so only one tile is ever drawn with its tools in a frame. The press only picks the tile: its
/// pages were navigation-only when the frame before was drawn, and egui decides what a press
/// lands on from that frame, so nothing there is selected, drawn on or placed by it. The Hand
/// tool's press pans at once all the same: its scroll area takes the drag, not the page.
fn activate(app: &mut PdfCraftApp, ui: &egui::Ui, rects: &[Rect], clip: Rect) {
    let press = ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::PointerButton { pos, button, pressed: true, .. }
                if matches!(button, PointerButton::Primary | PointerButton::Secondary) && pos.is_finite() =>
            {
                Some(*pos)
            }
            _ => None,
        })
    });
    let Some(pos) = press else { return };
    // Not through a dialog, the palette, or a window or menu drawn over the tiles.
    if app.modal_open() || app.palette_open || !clip.contains(pos) || ui.ctx().layer_id_at(pos) != Some(ui.layer_id()) {
        return;
    }
    if let Some(i) = rects.iter().position(|r| r.contains(pos))
        && i < app.views.len()
    {
        app.active = Some(i);
    }
}

/// The strip naming a tile's document, with the unsaved-changes dot; the active tile's is
/// highlighted.
fn draw_caption(app: &PdfCraftApp, ui: &mut egui::Ui, i: usize, key: ViewKey, rect: Rect, active: bool, t: &Tokens) {
    let name = crate::canvas::tab_label(app, i).unwrap_or_default();
    let dirty = app.views.get(i).and_then(|v| app.session.get(v.id)).is_some_and(|d| d.dirty);
    // Named for assistive technology; a press here picks the tile like one on its page.
    let label = if dirty { format!("{name} (edited)") } else { name.clone() };
    let resp = ui.interact(rect, ui.id().with(("tile-caption", key)), egui::Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &label));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, if active { t.selected } else { t.panel });
    let ink = if active { t.text } else { t.text_muted };
    let mut x = rect.min.x + 10.0;
    if dirty {
        painter.circle_filled(pos2(x + 3.0, rect.center().y), 3.5, ink);
        x += 12.0;
    }
    painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, name, theme::medium(12.0), ink);
}

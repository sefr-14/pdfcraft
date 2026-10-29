//! Guides on the pages (this fork, OV11), as in Foxit PDF Reader: made by dragging from a ruler or
//! double-clicking it ([`crate::rulers`]); with the Hand tool, a guide is moved by dragging it and
//! selected by clicking it, and Delete removes the selected one. They are drawn over the page as
//! the view's own marks, never into the page, so printing, exporting and Overlay output never
//! show them. Every view of a document shows the same guides ([`crate::doc_prefs`]); which one is
//! selected or dragged is the view's own.

use egui::{Pos2, Rect, Stroke, vec2};
use pdfcraft_engine::DocId;
use pdfcraft_render::{DocInfo, PageInfo};

use crate::canvas::{DocView, PageXform};
use crate::doc_prefs::{Axis, DocPrefs, GuideRef, PageGuides};
use crate::page_pointer::PagePointer;

/// How near (points) the pointer must be to a guide to take it.
pub(crate) const REACH: f32 = 4.0;
/// A colour for guides that stands out on drawings (cyan, as guides in other applications).
pub(crate) const GUIDE: egui::Color32 = egui::Color32::from_rgb(0, 174, 239);

/// A view's guide state: the guide selected, and a drag under way.
#[derive(Clone, Debug, Default)]
pub struct GuideView {
    pub selected: Option<GuideRef>,
    pub drag: Option<GuideDrag>,
    /// A guide being pulled out of a ruler: from the top one (a horizontal guide) or the left.
    pub pulling: Option<Pull>,
    /// Where the pages' pointer (`PagePointer`) was this frame, for the rulers' marks.
    pub pointer: Option<Pos2>,
}

/// A guide held by the pointer: where it was when taken. Escape puts it back and lets go of it
/// for the rest of the press (`live` is then false), so the press doesn't pan either.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideDrag {
    pub guide: GuideRef,
    pub from: f64,
    pub live: bool,
}

/// A guide being pulled out of a ruler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// From the top ruler: a guide horizontal on screen.
    Horizontal,
    /// From the left ruler: a guide vertical on screen.
    Vertical,
}

/// A user-space point on `page` → screen.
pub(crate) fn to_screen(page: &PageInfo, xf: &PageXform, x: f64, y: f64) -> Pos2 {
    let v = page.user_to_view(x as f32, y as f32);
    xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
}

/// A screen point → user space on `page`.
pub(crate) fn to_user(page: &PageInfo, xf: &PageXform, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let [x, y] = page.view_to_user(vx, vy);
    [f64::from(x), f64::from(y)]
}

/// A guide's line on screen, across its page.
pub(crate) fn segment(page: &PageInfo, xf: &PageXform, axis: Axis, at: f64) -> [Pos2; 2] {
    let [x0, y0, x1, y1] = page.crop.map(f64::from);
    match axis {
        Axis::X => [to_screen(page, xf, at, y0), to_screen(page, xf, at, y1)],
        Axis::Y => [to_screen(page, xf, x0, at), to_screen(page, xf, x1, at)],
    }
}

/// The guide through screen point `p` that is horizontal on screen (else vertical): which user
/// coordinate it fixes, and its value. On a page turned a quarter (by `/Rotate` or the view) a
/// line horizontal on screen fixes x.
pub(crate) fn through(page: &PageInfo, xf: &PageXform, p: Pos2, horizontal: bool) -> (Axis, f64) {
    let q = if horizontal { p + vec2(16.0, 0.0) } else { p + vec2(0.0, 16.0) };
    let (a, b) = (to_user(page, xf, p), to_user(page, xf, q));
    if (a[0] - b[0]).abs() < (a[1] - b[1]).abs() { (Axis::X, a[0]) } else { (Axis::Y, a[1]) }
}

/// A guide made through screen point `p` on page `i` shown at `xf`, horizontal on screen or
/// vertical, kept within the page. `Ok(None)` when `p` isn't on the page; `Err` when the page or
/// document already has as many guides as are kept.
pub(crate) fn create(
    info: &DocInfo,
    prefs: &mut DocPrefs,
    id: DocId,
    (i, xf): (usize, PageXform),
    p: Pos2,
    horizontal: bool,
) -> Result<Option<GuideRef>, ()> {
    let Some(page) = info.pages.get(i).filter(|_| xf.rect.contains(p)) else { return Ok(None) };
    let (axis, at) = through(page, &xf, p, horizontal);
    let (lo, hi) = range(page, axis);
    prefs.add_guide(id, i, axis, at.clamp(lo, hi)).map(Some).ok_or(())
}

/// The range a guide on `page` may take along `axis`: the page's visible area.
fn range(page: &PageInfo, axis: Axis) -> (f64, f64) {
    let [x0, y0, x1, y1] = page.crop.map(f64::from);
    match axis {
        Axis::X => (x0.min(x1), x0.max(x1)),
        Axis::Y => (y0.min(y1), y0.max(y1)),
    }
}

/// Whether a guide shows horizontal on screen.
fn horizontal(seg: [Pos2; 2]) -> bool {
    (seg[0].y - seg[1].y).abs() < (seg[0].x - seg[1].x).abs()
}

fn distance(p: Pos2, [a, b]: [Pos2; 2]) -> f32 {
    let ab = b - a;
    let len = ab.length_sq();
    let t = if len > 0.0 { ((p - a).dot(ab) / len).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// The guide under `p` in this frame's (or the last frame's) page positions, nearest first, and
/// whether it shows horizontal.
pub(crate) fn hit(view: &DocView, info: &DocInfo, prefs: &DocPrefs, id: DocId, p: Pos2) -> Option<(GuideRef, bool)> {
    let state = prefs.state(id).filter(|s| s.prefs.guides_shown)?;
    let mut best: Option<(f32, GuideRef, bool)> = None;
    for (i, xf) in &view.screen_xforms {
        let (Some(page), Some(guides)) = (info.pages.get(*i), state.page(*i)) else { continue };
        if !xf.rect.expand(REACH).contains(p) {
            continue;
        }
        for axis in [Axis::X, Axis::Y] {
            for &at in guides.axis(axis) {
                let seg = segment(page, xf, axis, at);
                let d = distance(p, seg);
                if d <= REACH && best.is_none_or(|(b, ..)| d < b) {
                    best = Some((d, GuideRef { page: *i, axis, at }, horizontal(seg)));
                }
            }
        }
    }
    best.map(|(_, g, h)| (g, h))
}

/// The guides' part of the page view before the pages are laid out: with the Hand tool (`hand`:
/// the active view, nothing over the page) whether a guide holds the pointer, so the press
/// doesn't also pan. Without it nothing is selected or held. Reads no input: the pointer is the
/// pages' own, read where they are drawn ([`pointer_input`]).
pub(crate) fn before_pages(view: &mut DocView, prefs: &DocPrefs, id: DocId, hand: bool) -> bool {
    if !hand {
        view.guides.selected = None;
        view.guides.drag = None;
        return false;
    }
    // Moved or deleted in another tab of the document.
    if view.guides.selected.is_some_and(|g| prefs.state(id).is_none_or(|s| !s.has(g) || !s.prefs.guides_shown)) {
        view.guides.selected = None;
    }
    view.guides.drag.is_some()
}

/// What the guides' pointer input acts on: the document's guides (`prefs`, `id`), the page view's
/// area on screen, and whether the Hand tool takes guides (see [`before_pages`]).
pub(crate) struct GuideFrame<'a> {
    pub(crate) prefs: &'a mut DocPrefs,
    pub(crate) id: DocId,
    pub(crate) area: Rect,
    pub(crate) hand: bool,
}

/// The Hand tool on the guides, from the pages' pointer (`ptr`, [`PagePointer`]) and where the
/// pages were last frame: a press on a guide takes it (and selects it), a drag moves the guide
/// taken, a release lets go, and a press elsewhere deselects. The pointer over a guide it can take
/// shows that it moves. What is drawn over the pages (a dialog, a menu, the quick bar) keeps all of
/// this from happening through it, as `PagePointer` knows.
pub(crate) fn pointer_input(ui: &egui::Ui, view: &mut DocView, info: &DocInfo, ptr: &PagePointer, frame: &mut GuideFrame<'_>) {
    let GuideFrame { prefs, id, area, hand } = frame;
    let (prefs, id, area) = (&mut **prefs, *id, *area);
    view.guides.pointer = ptr.pos;
    if !*hand {
        return;
    }
    if let Some(drag) = view.guides.drag {
        if !ptr.primary_down {
            view.guides.drag = None;
        } else if drag.live
            && let Some(p) = ptr.pos
            && let Some((page, xf)) =
                info.pages.get(drag.guide.page).zip(view.screen_xforms.iter().find(|(i, _)| *i == drag.guide.page).map(|(_, xf)| *xf))
        {
            let [x, y] = to_user(page, &xf, p);
            let (lo, hi) = range(page, drag.guide.axis);
            let to = match drag.guide.axis {
                Axis::X => x,
                Axis::Y => y,
            }
            .clamp(lo, hi);
            match prefs.move_guide(id, drag.guide, to) {
                Some(moved) => {
                    view.guides.selected = Some(moved);
                    if let Some(d) = view.guides.drag.as_mut() {
                        d.guide = moved;
                    }
                }
                // Deleted from another tab meanwhile.
                None => view.guides.drag = None,
            }
        }
    } else if let Some(p) = ptr.pressed().filter(|p| area.contains(*p)) {
        match hit(view, info, prefs, id, p) {
            Some((guide, _)) => {
                view.guides.selected = Some(guide);
                view.guides.drag = Some(GuideDrag { guide, from: guide.at, live: true });
            }
            None => view.guides.selected = None,
        }
    }
    hover_cursor(ui, view, info, prefs, id, ptr.within(area));
}

/// The guides of `pages` when the document's guides are shown, copied so the rulers can change them
/// while the pages are drawn.
pub(crate) fn shown(prefs: &DocPrefs, id: DocId, pages: &[usize]) -> Vec<(usize, PageGuides)> {
    let Some(state) = prefs.state(id).filter(|s| s.prefs.guides_shown) else { return Vec::new() };
    pages.iter().filter_map(|&i| Some((i, state.page(i)?.clone()))).collect()
}

/// Escape on a guide held by the pointer: back where it was, and let go of. Returns whether one
/// was held.
pub(crate) fn cancel_drag(view: &mut DocView, prefs: &mut DocPrefs, id: DocId) -> bool {
    if view.guides.pulling.take().is_some() {
        return true;
    }
    let Some(drag) = view.guides.drag.as_mut().filter(|d| d.live) else { return false };
    drag.live = false;
    if let Some(back) = prefs.move_guide(id, drag.guide, drag.from) {
        drag.guide = back;
        view.guides.selected = Some(back);
    }
    true
}

/// Draw `guides` on a page shown at `xf`: one device pixel wide, the selected one stronger.
pub(crate) fn paint(
    painter: &egui::Painter,
    page: &PageInfo,
    xf: &PageXform,
    page_index: usize,
    guides: &PageGuides,
    selected: Option<GuideRef>,
    accent: egui::Color32,
) {
    let ppp = painter.ctx().pixels_per_point().max(0.1);
    let clipped = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
    for axis in [Axis::X, Axis::Y] {
        for &at in guides.axis(axis) {
            let chosen = selected.is_some_and(|g| g.page == page_index && g.axis == axis && g.at == at);
            let stroke = if chosen { Stroke::new(2.0 / ppp, accent) } else { Stroke::new(1.0 / ppp, GUIDE) };
            clipped.line_segment(segment(page, xf, axis, at), stroke);
        }
    }
}

/// The pointer (`pointer`, from [`PagePointer`]) is over a guide the Hand tool can take, or one is
/// held: show that it moves.
fn hover_cursor(ui: &egui::Ui, view: &DocView, info: &DocInfo, prefs: &DocPrefs, id: DocId, pointer: Option<Pos2>) {
    let dragging = view.guides.drag.filter(|d| d.live);
    let shown = match dragging {
        Some(d) => info
            .pages
            .get(d.guide.page)
            .zip(view.screen_xforms.iter().find(|(i, _)| *i == d.guide.page))
            .map(|(page, (_, xf))| horizontal(segment(page, xf, d.guide.axis, d.guide.at))),
        None => pointer.and_then(|p| hit(view, info, prefs, id, p)).map(|(_, h)| h),
    };
    if let Some(h) = shown {
        ui.ctx().set_cursor_icon(if h { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
    }
}

/// Delete or Backspace with a guide selected (and no text field taking the keys): delete it.
/// The key is consumed, so a selected comment never goes with it.
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView, prefs: &mut DocPrefs, id: DocId) {
    let Some(guide) = view.guides.selected else { return };
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let delete =
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) | i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
    if delete {
        prefs.remove_guide(id, guide);
        view.guides.selected = None;
        view.guides.drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    /// A page whose crop box is `crop`, turned by `rotate`, with `/UserUnit` `unit`.
    fn page(crop: [f32; 4], rotate: u16, unit: f32) -> PageInfo {
        let (w, h) = ((crop[2] - crop[0]) * unit, (crop[3] - crop[1]) * unit);
        let (width, height) = if rotate.is_multiple_of(180) { (w, h) } else { (h, w) };
        PageInfo { width, height, label: String::new(), crop, rotation: rotate }
    }

    /// The page shown at `zoom` (screen points per point), turned a further `view` degrees, with
    /// its top-left corner at `at` (where scrolling put it).
    fn shown(p: &PageInfo, zoom: f32, view: u16, at: Pos2) -> PageXform {
        let (w, h) = if view.is_multiple_of(180) { (p.width, p.height) } else { (p.height, p.width) };
        PageXform { rect: Rect::from_min_size(at, vec2(w * zoom, h * zoom)), rot: view, pw: p.width, ph: p.height }
    }

    #[test]
    fn guides_stay_on_the_same_user_space_line_at_every_zoom_rotation_crop_and_user_unit() {
        let crops = [[0.0, 0.0, 612.0, 792.0], [100.0, -50.0, 700.0, 850.0], [-300.0, 200.0, 2500.0, 1900.0]];
        for crop in crops {
            for rotate in [0, 90, 180, 270] {
                for unit in [1.0, 2.0, 10.0] {
                    let p = page(crop, rotate, unit);
                    for view in [0, 90, 180, 270] {
                        for zoom in [0.25, 1.0, 3.7, 16.0] {
                            for at in [pos2(0.0, 0.0), pos2(-5000.0, 1234.5)] {
                                let xf = shown(&p, zoom, view, at);
                                let (ax, ay) = (crop[0] + 0.3 * (crop[2] - crop[0]), crop[1] + 0.7 * (crop[3] - crop[1]));
                                for (axis, value) in [(Axis::X, f64::from(ax)), (Axis::Y, f64::from(ay))] {
                                    let seg = segment(&p, &xf, axis, value);
                                    let case = format!("crop {crop:?} /Rotate {rotate} unit {unit} view {view} zoom {zoom} at {at:?} {axis:?}");
                                    // The line runs straight across the page on screen, edge to edge.
                                    assert!((seg[0].x - seg[1].x).abs() < 0.01 || (seg[0].y - seg[1].y).abs() < 0.01, "{case}: {seg:?}");
                                    assert!(xf.rect.expand(0.01).contains(seg[0]) && xf.rect.expand(0.01).contains(seg[1]), "{case}");
                                    // Every point of it reads back as that user coordinate.
                                    for t in [0.0, 0.5, 1.0] {
                                        let q = seg[0] + (seg[1] - seg[0]) * t;
                                        let u = to_user(&p, &xf, q);
                                        let got = if axis == Axis::X { u[0] } else { u[1] };
                                        let tolerance = 2e-3 * f64::from((crop[2] - crop[0]).abs().max((crop[3] - crop[1]).abs()));
                                        assert!((got - value).abs() < tolerance, "{case}: {got} for {value}");
                                    }
                                    // And it is where the page's own transform puts that line
                                    // (what comments and the measure tool draw with).
                                    let corner = to_screen(
                                        &p,
                                        &xf,
                                        if axis == Axis::X { value } else { f64::from(crop[0]) },
                                        if axis == Axis::Y { value } else { f64::from(crop[1]) },
                                    );
                                    assert!(corner.distance(seg[0]) < 0.05 || corner.distance(seg[1]) < 0.05, "{case}");
                                    // A guide made through a point of it, in its screen direction, is the same guide.
                                    let mid = seg[0] + (seg[1] - seg[0]) * 0.5;
                                    let (again, v) = through(&p, &xf, mid, horizontal(seg));
                                    assert_eq!(again, axis, "{case}");
                                    assert!((v - value).abs() < 2e-3 * f64::from(crop[2].abs().max(crop[3].abs()).max(1.0)), "{case}: {v}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_quarter_turn_shows_an_x_guide_horizontal() {
        let p = page([0.0, 0.0, 600.0, 800.0], 90, 1.0);
        let seg = segment(&p, &shown(&p, 1.0, 0, Pos2::ZERO), Axis::X, 300.0);
        assert!(horizontal(seg));
        let seg = segment(&p, &shown(&p, 1.0, 90, Pos2::ZERO), Axis::X, 300.0);
        assert!(!horizontal(seg), "turned back by the view");
    }

    #[test]
    fn distance_to_a_segment() {
        let seg = [pos2(0.0, 0.0), pos2(10.0, 0.0)];
        assert_eq!(distance(pos2(5.0, 3.0), seg), 3.0);
        assert_eq!(distance(pos2(13.0, 4.0), seg), 5.0);
        assert_eq!(distance(pos2(1.0, 1.0), [pos2(1.0, 0.0), pos2(1.0, 0.0)]), 1.0);
    }
}

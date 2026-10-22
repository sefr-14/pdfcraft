//! Rulers along the top and left of the page view (this fork, OV11), as in Foxit PDF Reader:
//! View ▸ Page display ▸ Rulers shows them, per PDF; right-click one to pick its unit. They
//! measure the page as printed (points, `/UserUnit` included), from the top-left corner of the
//! page under the pointer, or of the current page; a calibrated drawing scale doesn't change
//! them. Dragging out of a ruler, or double-clicking it, makes a guide ([`crate::guides`]). What
//! is remembered, and where, is in [`crate::doc_prefs`].

use egui::{Align2, Pos2, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::DocId;
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::doc_prefs::{DocPrefs, ViewPrefs};
use crate::guides::Pull;
use crate::theme::{self, Tokens};
use crate::units::Unit;

/// A ruler's thickness, in points.
pub(crate) const STRIP: f32 = 20.0;
/// Labelled ticks are at least this far apart, in points.
const MAJOR_GAP: f32 = 56.0;
/// Unlabelled ticks are at least this far apart, in points.
const MINOR_GAP: f32 = 5.0;
/// More ticks than any ruler on any screen shows: past this, none are drawn.
const MAX_TICKS: i64 = 4000;

/// The units a ruler can show, in the order its menu lists them.
const UNITS: [Unit; 4] = [Unit::Points, Unit::Inches, Unit::Millimetres, Unit::Centimetres];

fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Points => tl!("Points (pt)"),
        Unit::Inches => tl!("Inches (in)"),
        Unit::Millimetres => tl!("Millimeters (mm)"),
        Unit::Centimetres => tl!("Centimeters (cm)"),
    }
}

/// The page view's area once the rulers take their strips off its top and left (all of it when
/// it is too small to give them up).
pub(crate) fn inner(full: Rect) -> Rect {
    if full.width() <= 2.0 * STRIP || full.height() <= 2.0 * STRIP {
        return full;
    }
    Rect::from_min_max(full.min + vec2(STRIP, STRIP), full.max)
}

/// A tick on a ruler: where (screen points along it), whether it is labelled, and its value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tick {
    pub at: f32,
    pub major: bool,
    pub value: f64,
}

/// The ticks of a ruler from `start` to `end` (screen points along it), reading 0 at `origin`
/// with `px` screen points per unit: labelled every 1, 2 or 5 × 10ⁿ units, as close as
/// [`MAJOR_GAP`] allows, with unlabelled ones between. Nothing for a scale that isn't one.
pub(crate) fn ticks(origin: f32, start: f32, end: f32, px: f32) -> Vec<Tick> {
    if !(px.is_finite() && px > 0.0 && origin.is_finite() && start.is_finite() && end.is_finite() && start < end) {
        return Vec::new();
    }
    let px = f64::from(px);
    let major = nice_step(f64::from(MAJOR_GAP) / px);
    let parts = [10.0, 5.0, 2.0, 1.0].into_iter().find(|n| major / n * px >= f64::from(MINOR_GAP)).unwrap_or(1.0);
    let minor = major / parts;
    let first = ((f64::from(start) - f64::from(origin)) / (minor * px)).ceil();
    let last = ((f64::from(end) - f64::from(origin)) / (minor * px)).floor();
    if !(first.is_finite() && last.is_finite()) || last - first > MAX_TICKS as f64 || first.abs() > 1e12 || last.abs() > 1e12 {
        return Vec::new();
    }
    // In range and finite (checked above), so these casts neither saturate nor wrap.
    let (first, last, parts) = (first as i64, last as i64, parts as i64);
    (first..=last)
        .map(|k| {
            let value = k as f64 * minor;
            Tick { at: (f64::from(origin) + value * px) as f32, major: k.rem_euclid(parts.max(1)) == 0, value }
        })
        .collect()
}

/// The smallest 1, 2 or 5 × 10ⁿ at least `min` (positive and finite; 1 otherwise).
fn nice_step(min: f64) -> f64 {
    if !(min.is_finite() && min > 0.0) {
        return 1.0;
    }
    let base = 10f64.powf(min.log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * base).find(|s| *s >= min * (1.0 - 1e-9)).unwrap_or(10.0 * base)
}

/// A label for `value`, with the decimals a `step` between labels needs.
fn label(value: f64, step: f64) -> String {
    let decimals = if step >= 1.0 { 0 } else { (-step.log10()).ceil().clamp(0.0, 6.0) as usize };
    let text = format!("{value:.decimals$}");
    // "-0" reads as a sign error.
    if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { text.trim_start_matches('-').to_string() } else { text }
}

/// The page the rulers measure: the one under the pointer, else the current one, else the first
/// shown. With its place on screen.
pub(crate) fn aligned_page(view: &DocView, pointer: Option<Pos2>) -> Option<(usize, PageXform)> {
    let shown = &view.screen_xforms;
    pointer
        .and_then(|p| shown.iter().find(|(_, xf)| xf.rect.contains(p)))
        .or_else(|| shown.iter().find(|(i, _)| *i == view.current))
        .or_else(|| shown.first())
        .copied()
}

/// Screen points per point of the page as shown (the zoom).
fn px_per_point(xf: &PageXform) -> f32 {
    let shown_w = if xf.rot.is_multiple_of(180) { xf.pw } else { xf.ph };
    xf.rect.width() / shown_w.max(1e-3)
}

/// What a ruler's menu asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    Unit(Unit),
    Rulers(bool),
    Guides(bool),
    /// Clear Guides on Page, for this page.
    ClearPage(usize),
    ClearAll,
}

/// Draw the rulers in the strips `full` has beyond `inner`, from this frame's page positions,
/// and take their input when `input` (the active view, nothing over the page): their menu, and
/// guides pulled out of them or made by double-clicking them. Returns why a guide wasn't made.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut egui::Ui,
    view: &mut DocView,
    info: &DocInfo,
    prefs: &mut DocPrefs,
    id: DocId,
    full: Rect,
    inner: Rect,
    input: bool,
) -> Option<&'static str> {
    let state = prefs.state(id)?;
    let current = state.prefs;
    let aligned = aligned_page(view, None).map(|(i, _)| i);
    let has_page = aligned.is_some_and(|i| state.page(i).is_some_and(|g| !g.is_empty()));
    let has_any = state.pages_with_guides().next().is_some();
    if !current.rulers || inner == full || !input {
        view.guides.pulling = None;
    }
    if !current.rulers || inner == full {
        return None;
    }
    let t = Tokens::get(ui.ctx());
    let top = Rect::from_min_max(Pos2::new(inner.left(), full.top()), Pos2::new(full.right(), inner.top()));
    let left = Rect::from_min_max(Pos2::new(full.left(), inner.top()), Pos2::new(inner.left(), full.bottom()));
    let corner = Rect::from_min_max(full.min, inner.min);
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let page = aligned_page(view, pointer.filter(|p| inner.contains(*p)));
    let mut pick = None;
    let mut made = Ok(None);
    for (rect, horizontal) in [(top, true), (left, false)] {
        paint(ui, rect, horizontal, page, current.unit, pointer, &t);
        if !input {
            continue;
        }
        let resp = ui.interact(rect, ui.id().with(("ruler", view.key, horizontal)), Sense::click_and_drag());
        let pull = if horizontal { Pull::Horizontal } else { Pull::Vertical };
        if resp.drag_started_by(egui::PointerButton::Primary) {
            view.guides.pulling = Some(pull);
        }
        let at = ui.input(|i| i.pointer.latest_pos());
        if resp.drag_stopped_by(egui::PointerButton::Primary)
            && view.guides.pulling.take() == Some(pull)
            && let Some(p) = at.filter(|p| inner.contains(*p))
            && let Some(&shown) = view.screen_xforms.iter().find(|(_, xf)| xf.rect.contains(p))
        {
            // Released over a page: the guide goes there. Off every page, nothing is made.
            made = crate::guides::create(info, prefs, id, shown, p, horizontal);
        }
        if resp.double_clicked()
            && let (Some(p), Some(shown)) = (at, aligned_page(view, None))
        {
            // The top ruler makes a guide across the page at that x, the left one at that y.
            let on_page = if horizontal { Pos2::new(p.x, shown.1.rect.center().y) } else { Pos2::new(shown.1.rect.center().x, p.y) };
            made = crate::guides::create(info, prefs, id, shown, on_page, !horizontal);
        }
        resp.context_menu(|ui| {
            if let Some(p) = menu(ui, current, aligned.filter(|_| has_page), has_any) {
                pick = Some(p);
                ui.close();
            }
        });
    }
    if !ui.input(|i| i.pointer.primary_down()) {
        view.guides.pulling = None;
    }
    // The guide being pulled out, across the page view at the pointer.
    if let (Some(pull), Some(p)) = (view.guides.pulling, pointer.filter(|p| inner.contains(*p))) {
        let line = match pull {
            Pull::Horizontal => [Pos2::new(inner.left(), p.y), Pos2::new(inner.right(), p.y)],
            Pull::Vertical => [Pos2::new(p.x, inner.top()), Pos2::new(p.x, inner.bottom())],
        };
        let ppp = ui.ctx().pixels_per_point().max(0.1);
        ui.painter_at(inner).line_segment(line, Stroke::new(1.0 / ppp, crate::guides::GUIDE));
    }
    let painter = ui.painter_at(corner);
    painter.rect_filled(corner, 0.0, t.chrome);
    painter.text(corner.center(), Align2::CENTER_CENTER, current.unit.label(), theme::regular(9.0), t.text_muted);
    match pick {
        Some(Pick::Unit(unit)) => prefs.set_unit(id, unit),
        Some(Pick::Rulers(on)) => prefs.set_rulers(id, on),
        Some(Pick::Guides(on)) => prefs.set_guides_shown(id, on),
        Some(Pick::ClearPage(page)) => {
            prefs.clear_page(id, page);
        }
        Some(Pick::ClearAll) => {
            prefs.clear_all(id);
        }
        None => {}
    }
    made.err().map(|()| tl!("This page already has as many guides as PdfCraft keeps."))
}

/// A ruler's context menu: its unit, whether the rulers and guides show, and clearing guides
/// (on `page` when it has any; on every page when `any`).
fn menu(ui: &mut egui::Ui, current: ViewPrefs, page: Option<usize>, any: bool) -> Option<Pick> {
    let mut pick = None;
    for unit in UNITS {
        if ui.add(crate::widgets::radio(current.unit == unit, unit_name(unit))).clicked() {
            pick = Some(Pick::Unit(unit));
        }
    }
    ui.separator();
    let (mut rulers, mut guides) = (current.rulers, current.guides_shown);
    if ui.add(crate::widgets::checkbox(&mut rulers, tl!("Rulers"))).changed() {
        pick = Some(Pick::Rulers(rulers));
    }
    if ui.add(crate::widgets::checkbox(&mut guides, tl!("Guides"))).changed() {
        pick = Some(Pick::Guides(guides));
    }
    ui.separator();
    if ui.add_enabled(page.is_some(), egui::Button::new(tl!("Clear Guides on Page"))).clicked() {
        pick = page.map(Pick::ClearPage);
    }
    if ui.add_enabled(any, egui::Button::new(tl!("Clear All Guides"))).clicked() {
        pick = Some(Pick::ClearAll);
    }
    pick
}

/// One ruler: its ticks from `page`'s top-left corner, and where the pointer is.
fn paint(ui: &egui::Ui, rect: Rect, horizontal: bool, page: Option<(usize, PageXform)>, unit: Unit, pointer: Option<Pos2>, t: &Tokens) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, t.chrome);
    let edge = if horizontal { [rect.left_bottom(), rect.right_bottom()] } else { [rect.right_top(), rect.right_bottom()] };
    painter.line_segment(edge, Stroke::new(1.0, t.divider));
    let Some((_, xf)) = page else { return };
    let px = px_per_point(&xf) / unit.per_point() as f32;
    let (origin, start, end) = if horizontal { (xf.rect.left(), rect.left(), rect.right()) } else { (xf.rect.top(), rect.top(), rect.bottom()) };
    let ticks = ticks(origin, start, end, px);
    let step = ticks.iter().filter(|k| k.major).map(|k| k.value).collect::<Vec<_>>();
    let step = step.windows(2).find_map(|w| w.get(1).zip(w.first()).map(|(b, a)| b - a)).unwrap_or(1.0);
    let ink = t.text_muted;
    for tick in &ticks {
        let len = if tick.major { STRIP * 0.55 } else { STRIP * 0.25 };
        let (a, b) = if horizontal {
            (Pos2::new(tick.at, rect.bottom()), Pos2::new(tick.at, rect.bottom() - len))
        } else {
            (Pos2::new(rect.right(), tick.at), Pos2::new(rect.right() - len, tick.at))
        };
        painter.line_segment([a, b], Stroke::new(1.0, ink));
        if tick.major {
            let text = label(tick.value, step);
            if horizontal {
                painter.text(Pos2::new(tick.at + 2.0, rect.top() + 1.0), Align2::LEFT_TOP, text, theme::regular(9.0), ink);
            } else {
                let galley = painter.layout_no_wrap(text, theme::regular(9.0), ink);
                // Turned to read along the ruler, upwards.
                let at = Pos2::new(rect.left() + 1.0, tick.at - 2.0);
                painter.add(egui::epaint::TextShape::new(at, galley, ink).with_angle(-std::f32::consts::FRAC_PI_2));
            }
        }
    }
    // Where the pointer is, on both rulers.
    if let Some(p) = pointer {
        let mark = Stroke::new(1.0, t.accent);
        if horizontal && rect.x_range().contains(p.x) {
            painter.line_segment([Pos2::new(p.x, rect.top()), Pos2::new(p.x, rect.bottom())], mark);
        } else if !horizontal && rect.y_range().contains(p.y) {
            painter.line_segment([Pos2::new(rect.left(), p.y), Pos2::new(rect.right(), p.y)], mark);
        }
    }
}

impl crate::PdfCraftApp {
    /// View ▸ Page display ▸ Rulers (or, with `guides`, Guides) for the active document: shown or
    /// hidden, and remembered for that PDF.
    pub(crate) fn toggle_rulers(&mut self, guides: bool) {
        self.sync_doc_prefs();
        let Some((_, id)) = self.active_ids() else { return };
        let Some(now) = self.doc_prefs.state(id).map(|s| s.prefs) else { return };
        if guides {
            self.doc_prefs.set_guides_shown(id, !now.guides_shown);
        } else {
            self.doc_prefs.set_rulers(id, !now.rulers);
        }
    }

    /// Clear Guides on Page (the active view's current page) or, without `page_only`, Clear All
    /// Guides, for the active document.
    pub(crate) fn clear_guides(&mut self, page_only: bool) {
        self.sync_doc_prefs();
        let Some((i, id)) = self.active_ids() else { return };
        if page_only {
            let page = self.views[i].current;
            self.doc_prefs.clear_page(id, page);
        } else {
            self.doc_prefs.clear_all(id);
        }
        self.views[i].guides = Default::default();
    }

    /// `ui.set` / `--rulers`-style options for the active document: `rulers` and `guides` (on or
    /// off) and `ruler_unit` (pt, in, mm or cm).
    pub(crate) fn set_ruler_option(&mut self, key: &str, value: &str) -> Result<(), String> {
        self.sync_doc_prefs();
        let id = self.active_ids().map(|(_, id)| id).ok_or("no document is open")?;
        let on = || match value {
            "on" | "true" => Ok(true),
            "off" | "false" => Ok(false),
            _ => Err(format!("{key} must be on or off")),
        };
        match key {
            "rulers" => self.doc_prefs.set_rulers(id, on()?),
            "guides" => self.doc_prefs.set_guides_shown(id, on()?),
            _ => {
                let unit = UNITS.into_iter().find(|u| u.label() == value).ok_or("ruler_unit must be pt, in, mm or cm")?;
                self.doc_prefs.set_unit(id, unit);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_read_from_the_page_corner_at_a_readable_spacing() {
        // 1 unit = 10 screen points, page corner at 100: labels every 10 units (100 points apart
        // is the first 1-2-5 step past 56), unlabelled ticks every unit.
        let ticks = ticks(100.0, 0.0, 400.0, 10.0);
        let major: Vec<(f32, f64)> = ticks.iter().filter(|t| t.major).map(|t| (t.at, t.value)).collect();
        assert_eq!(major, [(0.0, -10.0), (100.0, 0.0), (200.0, 10.0), (300.0, 20.0), (400.0, 30.0)], "before the page, negative");
        let minor = ticks.iter().filter(|t| !t.major).count();
        assert_eq!(minor, 36, "nine between each pair of labels");
        // Zoomed far out (0.01 points per unit) or in (1000), still readable.
        for px in [0.01f32, 0.37, 1000.0, 123_456.0] {
            let t = super::ticks(0.0, 0.0, 1000.0, px);
            let major: Vec<f32> = t.iter().filter(|t| t.major).map(|t| t.at).collect();
            assert!(major.windows(2).all(|w| w[1] - w[0] >= MAJOR_GAP - 0.01), "{px}: {major:?}");
            assert!(major.windows(2).all(|w| w[1] - w[0] <= 5.0 * MAJOR_GAP), "{px}: {major:?}");
            assert!(t.windows(2).all(|w| w[1].at - w[0].at >= MINOR_GAP - 0.01), "{px}");
        }
    }

    #[test]
    fn ticks_never_run_away_on_odd_input() {
        for (origin, start, end, px) in [
            (0.0, 0.0, 100.0, 0.0),
            (0.0, 0.0, 100.0, -1.0),
            (0.0, 0.0, 100.0, f32::NAN),
            (0.0, 0.0, 100.0, f32::INFINITY),
            (f32::NAN, 0.0, 100.0, 1.0),
            (0.0, 100.0, 0.0, 1.0),
            (1e30, 0.0, 100.0, 1e-30),
            (0.0, -1e30, 1e30, 1.0),
        ] {
            assert!(ticks(origin, start, end, px).len() as i64 <= MAX_TICKS + 1, "{origin} {start} {end} {px}");
        }
        assert!(ticks(0.0, 0.0, 100.0, 0.0).is_empty());
        assert!(ticks(0.0, 100.0, 0.0, 1.0).is_empty());
    }

    #[test]
    fn labels_have_the_decimals_their_step_needs() {
        assert_eq!(label(20.0, 10.0), "20");
        assert_eq!(label(0.5, 0.5), "0.5");
        assert_eq!(label(0.25, 0.05), "0.25");
        assert_eq!(label(-0.0, 1.0), "0");
        assert_eq!(label(-1e-9, 0.1), "0.0");
        assert_eq!(nice_step(0.0), 1.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(12.0), 20.0);
        assert_eq!(nice_step(0.07), 0.1);
    }

    #[test]
    fn the_strips_come_off_the_top_and_left() {
        let full = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(810.0, 620.0));
        assert_eq!(inner(full), Rect::from_min_max(Pos2::new(30.0, 40.0), Pos2::new(810.0, 620.0)));
        let tiny = Rect::from_min_max(Pos2::ZERO, Pos2::new(30.0, 30.0));
        assert_eq!(inner(tiny), tiny, "too small to give them up");
    }
}

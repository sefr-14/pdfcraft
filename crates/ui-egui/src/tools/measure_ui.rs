// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Measure objects: page-space clicks, live readings and persistent drawing scales.
use std::collections::HashSet;
use std::sync::{Arc, mpsc};

use crate::{LeftPanel, PdfCraftApp, QuickTool, canvas::DocView, theme::Tokens, widgets};
use egui::{Color32, Pos2, Stroke};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::{
    Document, Edit, Style,
    measure::{
        self, Kind, NewMeasurement, PageUnit, Point, Reading, Scale,
        presets::{IMPERIAL, Imperial, METRIC, RATIO_RANGE, format_ratio, parse_ratio},
        snap::SnapKind,
        units::{self, EIGHTHS, LengthUnit},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Distance,
    Perimeter,
    Area,
    Calibrate,
}
impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Perimeter => "perimeter",
            Self::Area => "area",
            Self::Calibrate => "calibrate",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "distance" => Some(Self::Distance),
            "perimeter" => Some(Self::Perimeter),
            "area" => Some(Self::Area),
            "calibrate" => Some(Self::Calibrate),
            _ => None,
        }
    }
    fn kind(self) -> Kind {
        match self {
            Self::Area => Kind::Area,
            Self::Perimeter => Kind::Perimeter,
            _ => Kind::Distance,
        }
    }
}
pub struct MeasureView {
    pub points: Vec<Point>,
    pub page: Option<usize>,
    pub tool: Option<Tool>,
    pub reading: Option<Reading>,
    pub scale: Option<Scale>,
    pub error: Option<String>,
    pub snaps: measure::snap::SnapOptions,
    pub snap_enabled: bool,
    pub sensitivity: f64,
    /// The page side of the drawing scale, in its page unit ([`page_unit_of`]).
    pub drawing_points: f64,
    pub real_distance: f64,
    pub unit: String,
    /// The scale shows feet and inches (`unit` is then `ft`) (this fork, OV10).
    pub feet_inches: bool,
    /// The unit is typed in (Custom) rather than picked from the list.
    pub custom_unit: bool,
    /// The known distance as typed in feet and inches.
    pub known_text: String,
    /// Two points were just clicked to calibrate: the known distance is asked for next, and
    /// these are the scale fields from before the clicks, which Escape puts back (this fork,
    /// OV11). Kept apart from `points`, so it is never taken for a measurement in progress.
    pub pending_calibration: Option<CalibrationBefore>,
    focus_known: bool,
    /// The known-distance field, as last drawn: Escape in it cancels the pending calibration.
    pub(crate) known_field: Option<egui::Id>,
    /// The unit picked for the page side of the scale; `None` follows `unit`.
    pub page_unit: Option<PageUnit>,
    /// The "1:N" field of the drawing scale, as typed.
    pub ratio_text: String,
    /// Which common scales the list offers (this fork, MEASURE.2). Picking it changes no field.
    pub system: UnitSystem,
    /// How a plan writes the scale typed or picked last ("1:50", `1/4" = 1'-0"`), saved as its
    /// ratio text; `None` once the fields are edited by hand or calibrated (this fork, MEASURE.2).
    pub ratio_label: Option<String>,
    /// The "1:N" field was edited and not yet applied.
    ratio_dirty: bool,
    pub precision: u8,
    pub name: String,
    pub whole_page: bool,
    pub rect: [f64; 4],
    pub label: String,
    seeded_page: Option<usize>,
    /// What the pointer snapped to, and where, while it is over a page (tests read it).
    pub snapped: Option<(Point, SnapKind)>,
    /// The page `snapped` is on.
    snapped_on: Option<usize>,
    /// Why the last click placed or kept nothing, on the page it was on (shown at the pointer
    /// and in the panel).
    pub notice: Option<(usize, String)>,
    /// Drawn lines for snapping (this fork, OV10: the Overlay wizard's snapper), read on a worker
    /// per (page, content generation, hidden layers), most recent first.
    snappers: Vec<(SnapKey, Lines)>,
    /// How many times a page's lines have been read for snapping (diagnostics and tests).
    pub lines_read: usize,
    /// How many placements had to wait for a page's lines (diagnostics and tests).
    pub click_waits: usize,
    /// Saved measurements per edit generation, for the panel.
    listing: Option<(u64, Result<measure::Listing, String>)>,
}
impl Default for MeasureView {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            page: None,
            tool: None,
            reading: None,
            scale: None,
            error: None,
            snaps: Default::default(),
            snap_enabled: true,
            sensitivity: 6.0,
            drawing_points: 1.0,
            real_distance: 1.0,
            unit: "in".into(),
            feet_inches: false,
            custom_unit: false,
            known_text: String::new(),
            pending_calibration: None,
            focus_known: false,
            known_field: None,
            page_unit: None,
            ratio_text: String::new(),
            system: UnitSystem::Imperial,
            ratio_label: None,
            ratio_dirty: false,
            precision: 2,
            name: "Drawing scale".into(),
            whole_page: true,
            rect: [0.0, 0.0, 100.0, 100.0],
            label: String::new(),
            seeded_page: None,
            snapped: None,
            snapped_on: None,
            notice: None,
            snappers: Vec::new(),
            lines_read: 0,
            click_waits: 0,
            listing: None,
        }
    }
}
/// The Drawing scale fields as they were before two points were clicked to calibrate.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationBefore {
    drawing_points: f64,
    real_distance: f64,
    unit: String,
    feet_inches: bool,
    custom_unit: bool,
    known_text: String,
    page_unit: Option<PageUnit>,
    ratio_label: Option<String>,
}

/// The unit of the page side of a drawing scale: the one picked, else the real-world unit when it
/// is pt, in, mm or cm (a plan at 1:50 in millimetres is 1 mm = 50 mm), else the display unit.
pub(crate) fn page_unit_of(picked: Option<PageUnit>, unit: &str, display: crate::units::Unit) -> PageUnit {
    picked.or_else(|| PageUnit::from_label(unit)).or_else(|| PageUnit::from_label(display.label())).unwrap_or(PageUnit::Pt)
}
/// Which common scales the list offers (this fork, MEASURE.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitSystem {
    Metric,
    Imperial,
}
/// `a` and `b` are the same ratio, but for float noise.
fn same_ratio(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs()
}
/// The system a unit belongs to; `None` for a typed (Custom) one.
fn system_of(unit: Option<LengthUnit>) -> Option<UnitSystem> {
    match unit? {
        LengthUnit::Inches | LengthUnit::Feet | LengthUnit::FeetInches => Some(UnitSystem::Imperial),
        LengthUnit::Millimeters | LengthUnit::Centimeters | LengthUnit::Meters => Some(UnitSystem::Metric),
    }
}
/// The N of 1:N the scale fields stand for, in any listed unit (this fork, MEASURE.2: inches, feet
/// and feet and inches too). `None` for a Custom unit or a scale that isn't a clean ratio.
fn ratio_of_fields(state: &MeasureView, display: crate::units::Unit) -> Option<f64> {
    let real_in = state.real_distance * unit_choice(state)?.inches();
    let paper_in = state.drawing_points * page_unit_of(state.page_unit, &state.unit, display).points() / 72.0;
    let n = real_in / paper_in;
    // Float noise from the unit conversions is rounded away; anything that isn't a clean ratio isn't one.
    let clean = (n * 1e6).round() / 1e6;
    (RATIO_RANGE.contains(&clean) && (clean - n).abs() <= 1e-9 * n).then_some(clean)
}
/// Make the "1:N" field and the common scale picked in its list say what the scale fields now hold: the ratio, or nothing.
fn sync_ratio_text(state: &mut MeasureView, display: crate::units::Unit) {
    state.ratio_text = ratio_of_fields(state, display).map(format_ratio).unwrap_or_default();
    state.ratio_dirty = false;
}
/// What the scale fields hold, to notice a hand edit.
fn scale_fields(state: &MeasureView) -> (f64, f64, String, Option<PageUnit>) {
    (state.drawing_points, state.real_distance, state.unit.clone(), state.page_unit)
}
/// Show the actual distance in `unit` from the list (the known feet-and-inches text too).
fn use_unit(state: &mut MeasureView, unit: LengthUnit) {
    state.unit = unit.label().into();
    state.feet_inches = unit == LengthUnit::FeetInches;
    state.custom_unit = false;
}
/// Fill the scale fields for a typed 1:N, keeping the unit system (this fork, MEASURE.2): one
/// inch on the page is N inches in an imperial unit, one millimetre is N millimetres in a metric
/// one, and a Custom unit becomes metres. Returns `false`, changing nothing, when `n` is not a
/// usable ratio.
pub(crate) fn set_ratio(state: &mut MeasureView, n: f64) -> bool {
    if !RATIO_RANGE.contains(&n) {
        return false;
    }
    let (unit, page_unit, from) = match unit_choice(state) {
        Some(unit) if system_of(Some(unit)) == Some(UnitSystem::Imperial) => (unit, PageUnit::In, LengthUnit::Inches),
        Some(unit) => (unit, PageUnit::Mm, LengthUnit::Millimeters),
        None => (LengthUnit::Meters, PageUnit::Mm, LengthUnit::Millimeters),
    };
    use_unit(state, unit);
    state.page_unit = Some(page_unit);
    state.drawing_points = 1.0;
    state.real_distance = unit.convert(n, from);
    state.known_text = units::feet_inches(state.real_distance, EIGHTHS);
    state.ratio_text = format_ratio(n);
    state.ratio_label = Some(format_ratio(n));
    true
}
/// Fill the scale fields for a metric common scale 1:N: in the metric unit shown, else in metres.
pub(crate) fn set_metric(state: &mut MeasureView, n: f64) -> bool {
    if !RATIO_RANGE.contains(&n) {
        return false;
    }
    if system_of(unit_choice(state)) != Some(UnitSystem::Metric) {
        use_unit(state, LengthUnit::Meters);
    }
    set_ratio(state, n)
}
/// Fill the scale fields for an imperial common scale, as it is written (`1/4"` on the page is
/// `1'-0"`): in the imperial unit shown, else in feet and inches. Never switches to SI units.
pub(crate) fn set_imperial(state: &mut MeasureView, preset: Imperial) {
    let unit = match unit_choice(state) {
        Some(unit) if system_of(Some(unit)) == Some(UnitSystem::Imperial) => unit,
        _ => LengthUnit::FeetInches,
    };
    use_unit(state, unit);
    state.page_unit = Some(PageUnit::In);
    state.drawing_points = preset.paper;
    state.real_distance = unit.convert(preset.real, LengthUnit::Feet);
    state.known_text = units::feet_inches(state.real_distance, EIGHTHS);
    state.ratio_text = format_ratio(preset.ratio());
    state.ratio_label = Some(preset.label.to_owned());
}
/// A scale typed or picked in the panel, applied at once.
#[derive(Clone, Copy, Debug)]
enum Chosen {
    Typed(f64),
    Metric(f64),
    Imperial(Imperial),
}
impl MeasureView {
    /// Drop the measurement in progress and what it shows: points, live reading, the notice at
    /// the pointer, the last error and snap target. A pending calibration is a separate step.
    pub fn cancel(&mut self) {
        self.points.clear();
        self.page = None;
        self.reading = None;
        self.scale = None;
        self.notice = None;
        self.error = None;
        self.snapped = None;
    }

    /// The known distance is asked for: two points were clicked to calibrate.
    pub fn calibrating(&self) -> bool {
        self.pending_calibration.is_some()
    }

    /// Cancel a calibration waiting for its known distance: the scale fields go back to what
    /// they held before the two clicks. A scale applied earlier is in the document and stays.
    /// Returns whether there was one.
    pub fn cancel_calibration(&mut self) -> bool {
        let Some(before) = self.pending_calibration.take() else { return false };
        self.drawing_points = before.drawing_points;
        self.real_distance = before.real_distance;
        self.unit = before.unit;
        self.feet_inches = before.feet_inches;
        self.custom_unit = before.custom_unit;
        self.known_text = before.known_text;
        self.page_unit = before.page_unit;
        self.ratio_label = before.ratio_label;
        self.focus_known = false;
        true
    }

    fn calibration_before(&self) -> CalibrationBefore {
        CalibrationBefore {
            drawing_points: self.drawing_points,
            real_distance: self.real_distance,
            unit: self.unit.clone(),
            feet_inches: self.feet_inches,
            custom_unit: self.custom_unit,
            known_text: self.known_text.clone(),
            page_unit: self.page_unit,
            ratio_label: self.ratio_label.clone(),
        }
    }

    /// The drawn lines of `page` for snapping: read once on a worker per content generation and
    /// set of hidden layers, `Loading` until it is done.
    fn snapping(&mut self, ctx: &egui::Context, doc: &Document, page: usize) -> Snapping<'_> {
        let key = SnapKey::of(doc, page);
        match self.snappers.iter().position(|(k, _)| *k == key) {
            Some(i) => {
                let entry = self.snappers.remove(i);
                self.snappers.insert(0, entry);
            }
            None => {
                self.snappers.insert(0, (key, Lines::Loading(read_lines(ctx, doc, page))));
                self.lines_read = self.lines_read.saturating_add(1);
                self.snappers.truncate(SNAPPERS);
            }
        }
        self.poll();
        match self.snappers.first() {
            Some((_, Lines::Ready(Ok(s)))) => Snapping::Ready(s),
            Some((_, Lines::Ready(Err(e)))) => Snapping::Failed(e),
            Some((_, Lines::Loading(_))) | None => Snapping::Loading,
        }
    }

    /// Take the lines of every worker that has finished.
    fn poll(&mut self) {
        for (_, lines) in &mut self.snappers {
            if let Lines::Loading(rx) = lines {
                match rx.try_recv() {
                    Ok(done) => *lines = Lines::Ready(done),
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => *lines = Lines::Ready(Err(tl!("the worker stopped").into())),
                }
            }
        }
    }

    /// A point is being placed on `page` while its lines load: wait for them, up to `CLICK_WAIT`,
    /// so the click isn't lost to a worker that is a moment behind (a normal sheet takes
    /// milliseconds; Windows build #22 lost the control channel's clicks to that race). Only the
    /// read for the page as it is now (`SnapKey`) is waited for and filled in; on a timeout it
    /// stays loading.
    #[cfg(not(target_arch = "wasm32"))]
    fn wait_for_lines(&mut self, doc: &Document, page: usize) {
        self.click_waits = self.click_waits.saturating_add(1);
        let key = SnapKey::of(doc, page);
        if let Some((_, lines)) = self.snappers.iter_mut().find(|(k, _)| *k == key)
            && let Lines::Loading(rx) = lines
        {
            match rx.recv_timeout(CLICK_WAIT) {
                Ok(done) => *lines = Lines::Ready(done),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => *lines = Lines::Ready(Err(tl!("the worker stopped").into())),
            }
        }
    }

    /// A page's drawn lines are being read for snapping: a click on it waits for them (up to
    /// `CLICK_WAIT`) and places no point if they still aren't in.
    pub fn snapping_loading(&mut self) -> bool {
        self.poll();
        self.snappers.iter().any(|(_, l)| matches!(l, Lines::Loading(_)))
    }

    /// Tests: `page`'s lines stay loading until the returned sender sends them.
    #[cfg(test)]
    pub(crate) fn hold_snapping(&mut self, doc: &Document, page: usize) -> mpsc::Sender<Result<Snapper, String>> {
        let (tx, rx) = mpsc::channel();
        self.snappers.insert(0, (SnapKey::of(doc, page), Lines::Loading(rx)));
        tx
    }
}

/// How long a click made while a page's lines load waits for them before placing nothing.
#[cfg(not(target_arch = "wasm32"))]
const CLICK_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Pages whose lines are kept for snapping: the current one and a few the pointer was over.
const SNAPPERS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnapKey {
    page: usize,
    /// Not the edit generation: a measurement added doesn't change the lines.
    generation: u64,
    hidden: Vec<(u32, u16)>,
}
impl SnapKey {
    fn of(doc: &Document, page: usize) -> SnapKey {
        SnapKey { page, generation: doc.content_generation(), hidden: doc.info.layers.iter().filter(|l| !l.visible).map(|l| l.id).collect() }
    }
}

enum Lines {
    Loading(mpsc::Receiver<Result<Snapper, String>>),
    Ready(Result<Snapper, String>),
}

enum Snapping<'a> {
    Loading,
    Ready(&'a Snapper),
    Failed(&'a str),
}

/// A page's drawn lines, as the Overlay wizard snaps to them: up to 400,000 segments, content
/// in hidden layers left out, in the page's displayed frame, with the matrices between that
/// frame and user space.
pub(crate) struct Snapper {
    lines: pdfcraft_overlay::snap::Lines,
    to_display: pdfcraft_overlay::Matrix,
    to_user: pdfcraft_overlay::Matrix,
}
impl Snapper {
    /// Read page `page` of a document's working file, leaving out the layers `hidden` lists.
    /// Slow on a dense sheet: run it off the UI thread.
    pub(crate) fn read(bytes: Arc<Vec<u8>>, password: Option<&str>, page: usize, hidden: &HashSet<ObjRef>) -> Result<Snapper, String> {
        let doc = pdfcraft_cos::Document::open_with_password(bytes, password).map_err(|e| e.to_string())?;
        let (frame, _) = pdfcraft_overlay::frame_of_page(&doc, page).map_err(|e| e.to_string())?;
        let to_user = frame.to_display.invert().ok_or_else(|| tl!("the page can't be mapped").to_string())?;
        let lines = pdfcraft_overlay::snap::lines(&doc, page, hidden).map_err(|e| e.to_string())?;
        Ok(Snapper { lines, to_display: frame.to_display, to_user })
    }

    /// The target nearest `at` (user space) no further than `reach` is from it, and its kind.
    fn snap(&self, at: Point, reach: Point, options: measure::snap::SnapOptions) -> Option<(Point, SnapKind)> {
        use pdfcraft_overlay::snap::SnapKind as Overlay;
        let shown = |p: Point| {
            let (x, y) = self.to_display.apply(p[0], p[1]);
            pdfcraft_overlay::Point::new(x, y)
        };
        let (a, r) = (shown(at), shown(reach));
        let options = pdfcraft_overlay::snap::SnapOptions {
            ends: options.endpoints,
            intersections: options.intersections,
            midpoints: options.midpoints,
            on_line: options.paths,
        };
        let hit = self.lines.snap(a, (a.x - r.x).hypot(a.y - r.y), options)?;
        let (x, y) = self.to_user.apply(hit.point.x, hit.point.y);
        let kind = match hit.kind {
            Overlay::End => SnapKind::Endpoint,
            Overlay::Intersection => SnapKind::Intersection,
            Overlay::Midpoint => SnapKind::Midpoint,
            Overlay::OnLine => SnapKind::Path,
        };
        (x.is_finite() && y.is_finite()).then_some(([x, y], kind))
    }
}

/// Start reading `page`'s lines on a worker thread (in place on the web).
fn read_lines(ctx: &egui::Context, doc: &Document, page: usize) -> mpsc::Receiver<Result<Snapper, String>> {
    let (tx, rx) = mpsc::channel();
    let (bytes, password, ctx) = (doc.bytes.clone(), doc.password.clone(), ctx.clone());
    let hidden: HashSet<ObjRef> = doc.info.layers.iter().filter(|l| !l.visible).map(|l| ObjRef::new(l.id.0, l.id.1)).collect();
    let run = move || {
        let read = pdfcraft_engine::guard(|| Snapper::read(bytes, password.as_deref(), page, &hidden))
            .unwrap_or_else(|m| Err(crate::i18n::fmt(tl!("an internal error stopped it ({reason})"), &[("reason", &m)])));
        // The view may have moved on and dropped the receiver.
        let _ = tx.send(read);
        ctx.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    if std::thread::Builder::new().name("measure-snap".into()).spawn(run).is_err() {
        // `run`, and the sender with it, is dropped: the receiver reports a stopped worker.
        log::warn!("measure: couldn't start a worker thread");
    }
    #[cfg(target_arch = "wasm32")]
    run();
    rx
}

/// A message at the pointer: why a click placed or kept nothing, or that snapping is loading.
fn notice(ui: &egui::Ui, at: Pos2, text: &str) {
    egui::Area::new(ui.scope_id().with("measure-notice"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(at + egui::vec2(14.0, 30.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                ui.label(text);
            });
        });
}
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    doc: &Document,
    view: &mut DocView,
    cx: &crate::comments::PageCx<'_>,
    tool: Tool,
    units: crate::units::Unit,
) {
    let page = cx.page;
    let xf = cx.xf;
    let author = &cx.prefs.author;
    let Some(info) = doc.info.pages.get(page) else { return };
    // Not through a window drawn over the page, such as the Overlay wizard (this fork, OV11): the
    // page pointer already leaves those out.
    let pos = cx.ptr.within(xf.rect);
    let current = view.current;
    let state = &mut view.measure;
    if !doc.editable() {
        // Without a readable object graph there is nothing to measure against (this fork, OV10:
        // said at the pointer instead of ignoring clicks silently).
        if let Some(pos) = pos {
            let reason = doc.read_only_reason.as_deref().unwrap_or(tl!("This document can't be read for measuring."));
            notice(ui, pos, reason);
        }
        return;
    }
    if state.tool != Some(tool) {
        state.cancel();
        state.tool = Some(tool);
    }
    // Keys belong to a focused text field (the label, unit or viewport name) when there is one.
    // Escape is not read here: `PdfCraftApp::escape_step` takes one step back per press.
    let typing = ui.ctx().egui_wants_keyboard_input();
    let screen = |p: Point| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
    };
    let mut hover = None;
    let mut snap = None;
    if pos.is_none() && state.snap_enabled && page == current {
        // Read the current page's lines before the pointer gets there.
        let ctx = ui.ctx().clone();
        let _ = state.snapping(&ctx, doc, page);
    }
    if let Some(pos) = pos {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        let (vx, vy) = xf.screen_to_view(pos);
        let [x, y] = info.view_to_user(vx, vy);
        let mut point = [f64::from(x), f64::from(y)];
        let mut loading = false;
        // A press held past egui's click time, or moved more than a few pixels, isn't a click:
        // the point goes where the button is released (this fork, OV10). Once per release.
        let placed = resp.clicked() || resp.drag_stopped_by(egui::PointerButton::Primary);
        if state.snap_enabled {
            let pixels_per_unit = f64::from((xf.rect.width() / xf.pw).max(1e-6));
            // A point `sensitivity` pixels from the pointer: the snapping reach, in any frame.
            let reach = doc.measurement_to_user(page, [f64::from(vx) + state.sensitivity / pixels_per_unit, f64::from(vy)]).unwrap_or(point);
            match snap_to_lines(state, ui.ctx(), doc, page, Reach { point, reach, placed }) {
                Some(Some((p, kind))) => {
                    point = p;
                    snap = Some(kind);
                }
                Some(None) => {}
                None => loading = true,
            }
        }
        if ui.input(|i| i.modifiers.shift)
            && let Some(last) = state.points.last()
        {
            point = measure::snap::constrain(*last, point);
        }
        hover = Some(point);
        state.snapped = snap.map(|kind| (point, kind));
        state.snapped_on = Some(page);
        if loading {
            // Never a silent unsnapped point: a click that waited `CLICK_WAIT` for the lines in
            // vain places none and says why; Snap to drawing off places points at once.
            notice(ui, pos, tl!("Reading the drawing for snapping…"));
        } else if placed {
            // An area closes on a click within 6 pixels of its first vertex.
            let closes =
                tool == Tool::Area && state.points.len() >= 3 && state.points.first().is_some_and(|first| screen(*first).distance(pos) <= 6.0);
            place(state, &mut view.pending_edit, doc, Click { page, tool, point, closes }, author, units);
        }
        let allowed = doc.allows_annotation();
        match &state.notice {
            Some((p, text)) if *p == page && !loading => notice(ui, pos, text),
            _ if !allowed && !loading => notice(ui, pos, tl!("This PDF doesn't allow comments, so measurements and scales can't be saved.")),
            _ => {}
        }
    } else if state.snapped_on == Some(page) {
        // The pointer left this page, or a window covers it: no target here any more.
        state.snapped = None;
        state.snapped_on = None;
    }
    // The pointer's target, before the first point too (this fork, OV10).
    let stroke = Stroke::new(1.5, Color32::from_rgb(0, 120, 214));
    if let Some(p) = hover {
        match snap {
            Some(kind) => {
                ui.painter().circle_stroke(screen(p), 6.0, Stroke::new(1.0, Color32::from_rgb(46, 158, 92)));
                let label = tl!(match kind {
                    SnapKind::Endpoint => "Endpoint",
                    SnapKind::Midpoint => "Midpoint",
                    SnapKind::Intersection => "Intersection",
                    SnapKind::Path => "Path",
                });
                // A label (not painted text), so assistive technology and tests read it too.
                egui::Area::new(ui.scope_id().with("measure-snap"))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .pivot(egui::Align2::LEFT_BOTTOM)
                    .fixed_pos(screen(p) + egui::vec2(10.0, -10.0))
                    .show(ui.ctx(), |ui| ui.label(egui::RichText::new(label).font(crate::theme::regular(11.0)).color(stroke.color)));
            }
            None if state.points.is_empty() => {
                ui.painter().circle_stroke(screen(p), 3.0, stroke);
            }
            None => {}
        }
    }
    if state.page == Some(page) && !state.points.is_empty() {
        if (resp.double_clicked() || !typing && ui.input(|i| i.key_pressed(egui::Key::Enter))) && matches!(tool, Tool::Perimeter | Tool::Area) {
            finish(state, &mut view.pending_edit, doc, page, tool, author, units);
        }
        if !typing && ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
            state.points.pop();
        }
        let mut points = state.points.clone();
        if let Some(p) = hover
            && points.last().is_none_or(|last| measure::distance(*last, p) > 1e-6)
        {
            points.push(p);
        }
        if let Some(scale) = &state.scale {
            match measure::reading(tool.kind(), &points, scale) {
                Ok(reading) => state.reading = Some(reading),
                Err(e) => state.error = Some(e.to_string()),
            }
        }
        let positions: Vec<Pos2> = points.iter().map(|&p| screen(p)).collect();
        for pair in positions.windows(2) {
            if let [a, b] = pair {
                ui.painter().line_segment([*a, *b], stroke);
            }
        }
        if tool == Tool::Area
            && let (Some(a), Some(b)) = (positions.first(), positions.last())
        {
            ui.painter().line_segment([*a, *b], stroke);
        }
        for &p in &positions {
            ui.painter().circle_filled(p, 3.0, stroke.color);
        }
        if let (Some(p), Some(reading)) = (hover, &state.reading) {
            ui.painter().text(screen(p) + egui::vec2(12.0, 12.0), egui::Align2::LEFT_TOP, &reading.label, crate::theme::semibold(12.0), stroke.color);
        }
    }
}
/// A point to snap: where the pointer is (user space), a point `sensitivity` pixels from it (the
/// reach), and whether a point is placed there this frame.
struct Reach {
    point: Point,
    reach: Point,
    placed: bool,
}

/// Snap `at` to the page's drawn lines (this fork, OV10.2: read on a worker, as the Overlay
/// wizard snaps), noting in `state.error` why snapping is limited. `None` while the lines are
/// still loading; a point being placed waits for them first (up to `CLICK_WAIT`).
fn snap_to_lines(state: &mut MeasureView, ctx: &egui::Context, doc: &Document, page: usize, at: Reach) -> Option<Option<(Point, SnapKind)>> {
    let Reach { point, reach, placed } = at;
    let snaps = state.snaps;
    #[cfg(not(target_arch = "wasm32"))]
    if placed && matches!(state.snapping(ctx, doc, page), Snapping::Loading) {
        state.wait_for_lines(doc, page);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = placed;
    let (hit, error) = match state.snapping(ctx, doc, page) {
        Snapping::Loading => return None,
        Snapping::Failed(e) => (None, Some(crate::i18n::fmt(tl!("Snapping isn't available on this page: {reason}"), &[("reason", e)]))),
        Snapping::Ready(s) => {
            let error = if s.lines.truncated {
                Some(tl!("This drawing exceeds the snapping limit; only the first paths are used.").to_string())
            } else if s.lines.unreadable > 0 {
                Some(tl!("Some page content couldn't be read; snapping uses the rest.").to_string())
            } else {
                None
            };
            (s.snap(point, reach, snaps), error)
        }
    };
    if error.is_some() {
        state.error = error;
    }
    Some(hit)
}

/// A click (or release) on the page, where it snapped to.
struct Click {
    page: usize,
    tool: Tool,
    point: Point,
    /// It is on an area's first vertex.
    closes: bool,
}
/// The next vertex at `click`, finishing a distance or a calibration on its second. What keeps
/// a click from counting is said in `state.notice`.
fn place(state: &mut MeasureView, pending: &mut Option<Edit>, doc: &Document, click: Click, author: &str, units: crate::units::Unit) {
    let Click { page, tool, point, closes } = click;
    if state.page != Some(page) {
        state.cancel();
        state.page = Some(page);
    }
    state.notice = None;
    if state.points.is_empty() {
        match doc.measurement_scale_or(page, point, units.per_point(), units.label()) {
            Ok(scale) => {
                state.scale = Some(scale);
                state.error = None;
            }
            // Calibrating sets a scale, so one that can't be read doesn't stop it (there is just
            // no live reading); anything else is never measured with another scale instead.
            Err(_) if tool == Tool::Calibrate => state.scale = None,
            Err(e) => {
                state.notice = Some((
                    page,
                    crate::i18n::fmt(
                        tl!("This part of the page has a drawing scale PDF Overlay can't read: {reason}. Calibrate to set one."),
                        &[("reason", &e)],
                    ),
                ));
                return;
            }
        }
    }
    let repeats = state.points.last().is_some_and(|last| measure::distance(*last, point) < 1e-6);
    if repeats {
        state.notice = Some((page, tl!("That is the point already placed. Click a different point.").into()));
    } else if !closes && state.points.len() < measure::MAX_POINTS {
        state.points.push(point);
    }
    let finishes = closes || matches!(tool, Tool::Distance | Tool::Calibrate) && state.points.len() == 2;
    if finishes {
        finish(state, pending, doc, page, tool, author, units);
    }
}
fn finish(state: &mut MeasureView, pending: &mut Option<Edit>, doc: &Document, page: usize, tool: Tool, author: &str, units: crate::units::Unit) {
    if tool == Tool::Calibrate {
        if let [a, b] = state.points.as_slice() {
            let a = doc.measurement_to_view(page, *a);
            let b = doc.measurement_to_view(page, *b);
            if let (Ok(a), Ok(b)) = (a, b) {
                // Clicking two more points keeps what the fields held before the first pair.
                if state.pending_calibration.is_none() {
                    state.pending_calibration = Some(state.calibration_before());
                }
                state.drawing_points = measure::distance(a, b) / page_unit_of(state.page_unit, &state.unit, units).points();
                state.ratio_label = None;
                state.focus_known = true;
            }
        }
        state.cancel();
        return;
    }
    let minimum = if tool == Tool::Area { 3 } else { 2 };
    if state.points.len() < minimum {
        state.error = Some(crate::i18n::fmt(tl!("This measurement needs at least {n} points."), &[("n", &minimum.to_string())]));
        return;
    }
    let Some(scale) = state.scale.clone() else { return };
    if !doc.allows_annotation() {
        // Measured, but not kept: the reading stays in the panel (this fork, OV10).
        let reading = measure::reading(tool.kind(), &state.points, &scale).ok();
        state.cancel();
        state.reading = reading;
        state.scale = Some(scale);
        state.notice = Some((page, tl!("This PDF doesn't allow comments, so measurements and scales can't be saved.").into()));
        return;
    }
    *pending = Some(Edit::AddMeasurement(NewMeasurement {
        page,
        kind: tool.kind(),
        points: state.points.clone(),
        scale,
        style: Style { color: [0.0, 0.47, 0.84], ..Style::default() },
        label: state.label.clone(),
        author: author.into(),
    }));
    state.cancel();
}
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::ScrollArea::vertical().id_salt("measure-panel").show(ui, |ui| panel_body(app, ui, t));
}
/// Draw the measurement panel: tool selection, scale calibration, snapping and the current reading.
fn panel_body(app: &mut PdfCraftApp, ui: &mut egui::Ui, _t: &Tokens) {
    crate::widgets::begin_dialog(ui);
    let Some((index, id)) = app.active_ids() else {
        ui.label(tl!("Open a PDF to measure it."));
        return;
    };
    let mut tool = None;
    ui.horizontal(|ui| {
        for (label, value) in [("Distance", Tool::Distance), ("Perimeter", Tool::Perimeter), ("Area", Tool::Area)] {
            if ui.selectable_label(app.quick_tool == QuickTool::Measure(value), tl!(label)).clicked() {
                tool = Some(value);
            }
        }
    });
    let Some(doc) = app.session.get(id) else { return };
    let units = app.units;
    let page = app.views[index].current;
    let info = doc.info.pages.get(page).cloned();
    let state = &mut app.views[index].measure;
    if state.listing.as_ref().is_none_or(|(g, _)| *g != doc.edit_generation()) {
        state.listing = Some((doc.edit_generation(), doc.measurements()));
    }
    if state.seeded_page != Some(page) {
        state.seeded_page = Some(page);
        // One page unit (the one the scale's unit gives) and what it measures.
        if let Ok(a) = doc.measurement_to_user(page, [0.0, 0.0])
            && let Ok(scale) = doc.measurement_scale_or(page, a, units.per_point(), units.label())
            && let Ok(b) = doc.measurement_to_user(page, [page_unit_of(None, &scale.unit, units).points(), 0.0])
            && let Ok(reading) = measure::reading(Kind::Distance, &[a, b], &scale)
        {
            state.drawing_points = 1.0;
            state.real_distance = reading.value;
            // The page side keeps its own unit, so picking another actual unit never changes the
            // scale (this fork, MEASURE.1).
            state.page_unit = Some(page_unit_of(None, &scale.unit, units));
            state.unit = scale.unit;
            state.precision = scale.precision;
            state.feet_inches = scale.feet_inches.is_some();
            state.custom_unit = !state.feet_inches && LengthUnit::from_label(&state.unit).is_none();
            state.known_text = units::feet_inches(state.real_distance, EIGHTHS);
            state.pending_calibration = None;
            state.ratio_label = None;
        }
        state.system = system_of(unit_choice(state)).unwrap_or(state.system);
        sync_ratio_text(state, units);
    }
    ui.label(tl!("Click vertices. Enter or double-click finishes a perimeter or area. Click the first point to close an area."));
    ui.label(tl!("Shift constrains to 45 degrees. Backspace removes a vertex. Escape cancels."));
    ui.separator();
    ui.label(tl!("Measurement information"));
    if let Some(reading) = &state.reading {
        ui.label(egui::RichText::new(&reading.label).strong());
        ui.label(format!("X: {:.3}   Y: {:.3}", reading.delta_x, reading.delta_y));
        ui.label(format!("{}: {:.2}°", tl!("Angle"), reading.angle));
        if let Some(scale) = &state.scale {
            ui.label(&scale.ratio);
        }
    } else {
        ui.label(tl!("Click the first point to begin."));
    }
    // What the pointer shows on the page, here too (this fork, OV10).
    let notice_color = Color32::from_rgb(190, 120, 30);
    let restricted = tl!("This PDF doesn't allow comments, so measurements and scales can't be saved.");
    if !doc.allows_annotation() {
        ui.colored_label(notice_color, restricted);
    }
    if let Some((_, notice)) = state.notice.as_ref().filter(|(_, n)| n != restricted) {
        ui.colored_label(notice_color, notice);
    }
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Label"));
        ui.add(crate::widgets::line(&mut state.label).id(egui::Id::unique("measure-label")).desired_width(130.0).char_limit(128))
            .labelled_by(label.id);
    });
    ui.separator();
    // While the drawing is read for snapping, it says so on this row: a row of its own would come
    // and go and move everything below it under the pointer (FIX.1).
    let loading = state.snap_enabled && state.snapping_loading();
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut state.snap_enabled, tl!("Snap to drawing")));
        if loading {
            ui.spinner();
            // One line, cut short in a narrow panel: the full text shows on hover.
            ui.add(egui::Label::new(tl!("Reading the drawing for snapping…")).truncate());
        }
    });
    if state.snap_enabled {
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut state.snaps.endpoints, tl!("Endpoints")));
            ui.add(widgets::checkbox(&mut state.snaps.midpoints, tl!("Midpoints")));
        });
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut state.snaps.intersections, tl!("Intersections")));
            ui.add(widgets::checkbox(&mut state.snaps.paths, tl!("Paths")));
        });
        ui.add(egui::Slider::new(&mut state.sensitivity, 1.0..=20.0).text(tl!("Sensitivity")));
    }
    ui.separator();
    // The drawing scale as one equation (this fork, MEASURE.1); "1:N" or a common scale fills it
    // and applies at once.
    let mut chosen = None;
    let fields = scale_fields(state);
    scale_header(ui, state, units, &mut chosen);
    scale_rows(ui, state, units);
    // A hand edit of the fields leaves the 1:N field showing the old ratio: say the new one.
    if scale_fields(state) != fields {
        sync_ratio_text(state, units);
        state.ratio_label = None;
    }
    // Two-point calibration is its own step: two clicks on the page, then the actual distance.
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(tl!("Two-point calibration")).strong());
        if ui.button(tl!("Calibrate from two points")).clicked() {
            tool = Some(Tool::Calibrate);
        }
    });
    if state.calibrating() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new(tl!("Type the real distance between the two points.")).strong());
        });
    }
    ui.add(widgets::checkbox(&mut state.whole_page, tl!("Entire page")));
    if !state.whole_page {
        ui.label(tl!("Viewport rectangle in page points (x, y, width, height)"));
        for (i, name) in ["X", "Y", "Width", "Height"].iter().enumerate() {
            if let Some(v) = state.rect.get_mut(i) {
                ui.add(egui::DragValue::new(v).range(0.0..=1e9).prefix(format!("{}: ", tl!(name))));
            }
        }
    }
    ui.horizontal(|ui| {
        let viewport = ui.label(tl!("Viewport"));
        ui.add(crate::widgets::line(&mut state.name).desired_width(130.0).char_limit(128)).labelled_by(viewport.id);
    });
    let apply = ui.add_enabled(doc.allows_annotation(), egui::Button::new(tl!("Apply scale"))).clicked();
    if let Some(error) = &state.error {
        ui.colored_label(Color32::from_rgb(190, 50, 50), error);
    }
    ui.separator();
    let mut select = None;
    match state.listing.as_ref().map(|(_, l)| l) {
        None => {}
        Some(Ok(listing)) => {
            ui.label(format!("{}: {}", tl!("Saved measurements"), listing.measurements.len()));
            if !listing.unsupported.is_empty() {
                let reasons: Vec<String> =
                    listing.unsupported.iter().map(|u| format!("{} {}: {}", tl!("Page"), u.page.saturating_add(1), u.reason)).collect();
                ui.colored_label(Color32::from_rgb(190, 120, 30), format!("{}: {}", tl!("Unsupported measurements"), listing.unsupported.len()))
                    .on_hover_text(reasons.join("\n"));
            }
            egui::ScrollArea::vertical().id_salt("measurement-list").max_height(140.0).show(ui, |ui| {
                for m in &listing.measurements {
                    if ui.button(format!("{} {}: {} {}", tl!("Page"), m.page.saturating_add(1), m.reading.label, m.label)).clicked() {
                        select = Some((m.page, m.index, m.reading.clone(), m.scale.clone()));
                    }
                }
            });
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::from_rgb(190, 50, 50), e);
        }
    }
    let export = ui.button(tl!("Export measurements as CSV")).clicked();
    let filled = match chosen {
        Some(Chosen::Typed(n)) => set_ratio(state, n),
        Some(Chosen::Metric(n)) => set_metric(state, n),
        Some(Chosen::Imperial(preset)) => {
            set_imperial(state, preset);
            true
        }
        None => false,
    };
    if filled {
        state.error = None;
    }
    // Apply scale, or a 1:N typed or picked: the fields are applied to the document at once.
    if (apply || chosen.is_some())
        && doc.allows_annotation()
        && let Some(info) = info
    {
        let result =
            doc.measurement_to_user(page, [0.0, 0.0]).and_then(|a| doc.measurement_to_user(page, [1.0, 0.0]).map(|b| (a, b))).and_then(|(a, b)| {
                if state.feet_inches && known_feet(&state.known_text).is_none() {
                    return Err(tl!("Type a length such as 12'-5 3/8\" or 12.5'.").to_string());
                }
                // The page side in its page unit, as upstream's ratio ("1 mm = 50 mm"); then feet and
                // inches when picked (this fork, OV10).
                let page_unit = page_unit_of(state.page_unit, &state.unit, units);
                let scale = Scale::ratio(a, b, state.drawing_points, page_unit, state.real_distance, &state.unit, state.precision);
                // Feet and inches to 1/8 inch (this fork, OV10).
                let scale = scale.and_then(|s| if state.feet_inches { s.in_feet_inches(EIGHTHS) } else { Ok(s) });
                // A drawing scale is written as plans write it, "1:50" or `1/4" = 1'-0"` (this fork, MEASURE.2).
                match &state.ratio_label {
                    Some(label) => scale.and_then(|s| s.with_ratio_text(label)),
                    None => scale,
                }
                .map_err(|e| e.to_string())
            });
        match result {
            Ok(scale) => {
                state.pending_calibration = None;
                let [x, y, w, h] = if state.whole_page { [0.0, 0.0, f64::from(info.width), f64::from(info.height)] } else { state.rect };
                let a = info.view_to_user(x as f32, y as f32);
                let b = info.view_to_user((x + w) as f32, (y + h) as f32);
                app.views[index].pending_edit = Some(Edit::SetMeasurementScale {
                    page,
                    bbox: [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1])), f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))],
                    name: state.name.clone(),
                    scale,
                });
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }
    if let Some((page, annotation, reading, scale)) = select {
        app.views[index].current = page;
        app.views[index].comments.selected = Some((page, annotation));
        app.views[index].measure.reading = Some(reading);
        app.views[index].measure.scale = Some(scale);
        app.quick_tool = QuickTool::Select;
    }
    if let Some(tool) = tool {
        pick(&mut app.views[index]);
        app.quick_tool = QuickTool::Measure(tool);
    }
    if export {
        export_csv(app);
    }
}
/// The unit picked in the list, `None` for a typed (Custom) one.
fn unit_choice(state: &MeasureView) -> Option<LengthUnit> {
    if state.feet_inches {
        Some(LengthUnit::FeetInches)
    } else if state.custom_unit {
        None
    } else {
        LengthUnit::from_label(&state.unit)
    }
}

fn unit_name(unit: Option<LengthUnit>) -> &'static str {
    match unit {
        Some(LengthUnit::Inches) => tl!("Inches (in)"),
        Some(LengthUnit::Feet) => tl!("Feet (ft)"),
        Some(LengthUnit::FeetInches) => tl!("Feet and inches (ft-in)"),
        Some(LengthUnit::Centimeters) => tl!("Centimeters (cm)"),
        Some(LengthUnit::Millimeters) => tl!("Millimeters (mm)"),
        Some(LengthUnit::Meters) => tl!("Meters (m)"),
        None => tl!("Custom"),
    }
}

/// A positive length typed in feet and inches, in feet.
fn known_feet(text: &str) -> Option<f64> {
    units::parse_feet_inches(text).filter(|v| *v > 0.0)
}

/// The drawing scale's title, the Metric/Imperial choice of common scales, the "1:N" field and the
/// common scales of the system picked (this fork, MEASURE.2). Picking a system changes no field.
fn scale_header(ui: &mut egui::Ui, state: &mut MeasureView, units: crate::units::Unit, chosen: &mut Option<Chosen>) {
    ui.horizontal_wrapped(|ui| {
        let title = ui.label(egui::RichText::new(tl!("Drawing scale")).strong());
        for (system, label) in [(UnitSystem::Metric, tl!("Metric")), (UnitSystem::Imperial, tl!("Imperial"))] {
            if ui.selectable_label(state.system == system, label).clicked() {
                state.system = system;
            }
        }
        let typed = ui.add(crate::widgets::line(&mut state.ratio_text).hint_text("1:100").desired_width(64.0).char_limit(16));
        typed.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, tl!("Scale")));
        state.ratio_dirty |= typed.changed();
        if typed.lost_focus() && std::mem::take(&mut state.ratio_dirty) {
            match parse_ratio(&state.ratio_text) {
                Some(n) => *chosen = Some(Chosen::Typed(n)),
                None => state.error = Some(tl!("Enter a scale such as 1:50.").into()),
            }
        }
        let current = ratio_of_fields(state, units);
        let applies = |n: f64| current.is_some_and(|c| same_ratio(c, n));
        craft_ui::ComboBox::from_id_salt("measure-common-scales")
            .selected_text(tl!("Common scales"))
            .width(20.0)
            .height(400.0)
            .show_ui(ui, |ui| match state.system {
                UnitSystem::Metric => {
                    for n in METRIC {
                        if ui.selectable_label(applies(n), format_ratio(n)).clicked() {
                            *chosen = Some(Chosen::Metric(n));
                        }
                    }
                }
                UnitSystem::Imperial => {
                    for preset in IMPERIAL {
                        let text = format!("{}   ({})", preset.label, format_ratio(preset.ratio()));
                        if ui.selectable_label(applies(preset.ratio()), text).clicked() {
                            *chosen = Some(Chosen::Imperial(preset));
                        }
                    }
                }
            })
            .response
            .labelled_by(title.id);
    });
}

/// The narrowest panel that shows the scale equation on one line: the drawing side, "=" and the
/// actual side, with their units (this fork, MEASURE.1).
fn equation_width(ui: &egui::Ui, state: &MeasureView) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    let actual = if state.feet_inches { 110.0 } else { 80.0 };
    let custom = if state.custom_unit { 64.0 + gap } else { 0.0 };
    // Drawing value, page unit, "=", actual value, the unit list (its longest name) and gaps.
    70.0 + 52.0 + 12.0 + actual + 170.0 + custom + 5.0 * gap
}

/// The drawing scale as one equation, each value labelled above it: "Drawing distance [1] [mm]
/// = Actual distance [50] [Meters (m)]", on two rows when the panel is narrow (this fork,
/// MEASURE.1). Feet and inches are typed as `12'-5 3/8"`, Custom takes any other label (OV10).
/// Picking another unit converts the value on its side only, so the scale stays the same.
fn scale_rows(ui: &mut egui::Ui, state: &mut MeasureView, units: crate::units::Unit) {
    // The page side keeps the unit it is shown in, whatever the actual unit becomes.
    if state.page_unit.is_none() {
        state.page_unit = Some(page_unit_of(None, &state.unit, units));
    }
    let focus = std::mem::take(&mut state.focus_known);
    let mut picked = None;
    if ui.available_width() >= equation_width(ui, state) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| drawing_side(ui, state, units));
            ui.vertical(|ui| {
                ui.label(" ");
                ui.label("=");
            });
            ui.vertical(|ui| actual_side(ui, state, focus, false, &mut picked));
        });
    } else {
        drawing_side(ui, state, units);
        actual_side(ui, state, focus, true, &mut picked);
    }
    if state.feet_inches && known_feet(&state.known_text).is_none() {
        ui.colored_label(Color32::from_rgb(190, 50, 50), tl!("Type a length such as 12'-5 3/8\" or 12.5'."));
    }
    let choice = unit_choice(state);
    match picked {
        Some(Some(unit)) => {
            if let Some(from) = choice {
                state.real_distance = unit.convert(state.real_distance, from);
            }
            use_unit(state, unit);
            state.known_text = units::feet_inches(state.real_distance, EIGHTHS);
            state.system = system_of(Some(unit)).unwrap_or(state.system);
        }
        Some(None) => {
            state.custom_unit = true;
            state.feet_inches = false;
        }
        None => {}
    }
    if state.feet_inches {
        ui.label(tl!("Nearest 1/8 inch"));
    } else {
        ui.add(egui::DragValue::new(&mut state.precision).range(0..=6).prefix(format!("{} ", tl!("Decimals"))));
    }
}

/// "Drawing distance" above the page side of the scale and its page unit.
fn drawing_side(ui: &mut egui::Ui, state: &mut MeasureView, units: crate::units::Unit) {
    let label = ui.label(tl!("Drawing distance"));
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut state.drawing_points).range(0.000001..=1e9)).labelled_by(label.id);
        let page_unit = page_unit_of(state.page_unit, &state.unit, units);
        let mut changed = None;
        craft_ui::ComboBox::from_id_salt("measure-page-unit")
            .selected_text(page_unit.label())
            .width(44.0)
            .show_ui(ui, |ui| {
                for u in PageUnit::ALL {
                    if ui.selectable_label(u == page_unit, u.label()).clicked() && u != page_unit {
                        changed = Some(u);
                    }
                }
            })
            .response
            .labelled_by(label.id);
        // Another page unit converts the drawing distance, so the scale stays the same.
        if let Some(u) = changed {
            state.drawing_points = state.drawing_points * page_unit.points() / u.points();
            state.page_unit = Some(u);
        }
    });
}

/// "Actual distance" above the real-world side of the scale and its unit; `equals` leads the
/// values with "=" when the equation takes two rows. The unit picked is returned in `picked`
/// (`Some(None)` for Custom).
fn actual_side(ui: &mut egui::Ui, state: &mut MeasureView, focus: bool, equals: bool, picked: &mut Option<Option<LengthUnit>>) {
    let label = ui.label(tl!("Actual distance"));
    let choice = unit_choice(state);
    ui.horizontal(|ui| {
        if equals {
            ui.label("=");
        }
        let known = if state.feet_inches {
            let resp = ui.add(crate::widgets::line(&mut state.known_text).desired_width(110.0).char_limit(32).hint_text("12'-5 3/8\""));
            if resp.changed()
                && let Some(feet) = known_feet(&state.known_text)
            {
                state.real_distance = feet;
            }
            resp
        } else {
            ui.add(egui::DragValue::new(&mut state.real_distance).range(0.000001..=1e12))
        };
        let known = known.labelled_by(label.id);
        if focus {
            known.request_focus();
        }
        state.known_field = Some(known.id);
        // Tall enough for all seven entries without scrolling.
        craft_ui::ComboBox::from_id_salt("measure-unit")
            .selected_text(unit_name(choice))
            .height(400.0)
            .show_ui(ui, |ui| {
                for unit in LengthUnit::ALL.into_iter().map(Some).chain([None]) {
                    if ui.selectable_label(choice == unit, unit_name(unit)).clicked() && choice != unit {
                        *picked = Some(unit);
                    }
                }
            })
            .response
            .labelled_by(label.id);
        if state.custom_unit {
            ui.add(crate::widgets::line(&mut state.unit).desired_width(64.0).char_limit(12)).labelled_by(label.id);
        }
    });
}

/// A measuring tool is picked in `view`: nothing in progress, and no comment left selected from
/// before, which the next Backspace (a vertex here) would delete too (this fork, OV11).
fn pick(view: &mut DocView) {
    view.measure.cancel();
    view.comments.selected = None;
}

pub(crate) fn command(app: &mut PdfCraftApp, id: &str) {
    app.left = LeftPanel::Tool("measure");
    app.left_open = true;
    if let Some((index, _)) = app.active_ids()
        && let Some(tool) = id.strip_prefix("measure.").and_then(Tool::from_name)
    {
        pick(&mut app.views[index]);
        app.quick_tool = QuickTool::Measure(tool);
    }
    if id == "measure.export" {
        export_csv(app);
    }
}
fn export_csv(app: &mut PdfCraftApp) {
    let Some((_, id)) = app.active_ids() else { return };
    let result = app.session.get(id).ok_or_else(|| "no such document".to_string()).and_then(|d| d.measurements());
    match result {
        Err(e) => app.notify(&e),
        Ok(listing) => {
            let csv = measure::csv(&listing.measurements);
            if !listing.unsupported.is_empty() {
                app.notify(crate::i18n::fmt(
                    tl!("{n} measurements use an unsupported format and are left out."),
                    &[("n", &listing.unsupported.len().to_string())],
                ));
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let write = move |app: &mut PdfCraftApp, path: std::path::PathBuf| match crate::editing::write_atomically(
                    &path.to_string_lossy(),
                    csv.as_bytes(),
                ) {
                    Ok(()) => app.notify(tl!("Saved measurements.")),
                    Err(e) => app.notify(e.to_string()),
                };
                match app.export_dir_override.as_ref().map(|d| std::path::PathBuf::from(d).join("measurements.csv")) {
                    Some(path) => write(app, path),
                    None => {
                        let dialog = rfd::AsyncFileDialog::new().set_file_name("measurements.csv").add_filter("CSV", &["csv"]);
                        app.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            if let Err(e) = crate::editing::download("measurements.csv", csv.as_bytes()) {
                app.notify(&e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Apply scale builds from the panel's fields, then a distance read with it.
    fn measured(state: &MeasureView, from: Point, to: Point) -> f64 {
        let page_unit = page_unit_of(state.page_unit, &state.unit, crate::units::Unit::Points);
        let scale = Scale::ratio([0.0, 0.0], [1.0, 0.0], state.drawing_points, page_unit, state.real_distance, &state.unit, 4).expect("scale");
        measure::reading(Kind::Distance, &[from, to], &scale).expect("reading").value
    }

    #[test]
    fn typed_ratios_are_read_leniently() {
        assert_eq!(parse_ratio("1:200"), Some(200.0));
        assert_eq!(parse_ratio(" 1 : 50 "), Some(50.0));
        assert_eq!(parse_ratio("1/20"), Some(20.0));
        assert_eq!(parse_ratio("250"), Some(250.0));
        assert_eq!(parse_ratio("1:2,5"), Some(2.5));
        assert_eq!(parse_ratio("2:1"), Some(0.5));
        assert_eq!(parse_ratio("1:0.001"), Some(1e-3));
    }

    #[test]
    fn invalid_ratios_are_refused() {
        for bad in [
            "", " ", ":", "1:", ":50", "1:0", "0", "-5", "1:-5", "0:5", "abc", "1:abc", "1:50:2", "1:inf", "NaN", "1e12", "1:1e12", "1:0.0001",
            "1:1e-6",
        ] {
            assert_eq!(parse_ratio(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn ratios_are_written_as_people_say_them() {
        assert_eq!(format_ratio(50.0), "1:50");
        assert_eq!(format_ratio(2.5), "1:2.5");
        assert_eq!(format_ratio(0.5), "1:0.5");
        // Nothing is rounded away: the field reads the ratio that applies.
        assert_eq!(format_ratio(1.234), "1:1.234");
        assert_eq!(format_ratio(0.001), "1:0.001");
        assert_eq!(parse_ratio(&format_ratio(333.333333)), Some(333.333333));
        for n in METRIC {
            assert_eq!(parse_ratio(&format_ratio(n)), Some(n));
        }
    }

    /// A metric common scale keeps a metric unit and turns any other into metres (this fork,
    /// MEASURE.2; upstream switched every unit to metres).
    #[test]
    fn every_metric_preset_reads_paper_length_times_n_in_a_metric_unit() {
        for (unit, shown) in [("mm", "mm"), ("cm", "cm"), ("m", "m"), ("km", "m"), ("in", "m"), ("ft", "m")] {
            for n in METRIC {
                let mut state = MeasureView { unit: unit.into(), custom_unit: unit == "km", ..Default::default() };
                assert!(set_metric(&mut state, n));
                assert_eq!(state.unit, shown);
                // 72 pt is one inch of paper: 25.4 mm x N, in the unit shown.
                let v = measured(&state, [0.0, 0.0], [0.0, 72.0]);
                let expected = LengthUnit::from_label(shown).expect("unit").convert(25.4 * n, LengthUnit::Millimeters);
                assert!((v - expected).abs() < 1e-9 * expected.max(1.0), "1:{n} in {unit} gave {v}, wanted {expected}");
            }
        }
    }

    #[test]
    fn any_starting_unit_becomes_metres() {
        let mut state = MeasureView { unit: "yards?".into(), ..Default::default() };
        assert!(set_ratio(&mut state, 50.0));
        assert_eq!(state.unit, "m");
        // 1 mm of paper at 1:50 is 50 mm.
        let v = measured(&state, [0.0, 0.0], [72.0 / 25.4, 0.0]);
        assert!((v - 0.05).abs() < 1e-9, "{v}");
    }

    #[test]
    fn the_smallest_ratio_keeps_the_real_distance_within_its_field() {
        let mut state = MeasureView::default();
        assert!(set_ratio(&mut state, parse_ratio("1:0.001").expect("ratio")));
        // The real-distance field accepts 0.000001 and no less.
        assert!(state.real_distance >= 0.000001, "{}", state.real_distance);
        assert!(!set_ratio(&mut state, 1e-4));
    }

    #[test]
    fn the_ratio_field_follows_the_scale_fields() {
        let display = crate::units::Unit::Points;
        let mut state = MeasureView { unit: "m".into(), ..Default::default() };
        for n in [50.0, 2.5, 1.234, 0.001, 1e9] {
            assert!(set_ratio(&mut state, n));
            sync_ratio_text(&mut state, display);
            assert_eq!(state.ratio_text, format_ratio(n), "1:{n}");
        }
        // A hand edit that is still a drawing scale reads as its ratio, in another page unit too.
        assert!(set_ratio(&mut state, 50.0));
        state.page_unit = Some(PageUnit::Cm);
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "1:5");
        state.page_unit = Some(PageUnit::Mm);
        state.real_distance = 0.1;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "1:100");
        // Any listed unit has a ratio (this fork, MEASURE.2): 0.1 ft on 1 mm is 1:30.48.
        state.unit = "ft".into();
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "1:30.48");
        // One that isn't (a Custom unit, or no ratio at all) empties it.
        state.custom_unit = true;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "");
        state.custom_unit = false;
        state.unit = "m".into();
        state.real_distance = 0.1 / 3.0;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "");
        state.drawing_points = 0.0;
        sync_ratio_text(&mut state, display);
        assert_eq!(state.ratio_text, "");
    }

    /// An imperial common scale is applied in the imperial unit shown, else in feet and inches:
    /// never in SI units (this fork, MEASURE.2). One inch of paper measures N inches.
    #[test]
    fn every_imperial_preset_applies_its_ratio_in_an_imperial_unit() {
        let display = crate::units::Unit::Points;
        for (unit, custom, shown, feet_inches) in [
            ("in", false, "in", false),
            ("ft", false, "ft", false),
            ("m", false, "ft", true),
            ("mm", false, "ft", true),
            ("yards?", true, "ft", true),
        ] {
            for preset in IMPERIAL {
                let mut state = MeasureView { unit: unit.into(), custom_unit: custom, ..Default::default() };
                set_imperial(&mut state, preset);
                assert_eq!((state.unit.as_str(), state.feet_inches, state.custom_unit), (shown, feet_inches, false), "{} from {unit}", preset.label);
                assert_eq!(ratio_of_fields(&state, display), Some(preset.ratio()), "{} from {unit}", preset.label);
                assert_eq!(state.ratio_text, format_ratio(preset.ratio()));
                assert_eq!(state.ratio_label.as_deref(), Some(preset.label));
                let v = measured(&state, [0.0, 0.0], [72.0, 0.0]);
                let expected = LengthUnit::from_label(shown).expect("unit").convert(preset.ratio(), LengthUnit::Inches);
                assert!((v - expected).abs() < 1e-9 * expected, "{} from {unit}: {v}, wanted {expected}", preset.label);
            }
        }
        // Feet and inches also feet-and-inches text the panel shows.
        let mut state = MeasureView { unit: "ft".into(), feet_inches: true, ..Default::default() };
        set_imperial(&mut state, IMPERIAL[8]);
        assert_eq!(state.known_text, "1'-0\"");
    }

    /// 1/4" = 1'-0" is 1:48 and 1/8" = 1'-0" is 1:96: an inch of paper is 4 ft and 8 ft.
    #[test]
    fn quarter_and_eighth_inch_scales_measure_an_inch_of_paper_as_4_and_8_feet() {
        for (label, feet, n) in [("1/4\" = 1'-0\"", 4.0, 48.0), ("1/8\" = 1'-0\"", 8.0, 96.0)] {
            let preset = measure::presets::imperial_named(label).expect("preset");
            let mut state = MeasureView { unit: "ft".into(), feet_inches: true, ..Default::default() };
            set_imperial(&mut state, preset);
            assert_eq!(ratio_of_fields(&state, crate::units::Unit::Points), Some(n));
            assert!((measured(&state, [0.0, 0.0], [0.0, 72.0]) - feet).abs() < 1e-12, "{label}");
        }
    }

    /// The label of a scale typed or picked is what Apply saves, until a hand edit or a
    /// calibration; Escape on a calibration gives it back with the fields.
    #[test]
    fn a_picked_scale_keeps_its_label_through_an_escaped_calibration() {
        let mut state = MeasureView { unit: "ft".into(), ..Default::default() };
        set_imperial(&mut state, IMPERIAL[8]);
        assert_eq!(state.ratio_label.as_deref(), Some("1/4\" = 1'-0\""));
        state.pending_calibration = Some(state.calibration_before());
        state.drawing_points = 3.0;
        state.ratio_label = None;
        assert!(state.cancel_calibration());
        assert_eq!(state.ratio_label.as_deref(), Some("1/4\" = 1'-0\""));
        assert_eq!(state.drawing_points, 0.25);
        assert!(set_metric(&mut state, 50.0));
        assert_eq!((state.unit.as_str(), state.ratio_label.as_deref()), ("m", Some("1:50")));
    }

    #[test]
    fn an_unusable_ratio_changes_nothing() {
        let mut state = MeasureView::default();
        let before = (state.drawing_points, state.real_distance, state.unit.clone(), state.page_unit);
        for n in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(!set_ratio(&mut state, n));
        }
        assert_eq!((state.drawing_points, state.real_distance, state.unit.clone(), state.page_unit), before);
    }
}

//! Several native windows in one process (this fork, TAB.WINDOW.1 W2a): the windows besides the
//! main one, the guarded pass that draws each, the checks a window's geometry passes before it
//! is created, and what happens when a window can't open.
//!
//! The main window is eframe's root viewport and keeps its state in [`PdfCraftApp::win`]. Every
//! other window is a [`SecondaryWindow`] in [`PdfCraftApp::windows`], drawn in an immediate
//! viewport during the main window's pass: its [`WindowUi`] is swapped into `win` for its pass,
//! so the code that draws a window keeps reading `app.win`, and swapped back after it.
//!
//! W2a is the architecture only. No command creates a window yet (W2b), so the app behaves as
//! before: [`Windows::others`] stays empty outside tests.
//!
//! Safety rules from the W0 spike (`w0-spike/W0-REPORT.md` §7, §11), each with a test here:
//! - A panic while a window draws is caught **inside** its viewport closure. Caught outside,
//!   egui's nested pass stays open and the next frame panics outside any guard (W0-7 (b)).
//! - A window's size and position are checked before the window is created: a size the
//!   platform can't make crashes the process in eframe's retry, outside app code (W0-7 (a)).
//! - A window the platform refuses is recognised because its closure never ran (a flag this
//!   module sets), never by reading a panic message. Its tabs go back where they came from.

use egui::{Pos2, Rect, Vec2, ViewportBuilder, ViewportClass, ViewportId};

use crate::PdfCraftApp;
use crate::canvas::DocView;
use crate::window_ui::WindowUi;

/// A window's identity for the life of the process; never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// The egui viewport that shows this window. The main window is always `ViewportId::ROOT`.
    pub fn viewport(self) -> ViewportId {
        ViewportId::from_hash_of(("pdfcraft-window", self.0))
    }
}

/// Where a window's tabs came from, so a window that can't open gives them back there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    /// `None`: the main window.
    pub window: Option<WindowId>,
    /// The tab position they came from in that window.
    pub index: usize,
}

/// A window other than the main one.
pub struct SecondaryWindow {
    pub id: WindowId,
    pub ui: WindowUi,
    /// Checked by [`validate`] when the window was made.
    pub geometry: Geometry,
    pub came_from: Option<Origin>,
    /// Passes in a row that panicked; at [`MAX_PANICKING_PASSES`] the window's tabs go back.
    pub(crate) panicking_passes: u32,
}

impl SecondaryWindow {
    fn builder(&self) -> ViewportBuilder {
        let title = if self.ui.window_title.is_empty() { "PDF Overlay" } else { self.ui.window_title.as_str() };
        let builder = ViewportBuilder::default().with_title(title).with_inner_size(self.geometry.size);
        match self.geometry.pos {
            Some(pos) => builder.with_position(pos),
            None => builder,
        }
    }
}

/// The windows besides the main one.
#[derive(Default)]
pub struct Windows {
    pub others: Vec<SecondaryWindow>,
    next: u64,
}

impl Windows {
    /// Add a window holding `ui`, at `geometry` (already checked by [`validate`]).
    pub fn add(&mut self, ui: WindowUi, geometry: Geometry, came_from: Option<Origin>) -> WindowId {
        self.next = self.next.saturating_add(1);
        let id = WindowId(self.next);
        self.others.push(SecondaryWindow { id, ui, geometry, came_from, panicking_passes: 0 });
        id
    }
}

/// A window panicking in this many passes in a row is given up: its tabs go back (unlike the main
/// window, which has nowhere to give them, it needn't end the app).
pub(crate) const MAX_PANICKING_PASSES: u32 = 3;

// ---------------------------------------------------------------------------------- geometry

/// Smallest side of a window, in points.
pub const MIN_SIDE_PT: f32 = 100.0;
/// Largest side of a window, in physical pixels: X11 stores sizes in 16 bits, and the W0 spike
/// crashed eframe with larger ones (W0-7 (a)).
pub const MAX_SIDE_PX: f32 = 16384.0;
/// Farthest a window's corner may be from the desktop origin, in physical pixels.
pub const MAX_COORD_PX: f32 = 16000.0;
/// How far a new window sits from the window it came from (O-3), in physical pixels.
pub const OFFSET_PX: f32 = 32.0;
/// The size used when a request has none that can be used.
const FALLBACK_SIZE: Vec2 = Vec2::new(900.0, 700.0);

/// What is known about the display a window goes on.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Physical pixels per point on that display.
    pub ppp: f32,
    /// The monitor's size in points (egui's `ViewportInfo::monitor_size`).
    pub monitor: Option<Vec2>,
    /// The monitor's usable area in desktop points, when the platform says. egui 0.37 reports a
    /// monitor's size but not where it is or which part is usable, so this is often `None`.
    pub usable: Option<Rect>,
}

/// A window's checked position and inner size, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// `None` leaves the position to the platform.
    pub pos: Option<Pos2>,
    pub size: Vec2,
}

fn finite_positive(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite() && *v > 0.0)
}

/// Make a requested position and size safe to create a window with: finite numbers, sides between
/// [`MIN_SIDE_PT`] and the smaller of the monitor and [`MAX_SIDE_PX`], a corner within
/// [`MAX_COORD_PX`] of the desktop origin and, when the usable area is known, inside it. Whatever
/// comes in, what comes out can be given to `ViewportBuilder`.
pub fn validate(pos: Option<Pos2>, size: Vec2, limits: Limits) -> Geometry {
    let ppp = finite_positive(Some(limits.ppp)).unwrap_or(1.0);
    let usable = limits.usable.filter(|r| r.min.is_finite() && r.max.is_finite() && r.width() > 0.0 && r.height() > 0.0);
    let mut max = Vec2::splat(MAX_SIDE_PX / ppp);
    if let Some(m) = limits.monitor {
        max.x = finite_positive(Some(m.x)).map_or(max.x, |w| w.min(max.x));
        max.y = finite_positive(Some(m.y)).map_or(max.y, |h| h.min(max.y));
    }
    if let Some(u) = usable {
        max = max.min(u.size());
    }
    let side = |want: f32, fallback: f32, max: f32| {
        let want = if want.is_finite() && want > 0.0 { want } else { fallback };
        // A display smaller than the minimum gets a window as large as the display.
        want.clamp(MIN_SIDE_PT.min(max), max.max(1.0))
    };
    let size = Vec2::new(side(size.x, FALLBACK_SIZE.x, max.x), side(size.y, FALLBACK_SIZE.y, max.y));
    let reach = MAX_COORD_PX / ppp;
    let pos = pos.filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| {
        let p = Pos2::new(p.x.clamp(-reach, reach), p.y.clamp(-reach, reach));
        match usable {
            Some(u) => Pos2::new(p.x.clamp(u.min.x, (u.max.x - size.x).max(u.min.x)), p.y.clamp(u.min.y, (u.max.y - size.y).max(u.min.y))),
            None => p,
        }
    });
    Geometry { pos, size }
}

/// Where a window made from `source` goes (O-3): [`OFFSET_PX`] right of and below the source
/// window's outer corner, on the same monitor, checked by [`validate`].
pub fn beside(source_outer: Rect, size: Vec2, limits: Limits) -> Geometry {
    let ppp = finite_positive(Some(limits.ppp)).unwrap_or(1.0);
    validate(Some(source_outer.min + Vec2::splat(OFFSET_PX / ppp)), size, limits)
}

// ------------------------------------------------------------------------------ the passes

/// Shows an immediate viewport. The app uses egui's; tests use their own, to make a window that
/// opens, one the platform refuses, and one whose framework fails.
pub(crate) trait ViewportHost {
    /// No native windows here (wasm, the test harness, a backend without them): the app then
    /// keeps every tab in the main window.
    fn embedded(&self) -> bool;
    /// Show `id`; call `body` with the window's `Ui` if the window is there.
    fn show(&mut self, id: ViewportId, builder: ViewportBuilder, body: &mut dyn FnMut(&mut egui::Ui, ViewportClass));
}

impl ViewportHost for egui::Context {
    fn embedded(&self) -> bool {
        self.embed_viewports()
    }

    fn show(&mut self, id: ViewportId, builder: ViewportBuilder, body: &mut dyn FnMut(&mut egui::Ui, ViewportClass)) {
        self.show_viewport_immediate(id, builder, |ui, class| body(ui, class));
    }
}

/// How one window's pass went.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PassOutcome {
    Shown,
    /// The user (or the platform) asked to close the window.
    CloseRequested,
    /// The window's drawing panicked; caught inside its closure, so egui's pass ended normally.
    Panicked(String),
    /// The closure never ran: the platform didn't make the window (W0-7 (c1)).
    NotOpened,
    /// The closure ran, then something outside it failed (egui or the backend).
    FrameworkFailed(String),
}

impl PdfCraftApp {
    /// After the main window's pass: draw every other window, each in its own guarded pass, and
    /// give back the tabs of a window that can't be shown. With no native windows (`embedded`),
    /// every window's tabs come back to the main window and none is shown.
    pub(crate) fn show_other_windows(&mut self, host: &mut dyn ViewportHost) {
        if self.windows.others.is_empty() {
            return;
        }
        let mut give_back = Vec::new();
        if host.embedded() {
            give_back = std::mem::take(&mut self.windows.others);
        } else {
            let mut keep = Vec::new();
            for mut window in std::mem::take(&mut self.windows.others) {
                match self.window_pass(host, &mut window) {
                    PassOutcome::Shown => {
                        window.panicking_passes = 0;
                        keep.push(window);
                    }
                    PassOutcome::CloseRequested => give_back.push(window),
                    PassOutcome::Panicked(message) => {
                        log::error!("drawing a window panicked and was caught: {message}");
                        window.panicking_passes = window.panicking_passes.saturating_add(1);
                        self.after_window_panic(&message);
                        if window.panicking_passes >= MAX_PANICKING_PASSES {
                            give_back.push(window);
                        } else {
                            keep.push(window);
                        }
                    }
                    PassOutcome::NotOpened => {
                        log::error!("a window could not be opened; its tabs go back");
                        self.notify_fmt("A new window could not be opened. Its tabs are back where they were.", &[]);
                        give_back.push(window);
                    }
                    PassOutcome::FrameworkFailed(message) => {
                        log::error!("a window failed outside its drawing: {message}");
                        self.notify_fmt("A window stopped working ({m}). Its tabs are back where they were.", &[("m", &message)]);
                        give_back.push(window);
                    }
                }
            }
            // Windows added while the others were drawn (none in W2a) follow the ones kept.
            keep.append(&mut self.windows.others);
            self.windows.others = keep;
        }
        for window in give_back {
            self.give_back(window);
        }
    }

    /// One window's pass. Its state is swapped into `win` and back inside the viewport closure,
    /// under a guard of its own, so a panic while drawing it neither escapes egui's pass nor leaves
    /// its state in the main window's place.
    fn window_pass(&mut self, host: &mut dyn ViewportHost, window: &mut SecondaryWindow) -> PassOutcome {
        let builder = window.builder();
        let id = window.id;
        let mut ran = false;
        let mut inner: Option<String> = None;
        let mut close = false;
        let shown = pdfcraft_engine::guard(|| {
            host.show(id.viewport(), builder, &mut |ui, _class| {
                ran = true;
                std::mem::swap(&mut self.win, &mut window.ui);
                self.drawing_window = Some(id);
                let result = pdfcraft_engine::guard(|| self.draw_window(ui));
                self.drawing_window = None;
                std::mem::swap(&mut self.win, &mut window.ui);
                if let Err(message) = result {
                    inner = Some(message);
                }
                close = ui.ctx().input(|i| i.viewport().close_requested());
            });
        });
        match (shown, ran) {
            // Whether egui then panicked or returned, the window isn't there: no message needed.
            (_, false) => PassOutcome::NotOpened,
            (Err(message), true) => PassOutcome::FrameworkFailed(message),
            (Ok(()), true) => match inner {
                Some(message) => PassOutcome::Panicked(message),
                None if close => PassOutcome::CloseRequested,
                None => PassOutcome::Shown,
            },
        }
    }

    /// Draw the window whose state is in `win` now. The main window's pass and every other
    /// window's run the same code; the parts the whole app does once per frame (dialogs, jobs,
    /// pending edits, the control channel) run in the main window's pass only.
    pub(crate) fn draw_window(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        self.injected_window_panic();
        self.ui_unguarded(ui);
    }

    /// `true` while another window than the main one is being drawn.
    pub(crate) fn drawing_other_window(&self) -> bool {
        self.drawing_window.is_some()
    }

    /// After a window's pass panicked: keep the work (snapshot unsaved documents now) and say so.
    fn after_window_panic(&mut self, message: &str) {
        if let Err(e) = pdfcraft_engine::guard(|| self.autosave_now()) {
            log::error!("snapshotting unsaved documents after a window panic failed: {e}");
        }
        self.notify_fmt("That didn't work: an internal error stopped it ({m}).", &[("m", message)]);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Give a window's tabs back to the window they came from, at the position they left, with
    /// the first of them active; to the end of the main window if that window is gone.
    pub(crate) fn give_back(&mut self, mut window: SecondaryWindow) {
        let views: Vec<DocView> = std::mem::take(&mut window.ui.views);
        if views.is_empty() {
            return;
        }
        let origin = window.came_from;
        let target = match origin.and_then(|o| o.window) {
            Some(id) => self.windows.others.iter_mut().find(|w| w.id == id).map(|w| &mut w.ui),
            None => None,
        };
        let (ui, at) = match target {
            Some(ui) => (ui, origin.map_or(usize::MAX, |o| o.index)),
            None if origin.is_some_and(|o| o.window.is_none()) => (&mut self.win, origin.map_or(usize::MAX, |o| o.index)),
            None => (&mut self.win, usize::MAX),
        };
        let at = at.min(ui.views.len());
        ui.views.splice(at..at, views);
        ui.active = Some(at);
    }
}

/// Tests: drawing another window panics while `panic_in_other_window` is set.
#[cfg(test)]
impl PdfCraftApp {
    fn injected_window_panic(&self) {
        if self.drawing_window.is_some() && self.panic_in_other_window {
            panic!("injected panic in another window");
        }
    }
}

#[cfg(test)]
#[path = "multi_window/tests.rs"]
mod tests;

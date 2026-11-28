//! Several native windows in one process (this fork, TAB.WINDOW.1 W2a): the windows besides the
//! main one, the guarded pass that draws each, the checks a window's geometry passes before it
//! is created, and what happens when a window can't open.
//!
//! The main window is eframe's root viewport and keeps its state in [`PdfCraftApp::win`]. Every
//! other window is a [`SecondaryWindow`] in [`PdfCraftApp::windows`], drawn in an immediate
//! viewport during the main window's pass: its [`WindowUi`] is swapped into `win` for its pass,
//! so the code that draws a window keeps reading `app.win`, and swapped back after it.
//!
//! W2b adds the commands that make and arrange windows (Move Tab to New Window, Move Tab to
//! Window, Merge All Windows, Close Window), off by default behind the "Keep all documents in
//! one window" preference.
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
    /// Where the window was at its last pass, in physical pixels: what the first window takes
    /// over when this one replaces it (H1).
    pub(crate) last: Option<Placement>,
    /// Opens maximized (a window restored as it was, W4).
    pub(crate) maximized: bool,
}

/// A window's place on the desktop, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub outer: Rect,
    pub inner_size: Vec2,
    pub maximized: bool,
}

impl SecondaryWindow {
    fn builder(&self, template: Option<&ViewportBuilder>) -> ViewportBuilder {
        let title = if self.ui.window_title.is_empty() { "PDF Overlay" } else { self.ui.window_title.as_str() };
        let mut builder = template.cloned().unwrap_or_default().with_title(title).with_inner_size(self.geometry.size);
        if self.maximized {
            builder = builder.with_maximized(true);
        }
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
    /// The documents each window had on screen at its last pass: a document another window shows
    /// keeps its render queue and results (`finish_render_frame`).
    pub(crate) on_screen: Vec<(WindowKey, Vec<pdfcraft_engine::DocId>)>,
    /// Window changes asked for, carried out between passes.
    pub(crate) pending: Vec<WindowOp>,
    /// Documents whose hidden tab in one window let go of render requests that the tab showing
    /// them in another window must queue again at its next pass.
    pub(crate) requeue: Vec<pdfcraft_engine::DocId>,
    /// The main window's settings (decorations, icon, minimum size) for the others.
    pub template: Option<ViewportBuilder>,
    /// The window the open dialog or question shows in (T-3), once one is open: the window it was
    /// opened from, or the one holding the document a question is about. `None` while none is.
    pub(crate) dialog_window: Option<WindowKey>,
    /// The windows by when they last had the keyboard, most recent last.
    pub(crate) focus_order: Vec<WindowKey>,
    /// The first window's physical pixels per point, at its last pass.
    pub(crate) root_ppp: f32,
    /// A tab being dragged out of its window (W3), while it is.
    pub(crate) drag: Option<CrossDrag>,
}

impl Windows {
    /// Add a window holding `ui`, at `geometry` (already checked by [`validate`]).
    pub fn add(&mut self, ui: WindowUi, geometry: Geometry, came_from: Option<Origin>) -> WindowId {
        self.next = self.next.saturating_add(1);
        let id = WindowId(self.next);
        self.others.push(SecondaryWindow { id, ui, geometry, came_from, panicking_passes: 0, last: None, maximized: false });
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

/// A window: the main one (`None`) or another.
pub type WindowKey = Option<WindowId>;

mod drag;
mod restore;
pub use drag::set_tracking as set_drag_tracking;
pub(crate) use drag::{CrossDrag, WindowFrame, marker_x};
pub(crate) use restore::Restore;

/// What an open of a file another window shows asks for there ([`WindowOp::Reveal`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reopen {
    /// Page layout and continuous scrolling (`--layout` on the command line, a launch display).
    pub(crate) display: Option<(crate::canvas::PageLayout, bool)>,
    /// Where to show it (`file.pdf#page=3`, a link into it).
    pub(crate) destination: Option<pdfcraft_render::RemoteDestination>,
    /// The tool it was opened for.
    pub(crate) then: Option<&'static str>,
}

/// A change to which window shows what. It can be asked for at any time (a menu, a key, a tab's
/// menu, in any window's pass) and is carried out between passes, when no window's state is
/// swapped into `win`, so no pass ever sees the list of windows change under it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WindowOp {
    /// Move Tab to New Window: the tab with view key `view` leaves window `from`.
    MoveToNew { from: WindowKey, view: crate::canvas::ViewKey, geometry: Geometry },
    /// Move Tab to Window: the tab goes to window `to`'s strip, before its tab `at` (W3: where it
    /// was dropped), or at the end.
    MoveTo { from: WindowKey, view: crate::canvas::ViewKey, to: WindowKey, at: Option<usize> },
    /// Merge All Windows: every tab into the main window.
    MergeAll,
    /// Close Window.
    Close(WindowKey),
    /// Bring a window to the front.
    Focus(WindowKey),
    /// A file opened again that window `window`'s tab `view` already shows: that tab becomes the
    /// window's active one, in the display, at the destination and with the tool the open asked
    /// for, and the window comes to the front.
    Reveal { window: WindowKey, view: crate::canvas::ViewKey, reopen: Box<Reopen> },
    /// Run a command in the main window, once the changes before it are done (Close all from
    /// another window: every tab joins the main window first, which then closes them as always).
    Run(&'static str),
    /// Quit, asked for in another window: every tab joins the main window, which then asks
    /// about unsaved work as it always has.
    Quit,
}

impl PdfCraftApp {
    /// The window whose state is in `win` now: the main window unless another one's pass (or an
    /// operation on it) is running.
    pub(crate) fn current_window(&self) -> WindowKey {
        self.drawing_window
    }

    /// Every window's state, the main window first. While another window's state is in `win`,
    /// that window's slot in the list holds the main window's.
    pub(crate) fn window_uis(&self) -> Vec<(WindowKey, &WindowUi)> {
        let current = self.drawing_window;
        let mut all = vec![(current, &self.win)];
        all.extend(self.windows.others.iter().map(|w| (if Some(w.id) == current { None } else { Some(w.id) }, &w.ui)));
        all.sort_by_key(|(key, _)| key.is_some());
        all
    }

    /// [`Self::window_uis`], to change them.
    pub(crate) fn window_uis_mut(&mut self) -> Vec<(WindowKey, &mut WindowUi)> {
        let current = self.drawing_window;
        let mut all = vec![(current, &mut self.win)];
        all.extend(self.windows.others.iter_mut().map(|w| (if Some(w.id) == current { None } else { Some(w.id) }, &mut w.ui)));
        all.sort_by_key(|(key, _)| key.is_some());
        all
    }

    /// Whether a window other than the current one has a tab on document `id`.
    pub(crate) fn viewed_in_other_window(&self, id: pdfcraft_engine::DocId) -> bool {
        let current = self.drawing_window;
        self.window_uis().iter().any(|(key, ui)| *key != current && ui.views.iter().any(|v| v.id == id))
    }

    /// Note what the current window has on screen, for the other windows' passes.
    pub(crate) fn note_on_screen(&mut self, docs: Vec<pdfcraft_engine::DocId>) {
        let current = self.drawing_window;
        match self.windows.on_screen.iter_mut().find(|(key, _)| *key == current) {
            Some((_, shown)) => *shown = docs,
            None => self.windows.on_screen.push((current, docs)),
        }
    }

    /// Run `f` with window `key`'s state in `win`, as its pass does, under a guard. `None` when
    /// there is no such window, `f` panicked, or another window's state is in `win` already.
    pub(crate) fn with_window<R>(&mut self, key: WindowKey, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if self.drawing_window.is_some() {
            return None;
        }
        let Some(id) = key else {
            return pdfcraft_engine::guard(|| f(self)).map_err(|m| log::error!("a window operation panicked: {m}")).ok();
        };
        let i = self.windows.others.iter().position(|w| w.id == id)?;
        std::mem::swap(&mut self.win, &mut self.windows.others[i].ui);
        self.drawing_window = Some(id);
        let result = pdfcraft_engine::guard(|| f(self));
        self.drawing_window = None;
        if let Some(w) = self.windows.others.get_mut(i) {
            std::mem::swap(&mut self.win, &mut w.ui);
        }
        result.map_err(|m| log::error!("a window operation panicked: {m}")).ok()
    }

    /// Ask for a change to the windows; it happens between passes ([`Self::apply_window_ops`]).
    pub(crate) fn request_window_op(&mut self, op: WindowOp) {
        self.windows.pending.push(op);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// After the main window's pass: carry out the window changes asked for, then draw every other
    /// window, each in its own guarded pass, and give back the tabs of a window that can't be
    /// shown. Where several windows aren't available (no native windows, the single-window
    /// preference, macOS for now), every tab comes back to the main window and none is shown.
    pub(crate) fn show_other_windows(&mut self, host: &mut dyn ViewportHost) {
        // The last session's windows, once their files are open (W4); none where windows are
        // embedded, which keeps every tab in the main window.
        if host.embedded() {
            self.restoring = None;
        }
        self.restore_windows();
        self.apply_window_ops();
        if !self.windows.others.is_empty() && (host.embedded() || self.windows_unavailable().is_some()) {
            self.merge_all_windows();
        }
        let ids: Vec<WindowId> = self.windows.others.iter().map(|w| w.id).collect();
        let mut failed = Vec::new();
        for id in ids {
            match self.window_pass(host, id) {
                PassOutcome::Shown => {
                    if let Some(w) = self.windows.others.iter_mut().find(|w| w.id == id) {
                        w.panicking_passes = 0;
                    }
                }
                PassOutcome::CloseRequested => self.request_window_op(WindowOp::Close(Some(id))),
                PassOutcome::Panicked(message) => {
                    log::error!("drawing a window panicked and was caught: {message}");
                    self.after_window_panic(&message);
                    if let Some(w) = self.windows.others.iter_mut().find(|w| w.id == id) {
                        w.panicking_passes = w.panicking_passes.saturating_add(1);
                        if w.panicking_passes >= MAX_PANICKING_PASSES {
                            failed.push(id);
                        }
                    }
                }
                PassOutcome::NotOpened => {
                    log::error!("a window could not be opened; its tabs go back");
                    self.notify_fmt("A new window could not be opened. Its tabs are back where they were.", &[]);
                    failed.push(id);
                }
                PassOutcome::FrameworkFailed(message) => {
                    log::error!("a window failed outside its drawing: {message}");
                    self.notify_fmt("A window stopped working ({m}). Its tabs are back where they were.", &[("m", &message)]);
                    failed.push(id);
                }
            }
        }
        for id in failed {
            if let Some(i) = self.windows.others.iter().position(|w| w.id == id) {
                let window = self.windows.others.remove(i);
                self.give_back(window);
            }
        }
        self.apply_window_ops();
    }

    /// One window's pass. Its state is swapped with the main window's inside the viewport closure,
    /// under a guard of its own, so a panic while drawing it neither escapes egui's pass nor leaves
    /// its state in the main window's place.
    fn window_pass(&mut self, host: &mut dyn ViewportHost, id: WindowId) -> PassOutcome {
        let Some(window) = self.windows.others.iter().find(|w| w.id == id) else { return PassOutcome::Shown };
        let builder = window.builder(self.windows.template.as_ref());
        let mut ran = false;
        let mut inner: Option<String> = None;
        let mut close = false;
        let shown = pdfcraft_engine::guard(|| {
            host.show(id.viewport(), builder, &mut |ui, _class| {
                ran = true;
                // Window operations wait until all passes are over, so its place is unchanged.
                let Some(i) = self.windows.others.iter().position(|w| w.id == id) else { return };
                std::mem::swap(&mut self.win, &mut self.windows.others[i].ui);
                self.drawing_window = Some(id);
                let result = pdfcraft_engine::guard(|| self.draw_window(ui));
                self.drawing_window = None;
                std::mem::swap(&mut self.win, &mut self.windows.others[i].ui);
                if let Err(message) = result {
                    inner = Some(message);
                }
                close = ui.ctx().input(|i| i.viewport().close_requested());
                let (placement, focused) = placement_now(ui.ctx());
                if let Some(w) = self.windows.others.get_mut(i) {
                    w.last = placement.or(w.last);
                }
                if focused {
                    self.note_focus(Some(id));
                }
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
        // A dialog open in another window holds this one (T-3).
        let blocked = self.blocked_by_dialog();
        if blocked {
            ui.disable();
        }
        // This window's keyboard, before anything else reads its keys.
        self.window_keys(ui.ctx());
        self.ui_unguarded(ui);
        if blocked {
            self.dialog_elsewhere_notice(ui.ctx());
        }
        self.note_dialog_window();
        // ⌘Q / Ctrl+Q in another window quits as it does in the main one, whose `logic` only reads
        // the main window's keys.
        if self.drawing_other_window() {
            let quit = ui.ctx().input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Q, pressed: true, modifiers, .. } if modifiers.matches_exact(egui::Modifiers::COMMAND)))
            });
            if quit {
                self.request_window_op(WindowOp::Quit);
            }
        }
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
        self.windows.on_screen.retain(|(key, _)| *key != Some(window.id));
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

    /// Every other window's tabs into the main window, after its own, in window order; the other
    /// windows close. No document closes, so nothing is asked.
    pub(crate) fn merge_all_windows(&mut self) {
        if self.drawing_window.is_some() {
            self.request_window_op(WindowOp::MergeAll);
            return;
        }
        for mut window in std::mem::take(&mut self.windows.others) {
            self.windows.on_screen.retain(|(key, _)| *key != Some(window.id));
            let views = std::mem::take(&mut window.ui.views);
            if self.win.active.is_none() && !views.is_empty() {
                self.win.active = Some(self.win.views.len());
            }
            self.win.views.extend(views);
        }
    }

    /// Carry out the window changes asked for since the last time. Only between passes.
    pub(crate) fn apply_window_ops(&mut self) {
        if self.drawing_window.is_some() {
            return;
        }
        for op in std::mem::take(&mut self.windows.pending) {
            match op {
                WindowOp::MoveToNew { from, view, geometry } => self.move_to_new_window(from, view, geometry),
                WindowOp::MoveTo { from, view, to, at } => self.move_to_window(from, view, to, at),
                WindowOp::MergeAll => self.merge_all_windows(),
                WindowOp::Close(key) => self.close_window(key),
                WindowOp::Focus(key) => self.focus_window(key),
                WindowOp::Reveal { window, view, reopen } => self.reveal(window, view, *reopen),
                WindowOp::Run(command) => {
                    let _ = self.execute(command);
                }
                WindowOp::Quit => {
                    if let Some(ctx) = self.ctx.clone() {
                        self.request_quit(&ctx);
                    }
                }
            }
        }
        // A window whose last tab closed (or moved away) closes too.
        let empty: Vec<WindowId> = self.windows.others.iter().filter(|w| w.ui.views.is_empty()).map(|w| w.id).collect();
        for id in empty {
            self.windows.others.retain(|w| w.id != id);
            self.windows.on_screen.retain(|(key, _)| *key != Some(id));
        }
    }

    fn move_to_new_window(&mut self, from: WindowKey, view: crate::canvas::ViewKey, geometry: Geometry) {
        let Some(Some((tab, index, mode))) = self.with_window(from, |app| app.take_tab(view, false)) else { return };
        let mut ui = WindowUi::new();
        ui.mode = mode;
        ui.views.push(tab);
        ui.active = Some(0);
        self.windows.add(ui, geometry, Some(Origin { window: from, index }));
    }

    fn move_to_window(&mut self, from: WindowKey, view: crate::canvas::ViewKey, to: WindowKey, at: Option<usize>) {
        if from == to || (to.is_some() && !self.windows.others.iter().any(|w| Some(w.id) == to)) {
            return;
        }
        let Some(Some((tab, index, _))) = self.with_window(from, |app| app.take_tab(view, from.is_some())) else { return };
        let mut tab = Some(tab);
        self.with_window(to, |app| {
            if let Some(t) = tab.take() {
                let at = at.unwrap_or(usize::MAX).min(app.win.views.len());
                app.win.views.insert(at, t);
                app.win.active = Some(at);
            }
        });
        // The window it was going to went away meanwhile: back where it was.
        if let Some(t) = tab.take() {
            self.with_window(from, |app| {
                let at = index.min(app.win.views.len());
                app.win.views.insert(at, t);
                app.win.active = Some(at);
            });
        }
        self.focus_window(to);
    }

    /// Close Window (W2c). Each document only this window shows asks about its unsaved work, in
    /// this window (Save, Don't Save, Cancel; Cancel leaves the window as it was). Then the window
    /// closes with its tabs; documents other windows show stay open there. The first window, with
    /// others open, hands its place to the one used last (H1); alone, closing it quits (O-4).
    pub(crate) fn close_window(&mut self, key: WindowKey) {
        if key.is_some_and(|id| !self.windows.others.iter().any(|w| w.id == id)) {
            return;
        }
        // One question at a time (T-3): finish the one that is open first.
        if self.modal_open() {
            self.focus_window(self.dialog_window());
            return;
        }
        if key.is_none() && self.windows.others.is_empty() {
            if let Some(ctx) = self.ctx.clone() {
                self.request_quit(&ctx);
            }
            return;
        }
        // A document under OCR review that closes with this window holds the close, as closing
        // its tab does (#6485): nothing closes, and the review comes forward.
        if self.hold_window_close_for_ocr_review(key) {
            return;
        }
        if self.with_window(key, |app| app.first_at_risk()).flatten().is_some() {
            self.ask_in_window(crate::CloseRequest::Window(key), key);
            return;
        }
        self.with_window(key, |app| {
            while let Some(last) = app.win.views.len().checked_sub(1) {
                app.close_tab(last);
            }
        });
        match key {
            Some(id) => {
                self.windows.others.retain(|w| w.id != id);
                self.forget_window(key);
            }
            None => self.rehome_main(),
        }
    }

    /// H1: the first window's tabs are closed and another window is open: that window's tabs,
    /// panes, panels and mode move into the first window, which takes its place and size; the
    /// other window then closes. The process and every other window carry on as they were.
    pub(crate) fn rehome_main(&mut self) {
        let recent = self.windows.focus_order.iter().rev().find_map(|key| key.and_then(|id| self.windows.others.iter().position(|w| w.id == id)));
        let Some(index) = recent.or((!self.windows.others.is_empty()).then_some(0)) else { return };
        let window = self.windows.others.remove(index);
        let id = window.id;
        self.win = window.ui;
        // The titles it last sent went to the other window: send them again to this one.
        self.win.window_title.clear();
        self.win.page_title.clear();
        if let (Some(ctx), Some(placement)) = (&self.ctx, window.last) {
            for command in rehome_commands(placement, self.windows.root_ppp) {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, command);
            }
        }
        // What named that window now names the first one.
        for w in &mut self.windows.others {
            if let Some(origin) = w.came_from.as_mut().filter(|o| o.window == Some(id)) {
                origin.window = None;
            }
        }
        self.windows.on_screen.retain(|(key, _)| key.is_some() && *key != Some(id));
        for key in &mut self.windows.focus_order {
            if *key == Some(id) {
                *key = None;
            }
        }
        if self.windows.dialog_window == Some(Some(id)) {
            self.windows.dialog_window = Some(None);
        }
    }

    /// A window is gone: nothing refers to it any more.
    fn forget_window(&mut self, key: WindowKey) {
        self.windows.on_screen.retain(|(k, _)| *k != key);
        self.windows.focus_order.retain(|k| *k != key);
        if self.windows.dialog_window == Some(key) {
            self.windows.dialog_window = Some(None);
        }
    }

    /// Ask `req`'s question in window `key`, which comes to the front (T-3).
    pub(crate) fn ask_in_window(&mut self, req: crate::CloseRequest, key: WindowKey) {
        self.close_request = Some(req);
        self.windows.dialog_window = Some(key);
        self.focus_window(key);
    }

    /// The first window with a tab whose work isn't saved, the first window first.
    pub(crate) fn first_dirty_window(&self) -> Option<WindowKey> {
        self.window_uis().into_iter().find(|(_, ui)| ui.views.iter().any(|v| self.view_has_unsaved_work(v))).map(|(key, _)| key)
    }

    /// The current window's first tab whose unsaved work would be lost with the window: no tab in
    /// another window shows its document.
    pub(crate) fn first_at_risk(&self) -> Option<usize> {
        (0..self.win.views.len()).find(|&i| self.has_unsaved_work(i) && self.win.views.get(i).is_some_and(|v| !self.viewed_in_other_window(v.id)))
    }

    /// The window the open dialog shows in; the first window when it is gone.
    pub(crate) fn dialog_window(&self) -> WindowKey {
        match self.windows.dialog_window {
            Some(Some(id)) if self.window_uis().iter().any(|(key, _)| *key == Some(id)) => Some(id),
            _ => None,
        }
    }

    /// Whether the open dialog (if any) belongs in the window being drawn.
    pub(crate) fn dialog_here(&self) -> bool {
        self.dialog_window() == self.current_window()
    }

    /// Whether a dialog open in another window blocks this one (T-3).
    pub(crate) fn blocked_by_dialog(&self) -> bool {
        self.modal_open() && !self.dialog_here()
    }

    /// After a window's pass: a dialog that opened during it belongs to this window; with none
    /// open, none does.
    pub(crate) fn note_dialog_window(&mut self) {
        if !self.modal_open() {
            self.windows.dialog_window = None;
        } else if self.windows.dialog_window.is_none() {
            self.windows.dialog_window = Some(self.current_window());
        }
    }

    fn note_focus(&mut self, key: WindowKey) {
        if self.windows.focus_order.last() != Some(&key) {
            self.windows.focus_order.retain(|k| *k != key);
            self.windows.focus_order.push(key);
        }
    }

    /// The first window's pass: its scale and whether it has the keyboard.
    pub(crate) fn note_main_window(&mut self, ctx: &egui::Context) {
        let (ppp, focused) = ctx.input(|i| (i.viewport().native_pixels_per_point.unwrap_or(i.pixels_per_point), i.viewport().focused == Some(true)));
        self.windows.root_ppp = finite_positive(Some(ppp)).unwrap_or(1.0);
        if focused {
            self.note_focus(None);
        }
    }

    /// In a window a dialog elsewhere blocks (T-3): a notice over it that says so and brings the
    /// dialog's window forward. The window's own content is drawn disabled under it.
    pub(crate) fn dialog_elsewhere_notice(&mut self, ctx: &egui::Context) {
        let rect = ctx.content_rect();
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("pdfcraft-dialog-elsewhere-shade"))).rect_filled(
            rect,
            0.0,
            egui::Color32::from_black_alpha(96),
        );
        let mut show = false;
        egui::Area::new(egui::Id::unique("pdfcraft-dialog-elsewhere"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(tl!("A dialog is open in another window."));
                    show = ui.button(tl!("Show it")).clicked();
                });
            });
        if show {
            self.focus_window(self.dialog_window());
        }
    }

    pub(crate) fn focus_window(&mut self, key: WindowKey) {
        let viewport = key.map_or(egui::ViewportId::ROOT, WindowId::viewport);
        if let Some(ctx) = &self.ctx {
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
        }
    }

    /// Why several windows can't be used here, or `None` when they can.
    pub(crate) fn windows_unavailable(&self) -> Option<&'static str> {
        if !self.native_windows_here() {
            Some("Only one window can be used here.")
        } else if cfg!(target_os = "macos") && !self.native_windows_forced() {
            // O-5: until quitting and closing several windows can be told apart there.
            Some("Several windows aren't available on macOS yet.")
        } else if self.keep_one_window {
            Some("Turn off \"Keep all documents in one window\" in Preferences to use several windows.")
        } else {
            None
        }
    }

    /// Whether the platform can make more native windows here (not wasm, not the test harness).
    fn native_windows_here(&self) -> bool {
        self.native_windows_forced() || (!cfg!(target_arch = "wasm32") && self.ctx.as_ref().is_some_and(|c| !c.embed_viewports()))
    }

    /// Tests: native windows stand in for the harness, which has none.
    fn native_windows_forced(&self) -> bool {
        #[cfg(test)]
        return self.force_native_windows;
        #[cfg(not(test))]
        false
    }

    /// The windows a tab can move to from the current one, for the Window menu: the main window
    /// and the others, with a name each.
    pub(crate) fn window_names(&self) -> Vec<(WindowKey, String)> {
        self.window_uis()
            .iter()
            .enumerate()
            .map(|(n, (key, ui))| {
                let doc = ui.active.and_then(|i| ui.views.get(i)).and_then(|v| self.session.get(v.id)).map(|d| d.display_name()).unwrap_or_default();
                let name = crate::i18n::fmt(tl!("Window {n}"), &[("n", &(n + 1).to_string())]);
                (*key, if doc.is_empty() { name } else { format!("{name} — {doc}") })
            })
            .collect()
    }

    /// How many windows there are, the main one included.
    pub(crate) fn window_count(&self) -> usize {
        1 + self.windows.others.len()
    }

    /// The window at `index` in the order of [`Self::window_names`] (the main window first).
    pub(crate) fn window_at(&self, index: usize) -> Option<WindowKey> {
        self.window_uis().get(index).map(|(key, _)| *key)
    }

    /// Why tab `index` of the current window can't move to a new window (`to` is `None`) or to
    /// window `to`; `None` when it can.
    pub(crate) fn move_unavailable(&self, index: usize, to: Option<WindowKey>) -> Option<&'static str> {
        self.move_unavailable_from(self.current_window(), self.win.views.len(), index, to)
    }

    /// [`Self::move_unavailable`] for tab `index` of window `from`, which has `tabs` tabs.
    pub(crate) fn move_unavailable_from(&self, from: WindowKey, tabs: usize, index: usize, to: Option<WindowKey>) -> Option<&'static str> {
        if let Some(reason) = self.windows_unavailable() {
            return Some(reason);
        }
        if index >= tabs {
            return Some("Open a document first");
        }
        match to {
            None if tabs < 2 => Some("A window's only tab can't move to a new window."),
            Some(to) if to == from => Some("The tab is in that window already."),
            Some(Some(id)) if !self.window_uis().iter().any(|(key, _)| *key == Some(id)) => Some("That window is closed."),
            Some(_) if from.is_none() && tabs < 2 => Some("The first window keeps at least one tab."),
            _ => None,
        }
    }

    /// Move Tab to New Window for tab `index` of the current window. `false` (and the user is told
    /// why) when it can't.
    pub(crate) fn request_move_tab_to_new_window(&mut self, index: usize) -> bool {
        if let Some(reason) = self.move_unavailable(index, None) {
            self.notify_tr(reason);
            return false;
        }
        let Some(view) = self.win.views.get(index).map(|v| crate::canvas::ViewKey(v.view_key())) else { return false };
        let geometry = self.new_window_geometry();
        self.request_window_op(WindowOp::MoveToNew { from: self.current_window(), view, geometry });
        true
    }

    /// Move Tab to Window: tab `index` of the current window to the end of window `to`'s strip.
    pub(crate) fn request_move_tab_to_window(&mut self, index: usize, to: WindowKey) -> bool {
        if let Some(reason) = self.move_unavailable(index, Some(to)) {
            self.notify_tr(reason);
            return false;
        }
        let Some(view) = self.win.views.get(index).map(|v| crate::canvas::ViewKey(v.view_key())) else { return false };
        self.request_window_op(WindowOp::MoveTo { from: self.current_window(), view, to, at: None });
        true
    }

    /// A file already open in a tab of another window: that tab becomes the window's active one and
    /// the window comes to the front. `false` when no other window shows it.
    pub(crate) fn focus_tab_in_other_window(&mut self, path: &str) -> bool {
        self.reveal_in_other_window(path, Reopen::default())
    }

    /// [`Self::focus_tab_in_other_window`], applying `reopen` there: the display, destination and
    /// tool the open asked for, as a tab of the current window gets them.
    pub(crate) fn reveal_in_other_window(&mut self, path: &str, reopen: Reopen) -> bool {
        let current = self.drawing_window;
        let session = &self.session;
        let shows = |v: &DocView| session.get(v.id).and_then(|d| d.path.as_deref()).is_some_and(|p| same_path(p, path));
        let found = self
            .window_uis()
            .into_iter()
            .filter(|(key, _)| *key != current)
            .find_map(|(key, ui)| ui.views.iter().find(|v| shows(v)).map(|v| (key, crate::canvas::ViewKey(v.view_key()))));
        let Some((window, view)) = found else { return false };
        self.request_window_op(WindowOp::Reveal { window, view, reopen: Box::new(reopen) });
        true
    }

    /// [`WindowOp::Reveal`], between passes.
    fn reveal(&mut self, window: WindowKey, view: crate::canvas::ViewKey, reopen: Reopen) {
        let shown = self.with_window(window, |app| {
            let Some(i) = app.view_index(view) else { return false };
            app.win.active = Some(i);
            if let Some((layout, continuous)) = reopen.display
                && let Some(v) = app.win.views.get_mut(i)
            {
                v.set_display(layout, continuous);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    app.launch_display_target = Some(v.id);
                }
            }
            if let Some(destination) = reopen.destination {
                app.apply_destination_at(i, destination);
            }
            if let Some(command) = reopen.then {
                app.execute(command);
            }
            true
        });
        if shown == Some(true) {
            self.focus_window(window);
        }
    }

    /// Where a window made from the current one goes (O-3): 32 px from it, on its monitor, inside
    /// that monitor's usable area, the size of the current window.
    pub(crate) fn new_window_geometry(&self) -> Geometry {
        let Some(ctx) = &self.ctx else { return validate(None, FALLBACK_SIZE, Limits { ppp: 1.0, monitor: None, usable: None }) };
        let (outer, inner, ppp, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.native_pixels_per_point.unwrap_or(i.pixels_per_point), v.monitor_size)
        });
        let size = inner.map_or(FALLBACK_SIZE, |r| r.size());
        let ppp = finite_positive(Some(ppp)).unwrap_or(1.0);
        match outer {
            Some(outer) => {
                let area = crate::platform::monitors::monitor_at(outer.center() * ppp);
                beside(outer, size, limits_on(area, ppp, monitor))
            }
            // Wayland: windows can't be placed; the platform picks.
            None => validate(None, size, Limits { ppp, monitor, usable: None }),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn same_path(a: &str, b: &str) -> bool {
    crate::same_file(a, b)
}

#[cfg(target_arch = "wasm32")]
fn same_path(a: &str, b: &str) -> bool {
    a == b
}

/// Where a window is now and whether it has the keyboard, from its viewport's report.
fn placement_now(ctx: &egui::Context) -> (Option<Placement>, bool) {
    ctx.input(|i| {
        let v = i.viewport();
        let ppp = finite_positive(v.native_pixels_per_point).unwrap_or(1.0);
        let placement = v.outer_rect.zip(v.inner_rect).map(|(outer, inner)| Placement {
            outer: Rect::from_min_max((outer.min.to_vec2() * ppp).to_pos2(), (outer.max.to_vec2() * ppp).to_pos2()),
            inner_size: inner.size() * ppp,
            maximized: v.maximized == Some(true),
        });
        (placement, v.focused == Some(true))
    })
}

/// H1: what the first window is told so that it takes `placement` (physical pixels), at its own
/// scale `root_ppp`: position and size in its points, then maximized as the other one was.
pub fn rehome_commands(placement: Placement, root_ppp: f32) -> Vec<egui::ViewportCommand> {
    let ppp = finite_positive(Some(root_ppp)).unwrap_or(1.0);
    let pos = (placement.outer.min.to_vec2() / ppp).to_pos2();
    let size = placement.inner_size / ppp;
    let mut commands = vec![egui::ViewportCommand::Maximized(false)];
    if pos.x.is_finite() && pos.y.is_finite() {
        commands.push(egui::ViewportCommand::OuterPosition(pos));
    }
    if size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0 {
        commands.push(egui::ViewportCommand::InnerSize(size));
    }
    if placement.maximized {
        commands.push(egui::ViewportCommand::Maximized(true));
    }
    commands.push(egui::ViewportCommand::Focus);
    commands
}

/// The limits on the monitor `area` (physical pixels) for a window at `ppp`, in points.
pub fn limits_on(area: Option<crate::platform::monitors::MonitorArea>, ppp: f32, monitor_size: Option<Vec2>) -> Limits {
    let ppp = finite_positive(Some(ppp)).unwrap_or(1.0);
    let points = |r: Rect| Rect::from_min_max((r.min.to_vec2() / ppp).to_pos2(), (r.max.to_vec2() / ppp).to_pos2());
    match area {
        Some(a) => Limits { ppp, monitor: Some(a.bounds.size() / ppp), usable: Some(points(a.work)) },
        None => Limits { ppp, monitor: monitor_size, usable: None },
    }
}

/// An egui id for state that belongs to the window egui is drawing (TAB.WINDOW.1 W2d): egui's
/// `data` is one store for every window, so a fixed id would be shared by all of them. The first
/// window keeps the id it had before there were other windows.
pub(crate) fn window_id(ctx: &egui::Context, name: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    let viewport = ctx.viewport_id();
    if viewport == ViewportId::ROOT { egui::Id::unique(name) } else { egui::Id::unique((name, viewport)) }
}

/// The first window's raw-input duties (`eframe::App::raw_input_hook`), for a window other than
/// the first: eframe calls the app's hook for the first window only, and the app is in the middle
/// of the first window's pass when another window's input arrives, so this needs nothing from the
/// app (TAB.WINDOW.1 W2d). Call it from `eframe::ViewportHooks::raw_input`; the first window's
/// input (`ViewportId::ROOT`) is left alone.
///
/// - The button release a window move or resize kept from egui goes back to this window.
/// - A wheel turned before the pointer moved is placed where the system says the pointer is.
/// - ⌘/Ctrl wheel lines zoom at egui's rate, whatever the scroll speed.
/// - While files are dragged over this window, egui is told where the pointer is.
///
/// The first window's hook also gives the control channel its input generation (the control
/// channel acts in the first window) and the macOS menu its key equivalents (other windows are
/// off on macOS, O-5).
pub fn other_window_raw_input(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let id = raw.viewport_id;
    if id == ViewportId::ROOT {
        return;
    }
    crate::platform::window_controls::release_after_desktop_grab(ctx, raw);
    let viewport = raw.viewports.get(&id).cloned().unwrap_or_default();
    let known = ctx.input_for(id, |i| i.pointer.latest_pos().is_some());
    let size = viewport.inner_rect.map_or(Vec2::ZERO, |r| r.size());
    crate::platform::drag_pointer::locate_wheel(raw, known, size, || crate::platform::drag_pointer::in_viewport(ctx, &viewport));
    crate::scroll_speed::keep_zoom_rate_in(ctx, raw);
    if raw.hovered_files.is_empty() && raw.dropped_files.is_empty() {
        return;
    }
    if let Some(pos) = crate::platform::drag_pointer::in_viewport(ctx, &viewport) {
        raw.events.push(egui::Event::PointerMoved(pos));
    }
    ctx.request_repaint_of(id);
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

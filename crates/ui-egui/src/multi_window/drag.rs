//! TAB.WINDOW.1 W3: dragging a tab out of its window, onto another window's tab strip or onto
//! the desktop (the execution plan's §2.4 state machine, decisions T-1 and T-5).
//!
//! A tab dragged inside its window does what it always did: it moves along the strip, or goes to
//! split view's other pane (`crate::panes`). Once the pointer leaves the window's frame, the
//! drag is between windows:
//! - over another window's tab strip, letting go attaches the tab there, at the marker the
//!   strip draws ([`CrossTarget::Strip`]);
//! - over another window's pages, panels or title bar, letting go does nothing: the tab stays
//!   where it was ([`CrossTarget::Invalid`], the no-drop cursor);
//! - over no window of the app, letting go makes a new window there with the tab
//!   ([`CrossTarget::Desktop`], the move cursor).
//!
//! The move itself is the Window menu's: [`super::WindowOp::MoveTo`] and
//! [`super::WindowOp::MoveToNew`], checked by the same rules ([`PdfCraftApp::move_unavailable_from`]).
//! Escape, the window losing the keyboard, or a dialog opening cancels the drag; nothing moves.
//!
//! Leaving the window never detaches by itself (T-1): only letting go over the desktop does,
//! and only where the windowing system reports the pointer outside the window during a drag
//! (Windows, X11; [`set_tracking`]). Elsewhere (Wayland) the drag stays in its window and the
//! Window menu moves tabs.
//!
//! Positions are compared in desktop physical pixels: each window's points scale by its own pixels
//! per point, so windows on monitors of different scales are hit where they are. Where windows
//! overlap, the one that had the keyboard last is on top.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Pos2, Rect};

use super::WindowKey;
use crate::PdfCraftApp;

/// The windowing system reports the pointer outside a window while a button is held over it
/// (Windows: `SetCapture`; X11: the implicit grab), so a drag can leave its window (W0-4).
static TRACKING: AtomicBool = AtomicBool::new(false);

/// Tell the app whether a drag can follow the pointer out of its window here. The desktop app
/// says so for Windows and X11 windows; Wayland, macOS and the web don't.
pub fn set_tracking(supported: bool) {
    TRACKING.store(supported, Ordering::Relaxed);
}

/// Where a window is and what its tab strip holds, recorded at each of its passes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WindowFrame {
    /// The client area, in desktop points of this window's scale.
    pub(crate) inner: Option<Rect>,
    /// The whole window, title bar and borders included, when the platform says.
    pub(crate) outer: Option<Rect>,
    /// Physical pixels per point.
    pub(crate) ppp: f32,
    /// The tab bar (the strip and its buttons), in window points: a tab dropped on it joins the
    /// window's tabs.
    pub(crate) strip: Option<Rect>,
    /// Its document tabs, in order: each tab's index in the window's views and its rect.
    pub(crate) slots: Vec<(usize, Rect)>,
}

impl WindowFrame {
    fn scale(&self) -> f32 {
        if self.ppp.is_finite() && self.ppp > 0.0 { self.ppp } else { 1.0 }
    }

    /// `r` (desktop points of this window) in desktop physical pixels.
    fn physical(&self, r: Rect) -> Rect {
        Rect::from_min_max((r.min.to_vec2() * self.scale()).to_pos2(), (r.max.to_vec2() * self.scale()).to_pos2())
    }

    /// Whether the desktop pixel `px` is on this window, title bar included.
    fn covers(&self, px: Pos2) -> bool {
        self.outer.or(self.inner).is_some_and(|r| self.physical(r).contains(px))
    }

    /// The desktop pixel `px` in this window's points, when it is over its client area.
    fn local(&self, px: Pos2) -> Option<Pos2> {
        let inner = self.inner?;
        if !self.physical(inner).contains(px) {
            return None;
        }
        Some((px.to_vec2() / self.scale() - inner.min.to_vec2()).to_pos2())
    }

    /// Where this window's pointer at `pos` (window points) is on the desktop, in physical pixels.
    pub(crate) fn desktop(&self, pos: Pos2) -> Option<Pos2> {
        let inner = self.inner?;
        let px = ((inner.min.to_vec2() + pos.to_vec2()) * self.scale()).to_pos2();
        (px.x.is_finite() && px.y.is_finite()).then_some(px)
    }
}

/// What is under a tab dragged out of its window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Under {
    /// Its own window (title bar included): the drag is the window's own.
    Own,
    /// Another window's tab strip: before tab `at` of that window (its index in the window's
    /// views; past the last one, the end).
    Strip { window: WindowKey, at: usize },
    /// Another window, not on its tab strip.
    Window(WindowKey),
    /// No window of the app.
    Desktop,
}

/// What is under the desktop pixel `px` for a tab dragged from window `source`, given every
/// window's frame and the order they last had the keyboard in (`order`, most recent last).
pub(crate) fn under(frames: &[(WindowKey, WindowFrame)], order: &[WindowKey], source: WindowKey, px: Pos2) -> Under {
    if !(px.x.is_finite() && px.y.is_finite()) {
        return Under::Own;
    }
    if frames.iter().any(|(key, f)| *key == source && f.covers(px)) {
        return Under::Own;
    }
    // Most recently focused first; windows never focused after them, in list order.
    let rank = |key: &WindowKey| order.iter().rposition(|k| k == key).map_or(usize::MAX, |p| order.len() - p);
    let mut others: Vec<&(WindowKey, WindowFrame)> = frames.iter().filter(|(key, _)| *key != source).collect();
    others.sort_by_key(|(key, _)| rank(key));
    let Some((window, frame)) = others.into_iter().find(|(_, f)| f.covers(px)) else { return Under::Desktop };
    match frame.local(px) {
        Some(local) if frame.strip.is_some_and(|s| s.contains(local)) => Under::Strip { window: *window, at: insertion(&frame.slots, local.x) },
        _ => Under::Window(*window),
    }
}

/// Where a tab dropped at `x` on a strip of `slots` lands: before the first tab whose centre is
/// right of the pointer, else after the last.
fn insertion(slots: &[(usize, Rect)], x: f32) -> usize {
    match slots.iter().find(|(_, r)| r.center().x > x) {
        Some((i, _)) => *i,
        None => slots.last().map_or(usize::MAX, |(i, _)| i.saturating_add(1)),
    }
}

/// Where a dragged-out tab would go if let go now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CrossTarget {
    /// Window `window`'s tab strip, before its tab `at`.
    Strip { window: WindowKey, at: usize },
    /// Nowhere: another window's pages or title bar, or a move the Window menu's rules refuse
    /// (`reason`). Letting go leaves the tab where it was.
    Invalid { window: Option<WindowKey>, reason: Option<&'static str> },
    /// A new window, with the pointer (desktop physical pixel `at`) where it was on the tab.
    Desktop { at: Pos2 },
}

/// A tab being dragged out of its window, while it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CrossDrag {
    pub(crate) from: WindowKey,
    pub(crate) view: crate::canvas::ViewKey,
    pub(crate) target: CrossTarget,
    /// The source window's pass that last saw it; a drag no pass continues is over.
    pub(crate) pass: u64,
}

impl PdfCraftApp {
    /// Whether a tab can be dragged out of its window now: several windows are available (not
    /// the "Keep all documents in one window" preference, macOS or embedded windows), the
    /// windowing system follows the pointer out, and no dialog holds the windows (T-3).
    pub(crate) fn cross_drag_available(&self) -> bool {
        #[cfg(test)]
        let tracking = TRACKING.load(Ordering::Relaxed) || self.force_native_windows;
        #[cfg(not(test))]
        let tracking = TRACKING.load(Ordering::Relaxed);
        tracking && self.windows_unavailable().is_none() && !self.modal_open()
    }

    /// Record where this window is, at the start of its pass; forget a drag its source window no
    /// longer continues.
    pub(crate) fn note_frame(&mut self, ctx: &egui::Context) {
        let (inner, outer, ppp) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.native_pixels_per_point.unwrap_or(i.pixels_per_point))
        });
        self.win.frame.inner = inner;
        self.win.frame.outer = outer;
        self.win.frame.ppp = ppp;
        // A cancelled drag is over once the button is up; while it is held, the window losing
        // the keyboard or a dialog opening cancels whatever it drags.
        if !ctx.input(|i| i.pointer.primary_down() || i.pointer.primary_released()) {
            self.win.cancelled_drag = false;
            self.win.drag_pos = None;
            self.win.drag_grab = None;
        } else if !ctx.input(|i| i.focused) || self.modal_open() {
            self.win.cancelled_drag = true;
        }
        let pass = ctx.cumulative_pass_nr();
        if let Some(drag) = self.windows.drag
            && drag.from == self.current_window()
            && drag.pass.saturating_add(1) < pass
        {
            self.end_cross_drag(ctx);
        }
    }

    /// A tab (`index`, view `view`) of this window dragged with the pointer at `pos` (window
    /// points); `released` when it was let go this pass. `false` while the pointer is on this
    /// window: the strip and the panes handle it. Otherwise this decides and draws what is under
    /// it, and carries out the drop on release.
    pub(crate) fn tab_dragged_out(&mut self, ctx: &egui::Context, index: usize, view: crate::canvas::ViewKey, pos: Pos2, released: bool) -> bool {
        if !self.cross_drag_available() {
            self.end_cross_drag(ctx);
            return false;
        }
        let from = self.current_window();
        let Some(px) = self.win.frame.desktop(pos) else { return false };
        let frames: Vec<(WindowKey, WindowFrame)> = self.window_uis().into_iter().map(|(key, ui)| (key, ui.frame.clone())).collect();
        let under = under(&frames, &self.windows.focus_order, from, px);
        let tabs = self.win.views.len();
        let target = match under {
            Under::Own => {
                self.end_cross_drag(ctx);
                return false;
            }
            Under::Strip { window, at } => match self.move_unavailable_from(from, tabs, index, Some(window)) {
                None => CrossTarget::Strip { window, at },
                Some(reason) => CrossTarget::Invalid { window: Some(window), reason: Some(reason) },
            },
            Under::Window(window) => CrossTarget::Invalid { window: Some(window), reason: None },
            Under::Desktop => match self.move_unavailable_from(from, tabs, index, None) {
                None => CrossTarget::Desktop { at: px },
                Some(reason) => CrossTarget::Invalid { window: None, reason: Some(reason) },
            },
        };
        if released {
            self.end_cross_drag(ctx);
            let grab = self.win.drag_grab.unwrap_or(pos);
            self.drop_tab_out(from, view, target, grab);
            return true;
        }
        ctx.set_cursor_icon(match target {
            CrossTarget::Strip { .. } => egui::CursorIcon::Grabbing,
            CrossTarget::Desktop { .. } => egui::CursorIcon::Move,
            CrossTarget::Invalid { .. } => egui::CursorIcon::NoDrop,
        });
        let previous = self.windows.drag.map(|d| d.target);
        self.windows.drag = Some(CrossDrag { from, view, target, pass: ctx.cumulative_pass_nr() });
        // The strip under the pointer draws its marker; the one it left takes it away.
        if previous != Some(target) {
            self.repaint_windows(ctx);
        }
        ctx.request_repaint();
        true
    }

    /// Let go of a dragged-out tab over `target`; the drag took hold of it at `grab` (window points).
    fn drop_tab_out(&mut self, from: WindowKey, view: crate::canvas::ViewKey, target: CrossTarget, grab: Pos2) {
        match target {
            CrossTarget::Strip { window, at } => self.request_window_op(super::WindowOp::MoveTo { from, view, to: window, at: Some(at) }),
            CrossTarget::Desktop { at } => {
                let geometry = self.window_at_pointer(at, grab);
                self.request_window_op(super::WindowOp::MoveToNew { from, view, geometry });
            }
            CrossTarget::Invalid { reason, .. } => {
                if let Some(reason) = reason {
                    self.notify_tr(reason);
                }
            }
        }
    }

    /// Stop a drag between windows, if one is under way: nothing moves.
    pub(crate) fn end_cross_drag(&mut self, ctx: &egui::Context) {
        if self.windows.drag.take().is_some() {
            self.repaint_windows(ctx);
        }
    }

    /// Repaint every window (a strip draws or takes away its marker).
    fn repaint_windows(&self, ctx: &egui::Context) {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        for w in &self.windows.others {
            ctx.request_repaint_of(w.id.viewport());
        }
    }

    /// Where the tab strip of the window being drawn shows the marker for a tab dragged in from
    /// another window: before its tab `at` (or after the last).
    pub(crate) fn cross_drag_marker(&self) -> Option<usize> {
        match self.windows.drag?.target {
            CrossTarget::Strip { window, at } if window == self.current_window() => Some(at),
            _ => None,
        }
    }

    /// A new window for a tab let go on the desktop at `px` (physical pixels), placed so the
    /// pointer is where it took hold of the tab in the window it came from (`pos`, window
    /// points), the size of
    /// that window, on the monitor under the pointer and inside its usable area (O-3's checks).
    pub(crate) fn window_at_pointer(&self, px: Pos2, pos: Pos2) -> super::Geometry {
        let frame = &self.win.frame;
        let ppp = frame.scale();
        let size = frame.inner.map_or(super::FALLBACK_SIZE, |r| r.size());
        let monitor = self.ctx.as_ref().and_then(|c| c.input(|i| i.viewport().monitor_size));
        let area = crate::platform::monitors::monitor_at(px);
        let at = (px.to_vec2() / ppp - pos.to_vec2()).to_pos2();
        super::validate(Some(at), size, super::limits_on(area, ppp, monitor))
    }
}

/// The gap a marker for a tab arriving before tab `at` sits in, on a strip of `slots`: before
/// that tab, or after the last; `gap` apart.
pub(crate) fn marker_x(slots: &[(usize, Rect)], at: usize, gap: f32) -> Option<f32> {
    match slots.iter().find(|(i, _)| *i == at) {
        Some((_, r)) => Some(r.left() - gap / 2.0),
        None => slots.last().map(|(_, r)| r.right() + gap / 2.0),
    }
}

#[cfg(test)]
#[path = "drag_tests.rs"]
mod tests;

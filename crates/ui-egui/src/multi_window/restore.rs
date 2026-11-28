//! TAB.WINDOW.1 W4: the window arrangement comes back at start-up (decision T-4).
//!
//! [`crate::PdfCraftApp::reopen_last_files`] opens every file of the last session in the first
//! window, as it always has, so nothing is opened twice and a session that only knew one window
//! restores as before. The windows after the first are then rebuilt from those tabs with the
//! Window menu's own moves (Move Tab to New Window, then Move Tab to Window): each tab moves as
//! it is, at the page it reopened at, and nothing is closed or reopened.
//!
//! - A file that is gone, or that didn't open, is left out; a window with none of its files
//!   left is not made. The other windows and files come back as they were.
//! - A window is made only where several windows are available: not with "Keep all documents in
//!   one window", on macOS, or without native windows. There everything stays in the first
//!   window, which is the fallback.
//! - A window goes back where it was when that place is on a monitor that is there now, kept
//!   inside it; otherwise (a monitor unplugged, a place off every screen, none saved) it opens
//!   beside the first window, as a new window does (O-3).
//! - It waits while one of its files is still being opened (in the background, or behind a
//!   password prompt), so a slow file still reaches its window.

use egui::{Pos2, Vec2, pos2, vec2};

use super::Geometry;
use crate::PdfCraftApp;
use crate::last_session::{SavedPlace, SessionWindow};

/// The windows of the last session still to be made, once their files are open.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Restore {
    pub(crate) windows: Vec<SessionWindow>,
}

impl PdfCraftApp {
    /// Make the last session's windows once their files are open (each pass of the first window,
    /// before its window changes are carried out). Does nothing once done.
    pub(crate) fn restore_windows(&mut self) {
        if self.drawing_window.is_some() {
            return;
        }
        let Some(plan) = self.restoring.as_ref() else { return };
        if plan.windows.iter().flat_map(|w| w.files.iter()).any(|f| self.still_opening(f)) {
            return;
        }
        let Some(plan) = self.restoring.take() else { return };
        // One window here: everything stays in the first, which is the fallback (T-4).
        if self.windows_unavailable().is_some() {
            return;
        }
        // The moves keep the first window's active tab (they never take it: it is one of its own
        // files), as `reopen_last_files` set it.
        for saved in &plan.windows {
            self.restore_window(saved);
        }
        self.focus_window(None);
    }

    /// Whether the file `path` is still on its way into a tab (in the background, or waiting for
    /// its password).
    fn still_opening(&self, path: &str) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let same = |p: Option<&str>| p.is_some_and(|p| crate::same_file(p, path));
            self.pending_opens.iter().any(|o| same(o.path.as_deref()))
                || self.deferred_opens.iter().any(|o| same(Some(o.path.as_str())))
                || self.password_prompt.as_ref().is_some_and(|p| same(p.path.as_deref()))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            false
        }
    }

    /// The first window's tab showing the file `path`.
    fn tab_with_path(&self, path: &str) -> Option<usize> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.win.views.iter().position(|v| self.session.get(v.id).and_then(|d| d.path.as_deref()).is_some_and(|p| crate::same_file(p, path)))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            None
        }
    }

    /// Make one saved window from the first window's tabs on its files, in its order.
    fn restore_window(&mut self, saved: &SessionWindow) {
        let views: Vec<crate::canvas::ViewKey> = saved
            .files
            .iter()
            .filter_map(|p| self.tab_with_path(p))
            .filter_map(|i| self.win.views.get(i))
            .map(|v| crate::canvas::ViewKey(v.view_key()))
            .collect();
        let Some((&first, rest)) = views.split_first() else { return };
        let geometry = self.restored_geometry(saved.place);
        let on_screen = saved.place.and_then(kept_on_screen);
        let before = self.windows.others.len();
        // The first window keeps a tab: a session whose first window lost all its files gives
        // one of this window's back to it (`take_tab` refuses the last one).
        self.move_to_new_window(None, first, geometry);
        if self.windows.others.len() == before {
            return;
        }
        let Some(id) = self.windows.others.last().map(|w| w.id) else { return };
        for &view in rest {
            self.move_to_window(None, view, Some(id), None);
        }
        let paths: Vec<Option<String>> =
            self.windows.others.last().map(|w| w.ui.views.iter().map(|v| self.path_of(v.id)).collect()).unwrap_or_default();
        if let Some(w) = self.windows.others.last_mut() {
            // Its saved place, in pixels at its own scale, from its first passes (only where it
            // is on a monitor there now; otherwise it stays beside the first window).
            w.pending_place = on_screen.map(|place| super::PendingPlace { place, set_at: None, tries: 0 });
            if let Some(active) = saved.active.as_deref()
                && let Some(i) = paths.iter().position(|p| p.as_deref().is_some_and(|p| same(p, active)))
            {
                w.ui.active = Some(i);
            }
        }
    }

    fn path_of(&self, id: pdfcraft_engine::DocId) -> Option<String> {
        self.session.get(id).and_then(|d| d.path.clone())
    }

    /// Where a restored window first opens: near its saved place (in points at the first
    /// window's scale: the system's own conversion corrects it from the window's pass, see
    /// [`super::SecondaryWindow::place_on_screen`]), or beside the first window.
    pub(crate) fn restored_geometry(&self, place: Option<SavedPlace>) -> Geometry {
        let ppp = super::finite_positive(Some(self.windows.root_ppp)).unwrap_or(1.0);
        let area = place.and_then(|p| crate::platform::monitors::monitor_at(grip(p)));
        place.and_then(|p| place_on(p, ppp, area)).unwrap_or_else(|| self.new_window_geometry())
    }
}

/// A point just inside a saved window's title bar: where it is on screen, it can be taken hold of.
fn grip(place: SavedPlace) -> Pos2 {
    pos2(place.x as f32, place.y as f32) + vec2(48.0, 12.0)
}

/// A saved place (physical pixels) kept inside the monitor its title bar is on now, in physical
/// pixels; `None` when it is on no monitor there now.
fn kept_on_screen(place: SavedPlace) -> Option<SavedPlace> {
    kept_on(place, crate::platform::monitors::monitor_at(grip(place)))
}

/// `place` kept inside `area` (both in physical pixels), as [`place_on`] keeps it at a scale of 1.
pub(crate) fn kept_on(place: SavedPlace, area: Option<crate::monitors::MonitorArea>) -> Option<SavedPlace> {
    let g = place_on(place, 1.0, area)?;
    let pos = g.pos?;
    Some(SavedPlace { x: pos.x.round() as i32, y: pos.y.round() as i32, width: g.size.x.round() as u32, height: g.size.y.round() as u32, ..place })
}

/// `place` (physical pixels) on the monitor `area` the platform found nearest it, in points at
/// `ppp`, kept inside that monitor; `None` when its title bar is on no monitor there now.
pub(crate) fn place_on(place: SavedPlace, ppp: f32, area: Option<crate::monitors::MonitorArea>) -> Option<Geometry> {
    let area = area.filter(|m| m.bounds.contains(grip(place)))?;
    let ppp = super::finite_positive(Some(ppp)).unwrap_or(1.0);
    let size = Vec2::new(place.width as f32, place.height as f32) / ppp;
    let monitor = Some(area.bounds.size() / ppp);
    Some(super::validate(Some(points(pos2(place.x as f32, place.y as f32), ppp)), size, super::limits_on(Some(area), ppp, monitor)))
}

fn points(px: Pos2, ppp: f32) -> Pos2 {
    (px.to_vec2() / ppp).to_pos2()
}

#[cfg(not(target_arch = "wasm32"))]
fn same(a: &str, b: &str) -> bool {
    crate::same_file(a, b)
}

#[cfg(target_arch = "wasm32")]
fn same(a: &str, b: &str) -> bool {
    a == b
}

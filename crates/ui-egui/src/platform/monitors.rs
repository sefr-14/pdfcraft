//! Where the monitors are and which part of each a window may use (this fork, TAB.WINDOW.1 W2b),
//! for placing a new window on the monitor of the window it came from (O-3).
//!
//! egui reports a monitor's size but not where it is or which part is usable, so:
//! - **Windows**: the monitor under a point and its work area (the screen without the taskbar)
//!   come from `MonitorFromPoint` and `GetMonitorInfo`, through `winsafe`'s safe wrappers.
//! - **Elsewhere**: the app passes winit's monitor list (position and size) in through
//!   [`set_known`] whenever eframe makes a window. winit has no work area, so the usable area is
//!   the whole monitor there ([`MonitorArea::work_known`] says which).
//! - **Wayland**: windows can't be placed, so nothing here is used.
//!
//! Every rectangle here is in physical pixels, in desktop coordinates.

use std::sync::Mutex;

use egui::{Pos2, Rect};

/// One monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorArea {
    /// The whole monitor.
    pub bounds: Rect,
    /// The part windows may use: without the taskbar or dock when the platform says.
    pub work: Rect,
    /// Whether `work` came from the platform (Windows) or is the whole monitor (elsewhere).
    pub work_known: bool,
}

impl MonitorArea {
    /// A monitor whose usable area isn't known: all of it.
    pub fn whole(bounds: Rect) -> MonitorArea {
        MonitorArea { bounds, work: bounds, work_known: false }
    }

    fn usable(&self) -> bool {
        let ok = |r: Rect| r.min.is_finite() && r.max.is_finite() && r.width() > 0.0 && r.height() > 0.0;
        ok(self.bounds) && ok(self.work)
    }
}

static KNOWN: Mutex<Vec<MonitorArea>> = Mutex::new(Vec::new());

/// The monitors the platform reported last (winit's list, from the app's window hook).
pub fn set_known(monitors: Vec<MonitorArea>) {
    // A poisoned lock only means another thread panicked while writing a list: replace it.
    let mut known = KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *known = monitors;
}

fn known() -> Vec<MonitorArea> {
    KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The monitor that shows `point` (at distance 0), else the nearest one, from `monitors`.
/// Monitors with no usable size are skipped; `None` when none is left or the point isn't a number.
pub fn pick(monitors: &[MonitorArea], point: Pos2) -> Option<MonitorArea> {
    if !(point.x.is_finite() && point.y.is_finite()) {
        return None;
    }
    monitors.iter().filter(|m| m.usable()).min_by(|a, b| a.bounds.distance_sq_to_pos(point).total_cmp(&b.bounds.distance_sq_to_pos(point))).copied()
}

/// The monitor at `point` (physical pixels, desktop coordinates): the platform's answer on Windows,
/// otherwise the nearest of the monitors last reported by winit.
pub fn monitor_at(point: Pos2) -> Option<MonitorArea> {
    #[cfg(target_os = "windows")]
    if let Some(m) = windows_monitor_at(point) {
        return Some(m);
    }
    pick(&known(), point)
}

#[cfg(target_os = "windows")]
fn windows_monitor_at(point: Pos2) -> Option<MonitorArea> {
    use winsafe::{HMONITOR, POINT, co};
    if !(point.x.is_finite() && point.y.is_finite()) {
        return None;
    }
    // `as` saturates a float that is too large for an i32; it is finite here.
    let pt = POINT { x: point.x.round() as i32, y: point.y.round() as i32 };
    let info = HMONITOR::MonitorFromPoint(pt, co::MONITOR::DEFAULTTONEAREST).GetMonitorInfo().ok()?;
    let rect = |r: winsafe::RECT| Rect::from_min_max(Pos2::new(r.left as f32, r.top as f32), Pos2::new(r.right as f32, r.bottom as f32));
    let area = MonitorArea { bounds: rect(info.rcMonitor), work: rect(info.rcWork), work_known: true };
    area.usable().then_some(area)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn m(x: f32, y: f32, w: f32, h: f32) -> MonitorArea {
        MonitorArea::whole(Rect::from_min_size(pos2(x, y), vec2(w, h)))
    }

    #[test]
    fn the_monitor_under_the_point_wins_else_the_nearest() {
        // A 4K monitor at 150 % left of the primary 1080p one, which has a taskbar.
        let left = m(-3840.0, 0.0, 3840.0, 2160.0);
        let mut primary = m(0.0, 0.0, 1920.0, 1080.0);
        primary.work = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1032.0));
        primary.work_known = true;
        let list = [left, primary];
        assert_eq!(pick(&list, pos2(-100.0, 500.0)), Some(left));
        assert_eq!(pick(&list, pos2(100.0, 500.0)), Some(primary));
        // Below the primary monitor (a window dragged half off-screen): the nearest.
        assert_eq!(pick(&list, pos2(500.0, 1500.0)), Some(primary));
        assert_eq!(pick(&list, pos2(-3000.0, 2500.0)), Some(left));
    }

    #[test]
    fn nothing_usable_gives_no_monitor() {
        assert_eq!(pick(&[], pos2(0.0, 0.0)), None);
        assert_eq!(pick(&[m(0.0, 0.0, 0.0, 1080.0), m(f32::NAN, 0.0, 10.0, 10.0)], pos2(0.0, 0.0)), None);
        assert_eq!(pick(&[m(0.0, 0.0, 1920.0, 1080.0)], pos2(f32::NAN, 0.0)), None);
    }

    /// Windows only, on a real desktop (the CI runner): the system names the monitor under a point
    /// and its work area, which lies inside it; a point far from every monitor gets the nearest.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reports_the_monitor_and_its_work_area() {
        let m = monitor_at(pos2(10.0, 10.0)).expect("Windows reports the monitor under a point");
        assert!(m.work_known, "the work area comes from the system");
        assert!(m.bounds.width() > 0.0 && m.bounds.height() > 0.0, "{m:?}");
        assert!(m.work.width() > 0.0 && m.work.height() > 0.0, "{m:?}");
        assert!(m.bounds.contains(m.work.min) && m.work.max.x <= m.bounds.max.x && m.work.max.y <= m.bounds.max.y, "{m:?}");
        assert!(m.bounds.contains(pos2(10.0, 10.0)), "{m:?}");
        let far = monitor_at(pos2(1.0e6, 1.0e6)).expect("the nearest monitor");
        assert!(far.work_known && far.bounds.width() > 0.0, "{far:?}");
        eprintln!("W2b on Windows: monitor {:?}, work area {:?}", m.bounds, m.work);
    }

    #[test]
    fn the_known_list_is_replaced_whole() {
        set_known(vec![m(0.0, 0.0, 800.0, 600.0)]);
        set_known(vec![m(0.0, 0.0, 1920.0, 1080.0)]);
        let found = pick(&known(), pos2(10.0, 10.0));
        assert_eq!(found.map(|a| a.bounds.width()), Some(1920.0));
        set_known(Vec::new());
    }
}

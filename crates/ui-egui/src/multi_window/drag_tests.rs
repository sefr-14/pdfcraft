//! TAB.WINDOW.1 W3: what is under a tab dragged out of its window.

use egui::{Rect, pos2, vec2};

use super::*;
use crate::multi_window::WindowId;

fn frame(inner: Rect, title: f32, ppp: f32, slots: &[(usize, f32, f32)]) -> WindowFrame {
    WindowFrame {
        inner: Some(inner),
        outer: Some(Rect::from_min_max(inner.min - vec2(0.0, title), inner.max)),
        ppp,
        strip: Some(Rect::from_min_max(pos2(40.0, 0.0), pos2(inner.width() - 40.0, 37.0))),
        slots: slots.iter().map(|(i, l, r)| (*i, Rect::from_min_max(pos2(*l, 0.0), pos2(*r, 37.0)))).collect(),
    }
}

fn win(n: u64) -> WindowKey {
    Some(WindowId(n))
}

/// The first window at the top left; a second to its right with two tabs.
fn two() -> Vec<(WindowKey, WindowFrame)> {
    vec![
        (None, frame(Rect::from_min_size(pos2(0.0, 30.0), vec2(1000.0, 800.0)), 30.0, 1.0, &[(0, 40.0, 135.0)])),
        (win(1), frame(Rect::from_min_size(pos2(1200.0, 30.0), vec2(800.0, 600.0)), 30.0, 1.0, &[(0, 40.0, 135.0), (1, 140.0, 235.0)])),
    ]
}

#[test]
fn over_its_own_window_title_bar_included_the_drag_stays_the_windows() {
    let frames = two();
    assert_eq!(under(&frames, &[], None, pos2(500.0, 400.0)), Under::Own);
    assert_eq!(under(&frames, &[], None, pos2(500.0, 10.0)), Under::Own, "its title bar");
    assert_eq!(under(&frames, &[], None, pos2(f32::NAN, 10.0)), Under::Own, "no number: nothing leaves");
}

#[test]
fn over_another_windows_strip_it_lands_between_the_tabs_the_pointer_is_between() {
    let frames = two();
    let at = |x: f32| under(&frames, &[], None, pos2(1200.0 + x, 30.0 + 18.0));
    assert_eq!(at(60.0), Under::Strip { window: win(1), at: 0 }, "before the first tab's centre");
    assert_eq!(at(100.0), Under::Strip { window: win(1), at: 1 }, "past it");
    assert_eq!(at(500.0), Under::Strip { window: win(1), at: 2 }, "past the last");
}

#[test]
fn over_another_windows_pages_or_title_bar_it_is_no_drop_and_off_every_window_the_desktop() {
    let frames = two();
    assert_eq!(under(&frames, &[], None, pos2(1500.0, 400.0)), Under::Window(win(1)), "its pages");
    assert_eq!(under(&frames, &[], None, pos2(1500.0, 10.0)), Under::Window(win(1)), "its title bar");
    assert_eq!(under(&frames, &[], None, pos2(1100.0, 400.0)), Under::Desktop, "between the windows");
    assert_eq!(under(&frames, &[], None, pos2(5000.0, 5000.0)), Under::Desktop);
}

#[test]
fn overlapping_windows_the_one_used_last_is_on_top() {
    let mut frames = two();
    // A third window over the second's strip.
    frames.push((win(2), frame(Rect::from_min_size(pos2(1250.0, 30.0), vec2(600.0, 400.0)), 30.0, 1.0, &[(0, 40.0, 135.0)])));
    let px = pos2(1300.0, 40.0);
    assert_eq!(under(&frames, &[win(1), win(2)], None, px), Under::Strip { window: win(2), at: 0 }, "the third, used last");
    assert_eq!(under(&frames, &[win(2), win(1)], None, px), Under::Strip { window: win(1), at: 1 }, "the second, used last: its strip there");
}

#[test]
fn a_window_on_a_monitor_of_another_scale_is_hit_where_it_is() {
    // The second window at 200 %: 800×600 points at (1200, 15) points is 1600×1200 physical
    // pixels from (2400, 30).
    let frames = vec![
        (None, frame(Rect::from_min_size(pos2(0.0, 30.0), vec2(1000.0, 800.0)), 30.0, 1.0, &[(0, 40.0, 135.0)])),
        (win(1), frame(Rect::from_min_size(pos2(1200.0, 15.0), vec2(800.0, 600.0)), 15.0, 2.0, &[(0, 40.0, 135.0), (1, 140.0, 235.0)])),
    ];
    // Physical (1300, 40) is left of it at 200 %, though inside its points rectangle at 100 %.
    assert_eq!(under(&frames, &[], None, pos2(1300.0, 40.0)), Under::Desktop);
    // Physical (2400 + 2·100, 30 + 2·18): its strip, past the first tab.
    assert_eq!(under(&frames, &[], None, pos2(2600.0, 66.0)), Under::Strip { window: win(1), at: 1 });
    let source = &frames[0].1;
    assert_eq!(source.desktop(pos2(10.0, 10.0)), Some(pos2(10.0, 40.0)));
}

#[test]
fn the_marker_sits_in_the_gap_the_tab_lands_in() {
    let slots = [(0, Rect::from_min_max(pos2(40.0, 0.0), pos2(135.0, 37.0))), (1, Rect::from_min_max(pos2(140.0, 0.0), pos2(235.0, 37.0)))];
    assert_eq!(marker_x(&slots, 0, 4.0), Some(38.0));
    assert_eq!(marker_x(&slots, 1, 4.0), Some(138.0));
    assert_eq!(marker_x(&slots, 2, 4.0), Some(237.0), "after the last");
    assert_eq!(marker_x(&[], 0, 4.0), None);
}

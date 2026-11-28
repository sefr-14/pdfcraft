//! TAB.WINDOW.1 W3: dragging a tab between windows and onto the desktop, through the Window
//! menu's moves.

use egui::{Rect, pos2, vec2};

use super::*;
use crate::multi_window::WindowFrame;
use crate::multi_window::drag::CrossTarget;

/// The app with a, c in the first window and b alone in a second one, and where both windows
/// are: the first at (0, 30) points, 1000×800; the second at (1200, 30), 800×600, each with a
/// strip along its top.
fn two_windows() -> (Harness<'static, PdfCraftApp>, WindowId) {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    app.show_other_windows(&mut host);
    let id = app.windows.others[0].id;
    app.win.frame = frame(Rect::from_min_size(pos2(0.0, 30.0), vec2(1000.0, 800.0)), 2);
    app.windows.others[0].ui.frame = frame(Rect::from_min_size(pos2(1200.0, 30.0), vec2(800.0, 600.0)), 1);
    (h, id)
}

fn frame(inner: Rect, tabs: usize) -> WindowFrame {
    WindowFrame {
        inner: Some(inner),
        outer: Some(Rect::from_min_max(inner.min - vec2(0.0, 30.0), inner.max)),
        ppp: 1.0,
        strip: Some(Rect::from_min_max(pos2(40.0, 0.0), pos2(inner.width() - 40.0, 37.0))),
        slots: (0..tabs).map(|i| (i, Rect::from_min_size(pos2(40.0 + 100.0 * i as f32, 0.0), vec2(95.0, 37.0)))).collect(),
    }
}

/// Window points in the first window of the desktop point `x`, `y`.
fn from_first(x: f32, y: f32) -> egui::Pos2 {
    pos2(x, y - 30.0)
}

fn drag(app: &mut PdfCraftApp, index: usize, pos: egui::Pos2, released: bool) -> bool {
    let ctx = app.ctx.clone().expect("a context");
    let view = crate::canvas::ViewKey(app.win.views[index].view_key());
    let handled = app.tab_dragged_out(&ctx, index, view, pos, released);
    if released {
        app.apply_window_ops();
    }
    handled
}

fn layout(app: &PdfCraftApp) -> Vec<(WindowKey, Vec<pdfcraft_engine::DocId>)> {
    app.window_uis().into_iter().map(|(key, ui)| (key, ids(&ui.views))).collect()
}

/// Dropped on the other window's strip, the tab goes there where it was let go, as the same tab
/// (its page, zoom and view rotation), and becomes that window's active one.
#[test]
fn a_tab_dropped_on_another_windows_strip_lands_there_as_it_was() {
    let (mut h, id) = two_windows();
    let app = h.state_mut();
    let (a, c, b) = (app.win.views[0].id, app.win.views[1].id, app.windows.others[0].ui.views[0].id);
    app.win.views[1].zoom = 1.75;
    app.win.views[1].rotation = 90;
    let key = app.win.views[1].view_key();
    // Over the second window's strip, before its only tab (b, centre at x 87.5).
    assert!(drag(app, 1, from_first(1200.0 + 60.0, 30.0 + 18.0), false), "the drag is between windows");
    assert_eq!(app.windows.drag.map(|d| d.target), Some(CrossTarget::Strip { window: Some(id), at: 0 }));
    assert!(drag(app, 1, from_first(1200.0 + 60.0, 30.0 + 18.0), true));
    assert_eq!(layout(app), vec![(None, vec![a]), (Some(id), vec![c, b])]);
    let moved = &app.windows.others[0].ui.views[0];
    assert_eq!((moved.view_key(), moved.zoom, moved.rotation), (key, 1.75, 90), "the same tab, as it was");
    assert_eq!(app.windows.others[0].ui.active, Some(0), "the window's active tab");
    assert!(app.windows.drag.is_none());
}

/// Over another window's pages, or over a refused move, letting go changes nothing.
#[test]
fn a_tab_let_go_over_another_windows_pages_stays_where_it_was() {
    let (mut h, id) = two_windows();
    let app = h.state_mut();
    let before = layout(app);
    assert!(drag(app, 1, from_first(1500.0, 400.0), false));
    assert_eq!(app.windows.drag.map(|d| d.target), Some(CrossTarget::Invalid { window: Some(Some(id)), reason: None }));
    assert!(drag(app, 1, from_first(1500.0, 400.0), true));
    assert_eq!(layout(app), before);
    // The first window's last tab can't leave it: refused with the reason, nothing moves.
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    for _ in 0..2 {
        assert!(app.request_move_tab_to_new_window(1));
        app.show_other_windows(&mut host);
    }
    app.win.frame = frame(Rect::from_min_size(pos2(0.0, 30.0), vec2(1000.0, 800.0)), 1);
    app.windows.others[0].ui.frame = frame(Rect::from_min_size(pos2(1200.0, 30.0), vec2(800.0, 600.0)), 1);
    let before = layout(app);
    assert!(drag(app, 0, from_first(1260.0, 48.0), false));
    assert!(matches!(app.windows.drag.map(|d| d.target), Some(CrossTarget::Invalid { reason: Some(_), .. })));
    assert!(drag(app, 0, from_first(1260.0, 48.0), true));
    assert_eq!(layout(app), before);
    assert!(app.win.toast.is_some(), "the reason is told");
}

/// Let go over the desktop, the tab gets a new window there, with the pointer where it held the
/// tab; a window's only tab doesn't.
#[test]
fn a_tab_let_go_on_the_desktop_gets_a_new_window_under_the_pointer() {
    let (mut h, _id) = two_windows();
    let app = h.state_mut();
    let c = app.win.views[1].id;
    let pos = from_first(300.0, 1000.0);
    assert!(drag(app, 1, pos, false));
    assert_eq!(app.windows.drag.map(|d| d.target), Some(CrossTarget::Desktop { at: pos2(300.0, 1000.0) }));
    assert!(drag(app, 1, pos, true));
    assert_eq!(app.window_count(), 3);
    let new = app.windows.others.last().expect("a new window");
    assert_eq!(ids(&new.ui.views), vec![c]);
    // Placed so the pointer is where it was in the first window: (300, 1000) − (300, 970).
    let placed = new.geometry.pos.expect("a position");
    assert!((placed - pos2(0.0, 30.0)).length() < 0.01, "{placed:?}");
    // The second window's only tab, on the desktop: nothing.
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    app.show_other_windows(&mut host);
    let count = app.window_count();
    app.drawing_window = Some(app.windows.others[0].id);
    std::mem::swap(&mut app.win, &mut app.windows.others[0].ui);
    app.win.frame = frame(Rect::from_min_size(pos2(1200.0, 30.0), vec2(800.0, 600.0)), 1);
    let ctx = app.ctx.clone().unwrap();
    let view = crate::canvas::ViewKey(app.win.views[0].view_key());
    assert!(app.tab_dragged_out(&ctx, 0, view, pos2(-900.0, 900.0), false));
    assert!(matches!(app.windows.drag.map(|d| d.target), Some(CrossTarget::Invalid { window: None, reason: Some(_) })));
    std::mem::swap(&mut app.win, &mut app.windows.others[0].ui);
    app.drawing_window = None;
    app.apply_window_ops();
    assert_eq!(app.window_count(), count);
}

/// No drag between windows with one window ("Keep all documents in one window"), or while a
/// dialog holds the windows (T-3): the window's own drag handles it.
#[test]
fn no_drag_leaves_the_window_in_one_window_mode_or_under_a_dialog() {
    let (mut h, _id) = two_windows();
    let app = h.state_mut();
    let before = layout(app);
    app.keep_one_window = true;
    assert!(!drag(app, 1, from_first(1260.0, 48.0), true));
    app.keep_one_window = false;
    app.dialog = Some(crate::Dialog::About);
    assert!(!drag(app, 1, from_first(1260.0, 48.0), true));
    app.dialog = None;
    app.apply_window_ops();
    assert_eq!(layout(app), before);
}

/// A drag the source window's passes no longer continue (the button let go elsewhere, the tab
/// closed) is forgotten, and no strip keeps its marker.
#[test]
fn a_drag_no_pass_continues_is_forgotten() {
    let (mut h, id) = two_windows();
    let app = h.state_mut();
    assert!(drag(app, 1, from_first(1260.0, 48.0), false));
    app.drawing_window = Some(id);
    assert_eq!(app.cross_drag_marker(), Some(0), "the second window shows where it would land");
    app.drawing_window = None;
    if let Some(d) = app.windows.drag.as_mut() {
        d.pass = 0;
    }
    h.run_steps(3);
    assert!(h.state().windows.drag.is_none());
}

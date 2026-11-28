//! TAB.WINDOW.1 W2b: the commands that make and arrange windows, and what every window shares.

use egui::{Pos2, Rect, vec2};
use pdfcraft_engine::Edit;

use super::*;
use crate::platform::monitors::MonitorArea;

/// Run the window changes asked for and one pass of every window.
fn settle(app: &mut PdfCraftApp, host: &mut Host) {
    app.show_other_windows(host);
}

fn key_of(app: &PdfCraftApp, index: usize) -> u64 {
    app.win.views[index].view_key()
}

#[test]
fn move_tab_to_new_window_keeps_the_document_its_tab_state_and_undo_history() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    app.win.active = Some(1);
    assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
    app.win.views[1].current = 0;
    let (key, instance) = (key_of(app, 1), app.win.views[1].instance);
    let depth = app.session.get(all[1]).map(|d| d.undo_depth());
    let mut host = Host::new(app);
    assert!(app.execute("window.move_tab_to_new"));
    settle(app, &mut host);
    assert_eq!(ids(&app.win.views), vec![all[0], all[2]]);
    let window = &app.windows.others[0];
    assert_eq!(ids(&window.ui.views), vec![all[1]], "the same document, not a copy");
    assert_eq!(window.ui.views[0].view_key(), key, "the same tab, moved");
    assert_eq!(window.ui.views[0].instance, instance);
    assert_eq!(window.ui.active, Some(0));
    assert_eq!(app.session.get(all[1]).map(|d| d.undo_depth()), depth, "its undo history stays");
    assert!(app.session.get(all[1]).is_some_and(|d| d.dirty), "and its unsaved change");
    assert_eq!(window.came_from, Some(Origin { window: None, index: 1 }));
    // The undo still works from the new window.
    let id = all[1];
    assert!(app.session.undo(id).is_ok());
}

#[test]
fn a_new_window_takes_the_mode_of_the_window_it_came_from_and_keeps_its_own() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    app.win.mode = crate::Mode::Read;
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(2));
    settle(app, &mut host);
    assert_eq!(app.windows.others[0].ui.mode, crate::Mode::Read);
    app.win.mode = crate::Mode::AllTools;
    settle(app, &mut host);
    assert_eq!(app.windows.others[0].ui.mode, crate::Mode::Read, "each window has its own mode");
}

#[test]
fn a_windows_only_tab_cant_move_to_a_new_window_and_the_main_window_keeps_one() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    for _ in 0..2 {
        assert!(app.request_move_tab_to_new_window(0));
        settle(app, &mut host);
    }
    assert_eq!(app.win.views.len(), 1);
    assert!(app.move_unavailable(0, None).is_some());
    assert!(!app.request_move_tab_to_new_window(0), "the main window's last tab stays");
    let second = app.windows.others[0].id;
    assert!(!app.request_move_tab_to_window(0, Some(second)), "also to another window");
    settle(app, &mut host);
    assert_eq!(app.win.views.len(), 1);
    assert_eq!(app.windows.others.len(), 2);
}

#[test]
fn the_main_windows_last_tab_stays_even_when_a_move_is_queued_directly() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    for _ in 0..2 {
        assert!(app.request_move_tab_to_new_window(0));
        settle(app, &mut host);
    }
    let only = crate::canvas::ViewKey(key_of(app, 0));
    let geometry = app.new_window_geometry();
    app.request_window_op(WindowOp::MoveToNew { from: None, view: only, geometry });
    app.request_window_op(WindowOp::MoveTo { from: None, view: only, to: Some(app.windows.others[0].id), at: None });
    settle(app, &mut host);
    assert_eq!(app.win.views.len(), 1, "the operation itself refuses, not only the command");
    assert_eq!(app.windows.others.len(), 2);
}

#[test]
fn move_tab_to_window_moves_it_to_the_end_and_an_emptied_window_closes() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(0));
    settle(app, &mut host);
    let first = app.windows.others[0].id;
    // From the main window into the other one.
    assert!(app.request_move_tab_to_window(0, Some(first)));
    settle(app, &mut host);
    assert_eq!(ids(&app.windows.others[0].ui.views), vec![all[0], all[1]]);
    assert_eq!(app.windows.others[0].ui.active, Some(1));
    assert_eq!(ids(&app.win.views), vec![all[2]]);
    // From the other window back, one by one: when its last tab leaves, it closes.
    for _ in 0..2 {
        let key = app.windows.others[0].ui.views[0].view_key();
        app.request_window_op(WindowOp::MoveTo { from: Some(first), view: crate::canvas::ViewKey(key), to: None, at: None });
        settle(app, &mut host);
    }
    assert!(app.windows.others.is_empty());
    assert_eq!(ids(&app.win.views), vec![all[2], all[0], all[1]]);
}

#[test]
fn merge_all_windows_brings_every_tab_into_the_main_window() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    let mut host = Host::new(app);
    for _ in 0..2 {
        assert!(app.request_move_tab_to_new_window(0));
        settle(app, &mut host);
    }
    assert_eq!(app.window_count(), 3);
    assert!(app.execute("window.merge_all"));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 1);
    assert_eq!(ids(&app.win.views), vec![all[2], all[0], all[1]]);
    assert_eq!(app.session.docs().len(), 3, "no document closed");
}

#[test]
fn a_document_shown_in_another_window_stays_open_when_its_tab_here_closes() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    app.win.active = Some(0);
    assert!(app.duplicate_view(0).is_some());
    let mut host = Host::new(app);
    // The duplicate of a goes to its own window; the original tab of a then closes here.
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    app.close_tab(0);
    assert!(app.session.get(all[0]).is_some(), "the other window still shows a");
    // Closing the document closes its tabs in every window, and the empty window with them.
    app.close_document(all[0]);
    settle(app, &mut host);
    assert!(app.session.get(all[0]).is_none());
    assert!(app.windows.others.is_empty());
}

#[test]
fn duplicate_numbers_and_the_tab_limit_count_every_window() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let a = app.win.views[0].id;
    assert!(app.duplicate_view(0).is_some());
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    assert_eq!(app.windows.others[0].ui.views[0].instance, 2);
    let at = app.duplicate_view(0).expect("a third tab of a");
    assert_eq!(app.win.views[at].instance, 3, "2 is taken in the other window");
    assert_eq!(app.all_views_of(a), 3);
    while app.all_views_of(a) < crate::menus::tab_context::MAX_VIEWS_PER_DOCUMENT {
        assert!(app.duplicate_view(0).is_some());
    }
    assert!(!app.can_duplicate(0), "the limit counts the other window's tab");
}

#[test]
fn an_edit_refreshes_the_documents_tab_in_another_window() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    assert!(app.duplicate_view(0).is_some());
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    app.sync_shared_views();
    let before = app.windows.others[0].ui.views[0].content_seen;
    assert!(before.is_some(), "a tab shared across windows is tracked");
    app.win.active = Some(0);
    assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
    app.sync_shared_views();
    assert_ne!(app.windows.others[0].ui.views[0].content_seen, before, "the other window's tab caught up");
}

#[test]
fn a_document_another_window_shows_keeps_its_rendering_when_it_is_hidden_here() {
    let mut h = app_with_tabs();
    let a = h.state().win.views[0].id;
    {
        let app = h.state_mut();
        // Two tabs of a: one moves to another window and stays on screen there; here a's tab is
        // hidden behind b.
        assert!(app.duplicate_view(0).is_some());
        let mut host = Host::new(app);
        assert!(app.request_move_tab_to_new_window(1));
        settle(app, &mut host);
        settle(app, &mut host);
        assert!(app.windows.on_screen.iter().any(|(key, docs)| key.is_some() && docs.contains(&a)));
        app.win.active = Some(1);
        app.suspended_docs.clear();
    }
    // The main window's real pass, with a hidden here.
    h.run_steps(1);
    assert!(!h.state().suspended_docs.contains(&a), "the other window's queue and results are kept");
    // Once no window shows it, it is suspended as before.
    let app = h.state_mut();
    app.windows.on_screen.retain(|(key, _)| key.is_none());
    app.merge_all_windows();
    app.win.active = Some(1);
    app.suspended_docs.clear();
    h.run_steps(1);
    assert!(h.state().suspended_docs.contains(&a));
}

#[test]
fn opening_a_file_open_in_another_window_brings_that_window_and_tab_forward() {
    let mut h = app_with_tabs();
    let path = std::env::temp_dir().join(format!("pdfcraft-w2b-open-{}.pdf", std::process::id()));
    std::fs::write(&path, pdf("on disk")).unwrap();
    let path = path.to_string_lossy().into_owned();
    let before = h.state().win.views.len();
    assert_ne!(h.state_mut().open_path(&path), crate::OpenOutcome::Failed);
    // The open may finish in the background: wait for its tab.
    for _ in 0..200 {
        if h.state().win.views.len() > before {
            break;
        }
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let app = h.state_mut();
    assert_eq!(app.win.views.len(), before + 1);
    let index = app.win.views.len() - 1;
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(index));
    settle(app, &mut host);
    let tabs = app.win.views.len();
    assert_eq!(app.open_path(&path), crate::OpenOutcome::Focused);
    assert_eq!(app.win.views.len(), tabs, "not opened twice");
    // W2e: brought forward by `Reveal`, which also applies what the open asked for there.
    assert!(app.windows.pending.iter().any(|op| matches!(op, WindowOp::Reveal { window: Some(_), .. })));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_window_commands_wait_for_the_preference_and_say_why() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    app.keep_one_window = true;
    assert!(app.windows_unavailable().is_some());
    assert!(!app.execute("window.move_tab_to_new"));
    assert!(toast(app).contains("Keep all documents in one window"), "{}", toast(app));
    app.keep_one_window = false;
    assert!(app.windows_unavailable().is_none());
}

#[test]
fn the_window_menu_lists_every_window_and_moves_the_active_tab() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(0));
    settle(app, &mut host);
    let names = app.window_names();
    assert_eq!(names.len(), 2);
    assert!(names[0].1.starts_with("Window 1"), "{names:?}");
    assert!(names[1].1.starts_with("Window 2") && names[1].1.contains("a.pdf"), "{names:?}");
    let ctx = app.ctx.clone().unwrap();
    app.win.active = Some(0);
    app.run_menu_command(&ctx, &craft_menu::CommandRef::with_params(crate::menus::MOVE_TO_WINDOW, serde_json::json!({ "index": 1 })));
    settle(app, &mut host);
    assert_eq!(app.windows.others[0].ui.views.len(), 2);
}

// ----------------------------------------------------------------------------- placement

#[test]
fn a_new_window_lands_32_pixels_from_its_source_inside_the_monitors_work_area() {
    // A 1920x1080 primary monitor with a 48 px taskbar, and a 150 % monitor to its left.
    let mut primary = MonitorArea::whole(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0)));
    primary.work = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1032.0));
    primary.work_known = true;
    let left = MonitorArea::whole(Rect::from_min_size(Pos2::new(-2880.0, 0.0), vec2(2880.0, 1620.0)));
    let monitors = [left, primary];
    // On the primary monitor at 100 %: 32 px away, kept above the taskbar.
    let source = Rect::from_min_size(Pos2::new(1200.0, 600.0), vec2(700.0, 450.0));
    let area = crate::platform::monitors::pick(&monitors, source.center());
    let g = beside(source, vec2(700.0, 450.0), limits_on(area, 1.0, None));
    assert_eq!(g.pos, Some(Pos2::new(1220.0, 582.0)), "shifted left and up to stay inside 1920x1032");
    // On the left monitor at 150 %: positions in points of that monitor's scale.
    let source = Rect::from_min_size(Pos2::new(-1800.0, 100.0), vec2(800.0, 500.0));
    let area = crate::platform::monitors::pick(&monitors, source.center() * 1.5);
    assert_eq!(area, Some(left));
    let g = beside(source, vec2(800.0, 500.0), limits_on(area, 1.5, None));
    let p = g.pos.unwrap();
    assert!((p.x - (-1800.0 + 32.0 / 1.5)).abs() < 0.01 && (p.y - (100.0 + 32.0 / 1.5)).abs() < 0.01, "{p:?}");
    assert!(g.size.x <= 2880.0 / 1.5 && g.size.y <= 1620.0 / 1.5);
}

#[test]
fn without_a_known_monitor_the_size_is_still_checked() {
    let g = beside(Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0)), vec2(1.0e9, f32::NAN), limits_on(None, 1.0, Some(vec2(1280.0, 720.0))));
    assert_eq!(g.size, vec2(1280.0, 700.0));
}

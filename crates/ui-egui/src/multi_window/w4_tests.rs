//! TAB.WINDOW.1 W4: the window arrangement saved with the last session and restored at start.

use egui::pos2;
use serde_json::json;

use super::*;
use crate::last_session::{LastSession, SavedPlace, SessionWindow};
use crate::multi_window::restore::{kept_on, place_on};

/// Files a.pdf … e.pdf in a fresh folder, their absolute paths in order.
fn files(tag: &str) -> (std::path::PathBuf, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-w4-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let paths = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|n| {
            let p = dir.join(format!("{n}.pdf"));
            std::fs::write(&p, pdf(n)).unwrap();
            crate::last_session::absolute(&p.to_string_lossy())
        })
        .collect();
    (dir, paths)
}

/// A fresh app (no tabs) that reopens the last session, with native windows.
fn fresh(keep_one_window: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.keep_one_window = keep_one_window;
        app.force_native_windows = true;
        app.run_inline = true;
        app.reopen_last_session = true;
        app
    });
    h.run_steps(4);
    h
}

/// Start `h` with the last session `saved` (its settings JSON), as the desktop app does.
fn restart(h: &mut Harness<'static, PdfCraftApp>, saved: &serde_json::Value) -> Host {
    let app = h.state_mut();
    app.last_session = LastSession::from_json(saved);
    app.reopen_last_files(&[]);
    let mut host = Host::new(app);
    app.show_other_windows(&mut host);
    host
}

fn names(app: &PdfCraftApp) -> Vec<Vec<String>> {
    app.window_uis().into_iter().map(|(_, ui)| ui.views.iter().filter_map(|v| app.session.get(v.id)).map(|d| d.name.clone()).collect()).collect()
}

fn active_names(app: &PdfCraftApp) -> Vec<Option<String>> {
    app.window_uis()
        .into_iter()
        .map(|(_, ui)| ui.active.and_then(|i| ui.views.get(i)).and_then(|v| app.session.get(v.id)).map(|d| d.name.clone()))
        .collect()
}

fn session(paths: &[String], windows: serde_json::Value, active: &str) -> serde_json::Value {
    json!({ "files": paths.iter().map(|p| json!({ "path": p, "page": 0 })).collect::<Vec<_>>(), "active": active, "windows": windows })
}

#[test]
fn saved_windows_are_read_defensively() {
    let known = ["/a.pdf", "/b.pdf", "/c.pdf"];
    let v = json!({
        "files": known.iter().map(|p| json!({ "path": p })).collect::<Vec<_>>(),
        "windows": [
            { "files": ["/b.pdf", "/zzz.pdf", "/b.pdf"], "active": "/zzz.pdf", "place": { "x": 100, "y": 50, "width": 800, "height": 600, "maximized": true } },
            { "files": ["/b.pdf", "/c.pdf"], "active": "/b.pdf", "place": { "x": 1e12, "y": 0, "width": 800, "height": 600 } },
            { "files": ["/zzz.pdf"] },
            "garbage",
            { "files": "/a.pdf" },
        ],
    });
    let s = LastSession::from_json(&v);
    assert_eq!(s.windows.len(), 2, "{s:?}");
    // Unknown and repeated files dropped; an active tab not in the window forgotten.
    assert_eq!(
        s.windows[0],
        SessionWindow {
            files: vec!["/b.pdf".into()],
            active: None,
            place: Some(SavedPlace { x: 100, y: 50, width: 800, height: 600, maximized: true })
        }
    );
    // A file in an earlier window isn't in a later one (a document is restored once); a place
    // out of range is dropped, the window kept.
    assert_eq!(s.windows[1], SessionWindow { files: vec!["/c.pdf".into()], active: None, place: None });
    // An old session (no windows) reads as before; a capped list stays capped.
    assert!(LastSession::from_json(&json!({ "files": [{ "path": "/a.pdf" }] })).windows.is_empty());
    let many: Vec<serde_json::Value> = (0..40).map(|_| json!({ "files": ["/a.pdf"] })).collect();
    assert_eq!(LastSession::from_json(&json!({ "files": [{ "path": "/a.pdf" }], "windows": many })).windows.len(), 1, "each file in one window");
    for bad in [json!({ "x": 0, "y": 0, "width": 0, "height": 10 }), json!({ "x": "1", "y": 0, "width": 9, "height": 9 }), json!(null)] {
        let v = json!({ "files": [{ "path": "/a.pdf" }], "windows": [{ "files": ["/a.pdf"], "place": bad }] });
        assert_eq!(LastSession::from_json(&v).windows[0].place, None, "{bad}");
    }
}

/// Saved: every file once, the first window's first; each later window's tabs, active tab and
/// place; a document two windows show belongs to the first.
#[test]
fn the_session_records_each_windows_tabs_and_place() {
    let (dir, paths) = files("save");
    let mut h = fresh(false);
    let app = h.state_mut();
    for p in &paths[..4] {
        app.open_path(p);
    }
    let mut host = Host::new(app);
    // a, b in the first window; c, d in a second, d active there; a duplicate of a in it too.
    assert!(app.duplicate_view(0).is_some());
    // Tabs now a, a (the duplicate), b, c, d: d to a new window, then c, then the duplicate.
    assert!(app.request_move_tab_to_new_window(4));
    app.show_other_windows(&mut host);
    let to = Some(app.windows.others[0].id);
    for index in [3, 1] {
        assert!(app.request_move_tab_to_window(index, to));
        app.show_other_windows(&mut host);
    }
    app.windows.others[0].ui.active = Some(0);
    let second = &mut app.windows.others[0];
    second.last =
        Some(Placement { outer: Rect::from_min_size(pos2(300.0, 200.0), vec2(900.0, 700.0)), inner_size: vec2(880.0, 650.0), maximized: false });
    assert_eq!(names(app), vec![vec!["a.pdf".to_string(), "b.pdf".into()], vec!["d.pdf".into(), "c.pdf".into(), "a.pdf".into()]]);
    let s = app.open_session();
    assert_eq!(s.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec![&paths[0], &paths[1], &paths[3], &paths[2]]);
    assert_eq!(s.windows.len(), 1);
    assert_eq!(s.windows[0].files, vec![paths[3].clone(), paths[2].clone()], "the duplicate of a stays with the first window");
    assert_eq!(s.windows[0].place, Some(SavedPlace { x: 300, y: 200, width: 880, height: 650, maximized: false }));
    assert_eq!(s.windows[0].active, Some(paths[3].clone()), "its active tab");
    // What is saved reads back the same.
    assert_eq!(LastSession::from_json(&serde_json::to_value(&s).unwrap()), s);
    let _ = std::fs::remove_dir_all(dir);
}

/// Restart: every file opens once, the windows come back with their tabs in order and their
/// active tabs, and the first window's active tab too.
#[test]
fn a_restart_brings_the_windows_back_as_they_were() {
    let (dir, paths) = files("restore");
    let mut h = fresh(false);
    let saved = session(
        &paths,
        json!([
            { "files": [&paths[3], &paths[2]], "active": &paths[2] },
            { "files": [&paths[4]], "place": { "x": 40, "y": 30, "width": 700, "height": 500, "maximized": true } },
        ]),
        &paths[1],
    );
    restart(&mut h, &saved);
    let app = h.state_mut();
    assert_eq!(names(app), vec![vec!["a.pdf".to_string(), "b.pdf".into()], vec!["d.pdf".into(), "c.pdf".into()], vec!["e.pdf".into()]]);
    assert_eq!(active_names(app), vec![Some("b.pdf".into()), Some("c.pdf".into()), Some("e.pdf".into())]);
    assert_eq!(app.session.docs().len(), 5, "each file opened once");
    assert!(app.restoring.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// A file that is gone is left out; a window with none of its files isn't made; the rest come
/// back as they were.
#[test]
fn missing_files_leave_out_only_what_is_missing() {
    let (dir, paths) = files("missing");
    std::fs::remove_file(&paths[2]).unwrap();
    std::fs::remove_file(&paths[4]).unwrap();
    let mut h = fresh(false);
    let saved = session(&paths, json!([{ "files": [&paths[2], &paths[3]] }, { "files": [&paths[4]] }]), &paths[0]);
    restart(&mut h, &saved);
    let app = h.state_mut();
    assert_eq!(names(app), vec![vec!["a.pdf".to_string(), "b.pdf".into()], vec!["d.pdf".into()]]);
    let _ = std::fs::remove_dir_all(dir);
}

/// With "Keep all documents in one window", every file opens in the one window, as do sessions
/// saved before there were windows.
#[test]
fn one_window_mode_and_old_sessions_open_everything_in_one_window() {
    let (dir, paths) = files("one");
    let mut h = fresh(true);
    restart(&mut h, &session(&paths, json!([{ "files": [&paths[3]] }]), &paths[0]));
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().win.views.len(), 5);
    assert!(h.state().restoring.is_none(), "the arrangement isn't kept for later");
    let mut h = fresh(false);
    let old = json!({ "files": paths.iter().map(|p| json!({ "path": p, "page": 0 })).collect::<Vec<_>>(), "active": &paths[0] });
    restart(&mut h, &old);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().win.views.len(), 5);
    let _ = std::fs::remove_dir_all(dir);
}

/// The first window keeps a tab: when all its own files are gone, a later window's file stays
/// in it rather than leaving it empty.
#[test]
fn the_first_window_keeps_a_tab_when_its_own_files_are_gone() {
    let (dir, paths) = files("first");
    std::fs::remove_file(&paths[0]).unwrap();
    let mut h = fresh(false);
    restart(&mut h, &json!({ "files": [{ "path": &paths[0] }, { "path": &paths[1] }], "active": &paths[0], "windows": [{ "files": [&paths[1]] }] }));
    assert_eq!(names(h.state()), vec![vec!["b.pdf".to_string()]]);
    let _ = std::fs::remove_dir_all(dir);
}

/// A saved place goes back on the monitor under it, kept inside; off every monitor it is not used.
#[test]
fn a_saved_place_is_used_only_on_a_monitor_there_now() {
    let monitor = crate::monitors::MonitorArea::whole(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0)));
    let place = SavedPlace { x: 100, y: 80, width: 800, height: 600, maximized: false };
    assert_eq!(place_on(place, 2.0, Some(monitor)), Some(Geometry { pos: Some(pos2(50.0, 40.0)), size: vec2(400.0, 300.0) }));
    // Too large for the monitor: kept inside it.
    let big = SavedPlace { x: 1500, y: 900, width: 3000, height: 2000, maximized: false };
    let g = place_on(big, 1.0, Some(monitor)).expect("on the monitor");
    let r = Rect::from_min_size(g.pos.unwrap(), g.size);
    assert!(monitor.bounds.contains_rect(r), "{r:?}");
    // On a monitor that is gone (the nearest one doesn't hold it), or none known: not used.
    let gone = SavedPlace { x: 4000, y: 100, width: 800, height: 600, maximized: false };
    assert_eq!(place_on(gone, 1.0, Some(monitor)), None);
    assert_eq!(place_on(place, 1.0, None), None);
}

/// The pixels a restored window is put at (from its own pass) are kept inside the monitor too:
/// a window saved with only its corner on screen comes back whole on it.
#[test]
fn the_pixels_a_window_is_restored_at_are_kept_inside_its_monitor() {
    let monitor = crate::monitors::MonitorArea::whole(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0)));
    let corner = SavedPlace { x: 1842, y: 1030, width: 800, height: 600, maximized: true };
    assert_eq!(kept_on(corner, Some(monitor)), Some(SavedPlace { x: 1120, y: 480, width: 800, height: 600, maximized: true }));
    let inside = SavedPlace { x: 100, y: 80, width: 800, height: 600, maximized: false };
    assert_eq!(kept_on(inside, Some(monitor)), Some(inside));
    assert_eq!(kept_on(SavedPlace { x: 4000, ..inside }, Some(monitor)), None);
}

/// A Quit cancelled at its question leaves the session as it was: windows and all. One that
/// goes ahead saves what was open when it began.
#[test]
fn a_cancelled_quit_or_close_keeps_the_saved_arrangement() {
    let (dir, paths) = files("cancel");
    let mut h = fresh(false);
    restart(&mut h, &session(&paths[..3], json!([{ "files": [&paths[2]] }]), &paths[0]));
    let app = h.state_mut();
    let mut host = Host::new(app);
    // c, in the second window, unsaved.
    let id = app.windows.others[0].id;
    app.with_window(Some(id), |a| {
        a.win.active = Some(0);
        assert!(a.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    });
    let before = app.open_session();
    assert_eq!(before.windows.len(), 1);
    let ctx = app.ctx.clone().unwrap();
    assert!(!app.request_quit(&ctx));
    app.show_other_windows(&mut host);
    let at = app.dialog_window();
    app.with_window(at, |a| a.resolve_close(&ctx, None));
    assert_eq!(app.session_to_save(), before, "cancelled: the session is what is open");
    // Close Window cancelled: the same.
    app.request_window_op(WindowOp::Close(Some(id)));
    app.show_other_windows(&mut host);
    let at = app.dialog_window();
    app.with_window(at, |a| a.resolve_close(&ctx, None));
    assert_eq!(app.session_to_save(), before);
    // Quit, Don't Save: c closes, and the arrangement saved is the one the quit began with.
    assert!(!app.request_quit(&ctx));
    app.show_other_windows(&mut host);
    let at = app.dialog_window();
    app.with_window(at, |a| a.resolve_close(&ctx, Some(false)));
    app.show_other_windows(&mut host);
    assert!(app.allow_quit, "quitting");
    assert_eq!(app.session_to_save(), before);
    let _ = std::fs::remove_dir_all(dir);
}

/// A window waits for its files: while one is still being opened (here, queued), nothing is
/// made; once it is open, the window comes back with it.
#[test]
fn a_window_waits_while_one_of_its_files_is_still_opening() {
    let (dir, paths) = files("wait");
    let mut h = fresh(false);
    let app = h.state_mut();
    app.last_session = LastSession::from_json(&session(&paths[..2], json!([{ "files": [&paths[1]] }]), &paths[0]));
    app.reopen_last_files(&[]);
    app.deferred_opens.push_back(crate::DeferredOpen { path: paths[1].clone(), after_open: None, then: None, display_override: None });
    let mut host = Host::new(app);
    app.show_other_windows(&mut host);
    assert_eq!(app.window_count(), 1, "still opening: no window yet");
    assert!(app.restoring.is_some(), "and the arrangement is kept for later");
    app.deferred_opens.clear();
    app.show_other_windows(&mut host);
    assert_eq!(names(app), vec![vec!["a.pdf".to_string()], vec!["b.pdf".into()]]);
    let _ = std::fs::remove_dir_all(dir);
}

/// In one-window mode no window is even made to be merged again: the arrangement is dropped and
/// every tab stays in the first window.
#[test]
fn one_window_mode_makes_no_window_at_all() {
    let (dir, paths) = files("none");
    let mut h = fresh(true);
    let app = h.state_mut();
    app.last_session = LastSession::from_json(&session(&paths[..2], json!([{ "files": [&paths[1]] }]), &paths[0]));
    app.reopen_last_files(&[]);
    app.restore_windows();
    assert!(app.windows.others.is_empty() && app.windows.pending.is_empty());
    assert!(app.restoring.is_none());
    assert_eq!(app.win.views.len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// Mixed DPI (simulated): a restored window is put at its saved pixels at its own scale, not
/// the first window's: on a 200 % monitor, (2400, 200) 1600×1200 px is (1200, 100) 800×600
/// points. If that lands it on a monitor of another scale, it is set once more there; then it
/// is left alone.
#[test]
fn a_restored_window_is_placed_at_its_own_scale() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    app.windows.root_ppp = 1.0;
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    app.show_other_windows(&mut host);
    let place = SavedPlace { x: 2400, y: 200, width: 1600, height: 1200, maximized: true };
    app.windows.others[0].pending_place = Some(super::PendingPlace { place, set_at: None, tries: 0 });
    let placed = |host: &mut Host| -> Vec<egui::ViewportCommand> {
        std::mem::take(&mut host.commands)
            .into_iter()
            .filter(|c| {
                matches!(c, egui::ViewportCommand::OuterPosition(_) | egui::ViewportCommand::InnerSize(_) | egui::ViewportCommand::Maximized(_))
            })
            .collect()
    };
    host.scale = Some(2.0);
    host.commands.clear();
    app.show_other_windows(&mut host);
    assert_eq!(
        placed(&mut host),
        vec![
            egui::ViewportCommand::OuterPosition(pos2(1200.0, 100.0)),
            egui::ViewportCommand::InnerSize(vec2(800.0, 600.0)),
            egui::ViewportCommand::Maximized(true)
        ],
        "the saved pixels at the window's 200 %, not the first window's 100 %"
    );
    // It landed on a 150 % monitor: once more, at that scale.
    host.scale = Some(1.5);
    app.show_other_windows(&mut host);
    assert_eq!(
        placed(&mut host)[..2],
        [egui::ViewportCommand::OuterPosition(pos2(1600.0, 2400.0 / 18.0)), egui::ViewportCommand::InnerSize(vec2(1600.0 / 1.5, 800.0))]
    );
    // Then it is left alone, whatever happens to it.
    host.scale = Some(1.25);
    app.show_other_windows(&mut host);
    app.show_other_windows(&mut host);
    assert!(placed(&mut host).is_empty());
    assert!(app.windows.others[0].pending_place.is_none());
}

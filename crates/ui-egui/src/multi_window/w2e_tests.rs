//! TAB.WINDOW.1 W2e: file dialogs belong to the window that asked, shortcuts in every window,
//! and the window control-channel tools.

use super::*;

/// The app with a, c in the first window and b alone in a second one.
fn two_windows() -> (Harness<'static, PdfCraftApp>, WindowId) {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    app.show_other_windows(&mut host);
    let id = app.windows.others[0].id;
    (h, id)
}

/// Ask for a file as window `key` would, during its pass.
fn ask_in(app: &mut PdfCraftApp, key: WindowKey) {
    let before = app.drawing_window;
    app.drawing_window = key;
    assert!(app.ask(crate::pickers::Ask::File(rfd::AsyncFileDialog::new()), None, |_, _| {}));
    app.drawing_window = before;
}

/// A file dialog belongs to the window that asked for it, including one that waited for
/// another dialog to close first.
#[test]
fn a_file_dialog_belongs_to_the_window_that_asked() {
    let (mut h, id) = two_windows();
    let app = h.state_mut();
    app.pick_override = Some(vec!["/nowhere/x.pdf".into()]);
    ask_in(app, None);
    assert_eq!(app.pickers.last_window, Some((egui::ViewportId::ROOT, false)), "the first window's (no native window in the test host)");
    use crate::control::Host as _;
    assert_eq!(app.state()["last_file_dialog"]["window"], 1);
    // The second window asks while that one is still answering: it waits, and keeps its window.
    ask_in(app, Some(id));
    assert_eq!(app.pickers.last_window.map(|(v, _)| v), Some(egui::ViewportId::ROOT));
    app.process_picked();
    assert_eq!(app.pickers.last_window, Some((id.viewport(), false)), "the waiting dialog belongs to the second window");
    assert_eq!(app.state()["last_file_dialog"]["window"], 2);
}

fn chord(key: egui::Key, shift: bool) -> egui::Event {
    let modifiers = if shift { egui::Modifiers::COMMAND | egui::Modifiers::SHIFT } else { egui::Modifiers::COMMAND };
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// Press `events` in the second window (its own input, in its pass).
fn press_in_second(app: &mut PdfCraftApp, host: &mut Host, events: Vec<egui::Event>) {
    host.events = events;
    app.show_other_windows(host);
    app.show_other_windows(host);
}

fn second_tabs(app: &PdfCraftApp) -> Vec<pdfcraft_engine::DocId> {
    ids(&app.windows.others[0].ui.views)
}

/// Shortcuts work in every window, on that window's own tab: Zoom in, Rotate view and Ctrl+W
/// pressed in the second window act there, and the first window is untouched.
#[test]
fn shortcuts_act_in_the_window_they_are_pressed_in() {
    let (mut h, _id) = two_windows();
    let app = h.state_mut();
    let mut host = Host::new(app);
    press_in_second(app, &mut host, Vec::new());
    let b = second_tabs(app)[0];
    let first: Vec<(f32, u16)> = app.win.views.iter().map(|v| (v.zoom, v.rotation)).collect();
    let zoom = app.windows.others[0].ui.views[0].zoom;
    press_in_second(app, &mut host, vec![chord(egui::Key::Plus, false)]);
    assert!(app.windows.others[0].ui.views[0].zoom > zoom, "Zoom in zoomed the second window's tab");
    press_in_second(app, &mut host, vec![chord(egui::Key::Plus, true)]);
    assert_eq!(app.windows.others[0].ui.views[0].rotation, 90, "Rotate view turned the second window's tab");
    assert_eq!(app.win.views.iter().map(|v| (v.zoom, v.rotation)).collect::<Vec<_>>(), first, "the first window untouched");
    // Ctrl+W closes b, the second window's only tab, and with it the window.
    press_in_second(app, &mut host, vec![chord(egui::Key::W, false)]);
    app.apply_window_ops();
    assert!(app.session.get(b).is_none(), "b closed from its own window");
    assert_eq!(app.window_count(), 1);
    assert_eq!(app.win.views.len(), 2);
}

/// T-3 still holds with shortcuts in every window: while a dialog is open in the first window,
/// the second window's shortcuts run nothing.
#[test]
fn a_held_window_runs_no_shortcuts() {
    let (mut h, _id) = two_windows();
    let app = h.state_mut();
    let mut host = Host::new(app);
    press_in_second(app, &mut host, Vec::new());
    let b = second_tabs(app)[0];
    app.dialog = Some(crate::Dialog::About);
    app.windows.dialog_window = Some(None);
    // (The zoom is not compared: a held window's page fits itself around the notice.)
    press_in_second(app, &mut host, vec![chord(egui::Key::Plus, false), chord(egui::Key::Plus, true), chord(egui::Key::W, false)]);
    app.apply_window_ops();
    assert_eq!(app.windows.others[0].ui.views[0].rotation, 0);
    assert_eq!(second_tabs(app), vec![b]);
    // The dialog closed, the same keys act.
    app.dialog = None;
    press_in_second(app, &mut host, vec![chord(egui::Key::Plus, true)]);
    assert_eq!(app.windows.others[0].ui.views[0].rotation, 90, "the keys reach the window once nothing holds it");
}

fn act(app: &mut PdfCraftApp, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    use crate::control::Host as _;
    let reply = app.window_action(method, &params);
    app.apply_window_ops();
    reply
}

/// The window tools (`ui.window.*`) do what the Window menu does, for any window and tab.
#[test]
fn the_window_tools_move_merge_focus_and_close() {
    use serde_json::json;
    let (mut h, id) = two_windows();
    let app = h.state_mut();
    let (a, b, c) = (app.win.views[0].id, app.windows.others[0].ui.views[0].id, app.win.views[1].id);
    // Tab 1 of window 1 (c) to a new window.
    act(app, "ui.window.new", json!({ "window": 1, "tab": 1 })).expect("moved");
    assert_eq!(app.window_count(), 3);
    let third = app.windows.others[1].id;
    assert_eq!(layout(app), vec![(None, vec![a]), (Some(id), vec![b]), (Some(third), vec![c])]);
    // b (window 2's only tab) to window 3: window 2 closes.
    act(app, "ui.window.move", json!({ "window": 2, "tab": 0, "to": 3 })).expect("moved");
    assert_eq!(layout(app), vec![(None, vec![a]), (Some(third), vec![c, b])]);
    // The rules of the menu: the first window keeps a tab, a lone tab doesn't make a new window.
    let e = act(app, "ui.window.move", json!({ "window": 1, "tab": 0, "to": 2 })).unwrap_err();
    assert!(e.contains("keeps at least one tab"), "{e}");
    let e = act(app, "ui.window.new", json!({ "window": 1, "tab": 0 })).unwrap_err();
    assert!(e.contains("only tab"), "{e}");
    act(app, "ui.window.focus", json!({ "window": 2 })).expect("focused");
    act(app, "ui.window.merge", json!({})).expect("merged");
    assert_eq!(app.window_count(), 1);
    assert_eq!(app.win.views.len(), 3);
    // Close: a second window again, closed by the tool; its clean document closes with it.
    act(app, "ui.window.new", json!({ "window": 1, "tab": 2 })).expect("moved");
    let last = app.windows.others[0].ui.views[0].id;
    act(app, "ui.window.close", json!({ "window": 2 })).expect("closed");
    assert_eq!(app.window_count(), 1);
    assert!(app.session.get(last).is_none());
}

fn layout(app: &PdfCraftApp) -> Vec<(WindowKey, Vec<pdfcraft_engine::DocId>)> {
    app.window_uis().into_iter().map(|(key, ui)| (key, ids(&ui.views))).collect()
}

/// A file opened again while another window shows it: that window's tab comes forward there, in
/// the display and with the tool the open asked for (W2d gap), and nothing changes in the window
/// that asked.
#[test]
fn opening_a_file_another_window_shows_applies_what_was_asked_there() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let dir = std::env::temp_dir().join(format!("pdfcraft-w2e-reveal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.pdf");
    std::fs::write(&path, pdf("d")).unwrap();
    let path = path.to_string_lossy().into_owned();
    app.open_bytes("d.pdf", Some(path.clone()), pdf("d")).unwrap();
    let d = app.win.views.last().unwrap().id;
    let mut host = Host::new(app);
    // d (tab 3) to a second window, which also gets a (so d is not its active tab).
    assert!(app.request_move_tab_to_new_window(3));
    app.show_other_windows(&mut host);
    assert!(app.request_move_tab_to_window(0, Some(app.windows.others[0].id)));
    app.show_other_windows(&mut host);
    assert_eq!(app.windows.others[0].ui.active, Some(1), "a is active in the second window");
    let first: Vec<(f32, u16)> = app.win.views.iter().map(|v| (v.zoom, v.rotation)).collect();
    let first_tool = app.win.quick_tool;
    let reopen = Reopen { display: Some((crate::canvas::PageLayout::TwoUp, false)), destination: None, then: Some("view.hand_tool") };
    assert!(app.reveal_in_other_window(&path, reopen));
    app.apply_window_ops();
    let second = &app.windows.others[0].ui;
    assert_eq!(second.active, Some(0), "d came forward in its window");
    assert_eq!(second.views[0].id, d);
    assert_eq!(second.quick_tool, crate::QuickTool::Hand, "the tool asked for, there");
    assert_eq!(second.views[0].layout, crate::canvas::PageLayout::TwoUp, "in the display asked for");
    assert_eq!(app.win.views.iter().map(|v| (v.zoom, v.rotation)).collect::<Vec<_>>(), first, "the first window untouched");
    assert_eq!(app.win.quick_tool, first_tool);
    let _ = std::fs::remove_dir_all(&dir);
}

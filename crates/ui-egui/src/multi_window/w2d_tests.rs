//! TAB.WINDOW.1 W2d: each window's own raw input, ids and control-channel capture.

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, RawInput, ViewportCommand, ViewportId};

use super::*;

fn other() -> ViewportId {
    ViewportId::from_hash_of("another window")
}

/// One pass of `ctx` as the window `viewport`.
fn pass_in(ctx: &egui::Context, viewport: ViewportId, f: impl FnMut(&egui::Context)) {
    let mut f = f;
    let mut raw = RawInput { viewport_id: viewport, ..Default::default() };
    raw.viewports.insert(viewport, egui::ViewportInfo::default());
    ctx.run_ui(raw, |ui| f(ui.ctx())).drop_without_applying_deltas();
}

fn releases(raw: &RawInput) -> usize {
    raw.events.iter().filter(|e| matches!(e, Event::PointerButton { button: PointerButton::Primary, pressed: false, .. })).count()
}

#[test]
fn a_desktop_grab_gives_the_release_back_to_the_window_it_was_made_in() {
    let ctx = egui::Context::default();
    pass_in(&ctx, other(), |ctx| crate::window_controls::desktop_grab(ctx, ViewportCommand::StartDrag));
    // The first window's next input keeps what it brings.
    let mut first = RawInput::default();
    crate::window_controls::release_after_desktop_grab(&ctx, &mut first);
    assert_eq!(releases(&first), 0, "the first window took another window's release");
    // The other window's next input gets the release, once.
    let mut input = RawInput { viewport_id: other(), ..Default::default() };
    other_window_raw_input(&ctx, &mut input);
    assert_eq!(releases(&input), 1, "{:?}", input.events);
    let mut again = RawInput { viewport_id: other(), ..Default::default() };
    other_window_raw_input(&ctx, &mut again);
    assert_eq!(releases(&again), 0);
    // And a grab in the first window still comes back there.
    pass_in(&ctx, ViewportId::ROOT, |ctx| crate::window_controls::desktop_grab(ctx, ViewportCommand::StartDrag));
    let mut input = RawInput { viewport_id: other(), ..Default::default() };
    other_window_raw_input(&ctx, &mut input);
    assert_eq!(releases(&input), 0);
    let mut first = RawInput::default();
    crate::window_controls::release_after_desktop_grab(&ctx, &mut first);
    assert_eq!(releases(&first), 1);
}

fn wheel(command: bool) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: egui::vec2(0.0, 1.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers { command, ..Modifiers::NONE },
        source: egui::MouseWheelSource::Unknown,
    }
}

fn dy(raw: &RawInput) -> Vec<f32> {
    raw.events.iter().filter_map(|e| if let Event::MouseWheel { delta, .. } = e { Some(delta.y) } else { None }).collect()
}

#[test]
fn ctrl_wheel_zooms_at_the_same_rate_in_every_window() {
    let ctx = egui::Context::default();
    for speed in crate::scroll_speed::ScrollSpeed::ALL {
        // The first window's input sets the line speed for every window.
        let mut first = RawInput { events: vec![wheel(true), wheel(false)], ..Default::default() };
        speed.apply(&ctx, &mut first);
        let mut input = RawInput { viewport_id: other(), events: vec![wheel(true), wheel(false)], ..Default::default() };
        other_window_raw_input(&ctx, &mut input);
        assert_eq!(dy(&input), dy(&first), "{speed:?}");
    }
}

#[test]
fn the_first_windows_input_is_left_to_its_own_hook() {
    let ctx = egui::Context::default();
    crate::scroll_speed::ScrollSpeed::Fast.set_line_speed(&ctx);
    pass_in(&ctx, ViewportId::ROOT, |ctx| crate::window_controls::desktop_grab(ctx, ViewportCommand::StartDrag));
    let mut first = RawInput { events: vec![wheel(true)], ..Default::default() };
    other_window_raw_input(&ctx, &mut first);
    assert_eq!(dy(&first), vec![1.0]);
    assert_eq!(releases(&first), 0);
}

#[test]
fn window_state_ids_are_per_window_and_the_first_window_keeps_its_own() {
    let ctx = egui::Context::default();
    let mut ids = Vec::new();
    for viewport in [ViewportId::ROOT, other(), ViewportId::from_hash_of("a third window")] {
        pass_in(&ctx, viewport, |ctx| ids.push(window_id(ctx, "palette_rect")));
    }
    assert_eq!(ids[0], egui::Id::unique("palette_rect"), "the first window's id changed");
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
}

/// Another window has no native capture: `ui.screenshot` gives the engine's render of the page it
/// shows, labelled so it can't pass for a screenshot (Q2); `ui.state` lists both windows.
#[test]
fn another_windows_screenshot_is_a_labelled_page_render() {
    use crate::control::Host as _;
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    app.show_other_windows(&mut host);
    let moved = app.windows.others[0].ui.views[0].id;
    let windows = app.state()["windows"].clone();
    assert_eq!(windows.as_array().map(Vec::len), Some(2), "{windows}");
    assert_eq!(windows[1]["screenshot"], "engine-render");
    assert_eq!(windows[1]["active_doc"], moved.0);
    assert_eq!(windows[1]["first"], false);

    let reply = app.window_page_render(1, 72.0).expect("a render");
    assert_eq!(reply["source"], "engine-render");
    assert_eq!(reply["native_window_capture"], false);
    assert!(reply["note"].as_str().is_some_and(|n| n.starts_with("Not a screenshot")), "{reply}");
    assert_eq!(
        (reply["window"].clone(), reply["doc"].clone(), reply["page"].clone()),
        (serde_json::json!(2), serde_json::json!(moved.0), serde_json::json!(1))
    );
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(reply["png_base64"].as_str().expect("png data")).expect("base64");
    assert_eq!(png.get(..8), Some(b"\x89PNG\r\n\x1a\n".as_slice()));
    assert!(reply["width"].as_u64().is_some_and(|w| w > 0));
    // A window that isn't there, and a window with no document, say so.
    assert!(app.window_page_render(2, 72.0).is_err_and(|e| e.contains("no window 3")));
    app.windows.others[0].ui.active = None;
    assert!(app.window_page_render(1, 72.0).is_err_and(|e| e.contains("shows no document")));
}

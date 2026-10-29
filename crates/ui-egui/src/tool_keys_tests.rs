//! The tool keys (this fork, KEY.1) in real frames: H, P, V and the pairs D I, D L, R C, their
//! timeout and cancelling, and the places where a letter is typing instead.

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;

use crate::comments::CommentTool;
use crate::{PdfCraftApp, QuickTool, measure_ui};

fn harness(documents: usize) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for n in 0..documents {
            let bytes = app.session.create_blank(612.0, 792.0, 2).unwrap();
            app.open_bytes(&format!("plan{n}.pdf"), None, bytes.as_ref().clone()).unwrap();
        }
        app
    });
    h.run_steps(4);
    h
}

/// Press `key` alone, then one frame (a quarter of a second in the harness).
fn press(h: &mut Harness<'static, PdfCraftApp>, key: Key) -> QuickTool {
    h.key_press(key);
    h.run_steps(1);
    h.state().quick_tool
}

fn typed(h: &mut Harness<'static, PdfCraftApp>, keys: &[Key]) -> QuickTool {
    for key in keys {
        press(h, *key);
    }
    h.state().quick_tool
}

#[test]
fn each_key_and_pair_picks_its_tool() {
    let mut h = harness(1);
    assert_eq!(press(&mut h, Key::H), QuickTool::Comment(CommentTool::Highlight));
    assert_eq!(typed(&mut h, &[Key::D, Key::I]), QuickTool::Measure(measure_ui::Tool::Distance));
    assert_eq!(typed(&mut h, &[Key::D, Key::L]), QuickTool::Comment(CommentTool::Line));
    assert_eq!(typed(&mut h, &[Key::R, Key::C]), QuickTool::Comment(CommentTool::Cloud));
    assert_eq!(press(&mut h, Key::P), QuickTool::Hand);
    assert_eq!(press(&mut h, Key::V), QuickTool::Select);
    // The second key alone is nothing.
    for key in [Key::I, Key::L, Key::C] {
        assert_eq!(press(&mut h, key), QuickTool::Select, "{key:?} alone");
    }
}

/// The first key of a pair waits, with a hint, and that is all it does; the pair's second key
/// finishes it.
#[test]
fn the_first_key_of_a_pair_waits_for_the_second() {
    let mut h = harness(1);
    assert_eq!(press(&mut h, Key::D), QuickTool::Select);
    assert_eq!(h.state().pending_keys.map(|p| p.first), Some(Key::D));
    assert_eq!(press(&mut h, Key::I), QuickTool::Measure(measure_ui::Tool::Distance));
    assert_eq!(h.state().pending_keys, None);
}

/// A pair not finished within the timeout is dropped: the second key alone does nothing.
#[test]
fn a_pair_times_out() {
    let mut h = harness(1);
    press(&mut h, Key::D);
    // The harness steps a quarter of a second: two seconds.
    h.run_steps(8);
    assert_eq!(h.state().pending_keys, None);
    assert_eq!(press(&mut h, Key::I), QuickTool::Select);
}

/// A key that doesn't finish a pair cancels it and counts on its own: D then H is Highlight, D
/// then R starts R's pair.
#[test]
fn another_key_cancels_a_pair_and_counts_alone() {
    let mut h = harness(1);
    assert_eq!(typed(&mut h, &[Key::D, Key::H]), QuickTool::Comment(CommentTool::Highlight));
    assert_eq!(h.state().pending_keys, None);
    assert_eq!(typed(&mut h, &[Key::D, Key::R, Key::C]), QuickTool::Comment(CommentTool::Cloud));
    // An arrow key cancels and is still the view's.
    press(&mut h, Key::D);
    press(&mut h, Key::ArrowDown);
    assert_eq!(h.state().pending_keys, None);
    assert_eq!(press(&mut h, Key::I), QuickTool::Comment(CommentTool::Cloud));
}

/// Escape drops a half-typed pair, and that is its one step: Select All stays.
#[test]
fn escape_drops_a_pair_and_nothing_else() {
    let mut h = harness(1);
    h.state_mut().views[0].all_text = true;
    press(&mut h, Key::D);
    press(&mut h, Key::Escape);
    assert_eq!(h.state().pending_keys, None);
    assert!(h.state().views[0].all_text, "the same press doesn't also clear Select All");
    assert_eq!(press(&mut h, Key::I), QuickTool::Select);
    press(&mut h, Key::Escape);
    assert!(!h.state().views[0].all_text, "the next press does");
}

/// With a modifier a letter is another shortcut, and a held key repeats nothing.
#[test]
fn modified_and_repeated_keys_pick_no_tool() {
    let mut h = harness(1);
    for modifiers in [Modifiers::SHIFT, Modifiers::ALT, Modifiers::COMMAND] {
        h.key_press_modifiers(modifiers, Key::H);
        h.run_steps(1);
        assert_eq!(h.state().quick_tool, QuickTool::Select, "{modifiers:?} H");
        h.key_press_modifiers(modifiers, Key::R);
        h.run_steps(1);
        assert_eq!(h.state().pending_keys, None, "{modifiers:?} R");
    }
    // A pair half typed is dropped by a modified key.
    press(&mut h, Key::D);
    h.key_press_modifiers(Modifiers::SHIFT, Key::I);
    h.run_steps(1);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    // Auto-repeat: only the first press of a held H counts.
    let repeat = Event::Key { key: Key::H, physical_key: None, pressed: true, repeat: true, modifiers: Modifiers::NONE };
    h.event(repeat);
    h.run_steps(1);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "a repeat is not a press");
}

/// Typing in the find bar keeps every letter for the text.
#[test]
fn letters_typed_in_the_find_bar_are_text() {
    let mut h = harness(1);
    h.state_mut().views[0].open_find();
    h.run_steps(4);
    for key in [Key::D, Key::I, Key::R, Key::C, Key::H, Key::P, Key::V] {
        assert_eq!(press(&mut h, key), QuickTool::Select, "{key:?} in the find bar");
        assert_eq!(h.state().pending_keys, None);
    }
}

/// A pair belongs to the tab it was started in.
#[test]
fn another_tab_drops_a_pair() {
    let mut h = harness(2);
    h.state_mut().active = Some(0);
    h.run_steps(2);
    press(&mut h, Key::D);
    h.state_mut().active = Some(1);
    h.run_steps(1);
    assert_eq!(h.state().pending_keys, None);
    assert_eq!(press(&mut h, Key::I), QuickTool::Select);
}

/// A key is taken once, also when `logic` runs again with that frame's input still in place (a
/// hidden window): a tool picked in between isn't picked over again.
#[test]
fn a_key_runs_once_whatever_the_logic_passes() {
    let mut h = harness(1);
    let ctx = h.ctx.clone();
    let screen = ctx.content_rect();
    let key = Event::Key { key: Key::H, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
    let raw = egui::RawInput { events: vec![key], screen_rect: Some(screen), ..Default::default() };
    let app = h.state_mut();
    let mut output = ctx.run_ui(raw, |ui| {
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::logic(app, ui.ctx(), &mut frame);
        eframe::App::ui(app, ui, &mut frame);
    });
    output.textures_delta.clear();
    assert_eq!(h.state().quick_tool, QuickTool::Comment(CommentTool::Highlight));
    h.state_mut().quick_tool = QuickTool::Select;
    for _ in 0..3 {
        let app = h.state_mut();
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| eframe::App::logic(app, ctx, &mut eframe::Frame::_new_kittest()));
    }
    assert_eq!(h.state().quick_tool, QuickTool::Select, "the same press isn't taken again");
}

/// The shortcuts dialog lists every tool key from the one table.
#[test]
fn the_shortcuts_dialog_lists_the_tool_keys() {
    crate::i18n::set_current(crate::i18n::Lang::from_code("en").expect("en"));
    let rows = crate::tool_keys::dialog_rows();
    let want = [
        ("V", "Select (V)"),
        ("P / Space (hold)", "Hand (P)"),
        ("H", "Highlight text"),
        ("D, then I", "Measure distance"),
        ("D, then L", "Draw a line"),
        ("R, then C", "Draw a cloud"),
    ];
    assert_eq!(rows, want.map(|(k, v)| (k.to_string(), v.to_string())).to_vec());
}

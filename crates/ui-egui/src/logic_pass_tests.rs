//! The fork's per-frame work under egui 0.37's `App::logic` / `App::ui` split (SYNC.13).
//!
//! `logic` may run more than once for one frame, and alone while the window is hidden
//! (`Context::run_logic`), where the input it sees is the last UI pass's, events included. None
//! of that may run a deferred shortcut twice, take a second Escape step for one press, or refresh
//! a duplicate tab twice for one edit.

use egui_kittest::Harness;
use pdfcraft_engine::Edit;

use crate::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_blank(612.0, 792.0, 8).unwrap();
        app.open_bytes("plan.pdf", None, bytes.as_ref().clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// `App::logic` alone, `times` times, as egui runs it while the window is hidden: no new input,
/// the last pass's left in place.
fn hidden_logic(h: &mut Harness<'static, PdfCraftApp>, times: usize) {
    let ctx = h.ctx.clone();
    for _ in 0..times {
        let app = h.state_mut();
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| eframe::App::logic(app, ctx, &mut eframe::Frame::_new_kittest()));
    }
}

/// A shortcut deferred while a field had the keyboard runs once, on the next `logic`, in the view
/// it was pressed in, however many times `logic` runs.
#[test]
fn a_deferred_shortcut_runs_once_whatever_the_logic_passes() {
    let mut h = harness();
    assert!(!h.state().thin_lines);
    let key = h.state().active_key();
    // Thin lines toggles: run twice, it would be off again.
    h.state_mut().deferred_commands.push(("view.thin_lines", key));
    hidden_logic(&mut h, 3);
    assert!(h.state().thin_lines, "run once");
    h.run_steps(3);
    assert!(h.state().thin_lines, "and not again");
}

/// One frame of the app with `events`, as eframe runs it: `logic`, then `ui`, in `Context::run_ui`.
fn frame_with(h: &mut Harness<'static, PdfCraftApp>, events: Vec<egui::Event>) {
    let ctx = h.ctx.clone();
    let screen = ctx.content_rect();
    let raw = egui::RawInput { events, screen_rect: Some(screen), ..Default::default() };
    let app = h.state_mut();
    let _ = ctx.run_ui(raw, |ui| {
        let mut frame = eframe::Frame::_new_kittest();
        eframe::App::logic(app, ui.ctx(), &mut frame);
        eframe::App::ui(app, ui, &mut frame);
    });
}

/// One Escape press takes one step back, also when `logic` runs again with that pass's input
/// still in place (a hidden window): Select All goes, the find bar stays.
#[test]
fn one_escape_takes_one_step_across_logic_passes() {
    let mut h = harness();
    h.state_mut().views[0].open_find();
    if let Some(f) = h.state_mut().views[0].find.as_mut() {
        f.focus = false;
    }
    h.run_steps(2);
    h.ctx.memory_mut(|m| m.surrender_focus(crate::canvas::find_input_id()));
    h.state_mut().views[0].all_text = true;
    h.run_steps(2);
    let key = |pressed| egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    frame_with(&mut h, vec![key(true), key(false)]);
    assert!(!h.state().views[0].all_text, "the press clears Select All");
    assert!(h.state().views[0].find.is_some());
    hidden_logic(&mut h, 3);
    assert!(h.state().views[0].find.is_some(), "the same press doesn't also close the find bar");
}

/// A duplicate tab catches up once with an edit made through the other tab: the next refresh,
/// with nothing changed since, leaves it as it is (`sync_shared_views` runs in every UI pass).
#[test]
fn a_duplicate_tab_catches_up_once_per_edit() {
    let mut h = harness();
    let twin = h.state_mut().duplicate_view(0).expect("a duplicate");
    h.state_mut().active = Some(0);
    h.run_steps(6);
    let id = h.state().views[0].id;
    let edit = |h: &mut Harness<'static, PdfCraftApp>, title: &str| {
        h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: title.into() }).unwrap();
    };
    edit(&mut h, "First");
    h.state_mut().sync_shared_views();
    // A jump asked of the other tab after it caught up: a second refresh would drop it.
    h.state_mut().views[twin].goto = Some((5, 0.0));
    h.state_mut().sync_shared_views();
    h.state_mut().sync_shared_views();
    assert_eq!(h.state().views[twin].goto, Some((5, 0.0)), "caught up once, not on every pass");
    edit(&mut h, "Second");
    h.state_mut().sync_shared_views();
    assert_eq!(h.state().views[twin].goto, None, "a new edit refreshes it again");
}

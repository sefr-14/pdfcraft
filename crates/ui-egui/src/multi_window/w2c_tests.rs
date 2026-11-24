//! TAB.WINDOW.1 W2c: Close and Quit across windows, the first window's handoff (H1) and the
//! application-modal dialog (T-3).

use egui::{Pos2, Rect, ViewportCommand, vec2};
use pdfcraft_engine::Edit;

use super::*;
use crate::{CloseRequest, Dialog};

fn settle(app: &mut PdfCraftApp, host: &mut Host) {
    app.show_other_windows(host);
}

/// An unsaved edit on tab `index` of the current window.
fn dirty(app: &mut PdfCraftApp, index: usize) {
    app.win.active = Some(index);
    assert!(app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
}

/// Answer the open question where it shows: `Some(false)` Don't Save, `None` Cancel.
fn answer(app: &mut PdfCraftApp, choice: Option<bool>) {
    let ctx = app.ctx.clone().expect("a context");
    let at = app.dialog_window();
    assert!(app.with_window(at, |a| a.resolve_close(&ctx, choice)).is_some());
}

/// The app with tabs a and c in the first window and b, unsaved, alone in a second one.
fn b_unsaved_in_its_own_window() -> (Harness<'static, PdfCraftApp>, WindowId) {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    dirty(app, 1);
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let id = app.windows.others[0].id;
    (h, id)
}

fn layout(app: &PdfCraftApp) -> Vec<(WindowKey, Vec<pdfcraft_engine::DocId>)> {
    app.window_uis().into_iter().map(|(key, ui)| (key, ids(&ui.views))).collect()
}

// ------------------------------------------------------------------------------ quit, close all

#[test]
fn quit_asks_in_the_window_holding_the_unsaved_document_and_cancel_leaves_every_window_as_it_was() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let before = layout(app);
    let ctx = app.ctx.clone().unwrap();
    assert!(!app.request_quit(&ctx));
    assert_eq!(app.close_request, Some(CloseRequest::Quit));
    assert_eq!(app.dialog_window(), Some(id), "asked in the window holding b");
    assert_eq!(layout(app), before, "nothing moved while asking");
    answer(app, None);
    assert_eq!(app.close_request, None);
    assert!(!app.allow_quit);
    assert_eq!(layout(app), before, "cancelled: every window and tab as it was");
}

#[test]
fn quit_asks_in_each_window_in_turn_and_quits_after_the_last_answer() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let all = app.open_documents();
    dirty(app, 0);
    let ctx = app.ctx.clone().unwrap();
    assert!(!app.request_quit(&ctx));
    assert_eq!(app.dialog_window(), None, "the first window's document first");
    answer(app, Some(false));
    assert!(app.session.get(all[0]).is_none(), "a closed without saving");
    assert_eq!(app.close_request, Some(CloseRequest::Quit), "and the question moves on");
    assert_eq!(app.dialog_window(), Some(id), "to the window holding b");
    answer(app, Some(false));
    assert!(app.allow_quit, "nothing left to ask: the app quits");
}

#[test]
fn quitting_from_another_window_asks_without_moving_any_tab() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let before = layout(app);
    let mut host = Host::new(app);
    // ⌘Q in the other window: done between passes, where every window can be reached.
    app.request_window_op(WindowOp::Quit);
    settle(app, &mut host);
    assert_eq!(app.close_request, Some(CloseRequest::Quit));
    assert_eq!(app.dialog_window(), Some(id));
    assert_eq!(layout(app), before);
}

#[test]
fn close_all_closes_clean_tabs_in_every_window_and_asks_where_the_rest_are() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let b = app.windows.others[0].ui.views[0].id;
    let mut host = Host::new(app);
    // A clean tab in the other window too.
    app.win.active = Some(1);
    assert!(app.request_move_tab_to_window(1, Some(id)));
    settle(app, &mut host);
    app.close_all();
    assert_eq!(app.win.views.len(), 0, "a closed");
    assert_eq!(ids(&app.windows.others[0].ui.views), vec![b], "c closed, b waits for its answer");
    assert_eq!(app.close_request, Some(CloseRequest::All));
    assert_eq!(app.dialog_window(), Some(id));
    answer(app, Some(false));
    settle(app, &mut host);
    assert!(app.windows.others.is_empty(), "its window closed with its last tab");
    assert!(app.session.docs().is_empty());
    assert!(!app.allow_quit, "Close all leaves the app open");
}

// ---------------------------------------------------------------------------- close window

#[test]
fn close_window_asks_about_its_unsaved_document_and_cancel_keeps_it() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let b = app.windows.others[0].ui.views[0].id;
    let mut host = Host::new(app);
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(app.close_request, Some(CloseRequest::Window(Some(id))));
    assert_eq!(app.dialog_window(), Some(id), "asked in that window");
    answer(app, None);
    assert_eq!(app.window_count(), 2, "cancelled: the window stays");
    assert!(app.session.get(b).is_some_and(|d| d.dirty));
    // Again, and Don't Save: the document closes, then the window.
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    answer(app, Some(false));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 1);
    assert!(app.session.get(b).is_none());
    assert_eq!(app.win.views.len(), 2, "the first window untouched");
}

#[test]
fn close_window_doesnt_ask_about_a_document_another_window_still_shows() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let a = app.win.views[0].id;
    dirty(app, 0);
    assert!(app.duplicate_view(0).is_some());
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let id = app.windows.others[0].id;
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(app.close_request, None, "nothing is lost: a stays open in the first window");
    assert_eq!(app.window_count(), 1);
    assert!(app.session.get(a).is_some_and(|d| d.dirty));
}

#[test]
fn the_other_windows_title_bar_close_asks_too() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let mut host = Host::new(app);
    host.close = true;
    settle(app, &mut host);
    host.close = false;
    assert_eq!(app.close_request, Some(CloseRequest::Window(Some(id))));
    assert_eq!(app.window_count(), 2);
}

// ------------------------------------------------------------------------------------- H1

#[test]
fn closing_the_first_window_hands_its_place_to_another_and_keeps_the_others() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = app.open_documents();
    let mut host = Host::new(app);
    // b to a second window, c to a third (in Read mode).
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    app.win.mode = crate::Mode::Read;
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    app.win.mode = crate::Mode::AllTools;
    let (second, third) = (app.windows.others[0].id, app.windows.others[1].id);
    app.windows.others[1].last = Some(Placement {
        outer: Rect::from_min_size(Pos2::new(212.0, 81.0), vec2(1560.0, 997.0)),
        inner_size: vec2(1558.0, 966.0),
        maximized: false,
    });
    // The third window was used last, though the second comes first in the list.
    app.windows.focus_order = vec![Some(second), Some(third)];
    assert!(app.execute("window.close"));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 2, "one window fewer");
    assert_eq!(ids(&app.win.views), vec![all[2]], "the first window now shows the window used last");
    assert_eq!(app.win.mode, crate::Mode::Read, "with its mode");
    assert!(app.win.window_title.is_empty(), "its title is sent again, now to the first window");
    assert_eq!(app.windows.others[0].id, second, "the second window carries on");
    assert_eq!(ids(&app.windows.others[0].ui.views), vec![all[1]]);
    assert!(app.session.get(all[0]).is_none(), "a, shown only in the first window, closed");
    assert!(app.session.get(all[1]).is_some() && app.session.get(all[2]).is_some());
    assert!(!app.allow_quit, "the app keeps running");
}

#[test]
fn closing_the_first_window_asks_about_its_unsaved_work_first() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    dirty(app, 0);
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let before = layout(app);
    app.request_window_op(WindowOp::Close(None));
    settle(app, &mut host);
    assert_eq!(app.close_request, Some(CloseRequest::Window(None)));
    assert_eq!(app.dialog_window(), None, "asked in the first window");
    answer(app, None);
    assert_eq!(layout(app), before, "cancelled: nothing changed");
    app.request_window_op(WindowOp::Close(None));
    settle(app, &mut host);
    answer(app, Some(false));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 1, "then the other window took the first one's place");
    assert_eq!(app.win.views.len(), 1);
}

#[test]
fn the_first_windows_title_bar_close_with_others_open_doesnt_quit() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let ctx = egui::Context::default();
    let mut input = egui::RawInput::default();
    input.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    let out = ctx.run_logic(&input, |ctx| app.hold_close_for_unsaved(ctx));
    let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
    assert!(commands.contains(&ViewportCommand::CancelClose), "{commands:?}");
    assert!(!app.allow_quit);
    assert!(app.windows.pending.contains(&WindowOp::Close(None)), "the first window closes instead (H1)");
}

#[test]
fn the_first_window_is_told_the_other_windows_place_in_its_own_points() {
    let placement =
        Placement { outer: Rect::from_min_size(Pos2::new(200.0, 100.0), vec2(800.0, 640.0)), inner_size: vec2(800.0, 600.0), maximized: false };
    let commands = rehome_commands(placement, 2.0);
    assert_eq!(
        commands,
        vec![
            ViewportCommand::Maximized(false),
            ViewportCommand::OuterPosition(Pos2::new(100.0, 50.0)),
            ViewportCommand::InnerSize(vec2(400.0, 300.0)),
            ViewportCommand::Focus
        ]
    );
    let commands = rehome_commands(Placement { maximized: true, ..placement }, f32::NAN);
    assert_eq!(commands[1], ViewportCommand::OuterPosition(Pos2::new(200.0, 100.0)), "an unusable scale counts as 1");
    assert!(commands.contains(&ViewportCommand::Maximized(true)));
}

// ------------------------------------------------------------------------------------- T-3

#[test]
fn a_dialog_shows_in_the_window_it_was_opened_from_and_holds_the_others() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let id = app.windows.others[0].id;
    // Opened during the other window's pass.
    app.with_window(Some(id), |a| {
        a.dialog = Some(Dialog::About);
        a.note_dialog_window();
    });
    assert_eq!(app.dialog_window(), Some(id));
    assert!(app.blocked_by_dialog(), "the first window is held");
    assert_eq!(app.with_window(Some(id), |a| a.blocked_by_dialog()), Some(false), "the dialog's own window isn't");
    // While it is open, window changes wait: closing a window brings the dialog forward instead.
    let before = layout(app);
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(layout(app), before);
    assert_eq!(app.dialog, Some(Dialog::About));
    // Closed: nothing is held any more.
    app.dialog = None;
    app.note_dialog_window();
    assert!(!app.blocked_by_dialog());
    assert_eq!(app.windows.dialog_window, None);
}

#[test]
fn a_dialog_opened_in_the_first_windows_pass_shows_there() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    app.dialog = Some(Dialog::About);
    app.note_dialog_window();
    assert_eq!(app.dialog_window(), None);
    assert!(!app.blocked_by_dialog());
    let id = app.windows.others[0].id;
    assert_eq!(app.with_window(Some(id), |a| a.blocked_by_dialog()), Some(true));
}

#[test]
fn a_question_whose_window_has_nothing_left_to_ask_moves_to_the_window_that_has() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let b = app.windows.others[0].ui.views[0].id;
    let ctx = app.ctx.clone().unwrap();
    assert!(!app.request_quit(&ctx));
    assert_eq!(app.dialog_window(), Some(id));
    // Before the answer, b's change is undone and a gets one in the first window.
    assert!(app.session.undo(b).is_ok());
    dirty(app, 0);
    answer(app, Some(false));
    assert!(!app.allow_quit, "a still has unsaved work: no quitting yet");
    assert_eq!(app.close_request, Some(CloseRequest::Quit));
    assert_eq!(app.dialog_window(), None, "the question moved to the first window");
}

/// A review of `doc` with one word corrected, as the OCR review screen holds it.
fn review(app: &mut PdfCraftApp, doc: pdfcraft_engine::DocId) {
    use pdfcraft_engine::ocr::{OcrPage, OcrSettings, PlacedWord};
    let job = app.session.ocr_job(doc, &[0], OcrSettings::default()).expect("an OCR job");
    let word =
        PlacedWord { text: "w".into(), origin: [72.0, 700.0], across: [40.0, 0.0], up: [0.0, 12.0], confidence: Some(95.0), source: "ocrs".into() };
    let page = OcrPage { already_on_page: 0, page: 0, words: vec![word], skipped: None, notes: Vec::new() };
    app.ocr_verify.load(doc, job.guard.clone(), vec![page]);
    assert!(app.ocr_verify.correct((0, 0), "Edited"));
    app.ocr_verify.open = false;
}

/// The app with a, c in the first window and b, clean and under OCR review, alone in a second one.
fn b_under_review_in_its_own_window() -> (Harness<'static, PdfCraftApp>, WindowId, pdfcraft_engine::DocId) {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let b = app.win.views[1].id;
    review(app, b);
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let id = app.windows.others[0].id;
    (h, id, b)
}

/// Close Window holds for a document under OCR review that closes with the window (#6485), as
/// closing its tab does: nothing closes, the review is kept and comes forward. After Reject the
/// window closes.
#[test]
fn close_window_holds_for_a_document_under_ocr_review() {
    let (mut h, id, b) = b_under_review_in_its_own_window();
    let app = h.state_mut();
    let mut host = Host::new(app);
    let before = layout(app);
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(layout(app), before, "the close was held: every window and tab as it was");
    assert!(app.session.get(b).is_some(), "the document stays open");
    assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"), "with its review as it was");
    assert!(app.ocr_verify.open, "the review comes forward");
    assert!(app.windows.others[0].ui.toast.is_some(), "a notice in that window says why");
    assert!(app.close_request.is_none(), "and nothing is left half done");
    assert!(app.ocr_verify.reject());
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 1, "after Reject the window closes");
    assert!(app.session.get(b).is_none());
}

/// The review holds the first window's close too: no handoff (H1) happens while it would close
/// the reviewed document.
#[test]
fn the_first_windows_close_holds_for_its_reviewed_document() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let a = app.win.views[0].id;
    review(app, a);
    let mut host = Host::new(app);
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let before = layout(app);
    app.request_window_op(WindowOp::Close(None));
    settle(app, &mut host);
    assert_eq!(layout(app), before, "the first window's close is held: no handoff");
    assert!(app.ocr_verify.open && app.session.get(a).is_some());
}

/// A window whose reviewed document another window still shows closes: the document stays
/// open there, with its review.
#[test]
fn close_window_doesnt_hold_for_a_reviewed_document_another_window_shows() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let a = app.win.views[0].id;
    review(app, a);
    assert!(app.duplicate_view(0).is_some());
    let mut host = Host::new(app);
    // The duplicate (tab 1) goes to a second window; the first window keeps a, b, c.
    assert!(app.request_move_tab_to_new_window(1));
    settle(app, &mut host);
    let id = app.windows.others[0].id;
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 1);
    assert!(app.session.get(a).is_some() && app.ocr_verify.holds_reading_of(a));
    assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"));
}

/// Close all and Quit from the first window hold for a review of a document in another window:
/// no tab closes anywhere, and Quit is called off.
#[test]
fn close_all_and_quit_hold_for_a_review_in_another_window() {
    let (mut h, _id, b) = b_under_review_in_its_own_window();
    let app = h.state_mut();
    let mut host = Host::new(app);
    let before = layout(app);
    app.close_all();
    settle(app, &mut host);
    assert_eq!(layout(app), before, "Close all closed nothing");
    assert!(app.ocr_verify.open);
    let ctx = app.ctx.clone().expect("a context");
    assert!(!app.request_quit(&ctx), "Quit is held");
    settle(app, &mut host);
    assert_eq!(layout(app), before);
    assert!(!app.allow_quit && app.session.get(b).is_some());
}

/// A review started while Close Window's question is open holds the close when the question is
/// answered: the reviewed document and its window stay, and the close is cancelled cleanly.
#[test]
fn a_review_started_during_close_windows_question_cancels_the_close() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let b = app.windows.others[0].ui.views[0].id;
    let mut host = Host::new(app);
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(app.close_request, Some(CloseRequest::Window(Some(id))));
    review(app, b);
    answer(app, Some(false));
    settle(app, &mut host);
    assert_eq!(app.window_count(), 2, "the window stays");
    assert!(app.session.get(b).is_some_and(|d| d.dirty), "its document stays open and unsaved");
    assert!(app.close_request.is_none(), "no question left open");
    assert_eq!(app.ocr_verify.text_of((0, 0)), Some("Edited"));
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
}

/// T-3 with the keyboard: while Close Window's question shows in the second window, the first
/// window's keys answer nothing and run nothing (Escape, Enter, Ctrl+W); the second window's own
/// keys answer it (Ctrl+D is Don't Save), and its window then closes.
#[test]
fn only_the_window_showing_the_question_reads_its_keys() {
    let (mut h, id) = b_unsaved_in_its_own_window();
    let app = h.state_mut();
    let b = app.windows.others[0].ui.views[0].id;
    let mut host = Host::new(app);
    app.request_window_op(WindowOp::Close(Some(id)));
    settle(app, &mut host);
    assert_eq!(app.close_request, Some(CloseRequest::Window(Some(id))));
    assert_eq!(app.dialog_window(), Some(id), "asked in the second window");
    let before = layout(app);
    // The first window's shortcut step, with its own input (a native window's input is its own;
    // the test harness would show every key to every window it embeds).
    let first = egui::Context::default();
    let ctrl = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
    for event in [key(egui::Key::Escape), key(egui::Key::Enter), ctrl(egui::Key::W)] {
        let input = egui::RawInput { events: vec![event.clone()], ..Default::default() };
        first.run_ui(input, |ui| app.shortcuts(ui.ctx())).textures_delta.clear();
        settle(app, &mut host);
        assert_eq!(app.close_request, Some(CloseRequest::Window(Some(id))), "{event:?} in the first window answered the question");
        assert_eq!(layout(app), before, "{event:?} in the first window ran something");
    }
    // In the second window, Ctrl+D is Don't Save: b closes, then its window.
    host.events.push(ctrl(egui::Key::D));
    settle(app, &mut host);
    settle(app, &mut host);
    assert!(app.close_request.is_none());
    assert!(app.session.get(b).is_none(), "answered Don't Save in its own window");
    assert_eq!(app.window_count(), 1);
}

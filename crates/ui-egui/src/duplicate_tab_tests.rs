//! Duplicate Tab (OV7.1): two tabs on one document navigate independently, and the document
//! (its edits, undo, unsaved state, saving and closing) stays one. These run real frames, so
//! egui's per-view memory (the scroll offset among it) is exercised as the app uses it.

use egui::{Vec2, vec2};
use egui_kittest::Harness;
use pdfcraft_engine::Edit;

use crate::PdfCraftApp;
use crate::canvas::{Fit, ViewKey};
use crate::tab_context::{Action, MAX_VIEWS_PER_DOCUMENT};

const PAGES: usize = 30;

fn app_with(pages: usize) -> PdfCraftApp {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    let bytes = app.session.create_blank(612.0, 792.0, pages).unwrap();
    app.open_bytes("plan.pdf", None, bytes.as_ref().clone()).unwrap();
    app
}

fn harness_with(pages: usize) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(move |_| app_with(pages));
    h.run_steps(4);
    h
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(PAGES)
}

/// Everything that makes up where a view is.
#[derive(Clone, Debug, PartialEq)]
struct Place {
    current: usize,
    zoom: f32,
    fit: Fit,
    scroll: Vec2,
    back: Vec<usize>,
    forward: Vec<usize>,
}

fn place(h: &Harness<'static, PdfCraftApp>, tab: usize) -> Place {
    let v = &h.state().views[tab];
    Place { current: v.current, zoom: v.zoom, fit: v.fit, scroll: v.last_scroll, back: v.back.clone(), forward: v.forward.clone() }
}

fn show(h: &mut Harness<'static, PdfCraftApp>, tab: usize) {
    h.state_mut().active = Some(tab);
    h.run_steps(6);
}

/// Wheel `dy` points over the active tab's page view.
fn wheel(h: &mut Harness<'static, PdfCraftApp>, dy: f32) {
    let tab = h.state().active.unwrap();
    let centre = h.state().views[tab].viewport_rect().center();
    h.hover_at(centre);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(8);
}

fn duplicate(h: &mut Harness<'static, PdfCraftApp>, tab: usize) -> usize {
    let at = h.state_mut().duplicate_view(tab).expect("the tab can be duplicated");
    h.run_steps(6);
    at
}

fn close_enough(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 1.0
}

/// Tab 1 on page 3 at 100% at the top of the sheet; tab 2 (its duplicate) on page 18 at 150%,
/// scrolled down into the sheet.
fn two_places() -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    {
        let v = &mut h.state_mut().views[0];
        v.set_zoom(1.0);
        v.go_to_page(2);
    }
    h.run_steps(6);
    duplicate(&mut h, 0);
    {
        let v = &mut h.state_mut().views[1];
        v.set_zoom(1.5);
        v.go_to_page(17);
    }
    h.run_steps(6);
    wheel(&mut h, -400.0);
    h
}

// ── Navigation independence ──────────────────────────────────────────────────────────────────

#[test]
fn a_duplicate_starts_at_the_source_place_then_keeps_its_own_page() {
    let mut h = harness();
    h.state_mut().views[0].go_to_page(4);
    h.run_steps(6);
    wheel(&mut h, -200.0);
    let source = place(&h, 0);
    let at = duplicate(&mut h, 0);
    assert_eq!(at, 1);
    assert_eq!(h.state().active, Some(1));
    let (a, b) = (&h.state().views[0], &h.state().views[1]);
    assert_eq!(a.id, b.id, "one document, two views");
    assert_ne!(a.key, b.key);
    assert_eq!(h.state().open_documents().len(), 1, "the document is not opened again");
    let copy = place(&h, 1);
    assert_eq!((copy.current, copy.zoom, copy.fit, &copy.back), (source.current, source.zoom, source.fit, &source.back));
    assert!(close_enough(copy.scroll, source.scroll), "the duplicate opens where the source was: {:?} vs {:?}", copy.scroll, source.scroll);

    // Tab 1 to page 3, tab 2 to page 18.
    show(&mut h, 0);
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(6);
    show(&mut h, 1);
    h.state_mut().views[1].go_to_page(17);
    h.run_steps(6);
    for _ in 0..3 {
        show(&mut h, 0);
        assert_eq!(h.state().views[0].current, 2);
        show(&mut h, 1);
        assert_eq!(h.state().views[1].current, 17);
    }
}

#[test]
fn b_a_page_change_in_one_tab_leaves_the_other_unchanged() {
    let mut h = two_places();
    let first = place(&h, 0);
    h.state_mut().views[1].step_page(true);
    h.run_steps(6);
    h.state_mut().views[1].go_to_page(25);
    h.run_steps(6);
    assert_eq!(place(&h, 0), first, "tab 1 is untouched by tab 2's navigation");
    show(&mut h, 0);
    assert_eq!(place(&h, 0).current, first.current);
    let second = place(&h, 1);
    h.state_mut().views[0].go_to_page(9);
    h.run_steps(6);
    assert_eq!(place(&h, 1), second, "and the other way round");
}

#[test]
fn c_scroll_and_zoom_are_kept_per_tab() {
    let mut h = two_places();
    let (first, second) = (place(&h, 0), place(&h, 1));
    assert_eq!((first.zoom, second.zoom), (1.0, 1.5));
    assert!(!close_enough(first.scroll, second.scroll), "the tabs are scrolled to different places");
    for _ in 0..3 {
        show(&mut h, 0);
        let now = place(&h, 0);
        assert_eq!(now.zoom, 1.0);
        assert!(close_enough(now.scroll, first.scroll), "tab 1 kept its own scroll position: {:?} vs {:?}", now.scroll, first.scroll);
        show(&mut h, 1);
        let now = place(&h, 1);
        assert_eq!(now.zoom, 1.5);
        assert!(close_enough(now.scroll, second.scroll), "tab 2 kept its own scroll position: {:?} vs {:?}", now.scroll, second.scroll);
    }
    // Scrolling and zooming one leaves the other where it is.
    wheel(&mut h, -300.0);
    h.state_mut().views[1].set_zoom(2.0);
    h.run_steps(6);
    show(&mut h, 0);
    assert_eq!(place(&h, 0).zoom, 1.0);
    assert!(close_enough(place(&h, 0).scroll, first.scroll));
}

#[test]
fn d_back_and_forward_are_per_tab() {
    let mut h = harness();
    duplicate(&mut h, 0);
    h.state_mut().views[1].go_to_page(11);
    h.run_steps(6);
    let second = place(&h, 1);
    show(&mut h, 0);
    for page in [0, 4, 8] {
        h.state_mut().views[0].go_to_page(page);
        h.run_steps(4);
    }
    assert!(h.state_mut().execute("view.previous_view"));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 4);
    assert!(h.state_mut().execute("view.previous_view"));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 0);
    assert!(h.state_mut().execute("view.next_view"));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 4);
    assert_eq!(place(&h, 1), second, "tab 2's page and history are its own");
    show(&mut h, 1);
    assert!(h.state_mut().execute("view.previous_view"));
    h.run_steps(4);
    assert_eq!(h.state().views[1].current, 0, "tab 2 goes back through its own history");
    assert_eq!(h.state().views[0].current, 4);
}

#[test]
fn e_closing_either_tab_leaves_the_other_where_it_was() {
    for close_original in [false, true] {
        let mut h = two_places();
        let (first, second) = (place(&h, 0), place(&h, 1));
        let survivor_key = h.state().views[if close_original { 1 } else { 0 }].key;
        let id = h.state().views[0].id;
        let closing = if close_original { 0 } else { 1 };
        let key = h.state().views[closing].key;
        h.state_mut().tab_action(Action::Close(key));
        h.run_steps(6);
        assert_eq!(h.state().views.len(), 1);
        assert_eq!(h.state().views[0].key, survivor_key, "the tab asked for closed, not the other");
        assert!(h.state().session.get(id).is_some(), "the document stays open with its other tab");
        let left = place(&h, 0);
        let expected = if close_original { second } else { first };
        assert_eq!((left.current, left.zoom, left.fit), (expected.current, expected.zoom, expected.fit));
        assert!(close_enough(left.scroll, expected.scroll), "{:?} vs {:?}", left.scroll, expected.scroll);
    }
}

#[test]
fn f_the_page_grid_scrolls_per_tab() {
    let mut h = harness_with(100);
    h.state_mut().views[0].organize = true;
    h.run_steps(4);
    duplicate(&mut h, 0);
    assert!(h.state().views[1].organize, "the duplicate opens on the page grid too");
    // Scroll tab 2's grid to the end.
    wheel(&mut h, -100_000.0);
    h.run_steps(30);
    let first_shown = |h: &Harness<'static, PdfCraftApp>, tab: usize| h.state().views[tab].first_visible_thumbnail();
    let end = first_shown(&h, 1).expect("tab 2 shows pages");
    assert!(end > 50, "tab 2's grid is near the end: {end}");
    show(&mut h, 0);
    h.run_steps(4);
    assert_eq!(first_shown(&h, 0), Some(0), "tab 1's grid is still at the top");
    show(&mut h, 1);
    h.run_steps(4);
    assert_eq!(first_shown(&h, 1), Some(end), "tab 2's grid is still at the end");
}

#[test]
fn g_an_edit_in_one_tab_refreshes_the_other_without_moving_it() {
    let mut h = two_places();
    let second = place(&h, 1);
    show(&mut h, 0);
    let id = h.state().views[0].id;
    let note = Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
        page: 2,
        shape: pdfcraft_engine::Shape::Note { at: [100.0, 600.0], icon: pdfcraft_engine::NoteIcon::Comment },
        style: Default::default(),
        contents: "Check this".into(),
        author: "Reviewer".into(),
    });
    assert!(h.state_mut().apply_edit(note));
    h.run_steps(4);
    let version = crate::content_version(h.state().session.get(id).unwrap());
    assert_eq!(h.state().views[1].content_seen, Some(version), "tab 2 caught up with the edit");
    assert_eq!(place(&h, 1), second, "and stayed where it was");
    show(&mut h, 1);
    let now = place(&h, 1);
    assert_eq!((now.current, now.zoom, &now.back), (second.current, second.zoom, &second.back));
    assert!(close_enough(now.scroll, second.scroll));
}

// ── The shared document ──────────────────────────────────────────────────────────────────────

#[test]
fn duplicates_are_numbered_and_numbers_are_reused() {
    let mut app = app_with(3);
    let label = |app: &PdfCraftApp, i: usize| crate::canvas::tab_label(app, i).unwrap();
    assert_eq!(app.duplicate_view(0), Some(1));
    assert_eq!(app.duplicate_view(1), Some(2));
    assert_eq!([label(&app, 0), label(&app, 1), label(&app, 2)], ["plan.pdf", "plan.pdf (2)", "plan.pdf (3)"]);
    app.close_tab(1);
    assert_eq!(app.duplicate_view(1), Some(2));
    assert_eq!(label(&app, 2), "plan.pdf (2)", "the lowest free number is taken");
    // A held shortcut can't fill the strip with one file.
    while app.duplicate_view(0).is_some() {}
    assert_eq!(app.views.len(), MAX_VIEWS_PER_DOCUMENT);
    assert!(!app.can_duplicate(0));
}

#[test]
fn closing_a_duplicate_keeps_the_document_and_asks_nothing() {
    let mut app = app_with(3);
    let id = app.views[0].id;
    app.duplicate_view(0);
    assert!(app.apply_edit(Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }));
    assert!(app.session.get(id).unwrap().dirty);
    let original = app.views[0].key;
    app.request_close_tab(1);
    assert!(app.close_request.is_none(), "the document stays open in its other tab: nothing to ask");
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.views[0].key, original);
    assert!(app.session.get(id).is_some_and(|d| d.dirty), "unsaved changes stay with the document");
    assert!(app.closed_tabs.is_empty(), "no closed-tab entry while the document is open");
    // Its last tab asks, as before.
    app.request_close_tab(0);
    assert_eq!(app.close_request, Some(crate::CloseRequest::Tab(original)));
    app.resolve_close(&egui::Context::default(), Some(false));
    assert!(app.views.is_empty());
    assert!(app.session.get(id).is_none(), "the document closes with its last tab");
}

#[test]
fn typing_in_a_closing_duplicate_goes_into_the_document() {
    let mut app = app_with(3);
    let id = app.views[0].id;
    app.duplicate_view(0);
    // Fill & Sign text typed in the duplicate and not committed yet.
    app.views[1].fill_text = Some(crate::fill_sign::TypeBox { page: 0, at: [72.0, 700.0], text: "Approved".into(), size: 12.0, focus: false });
    assert!(app.has_unsaved_typing(1));
    let annotations = app.session.get(id).unwrap().info.annotations.len();
    app.request_close_tab(1);
    assert!(app.close_request.is_none());
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.session.get(id).unwrap().info.annotations.len(), annotations + 1, "the text is in the document");
}

#[test]
fn an_edit_and_its_undo_reach_every_tab() {
    let mut h = harness_with(3);
    duplicate(&mut h, 0);
    show(&mut h, 0);
    assert!(h.state_mut().apply_edit(Edit::InsertBlankPage { at: 3, width: 300.0, height: 400.0 }));
    h.run_steps(4);
    assert_eq!(h.state().views[1].page_count(), 4, "the other tab sees the new page");
    show(&mut h, 1);
    h.state_mut().undo();
    h.run_steps(4);
    assert_eq!(h.state().views[0].page_count(), 3, "one history: undo from either tab");
    assert_eq!(h.state().views[1].page_count(), 3);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none());
}

#[test]
fn a_hidden_duplicate_does_not_starve_the_tab_on_screen() {
    let mut h = harness_with(3);
    duplicate(&mut h, 0);
    // A zoom the source never rendered: the tab on screen needs new page images from the
    // document's render pool, which the hidden source shares.
    h.state_mut().views[1].set_zoom(2.5);
    let mut sharp = false;
    for _ in 0..600 {
        h.run_steps(1);
        if h.state().views[1].current_page_sharp() {
            sharp = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(sharp, "the shown tab receives its page images");
}

#[test]
fn quitting_and_close_all_ask_once_per_document() {
    let mut app = app_with(3);
    let id = app.views[0].id;
    app.duplicate_view(0);
    assert!(app.apply_edit(Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }));
    app.close_all();
    assert_eq!(app.close_request, Some(crate::CloseRequest::All));
    app.resolve_close(&egui::Context::default(), Some(false));
    assert!(app.close_request.is_none(), "one answer for the document, not one per tab");
    assert!(app.views.is_empty(), "Close All leaves none of its tabs open");
    assert!(app.session.get(id).is_none());
}

#[test]
fn reloading_the_file_keeps_each_tab_in_its_place() {
    use std::io::Write as _;
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("pdfcraft-duplicate-{}-{nonce}.pdf", std::process::id()));
    let mut app = PdfCraftApp::new();
    let first = app.session.create_blank(612.0, 792.0, 20).unwrap();
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
    file.write_all(&first).unwrap();
    drop(file);
    app.open_path(path.to_str().unwrap());
    assert_eq!(app.views.len(), 1);
    let id = app.views[0].id;
    app.duplicate_view(0);
    app.views[0].go_to_page(3);
    app.views[1].go_to_page(15);
    app.views[1].set_zoom(1.5);
    // A rebuilt file with more pages.
    let second = app.session.create_blank(612.0, 792.0, 25).unwrap();
    std::fs::write(&path, second.as_ref()).unwrap();
    app.watch.reload_request = Some(id);
    app.watch_files();
    assert_eq!(app.session.get(id).unwrap().info.pages.len(), 25);
    let version = crate::content_version(app.session.get(id).unwrap());
    for (tab, page) in [(0, 3), (1, 15)] {
        assert_eq!(app.views[tab].current, page, "tab {tab} keeps its page");
        assert_eq!(app.views[tab].page_count(), 25, "tab {tab} shows the new version");
        assert_eq!(app.views[tab].content_seen, Some(version));
    }
    assert_eq!(app.views[1].zoom, 1.5);
    assert_eq!(app.open_session().files.len(), 1, "the last session remembers the file once");
    assert_eq!(app.open_documents(), [id], "pickers list the document once");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_deferred_shortcut_runs_only_in_its_own_tab() {
    let mut h = harness();
    duplicate(&mut h, 0);
    for page in [0, 6] {
        h.state_mut().views[1].go_to_page(page);
        h.run_steps(3);
    }
    let first: ViewKey = h.state().views[0].key;
    let second: ViewKey = h.state().views[1].key;
    // Pressed while tab 1 was active, but tab 2 is active now.
    h.state_mut().deferred_commands.push(("view.previous_view", Some(first)));
    h.run_steps(3);
    assert_eq!(h.state().views[1].current, 6, "not run in the other tab of the same document");
    h.state_mut().deferred_commands.push(("view.previous_view", Some(second)));
    h.run_steps(3);
    assert_eq!(h.state().views[1].current, 0);
}

#[test]
fn the_tab_menu_duplicates_a_tab() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness_with(3);
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "plan.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Duplicate Tab").click();
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(h.state().active, Some(1));
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "plan.pdf (2)");
    assert!(h.state_mut().execute("view.duplicate_tab"), "also a registered command");
    assert_eq!(h.state().views.len(), 3);
}

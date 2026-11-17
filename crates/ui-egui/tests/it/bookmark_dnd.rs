//! This fork (BOOKMARK.DND.1): dragging bookmarks in the Bookmarks panel to reorder and re-nest
//! them, one or several at a time.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::PdfCraftApp;

const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// The three-page document with the Bookmarks panel open and these bookmarks (parent, title,
/// page), each added last under its parent.
fn harness(bookmarks: &[(&[usize], &str, usize)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app
    });
    h.run_steps(4);
    for (parent, title, page) in bookmarks {
        h.state_mut().apply_edit(Edit::AddBookmark {
            parent: parent.to_vec(),
            index: usize::MAX,
            title: (*title).into(),
            page: *page,
            left: None,
            top: None,
        });
    }
    h.run_steps(4);
    h
}

/// The outline as `Title[Child,…]`.
fn outline(h: &Harness<'static, PdfCraftApp>) -> String {
    fn level(items: &[pdfcraft_render::OutlineItem]) -> String {
        items
            .iter()
            .map(|i| if i.children.is_empty() { i.title.clone() } else { format!("{}[{}]", i.title, level(&i.children)) })
            .collect::<Vec<_>>()
            .join(",")
    }
    let id = h.state().win.views[0].id;
    level(&h.state().session.get(id).unwrap().info.outline)
}

fn selected(h: &Harness<'static, PdfCraftApp>) -> Vec<Vec<usize>> {
    h.state().win.views[0].bookmarks.paths()
}

fn click(h: &mut Harness<'static, PdfCraftApp>, title: &str, modifiers: Modifiers) {
    h.get_by_label(title).click_modifiers(modifiers);
    h.run_steps(3);
}

/// Where on a row a drag is let go: its top quarter, middle or bottom quarter.
#[derive(Clone, Copy)]
enum On {
    Before,
    Onto,
    After,
}

fn row(h: &Harness<'static, PdfCraftApp>, title: &str) -> egui::Rect {
    h.get_by_label(title).rect()
}

fn aim(h: &Harness<'static, PdfCraftApp>, title: &str, on: On) -> egui::Pos2 {
    let r = row(h, title);
    let part = match on {
        On::Before => 0.1,
        On::Onto => 0.5,
        On::After => 0.9,
    };
    egui::pos2(r.left() + 60.0, r.top() + r.height() * part)
}

/// Press on the row `from` and move to `to` in steps, without letting go.
fn drag_to(h: &mut Harness<'static, PdfCraftApp>, from: &str, to: egui::Pos2) {
    let r = row(h, from);
    let start = egui::pos2(r.left() + 60.0, r.center().y);
    h.event(egui::Event::PointerMoved(start));
    h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=6 {
        h.event(egui::Event::PointerMoved(start.lerp(to, k as f32 / 6.0)));
        h.run_steps(1);
    }
}

fn release(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
}

/// Drag the row `from` onto a part of the row `to` and let go.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: &str, to: &str, on: On) {
    let at = aim(h, to, on);
    drag_to(h, from, at);
    release(h, at);
}

fn undo_label(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let id = h.state().win.views[0].id;
    h.state().session.get(id).unwrap().can_undo().map(str::to_owned)
}

const FLAT: &[(&[usize], &str, usize)] = &[(&[], "A", 0), (&[], "B", 1), (&[], "C", 2), (&[], "D", 0)];
const NESTED: &[(&[usize], &str, usize)] = &[(&[], "A", 0), (&[0], "a1", 1), (&[0], "a2", 2), (&[], "B", 2), (&[], "C", 0)];

#[test]
fn one_bookmark_drops_before_after_and_onto_another() {
    let mut h = harness(FLAT);
    // The top quarter of C: before it.
    drag(&mut h, "A", "C", On::Before);
    assert_eq!(outline(&h), "B,A,C,D");
    assert_eq!(undo_label(&h).as_deref(), Some("Move bookmarks"));
    assert_eq!(selected(&h), [vec![1]], "the moved bookmark is selected where it went");
    // The bottom quarter of D: after it.
    drag(&mut h, "B", "D", On::After);
    assert_eq!(outline(&h), "A,C,D,B");
    assert_eq!(selected(&h), [vec![3]]);
    // The middle of A: under it, as its last child, and A opens to show it.
    drag(&mut h, "D", "A", On::Onto);
    assert_eq!(outline(&h), "A[D],C,B");
    assert_eq!(selected(&h), [vec![0, 0]]);
    h.get_by_label("D");
    drag(&mut h, "B", "A", On::Onto);
    assert_eq!(outline(&h), "A[D,B],C", "onto adds the last child");
    // One undo step each.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), "A[D],C,B");
}

#[test]
fn a_selected_row_takes_the_selection_and_another_row_goes_alone() {
    let mut h = harness(FLAT);
    // D and B selected, in that order of clicking: they arrive in outline order.
    click(&mut h, "D", Modifiers::NONE);
    click(&mut h, "B", Modifiers::COMMAND);
    drag(&mut h, "D", "A", On::Before);
    assert_eq!(outline(&h), "B,D,A,C");
    assert_eq!(selected(&h), [vec![0], vec![1]], "the group is selected where it went");
    assert_eq!(undo_label(&h).as_deref(), Some("Move bookmarks"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), "A,B,C,D", "one undo step for the whole group");
    // B and D selected again, but the drag starts on A, which isn't: A goes alone and becomes
    // the selection.
    click(&mut h, "B", Modifiers::NONE);
    click(&mut h, "D", Modifiers::COMMAND);
    let page = h.state().win.views[0].current;
    drag(&mut h, "A", "C", On::After);
    assert_eq!(outline(&h), "B,C,A,D");
    assert_eq!(selected(&h), [vec![2]]);
    assert_eq!(h.state().win.views[0].current, page, "a drag doesn't go to the bookmark's page");
}

/// A group across levels moves whole subtrees, and a bookmark under a selected one moves once.
#[test]
fn a_group_across_levels_moves_whole_subtrees() {
    let mut h = harness(NESTED);
    click(&mut h, "A", Modifiers::NONE);
    click(&mut h, "a2", Modifiers::COMMAND);
    click(&mut h, "B", Modifiers::COMMAND);
    drag(&mut h, "B", "C", On::After);
    assert_eq!(outline(&h), "C,A[a1,a2],B");
    assert_eq!(selected(&h), [vec![1], vec![2]]);
}

/// After a bookmark is after everything under it, at its own level, whether it is open or closed.
#[test]
fn after_an_open_parent_is_after_its_whole_subtree_at_its_level() {
    let mut h = harness(NESTED);
    h.get_by_label("a1");
    drag(&mut h, "C", "A", On::After);
    assert_eq!(outline(&h), "A[a1,a2],C,B", "after A, not among its children");
    // The same with A closed.
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Collapse all bookmarks").click();
    h.run_steps(3);
    assert!(h.query_by_label("a1").is_none());
    drag(&mut h, "B", "A", On::After);
    assert_eq!(outline(&h), "A[a1,a2],B,C");
    // And before an open parent's first child is under the parent.
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Expand all bookmarks").click();
    h.run_steps(3);
    drag(&mut h, "C", "a1", On::Before);
    assert_eq!(outline(&h), "A[C,a1,a2],B");
    // After the last child is after that child, under the same parent.
    drag(&mut h, "B", "a2", On::After);
    assert_eq!(outline(&h), "A[C,a1,a2,B]");
}

#[test]
fn a_place_the_bookmarks_cannot_go_takes_no_drop() {
    let mut h = harness(NESTED);
    let before = undo_label(&h);
    // A onto its own child.
    drag(&mut h, "A", "a1", On::Onto);
    assert_eq!(outline(&h), "A[a1,a2],B,C");
    assert_eq!(undo_label(&h), before, "no undo step");
    assert!(!h.state().win.views[0].bookmarks.dragging(), "the drag is over");
    // The panel itself takes no drop there: the move is never asked for, so nothing is reported.
    assert!(h.state().win.toast.is_none(), "{:?}", h.state().win.toast);
    // A group with a bookmark the target is under.
    click(&mut h, "B", Modifiers::NONE);
    click(&mut h, "A", Modifiers::COMMAND);
    drag(&mut h, "B", "a2", On::After);
    assert_eq!(outline(&h), "A[a1,a2],B,C");
    assert_eq!(undo_label(&h), before);
    assert_eq!(selected(&h), [vec![0], vec![1]], "the selection stays");
    assert!(h.state().win.toast.is_none(), "{:?}", h.state().win.toast);
    // Asked for all the same (as a stale preview could), the move is refused and says why.
    let id = h.state().win.views[0].id;
    let stamp = {
        let doc = h.state().session.get(id).unwrap();
        (doc.edit_generation(), std::sync::Arc::as_ptr(&doc.bytes).addr())
    };
    h.state_mut().move_bookmarks_to(vec![vec![0]], vec![0, 0], 0, stamp);
    assert_eq!(outline(&h), "A[a1,a2],B,C");
    assert_eq!(undo_label(&h), before);
    assert!(h.state().win.toast.as_ref().is_some_and(|(text, _)| text.contains("inside itself")), "{:?}", h.state().win.toast);
}

#[test]
fn a_drop_where_the_bookmark_already_is_adds_no_undo_step() {
    let mut h = harness(FLAT);
    let before = undo_label(&h);
    let id = h.state().win.views[0].id;
    let generation = h.state().session.get(id).unwrap().edit_generation();
    drag(&mut h, "B", "C", On::Before);
    drag(&mut h, "B", "A", On::After);
    drag(&mut h, "B", "B", On::After);
    assert_eq!(outline(&h), "A,B,C,D");
    assert_eq!(undo_label(&h), before);
    assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "the document wasn't touched");
}

#[test]
fn escape_and_a_release_outside_the_list_give_the_drag_up() {
    let mut h = harness(FLAT);
    let before = undo_label(&h);
    click(&mut h, "A", Modifiers::NONE);
    let at = aim(&h, "C", On::After);
    drag_to(&mut h, "A", at);
    assert!(h.state().win.views[0].bookmarks.dragging());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().win.views[0].bookmarks.dragging(), "Escape ends the drag");
    assert_eq!(selected(&h), [vec![0]], "and keeps the selection");
    release(&mut h, at);
    assert_eq!(outline(&h), "A,B,C,D");
    // Let go over the page, far from the panel.
    drag_to(&mut h, "A", egui::pos2(300.0, 500.0));
    release(&mut h, egui::pos2(300.0, 500.0));
    assert_eq!(outline(&h), "A,B,C,D");
    assert_eq!(undo_label(&h), before);
    assert!(!h.state().win.views[0].bookmarks.dragging());
}

/// A search hides the bookmarks between the ones it shows: nothing is dragged while it is on.
#[test]
fn nothing_is_dragged_while_a_search_filters_the_list() {
    let mut h = harness(&[(&[], "Alpha", 0), (&[], "Beta", 1), (&[], "Alps", 2)]);
    let before = undo_label(&h);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search").type_text("al");
    h.run_steps(3);
    assert!(h.query_by_label("Beta").is_none(), "the search is filtering");
    let at = aim(&h, "Alpha", On::Before);
    drag_to(&mut h, "Alps", at);
    assert!(!h.state().win.views[0].bookmarks.dragging(), "no drag starts");
    h.get_by_label("Clear the search to rearrange bookmarks");
    release(&mut h, at);
    assert_eq!(outline(&h), "Alpha,Beta,Alps");
    assert_eq!(undo_label(&h), before);
}

/// Resting on a closed bookmark opens it in the panel only: the document isn't written to.
#[test]
fn resting_on_a_closed_bookmark_opens_it_without_touching_the_document() {
    let mut h = harness(NESTED);
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Collapse all bookmarks").click();
    h.run_steps(3);
    assert!(h.query_by_label("a1").is_none());
    let id = h.state().win.views[0].id;
    let (generation, undo) = (h.state().session.get(id).unwrap().edit_generation(), undo_label(&h));
    let at = aim(&h, "A", On::Onto);
    drag_to(&mut h, "C", at);
    // Hold still for well over the delay.
    for _ in 0..40 {
        h.event(egui::Event::PointerMoved(at));
        h.step();
    }
    h.get_by_label("a1");
    assert!(h.state().win.views[0].bookmarks.dragging(), "still dragging");
    assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "opening it wrote nothing");
    assert_eq!(undo_label(&h), undo);
    assert_eq!(outline(&h), "A[a1,a2],B,C");
    // Now under what it showed: before a2.
    let at = aim(&h, "a2", On::Before);
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(2);
    release(&mut h, at);
    assert_eq!(outline(&h), "A[a1,C,a2],B");
}

/// Paths name bookmarks only in the document version the drag began in.
#[test]
fn a_drop_after_the_document_changed_moves_nothing() {
    let mut h = harness(&[(&[], "Intro", 0), (&[], "Section", 1), (&[], "Section", 2), (&[], "End", 0)]);
    let id = h.state().win.views[0].id;
    // The document version, as the panel stamps a selection or a drag with it.
    let stale = {
        let doc = h.state().session.get(id).unwrap();
        (doc.edit_generation(), std::sync::Arc::as_ptr(&doc.bytes).addr())
    };
    // The outline changes underneath, as another tab or a script would change it.
    h.state_mut().session.apply(id, Edit::MoveBookmark { from: vec![1], to_parent: vec![], index: 2 }).unwrap();
    let before = outline(&h);
    h.state_mut().move_bookmarks_to(vec![vec![2]], vec![], 0, stale);
    assert_eq!(outline(&h), before, "nothing moved");
    assert_eq!(undo_label(&h).as_deref(), Some("Move bookmark"), "no group step");
    // A drag in flight ends when the panel sees the change.
    let mut h = harness(FLAT);
    let id = h.state().win.views[0].id;
    let at = aim(&h, "D", On::After);
    drag_to(&mut h, "A", at);
    assert!(h.state().win.views[0].bookmarks.dragging());
    h.state_mut().apply_edit(Edit::RenameBookmark { path: vec![1], title: "Renamed".into() });
    h.run_steps(2);
    assert!(!h.state().win.views[0].bookmarks.dragging(), "the drag ended with the change");
    release(&mut h, at);
    assert_eq!(outline(&h), "A,Renamed,C,D");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Rename bookmark"));
}

/// A press and release without moving is still a click: it selects and goes to the page.
#[test]
fn a_click_on_a_draggable_row_still_selects_and_goes_there() {
    let mut h = harness(FLAT);
    click(&mut h, "C", Modifiers::NONE);
    assert_eq!(selected(&h), [vec![2]]);
    assert_eq!(h.state().win.views[0].current, 2);
    assert!(!h.state().win.views[0].bookmarks.dragging());
    assert_eq!(outline(&h), "A,B,C,D");
}

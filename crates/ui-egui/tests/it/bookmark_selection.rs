//! This fork (BOOKMARK.1): selecting several bookmarks in the Bookmarks panel (click, Ctrl+Click,
//! Shift+Click) and indenting or outdenting them together.

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
    let id = h.state().views[0].id;
    level(&h.state().session.get(id).unwrap().info.outline)
}

fn selected(h: &Harness<'static, PdfCraftApp>) -> Vec<Vec<usize>> {
    h.state().views[0].bookmarks.paths()
}

fn click(h: &mut Harness<'static, PdfCraftApp>, title: &str, modifiers: Modifiers) {
    h.get_by_label(title).click_modifiers(modifiers);
    h.run_steps(3);
}

/// Right-click `title` and pick `item` from its menu.
fn menu(h: &mut Harness<'static, PdfCraftApp>, title: &str, item: &str) {
    h.get_by_label(title).click_secondary();
    h.run_steps(2);
    h.get_by_label(item).click();
    h.run_steps(3);
}

const NESTED: &[(&[usize], &str, usize)] = &[(&[], "A", 0), (&[0], "a1", 1), (&[0], "a2", 2), (&[], "B", 2)];

#[test]
fn click_ctrl_click_and_shift_click_select_across_levels() {
    let mut h = harness(NESTED);
    // A plain click selects one and goes there.
    click(&mut h, "a2", Modifiers::NONE);
    assert_eq!(selected(&h), [vec![0, 1]]);
    assert_eq!(h.state().views[0].current, 2, "a plain click goes to the bookmark's page");
    h.state_mut().set_option("page", "1").unwrap();
    h.run_steps(3);
    // Ctrl+Click adds one without going there; the selection crosses levels.
    click(&mut h, "B", Modifiers::COMMAND);
    assert_eq!(selected(&h), [vec![0, 1], vec![1]]);
    assert_eq!(h.state().views[0].current, 0, "Ctrl+Click doesn't navigate");
    // Shift+Click: the rows shown from the anchor (B) to A, in place of the selection.
    click(&mut h, "A", Modifiers::SHIFT);
    assert_eq!(selected(&h), [vec![0], vec![0, 0], vec![0, 1], vec![1]]);
    assert_eq!(h.state().views[0].current, 0, "Shift+Click doesn't navigate");
    // Ctrl+Click again takes one out.
    click(&mut h, "a1", Modifiers::COMMAND);
    assert_eq!(selected(&h), [vec![0], vec![0, 1], vec![1]]);
}

#[test]
fn folding_a_branch_drops_the_rows_it_hides_and_escape_clears() {
    let mut h = harness(NESTED);
    click(&mut h, "a1", Modifiers::NONE);
    click(&mut h, "B", Modifiers::COMMAND);
    h.get_by_label("Bookmark options").click();
    h.run_steps(2);
    h.get_by_label("Collapse all bookmarks").click();
    h.run_steps(3);
    assert!(h.query_by_label("a1").is_none());
    assert_eq!(selected(&h), [vec![1]], "a hidden row leaves the selection");
    // A Shift+Click range runs through the rows shown: A and B.
    click(&mut h, "A", Modifiers::SHIFT);
    assert_eq!(selected(&h), [vec![0], vec![1]]);
    h.hover_at(h.get_by_label("A").rect().center());
    h.run_steps(1);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(selected(&h).is_empty(), "Escape over the panel clears the selection");
}

#[test]
fn a_group_indents_and_outdents_as_one_undo_step() {
    let mut h = harness(&[(&[], "A", 0), (&[], "B", 1), (&[], "C", 2), (&[], "D", 0)]);
    click(&mut h, "B", Modifiers::NONE);
    click(&mut h, "C", Modifiers::COMMAND);
    menu(&mut h, "B", "Indent");
    assert_eq!(outline(&h), "A[B,C],D");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Indent bookmarks"));
    // The moved bookmarks stay selected where they went, and their new parent shows them.
    assert_eq!(selected(&h), [vec![0, 0], vec![0, 1]]);
    h.get_by_label("C");
    // A second group action works on that selection.
    menu(&mut h, "C", "Outdent");
    assert_eq!(outline(&h), "A,B,C,D");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Outdent bookmarks"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(outline(&h), "A[B,C],D", "one undo step undoes the whole group");
    assert!(selected(&h).is_empty(), "undo leaves paths that may name other bookmarks: the selection goes");
}

#[test]
fn a_group_that_cannot_move_is_not_offered_and_a_right_click_elsewhere_replaces_it() {
    let mut h = harness(NESTED);
    click(&mut h, "a1", Modifiers::NONE);
    click(&mut h, "a2", Modifiers::COMMAND);
    // a1 is first under A: Indent is not offered for the pair, so picking it does nothing.
    menu(&mut h, "a1", "Indent");
    assert_eq!(outline(&h), "A[a1,a2],B");
    h.key_press(Key::Escape);
    h.run_steps(2);
    // A right-click on a row outside the selection selects that row alone: its own menu.
    let page = h.state().views[0].current;
    h.get_by_label("B").click_secondary();
    h.run_steps(2);
    assert_eq!(selected(&h), [vec![1]]);
    assert_eq!(h.state().views[0].current, page, "a right-click doesn't navigate");
    h.get_by_label("Rename").click();
    h.run_steps(3);
    assert!(h.state().bookmark_rename.is_some(), "the single-bookmark menu, as before");
}

/// Paths name rows only in the document version they were selected in. Two bookmarks share a
/// title: after the outline changes, the stale path would name the other one.
#[test]
fn a_selection_made_before_a_change_is_dropped_and_moves_nothing() {
    let mut h = harness(&[(&[], "Intro", 0), (&[], "Section", 1), (&[], "Section", 2), (&[], "End", 0)]);
    h.query_all_by_label("Section").nth(1).expect("the second Section").click_modifiers(Modifiers::NONE);
    h.run_steps(3);
    assert_eq!(selected(&h), [vec![2]]);
    // The outline changes underneath (as another tab or a script would change it): the first
    // Section moves to where the selected one was.
    let id = h.state().views[0].id;
    h.state_mut().session.apply(id, Edit::MoveBookmark { from: vec![1], to_parent: vec![], index: 2 }).unwrap();
    // Asked before the panel sees the change, the group action does nothing.
    let before = outline(&h);
    h.state_mut().shift_selected_bookmarks(false);
    assert_eq!(outline(&h), before, "nothing moved");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Move bookmark"), "no group step");
    // And the next frame drops the selection.
    h.run_steps(2);
    assert!(selected(&h).is_empty());
}

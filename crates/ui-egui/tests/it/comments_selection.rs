//! This fork (COMMENTS.1): selecting several comment cards in the Comments panel, and Delete,
//! Collapse and Expand selected, driven like a user would.

use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::comments::group::CommentKey;

/// A PDF of `pages` pages (300 × 200) whose annotations are `annots`: (page, body) in order, as
/// objects after the pages; `{N}` in a body is replaced by the object number of the annotation N
/// places earlier in the list (for /IRT and /Popup).
fn pdf(pages: usize, annots: &[(usize, String)]) -> Vec<u8> {
    let first = 3 + pages;
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 300 200] >>", kids.join(" ")));
    for p in 0..pages {
        let on: Vec<String> = annots.iter().enumerate().filter(|(_, (page, _))| *page == p).map(|(k, _)| format!("{} 0 R", first + k)).collect();
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", on.join(" ")));
    }
    for (_, body) in annots {
        objs.push(body.clone());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn note(text: &str, extra: &str) -> String {
    format!("<< /Type /Annot /Subtype /Square /Rect [20 20 80 80] /C [1 0 0] /T (Ada) /Contents ({text}){extra} >>")
}

/// Object number of the `k`-th annotation (0-based) in a `pdf(pages, …)` list.
fn obj(pages: usize, k: usize) -> usize {
    3 + pages + k
}

/// Four pages: A, B on page 1 (B with reply b1 and b2, a reply to b1); C on page 2; D on page 3;
/// E on page 4.
fn cards() -> Vec<u8> {
    let p = 4;
    pdf(
        p,
        &[
            (0, note("Card A", "")),
            (0, note("Card B", "")),
            (0, note("Reply b1", &format!(" /IRT {} 0 R", obj(p, 1)))),
            (0, note("Reply b2", &format!(" /IRT {} 0 R", obj(p, 2)))),
            (1, note("Card C", "")),
            (2, note("Card D", "")),
            (3, note("Card E", "")),
        ],
    )
}

fn harness(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("cards.pdf", None, bytes).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        app.set_option("panel", "comments").unwrap();
        app
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// The annotation whose text is `text`.
fn annot(h: &Harness<'static, PdfCraftApp>, text: &str) -> pdfcraft_render::Annotation {
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    doc.info.annotations.iter().find(|a| a.contents.as_deref() == Some(text)).cloned().unwrap_or_else(|| panic!("no {text:?}"))
}

/// The texts of the selected cards, in list order.
fn group(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    let g = h.state().views[0].comments.group();
    doc.info.annotations.iter().filter(|a| g.contains(&CommentKey::of(a))).filter_map(|a| a.contents.clone()).collect()
}

fn active(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let (p, i) = h.state().views[0].comments.selected?;
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    doc.info.annotations.iter().find(|a| (a.page, a.index) == (p, i)).and_then(|a| a.contents.clone())
}

fn texts(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let id = h.state().views[0].id;
    h.state().session.get(id).unwrap().info.annotations.iter().filter_map(|a| a.contents.clone()).collect()
}

/// Click the card whose text is `text` on its header row (its author), which takes clicks on
/// every card, the active one included (whose text and reply box are its own widgets).
fn click(h: &mut Harness<'static, PdfCraftApp>, text: &str, modifiers: Modifiers) {
    let body = h.get_by_label(text).rect();
    let author = h
        .query_all_by_label("Ada")
        .filter(|n| n.rect().bottom() <= body.top() + 1.0 && (n.rect().left() - body.left()).abs() < 60.0)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap_or_else(|| panic!("the header of {text:?}"));
    author.click_modifiers(modifiers);
    h.run_steps(3);
}

/// Where the document view is: its page, the first page's place on screen, and whether a
/// comment was flashed or is being revealed.
fn place(h: &Harness<'static, PdfCraftApp>) -> (usize, Option<egui::Rect>, bool, bool) {
    let v = &h.state().views[0];
    (v.current, v.page_screen_rect(0), v.flash.is_some(), v.comments.reveal)
}

/// Pick `item` from the menu open now.
fn pick(h: &mut Harness<'static, PdfCraftApp>, item: &str) {
    h.get_by_label(item).click();
    h.run_steps(3);
}

#[test]
fn a_plain_click_selects_the_card_and_shows_it_as_before() {
    let mut h = harness(cards());
    click(&mut h, "Card C", Modifiers::NONE);
    assert_eq!(active(&h).as_deref(), Some("Card C"));
    assert_eq!(group(&h), ["Card C"]);
    assert!(h.state().views[0].flash.is_some(), "it is shown on the page, as before");
}

#[test]
fn ctrl_and_shift_select_cards_without_touching_the_page_or_the_active_comment() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    h.state_mut().views[0].flash = None;
    h.run_steps(2);
    let before = place(&h);
    click(&mut h, "Card C", Modifiers::COMMAND);
    click(&mut h, "Card D", Modifiers::COMMAND);
    assert_eq!(group(&h), ["Card A", "Card C", "Card D"]);
    assert_eq!(active(&h).as_deref(), Some("Card A"), "the active comment stays");
    assert_eq!(place(&h), before, "no page change, scroll, flash or reveal");
    // Shift: the cards shown from the anchor (D) to B, in place of the selection.
    click(&mut h, "Card B", Modifiers::SHIFT);
    assert_eq!(group(&h), ["Card B", "Card C", "Card D"]);
    assert_eq!(active(&h).as_deref(), Some("Card A"));
    assert_eq!(place(&h), before);
    // Ctrl+Click on the active card takes it out of the group; it stays active.
    click(&mut h, "Card A", Modifiers::COMMAND);
    click(&mut h, "Card A", Modifiers::COMMAND);
    assert!(group(&h).contains(&"Card B".to_string()) && !group(&h).contains(&"Card A".to_string()));
    assert_eq!(active(&h).as_deref(), Some("Card A"));
}

#[test]
fn a_comment_chosen_on_the_page_becomes_the_selection() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    let d = annot(&h, "Card D");
    h.state_mut().views[0].comments.selected = Some((d.page, d.index));
    h.run_steps(3);
    assert_eq!(group(&h), ["Card D"]);
}

/// Revision 3c: one Escape clears the active comment and the selected cards; a gesture in
/// progress is cancelled first, on its own.
#[test]
fn one_escape_clears_the_active_comment_and_the_selected_cards() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card B", Modifiers::COMMAND);
    click(&mut h, "Card C", Modifiers::COMMAND);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!((active(&h), group(&h).len()), (None, 0), "one press clears both");
    // The active card taken out of the group first.
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card B", Modifiers::COMMAND);
    click(&mut h, "Card C", Modifiers::COMMAND);
    click(&mut h, "Card A", Modifiers::COMMAND);
    assert_eq!(group(&h), ["Card B", "Card C"]);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!((active(&h), group(&h).len()), (None, 0));
    // Only cards selected, no active comment.
    click(&mut h, "Card B", Modifiers::COMMAND);
    click(&mut h, "Card C", Modifiers::COMMAND);
    assert_eq!(active(&h), None);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(group(&h).is_empty());
    // With the Comments panel closed (nothing else looks after the selection then), the press
    // still clears both.
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card B", Modifiers::COMMAND);
    h.state_mut().choose_right_panel(None);
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.group().len(), 2);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.selected, None);
    assert!(h.state().views[0].comments.group().is_empty(), "one press, panel closed");
    h.state_mut().set_option("panel", "comments").unwrap();
    h.run_steps(3);
    // A gesture in progress goes first, alone; the next press clears both.
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card B", Modifiers::COMMAND);
    h.state_mut().views[0].comments.gesture = Some(pdfcraft_ui_egui::comments::Gesture::Move { page: 0, index: 0, from: egui::pos2(1.0, 1.0) });
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].comments.gesture.is_none());
    assert_eq!((active(&h).as_deref(), group(&h).len()), (Some("Card A"), 2), "the selection stays");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!((active(&h), group(&h).len()), (None, 0));
}

/// Approved regression: with two cards selected, Delete and Backspace delete nothing, not even
/// the active comment taken out of the group.
#[test]
fn delete_and_backspace_do_nothing_while_several_cards_are_selected() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    click(&mut h, "Card D", Modifiers::COMMAND);
    click(&mut h, "Card A", Modifiers::COMMAND);
    assert_eq!(group(&h), ["Card C", "Card D"]);
    assert_eq!(active(&h).as_deref(), Some("Card A"));
    let id = h.state().views[0].id;
    let (before, undo) = (texts(&h), h.state().session.get(id).unwrap().can_undo().map(str::to_owned));
    for key in [Key::Delete, Key::Backspace] {
        h.key_press(key);
        h.run_steps(3);
    }
    assert_eq!(texts(&h), before, "nothing deleted");
    assert_eq!(h.state().session.get(id).unwrap().can_undo().map(str::to_owned), undo);
    assert!(h.state().views[0].pending_edit.is_none());
    // The active comment inside the group: nothing either.
    click(&mut h, "Card A", Modifiers::COMMAND);
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(texts(&h), before);
}

#[test]
fn with_one_card_selected_the_keys_delete_as_before() {
    let mut h = harness(cards());
    click(&mut h, "Card C", Modifiers::NONE);
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert!(!texts(&h).contains(&"Card C".to_string()));
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Delete comment"));
}

#[test]
fn a_right_click_keeps_a_group_and_elsewhere_selects_that_card_alone() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    let before = place(&h);
    h.get_by_label("Card C").click_secondary();
    h.run_steps(2);
    assert_eq!(group(&h), ["Card A", "Card C"]);
    h.get_by_label("Delete selected (2)");
    h.key_press(Key::Escape);
    h.run_steps(2);
    // Outside the group: that card alone, its own menu, no navigation, the active comment kept.
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    h.state_mut().views[0].flash = None;
    h.get_by_label("Card D").click_secondary();
    h.run_steps(2);
    assert_eq!(group(&h), ["Card D"]);
    assert_eq!(active(&h).as_deref(), Some("Card A"));
    assert_eq!(place(&h).0, before.0);
    assert!(h.state().views[0].flash.is_none());
    h.get_by_label("Properties…");
}

#[test]
fn delete_selected_asks_once_with_the_counts_and_is_one_undo_step() {
    let mut h = harness(cards());
    click(&mut h, "Card B", Modifiers::NONE);
    click(&mut h, "Card D", Modifiers::COMMAND);
    h.state_mut().views[0].flash = None;
    let before_place = place(&h);
    let before = texts(&h);
    h.get_by_label("Card D").click_secondary();
    h.run_steps(2);
    pick(&mut h, "Delete selected (2)");
    // B goes with b1 and b2.
    h.get_by_label("Delete 2 comments and 2 replies?");
    // Cancel: nothing changes.
    pick(&mut h, "Cancel");
    assert_eq!(texts(&h), before);
    assert!(h.state().dialog.is_none());
    // Again, and Delete.
    h.get_by_label("Card D").click_secondary();
    h.run_steps(2);
    pick(&mut h, "Delete selected (2)");
    h.query_all_by_label("Delete").last().expect("the dialog's Delete").click();
    h.run_steps(4);
    assert_eq!(texts(&h), ["Card A", "Card C", "Card E"]);
    assert!(group(&h).is_empty());
    assert_eq!(active(&h), None, "the active comment went with them");
    assert_eq!((place(&h).0, place(&h).1), (before_place.0, before_place.1), "the page view never moved");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Delete comments"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts(&h), before, "one undo puts them all back");
}

#[test]
fn after_a_bulk_delete_the_active_comment_keeps_its_place() {
    let mut h = harness(cards());
    // E is active; A and B (before nothing of E's) and C are deleted.
    click(&mut h, "Card E", Modifiers::NONE);
    click(&mut h, "Card A", Modifiers::COMMAND);
    click(&mut h, "Card E", Modifiers::COMMAND);
    click(&mut h, "Card B", Modifiers::COMMAND);
    assert_eq!(group(&h), ["Card A", "Card B"]);
    h.state_mut().ask_delete_comments();
    h.run_steps(2);
    h.state_mut().confirm_delete_comments();
    h.run_steps(3);
    assert_eq!(texts(&h), ["Card C", "Card D", "Card E"]);
    assert_eq!(active(&h).as_deref(), Some("Card E"), "found again by its object");
}

/// The confirmation holds the document version its counts came from: confirmed after a change
/// (here, made in another tab), it deletes nothing.
#[test]
fn a_confirmation_from_before_a_change_deletes_nothing() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    h.state_mut().ask_delete_comments();
    h.run_steps(1);
    assert!(h.state().comment_delete.is_some());
    let id = h.state().views[0].id;
    let e = annot(&h, "Card E");
    h.state_mut().session.apply(id, Edit::SetAnnotationContents { page: e.page, index: e.index, text: "Card E2".into() }).unwrap();
    let before = texts(&h);
    h.state_mut().confirm_delete_comments();
    h.run_steps(2);
    assert_eq!(texts(&h), before, "nothing deleted");
    assert!(h.state().toast.as_ref().is_some_and(|(m, _)| m.contains("changed")), "{:?}", h.state().toast);
}

#[test]
fn collapse_and_expand_selected_fold_the_selected_cards_page_groups() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    click(&mut h, "Card D", Modifiers::COMMAND);
    h.get_by_label("Card D").click_secondary();
    h.run_steps(2);
    pick(&mut h, "Collapse selected");
    let mut folded = h.state().views[0].comments.collapsed_pages.clone();
    folded.sort_unstable();
    assert_eq!(folded, [0, 1, 2], "pages 1-3, each once");
    assert!(h.query_by_label("Card B").is_none(), "an unselected card on a folded page is hidden too");
    h.get_by_label("Card E");
    assert_eq!(group(&h), ["Card A", "Card C", "Card D"], "the selection stays");
    // Expand selected, from the panel's "…" menu.
    h.get_by_label("More options").click();
    h.run_steps(2);
    pick(&mut h, "Expand selected");
    assert!(h.state().views[0].comments.collapsed_pages.is_empty());
    h.get_by_label("Card B");
    // Not sorted by page: no page groups, so both are disabled.
    h.state_mut().views[0].comments.sort = pdfcraft_ui_egui::comments::SortBy::Author;
    h.run_steps(2);
    h.get_by_label("More options").click();
    h.run_steps(2);
    pick(&mut h, "Collapse selected");
    assert!(h.state().views[0].comments.collapsed_pages.is_empty(), "disabled when sorted by author");
}

#[test]
fn a_filter_drops_the_cards_it_hides_and_a_sort_keeps_the_selection() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    h.state_mut().views[0].comments.sort = pdfcraft_ui_egui::comments::SortBy::Date;
    h.run_steps(3);
    assert_eq!(group(&h), ["Card A", "Card C"]);
    h.state_mut().views[0].comments.search = Some("Card C".into());
    h.run_steps(3);
    assert_eq!(group(&h), ["Card C"]);
}

#[test]
fn a_reply_selects_its_card() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    h.get_by_label("Reply b1").click_modifiers(Modifiers::COMMAND);
    h.run_steps(3);
    assert_eq!(group(&h), ["Card A", "Card B"], "a click on a reply is a click on its card");
}

/// 150 cards, one a page: a Shift range covers cards that are never drawn.
#[test]
fn a_shift_range_covers_cards_out_of_view() {
    let annots: Vec<(usize, String)> = (0..150).map(|i| (i, note(&format!("Card {}", i + 1), ""))).collect();
    let mut h = harness(pdf(150, &annots));
    click(&mut h, "Card 1", Modifiers::NONE);
    // Scroll the panel down until card 60 shows.
    let at: Pos2 = h.get_by_label("Card 1").rect().center();
    for _ in 0..200 {
        if h.query_by_label("Card 60").is_some() {
            break;
        }
        h.hover_at(at);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -200.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(2);
    }
    // Card 1 is the active card, which is always drawn; the cards between are not.
    assert!(h.query_by_label("Card 30").is_none(), "card 30 is out of view, not drawn");
    click(&mut h, "Card 60", Modifiers::SHIFT);
    assert_eq!(group(&h).len(), 60);
}

#[test]
fn each_tab_keeps_its_own_selection_and_a_duplicate_starts_empty() {
    let mut h = harness(cards());
    click(&mut h, "Card A", Modifiers::NONE);
    click(&mut h, "Card C", Modifiers::COMMAND);
    assert!(h.state_mut().execute("view.duplicate_tab"));
    h.run_steps(4);
    let views = &h.state().views;
    assert_eq!(views.len(), 2);
    assert!(views[1].comments.group().is_empty(), "a duplicate tab starts with no selection");
    assert_eq!(views[0].comments.group().len(), 2, "the first tab keeps its own");
}

//! This fork (SYNC.16): upstream's comment selection (#3484) holds comments by their place,
//! (page, index in `/Annots`). A change made outside the view that holds the selection (another
//! tab of the document, another tile, Undo or Redo there, an automation edit) moves those places,
//! and a Delete or Backspace after it must never delete another comment than the ones selected:
//! the selection is either still what it was, or dropped with nothing deleted.
//!
//! Identity here is a comment's text (each fixture comment has its own) and its object reference.

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::{PdfCraftApp, ViewLayout};

/// A PDF of `pages` pages (300 × 200) whose annotations are `annots`: (page, body) in order, as
/// objects after the pages; `{N}` in a body is replaced by the object number of annotation N.
fn pdf(pages: usize, annots: &[(usize, String)]) -> Vec<u8> {
    let first = 3 + pages;
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 300 200] >>", kids.join(" ")));
    for p in 0..pages {
        let on: Vec<String> = annots.iter().enumerate().filter(|(_, (page, _))| *page == p).map(|(k, _)| format!("{} 0 R", first + k)).collect();
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", on.join(" ")));
    }
    for (k, (_, body)) in annots.iter().enumerate() {
        objs.push(body.replace("{IRT}", &format!("{} 0 R", first + k.saturating_sub(1))));
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

/// One page with C1 … C5, C3 with a reply (R3, a reply to C3), then C4, C5.
fn comments() -> Vec<u8> {
    pdf(1, &[(0, note("C1", "")), (0, note("C2", "")), (0, note("C3", "")), (0, note("R3", " /IRT {IRT}")), (0, note("C4", "")), (0, note("C5", ""))])
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("comments.pdf", None, comments()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        app.set_option("panel", "comments").unwrap();
        app
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// A second view (Duplicate Tab) of the document; the first is active again.
fn with_duplicate() -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    h.state_mut().duplicate_view(0).expect("duplicated");
    h.state_mut().active = Some(0);
    h.run_steps(3);
    h
}

/// The comments' texts, in `/Annots` order, with their object numbers.
fn state(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, Option<u32>)> {
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    doc.info.annotations.iter().map(|a| (a.contents.clone().unwrap_or_default(), a.object.map(|o| o.0))).collect()
}

/// The comments (text, object number), the edit generation and Undo's label.
type Snapshot = (Vec<(String, Option<u32>)>, u64, Option<String>);

/// What the document and its history are: the comments, the edit generation and Undo's label.
fn snapshot(h: &Harness<'static, PdfCraftApp>) -> Snapshot {
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    (state(h), doc.edit_generation(), doc.can_undo().map(str::to_owned))
}

/// The (page, index) of the comment whose text is `text`, now.
fn key(h: &Harness<'static, PdfCraftApp>, text: &str) -> (usize, usize) {
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    doc.info.annotations.iter().find(|a| a.contents.as_deref() == Some(text)).map(|a| (a.page, a.index)).unwrap_or_else(|| panic!("no {text:?}"))
}

/// In view `view`, select the comments `texts` as Ctrl+Click does (the last has the handles).
fn select(h: &mut Harness<'static, PdfCraftApp>, view: usize, texts: &[&str]) {
    let keys: Vec<(usize, usize)> = texts.iter().map(|t| key(h, t)).collect();
    let cv = &mut h.state_mut().views[view].comments;
    cv.clear_selection();
    for k in keys {
        cv.toggle(k);
    }
    h.run_steps(2);
}

/// Make view `view` the active one and press `key` there.
fn press(h: &mut Harness<'static, PdfCraftApp>, view: usize, key: Key) {
    h.state_mut().active = Some(view);
    h.run_steps(3);
    h.key_press(key);
    h.run_steps(4);
}

/// The texts of `before`'s comments less `gone`.
fn without(before: &[(String, Option<u32>)], gone: &[&str]) -> Vec<(String, Option<u32>)> {
    before.iter().filter(|(t, _)| !gone.contains(&t.as_str())).cloned().collect()
}

/// After a change outside the view, a key deletes either exactly `selected` (what that view held,
/// by identity) or nothing at all, with the document and its Undo unchanged then.
fn deletes_only_the_selected_or_nothing(h: &Harness<'static, PdfCraftApp>, before: &Snapshot, selected: &[&str], what: &str) {
    let after = snapshot(h);
    if after.0 == before.0 {
        assert_eq!(&after, before, "{what}: nothing was deleted, and nothing else changed (document, history)");
    } else {
        assert_eq!(after.0, without(&before.0, selected), "{what}: only the comments that were selected went");
    }
}

/// The selection is made in the first tab; the Duplicate Tab deletes C1, which moves every
/// later comment's place down by one. Delete (or Backspace) in the first tab must not delete C3,
/// C4 (now where C2, C3 were) or anything but C2 and C3 (with C3's reply).
#[test]
fn a_comment_deleted_in_a_duplicate_tab_never_makes_the_selection_delete_another() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = with_duplicate();
        select(&mut h, 0, &["C2", "C3"]);
        select(&mut h, 1, &["C1"]);
        press(&mut h, 1, Key::Delete);
        assert_eq!(state(&h).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["C2", "C3", "R3", "C4", "C5"], "the duplicate deleted C1");
        let before = snapshot(&h);
        press(&mut h, 0, k);
        deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C3", "R3"], &format!("{k:?} after a delete in a duplicate tab"));
    }
}

/// Undo in the Duplicate Tab puts back a comment before the selected ones: an insertion that
/// moves their places up by one.
#[test]
fn a_comment_put_back_by_undo_in_a_duplicate_tab_never_makes_the_selection_delete_another() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = with_duplicate();
        select(&mut h, 1, &["C1"]);
        press(&mut h, 1, Key::Delete);
        select(&mut h, 0, &["C2", "C4"]);
        h.state_mut().active = Some(1);
        h.run_steps(2);
        assert!(h.state_mut().execute("edit.undo"));
        h.run_steps(3);
        assert_eq!(state(&h).first().map(|(t, _)| t.as_str()), Some("C1"), "undo put C1 back first");
        let before = snapshot(&h);
        press(&mut h, 0, k);
        deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C4"], &format!("{k:?} after an undo in a duplicate tab"));
    }
}

/// An edit made outside the interface (automation, the control channel) deletes C1.
#[test]
fn an_automation_edit_never_makes_the_selection_delete_another() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = harness();
        select(&mut h, 0, &["C2", "C3"]);
        let id = h.state().views[0].id;
        let (page, index) = key(&h, "C1");
        h.state_mut().session.apply(id, Edit::DeleteAnnotation { page, index }).expect("deleted");
        h.run_steps(3);
        let before = snapshot(&h);
        press(&mut h, 0, k);
        deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C3", "R3"], &format!("{k:?} after an automation edit"));
        assert_eq!(snapshot(&h), before, "{k:?}: the selection was dropped, so nothing was deleted");
        let toast = h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
        assert!(toast.contains("selected comments were deselected"), "{k:?}: said why: {toast:?}");
    }
}

/// After another tab's delete, a Ctrl+Click here adds to what is left of the selection: every
/// earlier place must be gone by then, or Delete would delete the comments now at them.
#[test]
fn a_ctrl_click_after_another_tabs_delete_adds_to_nothing_stale() {
    let mut h = with_duplicate();
    select(&mut h, 0, &["C2", "C3"]);
    select(&mut h, 1, &["C1"]);
    press(&mut h, 1, Key::Delete);
    h.state_mut().active = Some(0);
    h.run_steps(3);
    let c5 = key(&h, "C5");
    h.state_mut().views[0].comments.toggle(c5);
    h.run_steps(2);
    let before = state(&h);
    press(&mut h, 0, Key::Delete);
    assert_eq!(state(&h), without(&before, &["C5"]), "only C5, the one clicked since");
}

/// Tile View: the same document in two tiles; a delete in one tile moves the other's places.
#[test]
fn a_delete_in_another_tile_never_makes_the_selection_delete_another() {
    let mut h = with_duplicate();
    h.state_mut().set_view_layout(ViewLayout::Tiles);
    h.run_steps(3);
    select(&mut h, 0, &["C3", "C4"]);
    select(&mut h, 1, &["C2"]);
    press(&mut h, 1, Key::Delete);
    let before = snapshot(&h);
    press(&mut h, 0, Key::Delete);
    deletes_only_the_selected_or_nothing(&h, &before, &["C3", "R3", "C4"], "after a delete in another tile");
}

/// Undo in the same view after selecting: upstream's history step drops the selection; Delete
/// then deletes nothing.
#[test]
fn undo_and_redo_in_the_same_view_never_make_the_selection_delete_another() {
    let mut h = harness();
    select(&mut h, 0, &["C1"]);
    press(&mut h, 0, Key::Delete);
    select(&mut h, 0, &["C2", "C3"]);
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    let before = snapshot(&h);
    press(&mut h, 0, Key::Delete);
    deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C3", "R3"], "after Undo");
    select(&mut h, 0, &["C4"]);
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(3);
    let before = snapshot(&h);
    press(&mut h, 0, Key::Backspace);
    deletes_only_the_selected_or_nothing(&h, &before, &["C4"], "after Redo");
}

/// A selection the view keeps through its own delete (upstream remaps it, `annotation_deleted`)
/// still deletes exactly what it holds: here a reply goes from the panel, then Delete.
#[test]
fn a_selection_kept_through_the_views_own_delete_deletes_what_it_holds() {
    let mut h = harness();
    select(&mut h, 0, &["C4", "C5"]);
    let (page, index) = key(&h, "R3");
    assert!(h.state_mut().apply_edit(Edit::DeleteAnnotation { page, index }));
    h.run_steps(3);
    let before = state(&h);
    press(&mut h, 0, Key::Delete);
    assert_eq!(state(&h), without(&before, &["C4", "C5"]), "C4 and C5, nothing else");
}

/// A current selection deletes what it holds, a comment with its reply and a reply alone, as one
/// undo step that brings them back.
#[test]
fn a_current_selection_deletes_its_comments_and_replies_as_one_undo_step() {
    let mut h = harness();
    let all = state(&h);
    select(&mut h, 0, &["C3", "C5"]);
    press(&mut h, 0, Key::Delete);
    assert_eq!(state(&h), without(&all, &["C3", "R3", "C5"]), "C3 with its reply, and C5");
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(state(&h), all, "one undo step");
    select(&mut h, 0, &["R3"]);
    press(&mut h, 0, Key::Backspace);
    assert_eq!(state(&h), without(&all, &["R3"]), "a reply alone");
}

/// The view's own page move takes comments to other places without remapping the selection
/// (upstream remaps it for deletes only): the selection goes with the move, and Delete deletes
/// nothing rather than the comments that took its places.
#[test]
fn moving_the_selected_comments_page_drops_the_selection() {
    let bytes = pdf(2, &[(0, note("P1 first", "")), (1, note("P2 first", "")), (1, note("P2 second", ""))]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("two.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    select(&mut h, 0, &["P1 first"]);
    assert!(h.state_mut().apply_edit(Edit::MovePages { pages: vec![0], to: 2 }));
    h.run_steps(3);
    let before = snapshot(&h);
    press(&mut h, 0, Key::Delete);
    assert_eq!(snapshot(&h), before, "nothing deleted after the page moved");
}

/// A selection set before the view's first pass (as automation, the control channel and tests
/// do) is the document's current one: the guard keeps it, and Delete deletes what it names.
#[test]
fn a_selection_set_before_the_first_pass_is_kept() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("comments.pdf", None, comments()).expect("opens");
        app.views[0].comments.select_only((0, 1));
        app
    });
    h.run_steps(4);
    let before = state(&h);
    assert!(h.state().views[0].comments.selected.is_some(), "the selection survives the first pass");
    press(&mut h, 0, Key::Delete);
    assert_eq!(state(&h), without(&before, &["C2"]));
}

// SYNC.16m: a selection made before the view's first pass (before it was ever stamped), then a
// change before that pass. Each setup runs in the harness builder, before any frame.

/// `comments()` opened with the Comments panel, then `setup` before the first pass.
fn harness_with(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("comments.pdf", None, comments()).expect("opens");
        app.set_option("panel", "comments").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    h
}

/// The (page, index) of `text` in `app`'s first document, now.
fn key_in(app: &PdfCraftApp, text: &str) -> (usize, usize) {
    let doc = app.session.get(app.views[0].id).unwrap();
    doc.info.annotations.iter().find(|a| a.contents.as_deref() == Some(text)).map(|a| (a.page, a.index)).unwrap_or_else(|| panic!("no {text:?}"))
}

/// In view `view` of `app`, select `texts` as Ctrl+Click does.
fn select_in(app: &mut PdfCraftApp, view: usize, texts: &[&str]) {
    let keys: Vec<(usize, usize)> = texts.iter().map(|t| key_in(app, t)).collect();
    let cv = &mut app.views[view].comments;
    cv.clear_selection();
    for k in keys {
        cv.toggle(k);
    }
}

/// The texts and objects of the first document's comments, before any frame (for builders).
fn state_in(app: &PdfCraftApp) -> Vec<(String, Option<u32>)> {
    let doc = app.session.get(app.views[0].id).unwrap();
    doc.info.annotations.iter().map(|a| (a.contents.clone().unwrap_or_default(), a.object.map(|o| o.0))).collect()
}

/// Case 1: a selection made before the first pass, with nothing changed since, is the current
/// one; the real path (`--comment`, the control channel's `set`) included.
#[test]
fn a_fresh_selection_made_before_the_first_pass_deletes_what_it_names() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = harness_with(|app| select_in(app, 0, &["C2", "C4"]));
        let before = state(&h);
        press(&mut h, 0, k);
        assert_eq!(state(&h), without(&before, &["C2", "C4"]), "{k:?}");
        let mut h = harness_with(|app| app.set_option("comment", "1:2").unwrap());
        let before = state(&h);
        press(&mut h, 0, k);
        assert_eq!(state(&h), without(&before, &["C2"]), "{k:?} after --comment 1:2");
    }
}

/// Case 4: a selection made before the first pass, then an automation edit before that pass
/// (here C1 deleted, so every later place names the next comment). On both paths the selection
/// must go with nothing deleted; adopting it as current would delete C3 (and its reply) for C2.
#[test]
fn an_automation_edit_before_the_first_pass_drops_an_earlier_selection() {
    for k in [Key::Delete, Key::Backspace] {
        for via_option in [false, true] {
            let mut h = harness_with(move |app| {
                if via_option {
                    app.set_option("comment", "1:2").unwrap();
                } else {
                    select_in(app, 0, &["C2"]);
                }
                let id = app.views[0].id;
                let (page, index) = key_in(app, "C1");
                app.session.apply(id, Edit::DeleteAnnotation { page, index }).expect("deleted");
            });
            let before = snapshot(&h);
            press(&mut h, 0, k);
            assert_eq!(snapshot(&h), before, "{k:?} (via --comment: {via_option}): nothing deleted, history unchanged");
            assert!(state(&h).iter().any(|(t, _)| t == "C3"), "{k:?}: C3 is not taken for C2");
        }
    }
}

/// Case 2: a selection made before the first pass, then a delete in a Duplicate Tab.
#[test]
fn a_duplicate_tabs_delete_before_the_first_pass_never_makes_the_selection_delete_another() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = harness_with(|app| {
            select_in(app, 0, &["C2", "C3"]);
            app.duplicate_view(0).expect("duplicated");
            let (page, index) = key_in(app, "C1");
            assert!(app.apply_edit(Edit::DeleteAnnotation { page, index }), "deleted in the duplicate");
            app.active = Some(0);
        });
        let before = snapshot(&h);
        press(&mut h, 0, k);
        deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C3", "R3"], &format!("{k:?}"));
    }
}

/// Case 3: a selection made before the first pass, then an insertion before it (an undo in a
/// Duplicate Tab puts C1 back), which moves every later place up.
#[test]
fn an_insertion_before_the_first_pass_never_makes_the_selection_delete_another() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = harness_with(|app| {
            let (page, index) = key_in(app, "C1");
            assert!(app.apply_edit(Edit::DeleteAnnotation { page, index }));
            select_in(app, 0, &["C2", "C4"]);
            app.duplicate_view(0).expect("duplicated");
            assert!(app.execute("edit.undo"), "undo in the duplicate");
            app.active = Some(0);
        });
        assert_eq!(state(&h).first().map(|(t, _)| t.as_str()), Some("C1"));
        let before = snapshot(&h);
        press(&mut h, 0, k);
        deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C4"], &format!("{k:?}"));
    }
}

/// Case 5: a selection made before the first pass, then Undo or Redo in the same view.
#[test]
fn undo_or_redo_before_the_first_pass_never_makes_the_selection_delete_another() {
    for (k, redo) in [(Key::Delete, false), (Key::Backspace, false), (Key::Delete, true), (Key::Backspace, true)] {
        let mut h = harness_with(move |app| {
            let (page, index) = key_in(app, "C1");
            assert!(app.apply_edit(Edit::DeleteAnnotation { page, index }));
            if redo {
                assert!(app.execute("edit.undo"));
            }
            select_in(app, 0, &["C2", "C3"]);
            assert!(app.execute(if redo { "edit.redo" } else { "edit.undo" }));
        });
        let before = snapshot(&h);
        press(&mut h, 0, k);
        deletes_only_the_selected_or_nothing(&h, &before, &["C2", "C3", "R3"], &format!("{k:?}, redo {redo}"));
    }
}

/// Case 6: a selection made before the first pass that the view's own delete remaps is still
/// the current one, and deletes exactly what it holds.
#[test]
fn the_views_own_remapped_delete_before_the_first_pass_keeps_the_selection() {
    for k in [Key::Delete, Key::Backspace] {
        let mut h = harness_with(|app| {
            select_in(app, 0, &["C4", "C5"]);
            let (page, index) = key_in(app, "R3");
            assert!(app.apply_edit(Edit::DeleteAnnotation { page, index }));
            assert_eq!(state_in(app).len(), 5);
        });
        let before = state(&h);
        press(&mut h, 0, k);
        assert_eq!(state(&h), without(&before, &["C4", "C5"]), "{k:?}");
    }
}

/// A page of five comments side by side (Q1 … Q5), for clicks on the page.
fn spread() -> Vec<u8> {
    let annots: Vec<(usize, String)> = (0..5)
        .map(|k| {
            let x = 10 + 58 * k;
            (0, format!("<< /Type /Annot /Subtype /Square /Rect [{x} 60 {} 120] /C [1 0 0] /T (Ada) /Contents (Q{}) >>", x + 40, k + 1))
        })
        .collect();
    pdf(1, &annots)
}

/// Case 7: the page's context menu Delete after an automation edit: the stale selection is gone,
/// and the menu deletes the comment right-clicked, and only that one.
#[test]
fn the_page_menus_delete_after_an_automation_edit_deletes_only_the_comment_clicked() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("spread.pdf", None, spread()).expect("opens");
        app.set_option("quick", "select").unwrap();
        app
    });
    h.run_steps(4);
    crate::support::settle(&mut h);
    select(&mut h, 0, &["Q2", "Q3"]);
    let id = h.state().views[0].id;
    let (page, index) = key(&h, "Q1");
    h.state_mut().session.apply(id, Edit::DeleteAnnotation { page, index }).expect("deleted");
    h.run_steps(3);
    let before = state(&h);
    // Q4's middle, in page space (300 × 200, y up).
    let r = h.state().views[0].page_screen_rect(0).expect("the page is shown");
    let p = egui::pos2(r.left() + 194.0 / 300.0 * r.width(), r.top() + (200.0 - 90.0) / 200.0 * r.height());
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    let item = if h.query_by_label("Delete comments").is_some() { "Delete comments" } else { "Delete" };
    h.get_by_label(item).click();
    h.run_steps(4);
    assert_eq!(state(&h), without(&before, &["Q4"]), "only the comment right-clicked");
}

/// Cases 8 and 9: Ctrl+X and Ctrl+C with a stale comment selection change nothing in the
/// document and copy none of the comments now at the stale places (comments have no Cut; Ctrl+X
/// cuts a picture).
#[test]
fn cut_and_copy_with_a_stale_selection_take_nothing() {
    let mut h = harness();
    select(&mut h, 0, &["C2", "C3"]);
    let id = h.state().views[0].id;
    let (page, index) = key(&h, "C1");
    h.state_mut().session.apply(id, Edit::DeleteAnnotation { page, index }).expect("deleted");
    h.run_steps(3);
    let before = snapshot(&h);
    h.state_mut().active = Some(0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::X);
    h.run_steps(3);
    assert_eq!(snapshot(&h), before, "Ctrl+X changes nothing");
    assert!(!h.state_mut().copy_selected_comments(0), "nothing selected to copy");
}

/// Case 10: Save keeps a current selection (saving moves no comment), and Delete then deletes
/// exactly it.
#[test]
fn a_save_keeps_the_selection() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-stale-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("saved.pdf");
    let mut h = harness();
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    select(&mut h, 0, &["C2", "C4"]);
    assert!(h.state_mut().execute("file.save"));
    h.run_steps(4);
    assert!(out.exists(), "saved");
    assert_eq!(group_texts(&h), ["C2", "C4"], "the selection stays after Save");
    let before = state(&h);
    press(&mut h, 0, Key::Delete);
    assert_eq!(
        state(&h).iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
        without(&before, &["C2", "C4"]).iter().map(|(t, _)| t.clone()).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The texts of view 0's selected comments, in list order.
fn group_texts(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    let v = &h.state().views[0];
    let doc = h.state().session.get(v.id).unwrap();
    let keys = v.comments.selection();
    doc.info.annotations.iter().filter(|a| keys.contains(&(a.page, a.index))).filter_map(|a| a.contents.clone()).collect()
}

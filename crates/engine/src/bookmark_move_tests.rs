//! This fork (BOOKMARK.DND.1): moving a group of bookmarks through the session: one undo step,
//! saved and reopened, refused whole, and nothing at all for a dry run or a move to the same place.

use std::sync::Arc;

use crate::{BookmarkMove, DocId, Edit, EditError, OutlineError, Session};

/// A classic-xref PDF from object bodies (object i+1 = bodies[i]).
fn build(bodies: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

const PAGES: [&str; 5] = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>",
    "<< /Type /Page /Parent 2 0 R >>",
    "<< /Type /Page /Parent 2 0 R >>",
    "<< /Type /Page /Parent 2 0 R >>",
];

fn titles(items: &[pdfcraft_render::OutlineItem]) -> Vec<String> {
    items
        .iter()
        .map(|o| {
            let page = o.page.map_or(0, |p| p + 1);
            if o.children.is_empty() { format!("{}→{page}", o.title) } else { format!("{}→{page}[{}]", o.title, titles(&o.children).join(",")) }
        })
        .collect()
}

/// A, B[b1], C, D on pages 1, 2, 3, 1 (b1 on page 3).
fn session() -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("three.pdf", None, Arc::new(build(&PAGES)), None).expect("opens");
    for (i, (title, page)) in [("A", 0), ("B", 1), ("C", 2), ("D", 0)].into_iter().enumerate() {
        s.apply(id, Edit::AddBookmark { parent: vec![], index: i, title: title.into(), page, left: None, top: None }).unwrap();
    }
    s.apply(id, Edit::AddBookmark { parent: vec![1], index: 0, title: "b1".into(), page: 2, left: None, top: None }).unwrap();
    (s, id)
}

/// What must not move when nothing is meant to change.
fn state(s: &Session, id: DocId) -> (Option<String>, bool, u64, Arc<Vec<u8>>) {
    let d = s.get(id).unwrap();
    (d.can_undo().map(str::to_owned), d.dirty, d.edit_generation(), d.bytes.clone())
}

#[test]
fn a_group_move_is_one_undo_step_and_survives_save() {
    let (mut s, id) = session();
    let outline = |s: &Session| titles(&s.get(id).unwrap().info.outline);
    let before = outline(&s);
    assert_eq!(before, ["A→1", "B→2[b1→3]", "C→3", "D→1"]);
    // The preview of the viewer's outline says where they go: D and B, to before A.
    let info = s.get(id).unwrap().info.outline.clone();
    let expected = BookmarkMove { paths: vec![vec![0], vec![1]], changed: true };
    assert_eq!(crate::preview_move(&info, &[vec![3], vec![1]], &[], 0), Ok(expected.clone()));
    assert_eq!(s.move_bookmarks(id, &[vec![3], vec![1]], &[], 0, false).unwrap(), expected);
    assert_eq!(outline(&s), ["B→2[b1→3]", "D→1", "A→1", "C→3"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Move bookmarks"));
    s.undo(id).unwrap();
    assert_eq!(outline(&s), before, "one step undoes the whole group");
    s.redo(id).unwrap();
    assert_eq!(outline(&s), ["B→2[b1→3]", "D→1", "A→1", "C→3"]);
    // Under another bookmark, from two levels.
    s.move_bookmarks(id, &[vec![0, 0], vec![2]], &[3], 0, false).unwrap();
    assert_eq!(outline(&s), ["B→2", "D→1", "C→3[b1→3,A→1]"]);

    // An incremental save, reopened: titles, destinations, order and nesting.
    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let id2 = again.open("again.pdf", None, saved, None).unwrap();
    assert_eq!(titles(&again.get(id2).unwrap().info.outline), ["B→2", "D→1", "C→3[b1→3,A→1]"]);
}

#[test]
fn a_dry_run_says_what_would_happen_and_changes_nothing() {
    let (mut s, id) = session();
    let (before, bytes) = (state(&s, id), s.save_bytes(id).unwrap());
    let dry = s.move_bookmarks(id, &[vec![3], vec![1]], &[], 0, true).unwrap();
    assert_eq!(dry, BookmarkMove { paths: vec![vec![0], vec![1]], changed: true });
    let after = state(&s, id);
    assert_eq!((&after.0, after.1, after.2), (&before.0, before.1, before.2), "no undo step, same edited flag, same generation");
    assert!(Arc::ptr_eq(&after.3, &before.3), "the working file is the same");
    assert_eq!(s.save_bytes(id).unwrap(), bytes);
    // The real move then does exactly that.
    assert_eq!(s.move_bookmarks(id, &[vec![3], vec![1]], &[], 0, false).unwrap(), dry);
    // A refused dry run says why, as the move would.
    let e = s.move_bookmarks(id, &[vec![0]], &[0], 0, true).unwrap_err();
    assert!(matches!(e, EditError::Bookmark(OutlineError::IntoItself)), "{e}");
}

/// A dry run on a document nobody has edited: still not marked edited afterwards.
#[test]
fn a_dry_run_leaves_an_untouched_document_untouched() {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Outlines /First 5 0 R /Last 6 0 R /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Title (A) /Parent 3 0 R /Next 6 0 R >>",
        "<< /Title (B) /Parent 3 0 R /Prev 5 0 R >>",
    ];
    let mut s = Session::new();
    let id = s.open("two.pdf", None, Arc::new(build(&bodies)), None).expect("opens");
    let before = s.get(id).unwrap().bytes.clone();
    assert_eq!(s.move_bookmarks(id, &[vec![1]], &[], 0, true).unwrap(), BookmarkMove { paths: vec![vec![0]], changed: true });
    let d = s.get(id).unwrap();
    assert!(!d.dirty && d.can_undo().is_none() && Arc::ptr_eq(&d.bytes, &before));
}

#[test]
fn a_move_to_where_the_bookmarks_already_are_adds_no_undo_step() {
    let (mut s, id) = session();
    let (before, bytes) = (state(&s, id), s.save_bytes(id).unwrap());
    // B is already second: before C, and after A, are where it is.
    for index in [1, 2] {
        let r = s.move_bookmarks(id, &[vec![1]], &[], index, false).unwrap();
        assert_eq!(r, BookmarkMove { paths: vec![vec![1]], changed: false });
    }
    let after = state(&s, id);
    assert_eq!((&after.0, after.1, after.2), (&before.0, before.1, before.2));
    assert!(Arc::ptr_eq(&after.3, &before.3));
    assert_eq!(s.save_bytes(id).unwrap(), bytes);
}

#[test]
fn a_refused_group_changes_nothing_and_adds_no_undo_step() {
    let (mut s, id) = session();
    let (before, bytes) = (state(&s, id), s.save_bytes(id).unwrap());
    // Through the session's own entry point, and through the edit applied directly.
    let e = s.move_bookmarks(id, &[vec![0], vec![1]], &[1, 0], 0, false).unwrap_err();
    assert!(matches!(e, EditError::Bookmark(OutlineError::IntoItself)), "{e}");
    let e = s.apply(id, Edit::MoveBookmarks { paths: vec![vec![0], vec![9]], to_parent: vec![], index: 0 }).unwrap_err();
    assert!(matches!(e, EditError::Bookmark(OutlineError::NoSuchBookmark(ref p)) if p == &vec![9]), "{e}");
    let after = state(&s, id);
    assert_eq!((&after.0, after.1, after.2), (&before.0, before.1, before.2));
    assert_eq!(s.save_bytes(id).unwrap(), bytes, "the document is as it was");
}

/// The preview on the viewer's outline and the move on the document agree, target by target.
#[test]
fn the_preview_agrees_with_the_move_for_every_target() {
    let (s, id) = session();
    let info = s.get(id).unwrap().info.outline.clone();
    let targets: [&[usize]; 6] = [&[], &[0], &[1], &[1, 0], &[2], &[3]];
    for paths in [vec![vec![0]], vec![vec![1]], vec![vec![1, 0], vec![3]], vec![vec![2], vec![0]], vec![vec![1], vec![1, 0]]] {
        for to in targets {
            for index in 0..5 {
                // A fresh document each time: the move is real.
                let (mut each, id) = session();
                let preview = crate::preview_move(&info, &paths, to, index);
                let real = each.move_bookmarks(id, &paths, to, index, false);
                match (preview, real) {
                    (Ok(p), Ok(r)) => assert_eq!(p, r, "{paths:?} -> {to:?} {index}"),
                    (Err(p), Err(EditError::Bookmark(r))) => assert_eq!(p, r, "{paths:?} -> {to:?} {index}"),
                    (p, r) => panic!("{paths:?} -> {to:?} {index}: preview {p:?}, move {r:?}"),
                }
            }
        }
    }
}

/// Moving bookmarks is assembling the document: refused without that permission, dry run included.
#[test]
fn moving_bookmarks_needs_the_assemble_permission() {
    let edit = Edit::MoveBookmarks { paths: vec![vec![0]], to_parent: vec![], index: 1 };
    let all = pdfcraft_cos::Permissions { bits: -1, owner: false, revision: 3 };
    let none = pdfcraft_cos::Permissions { bits: 0, owner: false, revision: 3 };
    assert!(all.assemble() && !none.assemble());
    assert_eq!(crate::edit::check_permission(&edit, &all), Ok(()));
    assert_eq!(crate::edit::check_permission(&edit, &none), Err(EditError::NotPermitted("page changes")));
}

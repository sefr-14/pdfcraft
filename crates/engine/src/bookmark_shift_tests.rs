//! This fork (BOOKMARK.1): group Indent and Outdent through the session: one undo step, saved and
//! reopened, refused whole.

use std::sync::Arc;

use crate::{Edit, EditError, OutlineError, Session};

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

#[test]
fn a_group_indent_is_one_undo_step_and_survives_save() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("three.pdf", None, Arc::new(build(&PAGES)), None).expect("opens");
    let outline = |s: &Session| titles(&s.get(id).unwrap().info.outline);
    for (i, (title, page)) in [("A", 0), ("B", 1), ("C", 2), ("D", 0)].into_iter().enumerate() {
        s.apply(id, Edit::AddBookmark { parent: vec![], index: i, title: title.into(), page, left: None, top: None }).unwrap();
    }
    let before = outline(&s);
    // The preview of the viewer's outline says where they go.
    let info = s.get(id).unwrap().info.outline.clone();
    assert_eq!(crate::preview_shift(&info, &[vec![1], vec![2]], false), Ok(vec![vec![0, 0], vec![0, 1]]));
    s.apply(id, Edit::ShiftBookmarks { paths: vec![vec![1], vec![2]], outdent: false }).unwrap();
    assert_eq!(outline(&s), ["A→1[B→2,C→3]", "D→1"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Indent bookmarks"));
    s.undo(id).unwrap();
    assert_eq!(outline(&s), before, "one step undoes the whole group");
    s.redo(id).unwrap();
    assert_eq!(outline(&s), ["A→1[B→2,C→3]", "D→1"]);
    s.apply(id, Edit::ShiftBookmarks { paths: vec![vec![0, 1]], outdent: true }).unwrap();
    assert_eq!(outline(&s), ["A→1[B→2]", "C→3", "D→1"]);
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Outdent bookmarks"));

    // An incremental save, reopened: titles, destinations, order and nesting.
    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let id2 = again.open("again.pdf", None, saved, None).unwrap();
    assert_eq!(titles(&again.get(id2).unwrap().info.outline), ["A→1[B→2]", "C→3", "D→1"]);
}

#[test]
fn a_refused_group_changes_nothing_and_adds_no_undo_step() {
    let mut s = Session::new();
    let id = s.open("three.pdf", None, Arc::new(build(&PAGES)), None).expect("opens");
    for (i, title) in ["A", "B"].into_iter().enumerate() {
        s.apply(id, Edit::AddBookmark { parent: vec![], index: i, title: title.into(), page: 0, left: None, top: None }).unwrap();
    }
    let (label, bytes) = (s.get(id).unwrap().can_undo().map(str::to_owned), s.save_bytes(id).unwrap());
    let e = s.apply(id, Edit::ShiftBookmarks { paths: vec![vec![1], vec![0]], outdent: false }).unwrap_err();
    assert!(matches!(e, EditError::Bookmark(OutlineError::CannotIndent(ref p)) if p == &vec![0]), "{e}");
    assert_eq!(s.get(id).unwrap().can_undo().map(str::to_owned), label, "no undo step");
    assert_eq!(s.save_bytes(id).unwrap(), bytes, "the document is as it was");
}

/// Regression (BOOKMARK.1 review): one bookmark object listed by two parents, only one of which
/// the group changes, is refused whole, with no undo step and no change.
#[test]
fn a_bookmark_linked_from_two_parents_is_refused_with_no_undo_step() {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Outlines /First 5 0 R /Last 7 0 R /Count 3 >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Title (A) /Parent 3 0 R /Next 6 0 R /First 8 0 R /Last 8 0 R /Count 1 >>",
        "<< /Title (B) /Parent 3 0 R /Prev 5 0 R /Next 7 0 R >>",
        "<< /Title (C) /Parent 3 0 R /Prev 6 0 R /First 8 0 R /Last 8 0 R /Count 1 >>",
        "<< /Title (X) /Parent 5 0 R >>",
    ];
    let mut s = Session::new();
    let id = s.open("shared.pdf", None, Arc::new(build(&bodies)), None).expect("opens");
    let (before, bytes) = (s.get(id).unwrap().bytes.clone(), s.save_bytes(id).unwrap());
    let e = s.apply(id, Edit::ShiftBookmarks { paths: vec![vec![1]], outdent: false }).unwrap_err();
    assert!(matches!(e, EditError::Bookmark(OutlineError::Tangled)), "{e}");
    let doc = s.get(id).unwrap();
    assert!(Arc::ptr_eq(&doc.bytes, &before), "the working file is the same");
    assert_eq!(s.save_bytes(id).unwrap(), bytes, "and its contents");
    assert_eq!(s.get(id).unwrap().can_undo(), None, "no undo step");
    assert!(!s.get(id).unwrap().dirty, "not marked edited");
}

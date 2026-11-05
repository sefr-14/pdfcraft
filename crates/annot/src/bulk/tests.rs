//! This fork (COMMENTS.1): deleting several comments at once, below the engine: each slot must
//! hold what its target says, or nothing is deleted.

use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full};

use super::{DeleteTarget, delete_annotations, deletion_set};
use crate::AnnotError;

/// One page whose `/Annots` are objects 4.. (`bodies`), plus an inline comment last.
fn page(bodies: &[&str]) -> Document {
    let refs: Vec<String> = (0..bodies.len()).map(|k| format!("{} 0 R", 4 + k)).collect();
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /Annots [{} << /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (Inline) >>] >>", refs.join(" ")),
    ];
    objs.extend(bodies.iter().map(ToString::to_string));
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
    Document::open(Arc::new(out)).expect("opens")
}

const A: &str = "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (A) /Popup 6 0 R >>"; // 4
const A1: &str = "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (a1) /IRT 4 0 R >>"; // 5
const POP: &str = "<< /Type /Annot /Subtype /Popup /Rect [0 0 90 60] /Parent 4 0 R >>"; // 6
const B: &str = "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (B) >>"; // 7

fn at(index: usize, num: u32) -> DeleteTarget {
    DeleteTarget { page: 0, index, object: Some((num, 0)) }
}

fn bytes(d: &Document) -> Vec<u8> {
    write_full(d, &SaveOptions::default()).expect("writes")
}

#[test]
fn a_comment_goes_with_its_reply_and_pop_up_and_the_rest_keeps_its_order() {
    let mut d = page(&[A, A1, POP, B]);
    let set = delete_annotations(&mut d, &[at(0, 4), at(0, 4)]).unwrap();
    assert_eq!(set.removed, [(0, 0, false), (0, 1, false), (0, 2, true)], "the pop-up is marked as one");
    let left = crate::summaries(&d);
    assert_eq!(left.iter().map(|s| s.contents.clone().unwrap_or_default()).collect::<Vec<_>>(), ["B", "Inline"]);
    // A comment and its reply both named go once.
    let mut d = page(&[A, A1, POP, B]);
    assert_eq!(delete_annotations(&mut d, &[at(1, 5), at(0, 4)]).unwrap().removed.len(), 3);
    // An inline comment, named as one.
    let mut d = page(&[A, A1, POP, B]);
    delete_annotations(&mut d, &[DeleteTarget { page: 0, index: 4, object: None }]).unwrap();
    assert_eq!(crate::summaries(&d).len(), 3);
}

/// A slot that doesn't hold what its target says refuses everything, before anything is written.
#[test]
fn a_slot_holding_something_else_refuses_the_whole_deletion() {
    let cases: [(&str, Vec<DeleteTarget>); 5] = [
        ("another object", vec![at(3, 7), at(0, 5)]),
        ("an indirect comment claimed as inline", vec![at(3, 7), DeleteTarget { page: 0, index: 0, object: None }]),
        ("the inline comment claimed as an object", vec![at(3, 7), DeleteTarget { page: 0, index: 4, object: Some((4, 0)) }]),
        ("a position past the end", vec![at(3, 7), at(9, 4)]),
        ("a page that isn't there", vec![at(3, 7), DeleteTarget { page: 5, index: 0, object: Some((4, 0)) }]),
    ];
    for (what, targets) in cases {
        let mut d = page(&[A, A1, POP, B]);
        let before = bytes(&d);
        assert!(deletion_set(&d, &targets).is_err(), "{what}");
        assert!(delete_annotations(&mut d, &targets).is_err(), "{what}");
        assert!(d.modified_objects().is_empty() && bytes(&d) == before, "{what}: nothing written");
    }
    assert!(matches!(deletion_set(&page(&[A]), &[]), Err(AnnotError::Invalid(_))));
    assert!(matches!(deletion_set(&page(&[A]), &vec![at(0, 4); super::MAX_DELETE_TARGETS + 1]), Err(AnnotError::Invalid(_))));
}

#[test]
fn a_locked_reply_keeps_every_comment() {
    let locked = "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (a1) /IRT 4 0 R /F 128 >>";
    let mut d = page(&[A, locked, POP, B]);
    assert!(delete_annotations(&mut d, &[at(3, 7), at(0, 4)]).is_err());
    assert_eq!(crate::summaries(&d).len(), 4, "B stays too");
}

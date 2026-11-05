//! This fork (COMMENTS.1): a comment's object identity.

use std::sync::Arc;

use crate::{Edit, Session};

/// A classic-xref PDF from object bodies (object i+1 = bodies[i]).
pub(crate) fn build(bodies: &[&str]) -> Vec<u8> {
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

/// Both readers name every comment by the same object: the inspector at open, and the engine's
/// comment list after a comment edit. An inline comment has none; a reference to a reference
/// names the object it ends at; a comment two pages list is filed under the first.
#[test]
fn both_comment_readers_give_the_same_object() {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R << /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (Inline) >>] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [8 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (Shared) /P 3 0 R >>",
        "7 0 R",
        "<< /Type /Annot /Subtype /Text /Rect [30 0 50 20] /Contents (Chained) /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (Second page) /P 4 0 R >>",
    ];
    let mut s = Session::new();
    let id = s.open("ids.pdf", None, Arc::new(build(&bodies)), None).expect("opens");
    let key = |a: &pdfcraft_render::Annotation| (a.contents.clone().unwrap_or_default(), a.page, a.index, a.object);
    let mut opened: Vec<_> = s.get(id).unwrap().info.annotations.iter().map(key).collect();
    opened.sort();
    assert_eq!(
        opened,
        [
            ("Chained".to_string(), 0, 1, Some((7, 0))),
            ("Inline".to_string(), 0, 2, None),
            ("Second page".to_string(), 1, 0, Some((8, 0))),
            ("Shared".to_string(), 0, 0, Some((5, 0))),
        ]
    );
    // A comment edit refreshes the list from the engine's own reader.
    s.apply(id, Edit::SetAnnotationContents { page: 1, index: 0, text: "Second page".into() }).unwrap();
    let refreshed: Vec<_> = s.get(id).unwrap().info.annotations.iter().map(key).collect();
    for k in &opened {
        assert!(refreshed.contains(k), "{k:?} reads the same after the edit: {refreshed:?}");
    }
}

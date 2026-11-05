// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! This fork (COMMENTS.1): a comment's object identity, and deleting several comments at once.

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

use crate::{DeleteTarget, EditError};

/// Page 1: comment R (with a pop-up P), its reply r1, r2 replying to r1, a status s set on R,
/// comment Q with reply q1, and an unrelated comment U. Page 2: x, a reply to R filed on another
/// page, and comment V.
fn threads() -> Vec<u8> {
    let text = |what: &str, extra: &str| format!("<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents ({what}){extra} >>");
    let bodies: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [13 0 R 14 0 R] >>".into(),
        text("R", " /Popup 9 0 R"),                                                  // 5
        text("r1", " /IRT 5 0 R"),                                                   // 6
        text("r2", " /IRT 6 0 R"),                                                   // 7
        text("s", " /IRT 5 0 R /State (Accepted) /StateModel (Review)"),             // 8
        "<< /Type /Annot /Subtype /Popup /Rect [0 0 90 60] /Parent 5 0 R >>".into(), // 9
        text("Q", ""),                                                               // 10
        text("q1", " /IRT 10 0 R"),                                                  // 11
        text("U", ""),                                                               // 12
        text("x", " /IRT 5 0 R"),                                                    // 13
        text("V", ""),                                                               // 14
    ];
    build(&bodies.iter().map(String::as_str).collect::<Vec<_>>())
}

/// The target for the comment whose text is `what`, as the document lists it now.
fn target(s: &Session, id: crate::DocId, what: &str) -> DeleteTarget {
    let a = s.get(id).unwrap().info.annotations.iter().find(|a| a.contents.as_deref() == Some(what)).unwrap_or_else(|| panic!("{what}"));
    DeleteTarget { page: a.page, index: a.index, object: a.object }
}

fn texts(s: &Session, id: crate::DocId) -> Vec<String> {
    s.get(id).unwrap().info.annotations.iter().filter_map(|a| a.contents.clone()).collect()
}

fn open_threads() -> (Session, crate::DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("threads.pdf", None, Arc::new(threads()), None).expect("opens");
    (s, id)
}

/// Delete `whats` (by text) and return the preview's counts, the comments left, and the saved
/// bytes; the preview must say what the edit then removes.
fn delete(whats: &[&str]) -> ((usize, usize), Vec<String>, Arc<Vec<u8>>) {
    let (mut s, id) = open_threads();
    let targets: Vec<DeleteTarget> = whats.iter().map(|w| target(&s, id, w)).collect();
    let before = s.get(id).unwrap().info.annotations.len();
    let counts = s.get(id).unwrap().comments_deletion_preview(&targets).expect("previews");
    s.apply(id, Edit::DeleteAnnotations { targets }).expect("deletes");
    let after = s.get(id).unwrap().info.annotations.len();
    assert_eq!(before - after, counts.0 + counts.1, "{whats:?}: the preview counts every comment and reply that went, once");
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Delete comments"));
    (counts, texts(&s, id), s.save_full_bytes(id).unwrap())
}

#[test]
fn overlapping_targets_count_each_comment_and_reply_once() {
    // R goes with r1, r2 and its status s (and its pop-up, which isn't counted).
    let alone = delete(&["R"]);
    assert_eq!(alone.0, (1, 3));
    assert_eq!(alone.1, ["Q", "q1", "U", "x", "V"]);
    // A reply listed with its comment is one of its replies, not another comment.
    for whats in [&["R", "r1"][..], &["R", "r2", "r2", "R"], &["R", "s"], &["r1", "R"]] {
        let (counts, left, bytes) = delete(whats);
        assert_eq!(counts, (1, 3), "{whats:?}");
        assert_eq!(left, alone.1, "{whats:?}");
        assert_eq!(bytes, alone.2, "{whats:?}: the same file as deleting R alone");
    }
    let (counts, left, _) = delete(&["R", "Q", "q1"]);
    assert_eq!(counts, (2, 4));
    assert_eq!(left, ["U", "x", "V"], "U keeps its place");
}

#[test]
fn a_bulk_delete_is_one_undo_step() {
    let (mut s, id) = open_threads();
    let before = format!("{:?}", s.get(id).unwrap().info.annotations);
    let targets = vec![target(&s, id, "R"), target(&s, id, "Q")];
    s.apply(id, Edit::DeleteAnnotations { targets }).unwrap();
    assert_eq!(texts(&s, id), ["U", "x", "V"]);
    s.undo(id).unwrap();
    assert_eq!(format!("{:?}", s.get(id).unwrap().info.annotations), before, "one undo puts every comment back as it was");
    s.redo(id).unwrap();
    assert_eq!(texts(&s, id), ["U", "x", "V"]);
    // An incremental save, reopened.
    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let id2 = again.open("again.pdf", None, saved, None).unwrap();
    assert_eq!(texts(&again, id2), ["U", "x", "V"]);
}

/// Refused whole: nothing changes, no undo step, not marked edited.
fn refused(targets: impl Fn(&Session, crate::DocId) -> Vec<DeleteTarget>) -> EditError {
    let (mut s, id) = open_threads();
    let targets = targets(&s, id);
    let bytes = s.save_full_bytes(id).unwrap();
    let preview = s.get(id).unwrap().comments_deletion_preview(&targets);
    let e = s.apply(id, Edit::DeleteAnnotations { targets }).expect_err("refused");
    assert!(preview.is_err(), "{e}: the preview refuses too");
    assert_eq!(s.save_full_bytes(id).unwrap(), bytes, "{e}: the document is as it was");
    assert_eq!(s.get(id).unwrap().can_undo(), None, "{e}: no undo step");
    assert!(!s.get(id).unwrap().dirty, "{e}: not marked edited");
    e
}

#[test]
fn a_reply_goes_only_with_its_comment_and_stale_targets_refuse_everything() {
    // A reply on its own, or with another comment.
    refused(|s, id| vec![target(s, id, "r1")]);
    refused(|s, id| vec![target(s, id, "Q"), target(s, id, "r1")]);
    // A reply to R on another page: deleting R leaves it, so it can't go with R.
    refused(|s, id| vec![target(s, id, "R"), target(s, id, "x")]);
    // A slot that now holds another object, a position past the end, an inline claim for an
    // indirect comment.
    refused(|s, id| vec![target(s, id, "R"), DeleteTarget { object: Some((99, 0)), ..target(s, id, "r1") }]);
    refused(|s, id| vec![DeleteTarget { index: 40, ..target(s, id, "Q") }]);
    refused(|s, id| vec![DeleteTarget { object: None, ..target(s, id, "Q") }]);
    refused(|_, _| Vec::new());
}

#[test]
fn a_locked_reply_or_a_document_without_comment_rights_refuses() {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (A) >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (a1) /IRT 4 0 R /F 128 >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (B) >>",
    ];
    let mut s = Session::new();
    let id = s.open("locked.pdf", None, Arc::new(build(&bodies)), None).unwrap();
    let targets = vec![target(&s, id, "B"), target(&s, id, "A")];
    assert!(s.get(id).unwrap().comments_deletion_preview(&targets).is_err());
    assert!(s.apply(id, Edit::DeleteAnnotations { targets }).is_err());
    assert_eq!(texts(&s, id), ["A", "a1", "B"], "B stays too: all or nothing");
}

/// Two comments that reply to each other: neither settles as a reply, so both are cards.
#[test]
fn comments_that_reply_to_each_other_count_as_the_cards_chosen() {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R] >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (A) /IRT 5 0 R >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (B) /IRT 4 0 R >>",
        "<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (C) >>",
    ];
    let run = |whats: &[&str]| {
        let mut s = Session::new();
        let id = s.open("loop.pdf", None, Arc::new(build(&bodies)), None).unwrap();
        let targets: Vec<DeleteTarget> = whats.iter().map(|w| target(&s, id, w)).collect();
        let counts = s.get(id).unwrap().comments_deletion_preview(&targets).unwrap();
        s.apply(id, Edit::DeleteAnnotations { targets }).unwrap();
        (counts, texts(&s, id))
    };
    assert_eq!(run(&["A", "B"]), ((2, 0), vec!["C".to_string()]));
    assert_eq!(run(&["A"]), ((1, 1), vec!["C".to_string()]));
}

/// A small xorshift: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if n == 0 { 0 } else { (self.0 % n as u64) as usize }
    }
}

/// Random threads on one page (replies to earlier comments or replies, pop-ups on some comments)
/// and random targets, comments and replies alike. When the group is accepted, its counts are
/// what goes and it leaves what deleting its comments one at a time leaves; when it is refused,
/// nothing changes.
#[test]
fn a_group_is_its_comments_deleted_one_at_a_time() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut accepted = 0;
    for round in 0..400 {
        let n = 1 + rng.below(12);
        // Object 4 + k is annotation k; pop-ups follow as 4 + n + j.
        let mut annots: Vec<String> = Vec::new();
        let mut popups: Vec<String> = Vec::new();
        for k in 0..n {
            let irt = if k > 0 && rng.below(2) == 0 { format!(" /IRT {} 0 R", 4 + rng.below(k)) } else { String::new() };
            let popup = if irt.is_empty() && rng.below(3) == 0 {
                popups.push(format!("<< /Type /Annot /Subtype /Popup /Rect [0 0 90 60] /Parent {} 0 R >>", 4 + k));
                format!(" /Popup {} 0 R", 3 + n + popups.len())
            } else {
                String::new()
            };
            annots.push(format!("<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (N{k}){irt}{popup} >>"));
        }
        let all = (0..n + popups.len()).map(|k| format!("{} 0 R", 4 + k)).collect::<Vec<_>>().join(" ");
        let mut bodies: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
            format!("<< /Type /Page /Parent 2 0 R /Annots [{all}] >>"),
        ];
        bodies.extend(annots);
        bodies.extend(popups);
        let bytes = Arc::new(build(&bodies.iter().map(String::as_str).collect::<Vec<_>>()));
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("random.pdf", None, bytes.clone(), None).unwrap();
        let listed = s.get(id).unwrap().info.annotations.clone();
        let picks: Vec<DeleteTarget> = (0..1 + rng.below(4))
            .map(|_| {
                let a = &listed[rng.below(listed.len())];
                DeleteTarget { page: a.page, index: a.index, object: a.object }
            })
            .collect();
        let before = s.save_full_bytes(id).unwrap();
        match s.apply(id, Edit::DeleteAnnotations { targets: picks.clone() }) {
            Ok(()) => {
                accepted += 1;
                let left: Vec<_> = s.get(id).unwrap().info.annotations.iter().map(|a| a.object).collect();
                let mut one = Session::new().with_clock(|| 1_700_000_000);
                let oid = one.open("one.pdf", None, bytes.clone(), None).unwrap();
                let preview = one.get(oid).unwrap().comments_deletion_preview(&picks).unwrap();
                assert_eq!(listed.len() - left.len(), preview.0 + preview.1, "round {round}: counts are what went");
                // The comments among the picks, deleted one at a time where they now are.
                let roots: Vec<_> =
                    picks.iter().filter(|p| listed.iter().any(|a| a.object == p.object && a.is_comment())).map(|p| p.object).collect();
                for object in roots {
                    if let Some(a) = one.get(oid).unwrap().info.annotations.iter().find(|a| a.object == object).cloned() {
                        one.apply(oid, Edit::DeleteAnnotation { page: a.page, index: a.index }).unwrap();
                    }
                }
                let single: Vec<_> = one.get(oid).unwrap().info.annotations.iter().map(|a| a.object).collect();
                assert_eq!(left, single, "round {round}: {picks:?}");
            }
            Err(_) => {
                assert_eq!(s.save_full_bytes(id).unwrap(), before, "round {round}: a refusal changes nothing");
                assert_eq!(s.get(id).unwrap().can_undo(), None);
            }
        }
    }
    assert!(accepted > 100, "enough groups were accepted to mean something ({accepted})");
}

//! BOOKMARK.1 (this fork): group Indent and Outdent.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_full};

use super::{Shift, shift_bookmarks};
use crate::OutlineError;

/// A classic-xref PDF from object bodies (object i+1 = bodies[i]).
fn build(bodies: &[String]) -> Vec<u8> {
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

fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).expect("opens")
}

/// A one-page document without bookmarks.
fn blank() -> Document {
    open(build(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ]))
}

/// A one-page document whose outline root is object 3 and whose bookmarks are `items`, as
/// objects 5, 6, … in order (object 4 is the page).
fn outline_with(items: &[&str]) -> Document {
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Outlines /First 5 0 R /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    b.extend(items.iter().map(ToString::to_string));
    open(build(&b))
}

/// Bookmarks from a spec like `"A[a1,a2],B"`: top-level titles, children in brackets.
fn tree(spec: &str) -> Document {
    let mut d = blank();
    let mut path: Vec<usize> = vec![0];
    let mut title = String::new();
    let add = |d: &mut Document, path: &[usize], title: &mut String| {
        if !title.is_empty() {
            let (last, parent) = path.split_last().expect("a path");
            crate::add_bookmark(d, parent, *last, title, 0).expect("adds");
            title.clear();
        }
    };
    for c in spec.chars() {
        match c {
            '[' => {
                add(&mut d, &path, &mut title);
                path.push(0);
            }
            ']' => {
                add(&mut d, &path, &mut title);
                path.pop();
                *path.last_mut().expect("a level") += 1;
            }
            ',' => {
                if !title.is_empty() {
                    add(&mut d, &path, &mut title);
                    *path.last_mut().expect("a level") += 1;
                }
            }
            c => title.push(c),
        }
    }
    add(&mut d, &path, &mut title);
    d
}

/// The outline as a spec, the reverse of [`tree`].
fn spec(d: &Document) -> String {
    let b = crate::bookmarks(d);
    fn level(b: &[crate::Bookmark], at: &mut usize, depth: usize) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(x) = b.get(*at).filter(|x| x.path.len() == depth + 1) {
            *at += 1;
            let kids = level(b, at, depth + 1);
            out.push(if kids.is_empty() { x.title.clone() } else { format!("{}[{}]", x.title, kids.join(",")) });
        }
        out
    }
    level(&b, &mut 0, 0).join(",")
}

fn bytes(d: &Document) -> Vec<u8> {
    write_full(d, &SaveOptions::default()).expect("writes")
}

/// A refusal changes nothing: no object is rewritten and the file is the same.
fn refused(d: &mut Document, paths: &[Vec<usize>], shift: Shift) -> OutlineError {
    let before = bytes(d);
    let mut touched = d.modified_objects();
    touched.sort_unstable();
    let e = shift_bookmarks(d, paths, shift).expect_err("refused");
    let mut now = d.modified_objects();
    now.sort_unstable();
    assert_eq!(now, touched, "{e}: nothing more is rewritten");
    assert_eq!(bytes(d), before, "{e}: the file is unchanged");
    e
}

#[test]
fn a_run_of_siblings_indents_under_the_bookmark_above_it_in_order() {
    let mut d = tree("A,B,C,D");
    assert_eq!(shift_bookmarks(&mut d, &[vec![2], vec![1]], Shift::In).unwrap(), [vec![0, 0], vec![0, 1]]);
    assert_eq!(spec(&d), "A[B,C],D");
}

#[test]
fn a_parent_and_its_child_both_picked_move_once() {
    let mut d = tree("A,B[b1,b2]");
    assert_eq!(shift_bookmarks(&mut d, &[vec![1], vec![1, 0]], Shift::In).unwrap(), [vec![0, 0]]);
    assert_eq!(spec(&d), "A[B[b1,b2]]");
    let mut d = tree("A[a1[x]],B");
    assert_eq!(shift_bookmarks(&mut d, &[vec![0, 0, 0], vec![0, 0]], Shift::Out).unwrap(), [vec![1]]);
    assert_eq!(spec(&d), "A,a1[x],B");
}

#[test]
fn picks_under_different_parents_each_move_by_their_own_rule() {
    let mut d = tree("A[a1,a2],B[b1,b2]");
    shift_bookmarks(&mut d, &[vec![0, 1], vec![1, 1]], Shift::In).unwrap();
    assert_eq!(spec(&d), "A[a1[a2]],B[b1[b2]]");
    // One pick's new parent is another pick's parent: both happen, in outline order.
    let mut d = tree("P[a,b],X");
    shift_bookmarks(&mut d, &[vec![1], vec![0, 1]], Shift::In).unwrap();
    assert_eq!(spec(&d), "P[a[b],X]");
}

#[test]
fn outdent_follows_the_parent_and_leaves_later_siblings() {
    // The owner's example: b.2 moves out to follow B.
    let mut d = tree("A,B[b1,b2],C");
    assert_eq!(shift_bookmarks(&mut d, &[vec![1, 1]], Shift::Out).unwrap(), [vec![2]]);
    assert_eq!(spec(&d), "A,B[b1],b2,C");
    // Two picks keep their order straight after the parent; the one between them stays.
    let mut d = tree("A,B[b1,b2,b3],C");
    assert_eq!(shift_bookmarks(&mut d, &[vec![1, 2], vec![1, 0]], Shift::Out).unwrap(), [vec![2], vec![3]]);
    assert_eq!(spec(&d), "A,B[b2],b1,b3,C");
}

#[test]
fn an_impossible_step_refuses_the_whole_group() {
    let mut d = tree("A,B,C");
    // B could go under A, but A is first at its level: nothing moves.
    assert_eq!(refused(&mut d, &[vec![1], vec![0]], Shift::In), OutlineError::CannotIndent(vec![0]));
    let mut d = tree("A[a1],B");
    assert_eq!(refused(&mut d, &[vec![0, 0], vec![1]], Shift::Out), OutlineError::CannotOutdent(vec![1]));
    let mut d = tree("A,B");
    assert_eq!(refused(&mut d, &[vec![1], vec![5]], Shift::In), OutlineError::NoSuchBookmark(vec![5]));
    assert_eq!(refused(&mut d, &[vec![]], Shift::In), OutlineError::NoSuchBookmark(vec![]));
    assert_eq!(refused(&mut d, &vec![vec![1]; super::MAX_SHIFT + 1], Shift::In), OutlineError::TooMany);
    assert_eq!(spec(&d), "A,B");
}

/// `P` then `Q`, with a chain `height` levels deep under `Q`, and `B` first when `lead` is set.
fn deep_after(height: usize, lead: bool) -> Document {
    let mut d = blank();
    let at = usize::from(lead);
    if lead {
        crate::add_bookmark(&mut d, &[], 0, "B", 0).unwrap();
    }
    crate::add_bookmark(&mut d, &[], at, "P", 0).unwrap();
    crate::add_bookmark(&mut d, &[], at + 1, "Q", 0).unwrap();
    let mut parent = vec![at + 1];
    for k in 0..height {
        crate::add_bookmark(&mut d, &parent, 0, &format!("D{k}"), 0).unwrap();
        parent.push(0);
    }
    d
}

/// Depth counts from 0 at the top level, as the listing and the viewer do: both list depth 32
/// (33 levels) and no deeper.
#[test]
fn a_group_moves_down_to_the_deepest_listed_level_and_no_further() {
    // Q's chain reaches depth 31; indented under P, its last bookmark is at depth 32.
    let mut d = deep_after(31, false);
    shift_bookmarks(&mut d, &[vec![1]], Shift::In).unwrap();
    let listed = crate::bookmarks(&d);
    assert_eq!(listed.iter().map(|b| b.path.len()).max(), Some(33), "depth 32 is still listed");
    assert_eq!(listed.last().map(|b| b.title.as_str()), Some("D30"));
    // One level more would be depth 33.
    let mut d = deep_after(32, false);
    assert_eq!(refused(&mut d, &[vec![1]], Shift::In), OutlineError::TooDeep(vec![1]));
    // Refused whole beside a pick that could move.
    let mut d = deep_after(32, true);
    crate::add_bookmark(&mut d, &[], 3, "Z", 0).unwrap();
    assert!(matches!(refused(&mut d, &[vec![3], vec![2]], Shift::In), OutlineError::TooDeep(_)));
}

/// Regression (BOOKMARK.1 review): the same bookmark object under two parents, only one of which
/// the group changes. The whole outline is checked, so this is refused before anything is written.
#[test]
fn a_bookmark_linked_from_two_parents_refuses_even_under_an_untouched_parent() {
    let items = [
        "<< /Title (A) /Parent 3 0 R /Next 6 0 R /First 8 0 R /Last 8 0 R /Count 1 >>", // 5
        "<< /Title (B) /Parent 3 0 R /Prev 5 0 R /Next 7 0 R >>",                       // 6
        "<< /Title (C) /Parent 3 0 R /Prev 6 0 R /First 8 0 R /Last 8 0 R /Count 1 >>", // 7
        "<< /Title (X) /Parent 5 0 R >>",                                               // 8
    ];
    let mut d = outline_with(&items);
    // Indenting B rewrites only the root's and A's children; C, which also lists X, is untouched.
    assert_eq!(refused(&mut d, &[vec![1]], Shift::In), OutlineError::Tangled);
}

#[test]
fn loops_and_broken_links_refuse_and_stop() {
    let cases: [&[&str]; 4] = [
        // A loop back to an earlier sibling.
        &["<< /Title (A) /Parent 3 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 3 0 R /Next 5 0 R >>"],
        // A child that is the outline root.
        &["<< /Title (A) /Parent 3 0 R /Next 6 0 R /First 3 0 R >>", "<< /Title (B) /Parent 3 0 R >>"],
        // A /Next to something that isn't a dictionary.
        &["<< /Title (A) /Parent 3 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 3 0 R /Next 7 0 R >>", "42"],
        // A /First that isn't a reference.
        &["<< /Title (A) /Parent 3 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 3 0 R /First << /Title (Inline) >> >>"],
    ];
    for items in cases {
        let mut d = outline_with(items);
        assert_eq!(refused(&mut d, &[vec![1]], Shift::In), OutlineError::Tangled, "{items:?}");
    }
    // A long chain that loops at its end: refused after one pass.
    let n = 10_000;
    let items: Vec<String> = (0..n).map(|k| format!("<< /Title (I{k}) /Parent 3 0 R /Next {} 0 R >>", 5 + (k + 1) % n)).collect();
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let mut d = outline_with(&refs);
    assert_eq!(shift_bookmarks(&mut d, &[vec![1]], Shift::In), Err(OutlineError::Tangled));
    assert!(d.modified_objects().is_empty());
}

#[test]
fn a_group_move_rewrites_only_links_and_keeps_everything_else() {
    let d = tree("A,B,C");
    let b = crate::bookmarks(&d)[1].obj;
    let mut d = d;
    d.update_dict(b, |x| x.set(b"C".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Int(0), Object::Int(0)]))).unwrap();
    d.update_dict(b, |x| x.set(b"PCKeep".to_vec(), Object::Int(7))).unwrap();
    let mut d = open(bytes(&d));
    let listed = crate::bookmarks(&d);
    let (a, b, c) = (listed[0].obj, listed[1].obj, listed[2].obj);
    shift_bookmarks(&mut d, &[vec![1]], Shift::In).unwrap();
    let item = d.get(b).as_dict().cloned().unwrap();
    assert!(item.get(b"C").is_some() && item.int(b"PCKeep") == Some(7), "colour and unknown keys kept");
    let root = d.get(d.root().unwrap()).as_dict().unwrap().reference(b"Outlines").unwrap();
    let touched = d.modified_objects();
    assert!(touched.iter().all(|n| [root.num, a.num, b.num, c.num].contains(n)), "only the relinked objects: {touched:?}");
    assert!([a.num, b.num, c.num].iter().all(|n| touched.contains(n)), "A gains B, B and C lose each other: {touched:?}");
    // A full save and reopen keeps the new nesting.
    assert_eq!(spec(&open(bytes(&d))), "A[B],C");
}

// ── The group equals moving its bookmarks one at a time ─────────────────────────────────────────

/// A small xorshift: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// The single-bookmark moves the Bookmarks panel makes (`ui-egui/src/editing.rs`), one pick at a
/// time; `false` when a step is impossible.
fn one_at_a_time(d: &mut Document, paths: &[Vec<usize>], shift: Shift) -> bool {
    let listing = crate::bookmarks(d);
    let mut picks: Vec<&Vec<usize>> = Vec::new();
    for p in paths {
        if !picks.contains(&p) {
            picks.push(p);
        }
    }
    let picked = picks.clone();
    picks.retain(|p| !picked.iter().any(|q| q.len() < p.len() && p.starts_with(q)));
    let mut objs: Vec<(usize, ObjRef)> =
        picks.iter().filter_map(|p| listing.iter().position(|b| &b.path == *p).map(|i| (i, listing[i].obj))).collect();
    objs.sort_unstable_by_key(|&(i, _)| i);
    if shift == Shift::Out {
        objs.reverse();
    }
    for (_, obj) in objs {
        let now = crate::bookmarks(d);
        let Some(at) = now.iter().position(|b| b.obj == obj) else { return false };
        let path = now[at].path.clone();
        let (&i, parent) = path.split_last().unwrap();
        match shift {
            Shift::In => {
                if i == 0 {
                    return false;
                }
                let height = now[at + 1..].iter().take_while(|b| b.path.starts_with(&path)).map(|b| b.path.len() - path.len()).max().unwrap_or(0);
                if path.len() + height > super::LIST_DEPTH {
                    return false;
                }
                let mut to = parent.to_vec();
                to.push(i - 1);
                crate::move_bookmark(d, &path, &to, usize::MAX).unwrap();
            }
            Shift::Out => {
                let Some((&j, grand)) = parent.split_last() else { return false };
                crate::move_bookmark(d, &path, grand, j + 1).unwrap();
            }
        }
    }
    true
}

/// Every outline object's links and count, by title (titles are unique here).
fn links(d: &Document) -> Vec<(String, Vec<Option<String>>, Option<i64>)> {
    let listed = crate::bookmarks(d);
    let root = d.get(d.root().unwrap()).as_dict().unwrap().reference(b"Outlines");
    let mut name: HashMap<ObjRef, String> = listed.iter().map(|b| (b.obj, b.title.clone())).collect();
    if let Some(r) = root {
        name.insert(r, "(root)".into());
    }
    let mut out = Vec::new();
    for r in root.into_iter().chain(listed.iter().map(|b| b.obj)) {
        let dict = d.get(r).as_dict().cloned().unwrap();
        let link = |k: &[u8]| dict.reference(k).map(|x| name.get(&x).cloned().unwrap_or_else(|| format!("{x:?}")));
        out.push((name[&r].clone(), [&b"Parent"[..], b"Prev", b"Next", b"First", b"Last"].iter().map(|k| link(k)).collect(), dict.int(b"Count")));
    }
    out.sort();
    out
}

#[test]
fn a_group_is_the_same_as_its_moves_one_at_a_time() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut moved = 0;
    for round in 0..1500 {
        let mut d = blank();
        let n = 1 + rng.below(24);
        for k in 0..n {
            let listed = crate::bookmarks(&d);
            let parent = if listed.is_empty() || rng.below(3) == 0 { Vec::new() } else { listed[rng.below(listed.len())].path.clone() };
            let siblings = listed.iter().filter(|b| b.path.len() == parent.len() + 1 && b.path.starts_with(&parent)).count();
            let index = rng.below(siblings + 1);
            crate::add_bookmark(&mut d, &parent, index, &format!("N{k}"), 0).unwrap();
        }
        for b in crate::bookmarks(&d) {
            if rng.below(4) == 0 {
                crate::set_bookmark_open(&mut d, &b.path, false).unwrap();
            }
        }
        let listed = crate::bookmarks(&d);
        let paths: Vec<Vec<usize>> = (0..1 + rng.below(4)).map(|_| listed[rng.below(listed.len())].path.clone()).collect();
        let shift = if rng.below(2) == 0 { Shift::In } else { Shift::Out };
        let (mut group, mut single) = (d.clone(), d.clone());
        let ok = one_at_a_time(&mut single, &paths, shift);
        match shift_bookmarks(&mut group, &paths, shift) {
            Ok(_) => {
                assert!(ok, "round {round}: {paths:?} {shift:?} moved as a group but not one at a time");
                assert_eq!(spec(&group), spec(&single), "round {round}: {paths:?} {shift:?}");
                assert_eq!(links(&group), links(&single), "round {round}: {paths:?} {shift:?}");
                moved += 1;
            }
            Err(e) => {
                assert!(!ok, "round {round}: {paths:?} {shift:?} refused ({e}) but moves one at a time");
                assert!(group.modified_objects() == d.modified_objects(), "round {round}: a refusal writes nothing");
            }
        }
    }
    assert!(moved > 300, "enough groups moved to mean something ({moved})");
}

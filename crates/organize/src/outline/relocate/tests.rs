//! BOOKMARK.DND.1 (this fork): moving several bookmarks to one place.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use super::super::shift::tests::{Rng, blank, bytes, links, open, outline_with, spec, tree};
use super::{move_bookmarks, plan_move, preview_move_bookmarks};
use crate::{OutlineError, OutlineShape};

/// A refusal changes nothing: no object is rewritten and the file is the same.
fn refused(d: &mut Document, paths: &[Vec<usize>], to: &[usize], index: usize) -> OutlineError {
    let before = bytes(d);
    let mut touched = d.modified_objects();
    touched.sort_unstable();
    assert!(preview_move_bookmarks(d, paths, to, index).is_err(), "the preview refuses too");
    let e = move_bookmarks(d, paths, to, index).expect_err("refused");
    let mut now = d.modified_objects();
    now.sort_unstable();
    assert_eq!(now, touched, "{e}: nothing more is rewritten");
    assert_eq!(bytes(d), before, "{e}: the file is unchanged");
    e
}

/// The outline's links agree with each other, read from the objects themselves: every child's
/// `/Parent`, `/Prev` and `/Next`, every parent's `/First` and `/Last`, and every `/Count`
/// (ISO 32000-2 §12.3.3). Each object is met once.
fn assert_consistent(d: &Document) {
    let Some(root) = d.get(d.root().unwrap()).as_dict().unwrap().reference(b"Outlines") else { return };
    let dict = |r: ObjRef| d.get(r).as_dict().cloned().expect("a bookmark is a dictionary");
    let mut seen = HashSet::from([root]);
    // (node, is the root); children are checked when their parent is.
    let mut todo = vec![(root, true)];
    // What each node shows below it when open, filled in children-first by a second pass.
    let mut kids_of: HashMap<ObjRef, Vec<ObjRef>> = HashMap::new();
    let mut order = Vec::new();
    while let Some((node, is_root)) = todo.pop() {
        order.push((node, is_root));
        let n = dict(node);
        let mut kids = Vec::new();
        let mut next = n.reference(b"First");
        let mut prev: Option<ObjRef> = None;
        while let Some(k) = next {
            assert!(seen.insert(k), "{k:?} is linked once");
            let kd = dict(k);
            assert_eq!(kd.reference(b"Parent"), Some(node), "{k:?} /Parent");
            assert_eq!(kd.reference(b"Prev"), prev, "{k:?} /Prev");
            kids.push(k);
            prev = Some(k);
            next = kd.reference(b"Next");
        }
        assert_eq!(n.reference(b"Last"), kids.last().copied(), "{node:?} /Last");
        todo.extend(kids.iter().map(|k| (*k, false)));
        kids_of.insert(node, kids);
    }
    let mut visible: HashMap<ObjRef, i64> = HashMap::new();
    for &(node, is_root) in order.iter().rev() {
        let kids = &kids_of[&node];
        let count = dict(node).int(b"Count");
        let shown: i64 = kids.iter().map(|k| 1 + if dict(*k).int(b"Count").is_some_and(|c| c > 0) { visible[k] } else { 0 }).sum();
        visible.insert(node, shown);
        if kids.is_empty() {
            assert_eq!(count, None, "{node:?}: no children, no /Count");
        } else if is_root || count.is_some_and(|c| c > 0) {
            assert_eq!(count, Some(shown), "{node:?}: an open /Count");
        } else {
            assert_eq!(count, Some(-shown), "{node:?}: a closed /Count");
        }
    }
}

/// Move in a fresh copy of `from` and give the outline afterwards and the paths returned.
fn moved(from: &str, paths: &[Vec<usize>], to: &[usize], index: usize) -> (String, Vec<Vec<usize>>) {
    let mut d = tree(from);
    let planned = preview_move_bookmarks(&d, paths, to, index).expect("previews");
    let new = move_bookmarks(&mut d, paths, to, index).expect("moves");
    assert_eq!(planned.paths, new, "the preview names the same paths");
    assert_consistent(&d);
    // The returned paths name the moved bookmarks.
    let listed = crate::bookmarks(&d);
    assert!(new.iter().all(|p| listed.iter().any(|b| &b.path == p)), "{new:?} are bookmarks");
    (spec(&d), new)
}

#[test]
fn one_bookmark_goes_before_after_and_under_another() {
    // Before C: index 2 of the top level as it stands.
    assert_eq!(moved("A,B,C,D", &[vec![0]], &[], 2), ("B,A,C,D".into(), vec![vec![1]]));
    // After C: index 3.
    assert_eq!(moved("A,B,C,D", &[vec![0]], &[], 3), ("B,C,A,D".into(), vec![vec![2]]));
    // Up the list.
    assert_eq!(moved("A,B,C,D", &[vec![3]], &[], 1), ("A,D,B,C".into(), vec![vec![1]]));
    // Under B, as its last child.
    assert_eq!(moved("A,B[b1],C", &[vec![0]], &[1], usize::MAX), ("B[b1,A],C".into(), vec![vec![0, 1]]));
    // Under a bookmark without children.
    assert_eq!(moved("A,B,C", &[vec![2]], &[0], usize::MAX), ("A[C],B".into(), vec![vec![0, 0]]));
    // Out to the top level, past the end.
    assert_eq!(moved("A[a1,a2],B", &[vec![0, 0]], &[], 99), ("A[a2],B,a1".into(), vec![vec![2]]));
    // A whole subtree comes along.
    assert_eq!(moved("A[a1[x,y]],B", &[vec![0, 0]], &[1], 0), ("A,B[a1[x,y]]".into(), vec![vec![1, 0]]));
}

#[test]
fn a_group_arrives_as_one_run_in_outline_order() {
    // Picked out of order, from one list, to before D.
    assert_eq!(moved("A,B,C,D,E", &[vec![2], vec![0]], &[], 3), ("B,A,C,D,E".into(), vec![vec![1], vec![2]]));
    // From different parents, under Z.
    assert_eq!(moved("A[a1,a2],B[b1],Z", &[vec![1, 0], vec![0, 1]], &[2], 0), ("A[a1],B,Z[a2,b1]".into(), vec![vec![2, 0], vec![2, 1]]));
    // Some of the group is already in the target list, on both sides of the place it goes to.
    assert_eq!(moved("A,B,C,D,E", &[vec![0], vec![4]], &[], 2), ("B,A,E,C,D".into(), vec![vec![1], vec![2]]));
    // A bookmark and something under it: the child moves with its parent, once.
    assert_eq!(moved("A[a1,a2],B,C", &[vec![0, 1], vec![0]], &[], 3), ("B,C,A[a1,a2]".into(), vec![vec![2]]));
    // The same path twice is one bookmark.
    assert_eq!(moved("A,B,C", &[vec![0], vec![0]], &[], 3), ("B,C,A".into(), vec![vec![2]]));
    // A parent and a bookmark from elsewhere, into a later sibling of the parent.
    assert_eq!(moved("P[p1],Q[q1],R", &[vec![0], vec![1, 0]], &[2], 0), ("Q,R[P[p1],q1]".into(), vec![vec![1, 0], vec![1, 1]]));
}

#[test]
fn a_move_to_where_the_bookmarks_already_are_writes_nothing() {
    for (from, paths, to, index) in [
        ("A,B,C", vec![vec![1]], vec![], 1),          // before itself
        ("A,B,C", vec![vec![1]], vec![], 2),          // after itself
        ("A,B,C", vec![vec![0], vec![1]], vec![], 1), // a run, into its own middle
        ("A,B,C", vec![vec![0], vec![1]], vec![], 2), // a run, after itself
        ("A[a1,a2],B", vec![vec![0, 1]], vec![0], 9), // already the last child
        ("A[a1],B", vec![vec![0], vec![0, 0]], vec![], 0),
    ] {
        let mut d = open(bytes(&tree(from)));
        let before = bytes(&d);
        let plan = preview_move_bookmarks(&d, &paths, &to, index).unwrap();
        assert!(plan.unchanged && plan.lists.is_empty() && plan.opened.is_empty(), "{from} {paths:?} -> {to:?} {index}");
        let roots: Vec<Vec<usize>> = plan.paths.clone();
        assert_eq!(move_bookmarks(&mut d, &paths, &to, index).unwrap(), roots);
        assert!(d.modified_objects().is_empty(), "{from} {paths:?}: nothing is rewritten");
        assert_eq!(bytes(&d), before);
        assert_eq!(spec(&d), from);
    }
}

#[test]
fn a_bookmark_cannot_go_into_itself_or_under_anything_moving_with_it() {
    let mut d = tree("A[a1[x]],B,C");
    assert_eq!(refused(&mut d, &[vec![0]], &[0], 0), OutlineError::IntoItself);
    assert_eq!(refused(&mut d, &[vec![0]], &[0, 0, 0], 0), OutlineError::IntoItself);
    // The target is under another bookmark of the group, not under the first one named.
    assert_eq!(refused(&mut d, &[vec![1], vec![0, 0]], &[0, 0, 0], 0), OutlineError::IntoItself);
    // The target is a bookmark of the group.
    assert_eq!(refused(&mut d, &[vec![1], vec![2]], &[2], 0), OutlineError::IntoItself);
    assert_eq!(spec(&d), "A[a1[x]],B,C");
    // Its own parent is fine: that is a reorder.
    assert_eq!(moved("A[a1,a2],B", &[vec![0, 1]], &[0], 0).0, "A[a2,a1],B");
}

#[test]
fn bad_paths_and_targets_refuse_the_whole_group() {
    let mut d = tree("A,B[b1],C");
    assert_eq!(refused(&mut d, &[vec![0], vec![7]], &[], 0), OutlineError::NoSuchBookmark(vec![7]));
    assert_eq!(refused(&mut d, &[vec![]], &[], 0), OutlineError::NoSuchBookmark(vec![]));
    assert_eq!(refused(&mut d, &[], &[], 0), OutlineError::NoSuchBookmark(vec![]));
    assert_eq!(refused(&mut d, &[vec![0]], &[5], 0), OutlineError::NoSuchBookmark(vec![5]));
    assert_eq!(refused(&mut d, &[vec![0]], &[1, 0, 3], 0), OutlineError::NoSuchBookmark(vec![1, 0, 3]));
    assert_eq!(refused(&mut d, &[vec![0; 40]], &[], 0), OutlineError::NoSuchBookmark(vec![0; 40]));
    assert_eq!(refused(&mut d, &[vec![0]], &[0; 40], 0), OutlineError::NoSuchBookmark(vec![0; 40]));
    assert_eq!(refused(&mut d, &vec![vec![1]; super::MAX_SHIFT + 1], &[], 0), OutlineError::TooMany);
    assert_eq!(spec(&d), "A,B[b1],C");
    // A document without an outline has no bookmark to move.
    let mut d = blank();
    assert_eq!(move_bookmarks(&mut d, &[vec![0]], &[], 0), Err(OutlineError::NoSuchBookmark(vec![0])));
    assert!(d.modified_objects().is_empty());
}

/// `Q` with a chain `height` levels under it, then `P` with a chain `depth` levels deep: the
/// path of the last bookmark of `P`'s chain is `[1, 0, 0, …]`.
fn two_chains(height: usize, depth: usize) -> (Document, Vec<usize>) {
    let mut d = blank();
    crate::add_bookmark(&mut d, &[], 0, "Q", 0).unwrap();
    let mut parent = vec![0];
    for k in 0..height {
        crate::add_bookmark(&mut d, &parent, 0, &format!("H{k}"), 0).unwrap();
        parent.push(0);
    }
    crate::add_bookmark(&mut d, &[], 1, "P", 0).unwrap();
    let mut end = vec![1];
    for k in 0..depth {
        crate::add_bookmark(&mut d, &end, 0, &format!("D{k}"), 0).unwrap();
        end.push(0);
    }
    (d, end)
}

/// Depth counts from 0 at the top level; depth 32 is the deepest listed.
#[test]
fn a_group_goes_down_to_the_deepest_listed_level_and_no_further() {
    // Under a bookmark at depth 30, Q (one level below it) reaches depth 32.
    let (mut d, end) = two_chains(1, 30);
    move_bookmarks(&mut d, &[vec![0]], &end, 0).unwrap();
    assert_eq!(crate::bookmarks(&d).iter().map(|b| b.path.len()).max(), Some(33), "depth 32 is still listed");
    assert_consistent(&d);
    // One level more under Q would be depth 33.
    let (mut d, end) = two_chains(2, 30);
    assert_eq!(refused(&mut d, &[vec![0]], &end, 0), OutlineError::GroupTooDeep(vec![0]));
    // Refused whole beside a bookmark that could go.
    let (mut d, end) = two_chains(2, 30);
    crate::add_bookmark(&mut d, &[], 2, "Z", 0).unwrap();
    assert_eq!(refused(&mut d, &[vec![2], vec![0]], &end, 0), OutlineError::GroupTooDeep(vec![0]));
}

/// Only going deeper is refused: a bookmark at the deepest listed level can be reordered there,
/// and one whose subtree already runs past the limit can be brought up.
#[test]
fn a_move_that_goes_no_deeper_is_not_stopped_by_the_depth_limit() {
    let (mut d, end) = two_chains(0, 32);
    let mut parent = end.clone();
    parent.pop();
    crate::add_bookmark(&mut d, &parent, 1, "Sib", 0).unwrap();
    // Reorder at depth 32.
    let new = move_bookmarks(&mut d, std::slice::from_ref(&end), &parent, 2).unwrap();
    assert_eq!(new.first().and_then(|p| p.last()), Some(&1));
    assert_consistent(&d);
    // P's chain runs to depth 36; its bookmark at depth 5 comes up to the top level.
    let (mut d, _) = two_chains(0, 36);
    assert_eq!(move_bookmarks(&mut d, &[vec![1, 0, 0, 0, 0, 0]], &[], 0).unwrap(), [vec![0]]);
    assert_consistent(&d);
    // And it can't go back down under Q's side at a deeper level than it had.
    let (mut d, _) = two_chains(0, 36);
    assert_eq!(refused(&mut d, &[vec![1, 0, 0]], &[1, 0, 0, 1], 0), OutlineError::NoSuchBookmark(vec![1, 0, 0, 1]));
}

#[test]
fn a_tangled_outline_refuses_before_anything_is_written() {
    // X is linked from both A and C; the move touches neither.
    let items = [
        "<< /Title (A) /Parent 3 0 R /Next 6 0 R /First 8 0 R /Last 8 0 R /Count 1 >>", // 5
        "<< /Title (B) /Parent 3 0 R /Prev 5 0 R /Next 7 0 R >>",                       // 6
        "<< /Title (C) /Parent 3 0 R /Prev 6 0 R /First 8 0 R /Last 8 0 R /Count 1 >>", // 7
        "<< /Title (X) /Parent 5 0 R >>",                                               // 8
    ];
    let mut d = outline_with(&items);
    assert_eq!(refused(&mut d, &[vec![1]], &[], 0), OutlineError::Tangled);
    // A loop back to an earlier sibling.
    let mut d = outline_with(&["<< /Title (A) /Parent 3 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 3 0 R /Next 5 0 R >>"]);
    assert_eq!(refused(&mut d, &[vec![1]], &[], 0), OutlineError::Tangled);
    // A long chain that loops at its end: refused after one pass.
    let n = 10_000;
    let items: Vec<String> = (0..n).map(|k| format!("<< /Title (I{k}) /Parent 3 0 R /Next {} 0 R >>", 5 + (k + 1) % n)).collect();
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let mut d = outline_with(&refs);
    assert_eq!(move_bookmarks(&mut d, &[vec![1]], &[], 0), Err(OutlineError::Tangled));
    assert!(d.modified_objects().is_empty());
}

/// Each bookmark keeps its object and everything on it; only links and counts are rewritten.
#[test]
fn a_move_keeps_each_bookmarks_object_and_everything_on_it() {
    let mut d = tree("A,B[b1],C,D");
    let listed = crate::bookmarks(&d);
    let (b, b1) = (listed[1].obj, listed[2].obj);
    let action = d.add(Object::Dict({
        let mut a = pdfcraft_cos::Dict::new();
        a.set(b"S".to_vec(), Object::name("URI"));
        a.set(b"URI".to_vec(), Object::String(pdfcraft_cos::PdfString::text("https://example.org/")));
        a
    }));
    let element = d.add(Object::Dict(pdfcraft_cos::Dict::new()));
    d.update_dict(b, |x| {
        x.set(b"C".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Int(0), Object::Int(0)]));
        x.set(b"F".to_vec(), Object::Int(3));
        x.set(b"A".to_vec(), Object::Ref(action));
        x.set(b"SE".to_vec(), Object::Ref(element));
        x.set(b"PCKeep".to_vec(), Object::Int(7));
        x.remove(b"Dest");
    })
    .unwrap();
    d.update_dict(b1, |x| x.set(b"PCChild".to_vec(), Object::name("Kept"))).unwrap();
    let mut d = open(bytes(&d));
    let listed = crate::bookmarks(&d);
    let by_title = |l: &[crate::Bookmark], t: &str| l.iter().find(|x| x.title == t).map(|x| x.obj).unwrap();
    let (a, b, b1, c, dd) = (by_title(&listed, "A"), by_title(&listed, "B"), by_title(&listed, "b1"), by_title(&listed, "C"), by_title(&listed, "D"));
    let link_keys: [&[u8]; 6] = [b"Parent", b"Prev", b"Next", b"First", b"Last", b"Count"];
    let rest = |d: &Document, r: ObjRef| {
        let mut x = d.get(r).as_dict().cloned().unwrap();
        for k in link_keys {
            x.remove(k);
        }
        x
    };
    let before: Vec<_> = [a, b, b1, c, dd].iter().map(|r| rest(&d, *r)).collect();
    // B, with b1 under it, goes under D.
    assert_eq!(move_bookmarks(&mut d, &[vec![1]], &[3], 0).unwrap(), [vec![2, 0]]);
    assert_eq!(spec(&d), "A,C,D[B[b1]]");
    assert_consistent(&d);
    // The same objects, in their new places.
    let now = crate::bookmarks(&d);
    for (title, obj) in [("A", a), ("B", b), ("b1", b1), ("C", c), ("D", dd)] {
        assert_eq!(by_title(&now, title), obj, "{title} is the same object");
    }
    let after: Vec<_> = [a, b, b1, c, dd].iter().map(|r| rest(&d, *r)).collect();
    assert_eq!(after, before, "nothing but links and counts changed on any bookmark");
    let moved = d.get(b).as_dict().cloned().unwrap();
    assert_eq!(moved.reference(b"A"), Some(action));
    assert_eq!(moved.reference(b"SE"), Some(element));
    assert_eq!(moved.int(b"F"), Some(3));
    assert_eq!(moved.int(b"PCKeep"), Some(7));
    // b1 moved with its parent and was not touched at all.
    let root = d.get(d.root().unwrap()).as_dict().unwrap().reference(b"Outlines").unwrap();
    let touched = d.modified_objects();
    assert!(!touched.contains(&b1.num), "a child that moves with its parent isn't rewritten: {touched:?}");
    assert!(touched.iter().all(|n| [root.num, a.num, b.num, c.num, dd.num].contains(n)), "only relinked objects: {touched:?}");
    // A full save and reopen keeps the new nesting and the kept keys.
    let again = open(bytes(&d));
    assert_eq!(spec(&again), "A,C,D[B[b1]]");
    assert_consistent(&again);
    let b_again = by_title(&crate::bookmarks(&again), "B");
    assert_eq!(again.get(b_again).as_dict().unwrap().int(b"PCKeep"), Some(7));
}

/// The new parent opens when a bookmark comes to it, as a single move does; a closed parent the
/// bookmarks are only reordered in stays closed, and the counts follow.
#[test]
fn the_new_parent_opens_and_a_reorder_leaves_a_closed_parent_closed() {
    let mut d = tree("A[a1,a2],B,C");
    crate::set_bookmark_open(&mut d, &[0], false).unwrap();
    move_bookmarks(&mut d, &[vec![0, 1]], &[0], 0).unwrap();
    assert_eq!(spec(&d), "A[a2,a1],B,C");
    assert!(!crate::bookmarks(&d)[0].open, "a reorder doesn't open the parent");
    assert_consistent(&d);
    move_bookmarks(&mut d, &[vec![1], vec![2]], &[0], 1).unwrap();
    assert_eq!(spec(&d), "A[a2,B,C,a1]");
    assert!(crate::bookmarks(&d)[0].open, "bookmarks that arrive open their new parent");
    assert_consistent(&d);
    // The parent they left has no children and so no /Count.
    let mut d = tree("A[a1],B");
    move_bookmarks(&mut d, &[vec![0, 0]], &[], 2).unwrap();
    assert_eq!(spec(&d), "A,B,a1");
    assert_eq!(d.get(crate::bookmarks(&d)[0].obj).as_dict().unwrap().int(b"Count"), None);
    assert_consistent(&d);
}

/// The plan alone, on a shape: what the panel's preview and a dry run read.
#[test]
fn the_plan_is_pure_and_names_the_lists_that_change() {
    // root: A(1)[a1(2)], B(3)
    let mut shape = OutlineShape::new();
    let a = shape.add(0).unwrap();
    let a1 = shape.add(a).unwrap();
    let b = shape.add(0).unwrap();
    let plan = plan_move(&shape, &[vec![1]], &[0], 0).unwrap();
    assert_eq!(plan.lists, [(0, vec![a]), (a, vec![b, a1])]);
    assert_eq!(plan.opened, [a]);
    assert_eq!(plan.paths, [vec![0, 0]]);
    assert!(!plan.unchanged);
    // The shape is as it was: the same plan again.
    assert_eq!(plan_move(&shape, &[vec![1]], &[0], 0).unwrap(), plan);
    assert_eq!(plan_move(&shape, &[vec![0]], &[0, 0], 0), Err(OutlineError::IntoItself));
}

// ── The group equals moving its bookmarks one at a time ─────────────────────────────────────────

/// The same move made with the single-bookmark edit, one bookmark of the group at a time, in
/// outline order; `false` when the target is a moved bookmark or under one.
fn one_at_a_time(d: &mut Document, paths: &[Vec<usize>], to: &[usize], index: usize) -> bool {
    let listing = crate::bookmarks(d);
    let mut picks: Vec<&Vec<usize>> = Vec::new();
    for p in paths {
        if !picks.contains(&p) {
            picks.push(p);
        }
    }
    let picked = picks.clone();
    picks.retain(|p| !picked.iter().any(|q| q.len() < p.len() && p.starts_with(q)));
    if picks.iter().any(|p| to.starts_with(p)) {
        return false;
    }
    let mut objs: Vec<(usize, ObjRef)> =
        picks.iter().filter_map(|p| listing.iter().position(|b| &b.path == *p).map(|i| (i, listing[i].obj))).collect();
    objs.sort_unstable_by_key(|&(i, _)| i);
    let moving: Vec<ObjRef> = objs.iter().map(|&(_, o)| o).collect();
    let child_objs = |d: &Document, parent: &[usize]| -> Vec<ObjRef> {
        crate::bookmarks(d).iter().filter(|b| b.path.len() == parent.len() + 1 && b.path.starts_with(parent)).map(|b| b.obj).collect()
    };
    // The target, by object, and the bookmark the group goes before: the first one at or after
    // `index` that stays.
    let target = listing.iter().find(|b| b.path == to).map(|b| b.obj);
    let anchor = child_objs(d, to).into_iter().skip(index).find(|k| !moving.contains(k));
    for obj in moving {
        let now = crate::bookmarks(d);
        let path = now.iter().find(|b| b.obj == obj).map(|b| b.path.clone()).unwrap();
        let to_now: Vec<usize> = match target {
            Some(t) => now.iter().find(|b| b.obj == t).map(|b| b.path.clone()).unwrap(),
            None => Vec::new(),
        };
        // The single move counts the position after taking the bookmark out.
        let rest: Vec<ObjRef> = child_objs(d, &to_now).into_iter().filter(|k| *k != obj).collect();
        let at = anchor.and_then(|a| rest.iter().position(|k| *k == a)).unwrap_or(rest.len());
        crate::move_bookmark(d, &path, &to_now, at).unwrap();
    }
    true
}

#[test]
fn a_group_is_the_same_as_its_moves_one_at_a_time() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut moved, mut still) = (0, 0);
    for round in 0..1500 {
        let mut d = blank();
        let n = 1 + rng.below(24);
        for k in 0..n {
            let listed = crate::bookmarks(&d);
            let parent = if listed.is_empty() || rng.below(3) == 0 { Vec::new() } else { listed[rng.below(listed.len())].path.clone() };
            let siblings = listed.iter().filter(|b| b.path.len() == parent.len() + 1 && b.path.starts_with(&parent)).count();
            crate::add_bookmark(&mut d, &parent, rng.below(siblings + 1), &format!("N{k}"), 0).unwrap();
        }
        for b in crate::bookmarks(&d) {
            if rng.below(4) == 0 {
                crate::set_bookmark_open(&mut d, &b.path, false).unwrap();
            }
        }
        let listed = crate::bookmarks(&d);
        let paths: Vec<Vec<usize>> = (0..1 + rng.below(4)).map(|_| listed[rng.below(listed.len())].path.clone()).collect();
        let to = if rng.below(4) == 0 { Vec::new() } else { listed[rng.below(listed.len())].path.clone() };
        let siblings = listed.iter().filter(|b| b.path.len() == to.len() + 1 && b.path.starts_with(&to)).count();
        let index = rng.below(siblings + 2);
        let (mut group, mut single) = (d.clone(), d.clone());
        let ok = one_at_a_time(&mut single, &paths, &to, index);
        match move_bookmarks(&mut group, &paths, &to, index) {
            Ok(new) => {
                assert!(ok, "round {round}: {paths:?} -> {to:?} {index} moved as a group but not one at a time");
                assert_eq!(spec(&group), spec(&single), "round {round}: {paths:?} -> {to:?} {index}");
                assert_eq!(links(&group), links(&single), "round {round}: {paths:?} -> {to:?} {index}");
                assert_consistent(&group);
                // The new paths are the group's bookmarks, in outline order.
                let mut sorted = new.clone();
                sorted.sort();
                assert_eq!(new, sorted, "round {round}: outline order");
                if spec(&group) == spec(&d) {
                    assert!(group.modified_objects() == d.modified_objects(), "round {round}: a move to the same place writes nothing");
                    still += 1;
                } else {
                    moved += 1;
                }
            }
            Err(e) => {
                assert!(!ok, "round {round}: {paths:?} -> {to:?} {index} refused ({e}) but moves one at a time");
                assert_eq!(e, OutlineError::IntoItself, "round {round}");
                assert!(group.modified_objects() == d.modified_objects(), "round {round}: a refusal writes nothing");
            }
        }
    }
    assert!(moved > 600 && still > 20, "enough groups moved ({moved}) and stayed ({still}) to mean something");
}

//! This fork (sefr-14/pdfcraft, BOOKMARK.1): indent or outdent several bookmarks at once.
//!
//! A group moves as if each of its bookmarks were moved one at a time with the single-bookmark
//! rule the Bookmarks panel already uses:
//!
//! - **Indent** makes a bookmark the last child of the bookmark above it at its level.
//! - **Outdent** moves it out a level, to follow its parent; the bookmarks after it at its old level
//!   stay where they are.
//!
//! Bookmarks under another picked bookmark move with it and are not moved again. Indent goes in
//! outline order and Outdent in reverse, which keeps a run of picked siblings in its order: under
//! the same new parent for Indent, straight after the old parent for Outdent.
//!
//! The group is all or nothing. Everything is checked before the first write:
//!
//! - the whole outline is a tree (no bookmark linked twice, no loop, no link to something that
//!   isn't a bookmark): see [`read_tree`];
//! - every path names a bookmark;
//! - every step of the replay is a valid move;
//! - nothing ends deeper than bookmarks are listed (depth 32, the top level being 0, as the
//!   listing and the viewer count).
//!
//! A refusal leaves the document untouched.

use std::collections::{BTreeSet, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use super::{LIST_DEPTH, OutlineError, Result, open_item, outline_root, recount, relink};

/// The most bookmarks one group move takes.
pub const MAX_SHIFT: usize = 10_000;

/// Which way a group moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    /// Each bookmark becomes the last child of the bookmark above it at its level.
    In,
    /// Each bookmark moves out a level, to straight after its parent.
    Out,
}

/// The shape of an outline: who is whose child, in order, and nothing else. Node 0 is the outline
/// root; [`OutlineShape::add`] appends the others. A shape built this way is always a tree.
#[derive(Clone, Debug)]
pub struct OutlineShape {
    kids: Vec<Vec<usize>>,
}

impl Default for OutlineShape {
    fn default() -> Self {
        Self::new()
    }
}

impl OutlineShape {
    /// Just the root (node 0), with no bookmarks.
    pub fn new() -> Self {
        Self { kids: vec![Vec::new()] }
    }

    /// Add a bookmark as the last child of node `parent`, returning its node, or `None` when
    /// there is no node `parent`.
    pub fn add(&mut self, parent: usize) -> Option<usize> {
        let id = self.kids.len();
        self.kids.get_mut(parent)?.push(id);
        self.kids.push(Vec::new());
        Some(id)
    }

    fn len(&self) -> usize {
        self.kids.len()
    }

    /// The node at `path`, walking child indices from the root.
    fn node(&self, path: &[usize]) -> Option<usize> {
        path.iter().try_fold(0usize, |node, &i| self.kids.get(node)?.get(i).copied())
    }
}

/// What a group move does: the new child lists of every node whose children change, the nodes
/// that must open so the moved bookmarks show, and the moved bookmarks' new paths, in outline
/// order.
#[derive(Clone, Debug, PartialEq)]
pub struct ShiftPlan {
    pub lists: Vec<(usize, Vec<usize>)>,
    pub opened: Vec<usize>,
    pub paths: Vec<Vec<usize>>,
}

/// Each node's parent, depth (the root's children are at 0), place in outline order and height
/// (how many levels lie below it), for the nodes reachable from the root.
struct Layout {
    parent: Vec<usize>,
    depth: Vec<usize>,
    order: Vec<usize>,
    height: Vec<usize>,
}

fn layout(shape: &OutlineShape) -> Layout {
    let n = shape.len();
    let (mut parent, mut depth, mut order, mut height): (Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>) =
        (vec![0; n], vec![0; n], vec![usize::MAX; n], vec![0; n]);
    let mut visited = Vec::with_capacity(n);
    // Root's children first, each subtree before the next sibling: kids go on the stack reversed.
    let mut stack: Vec<usize> = shape.kids.first().map(|k| k.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(node) = stack.pop() {
        // A shape built with `add` is a tree, so a node is reached once; the check keeps any
        // other shape from looping.
        if order.get(node).is_none_or(|&o| o != usize::MAX) {
            continue;
        }
        if let Some(o) = order.get_mut(node) {
            *o = visited.len();
        }
        visited.push(node);
        let d = depth.get(node).copied().unwrap_or(0);
        for &kid in shape.kids.get(node).into_iter().flatten().rev() {
            if let (Some(p), Some(kd)) = (parent.get_mut(kid), depth.get_mut(kid)) {
                *p = node;
                *kd = d.saturating_add(1);
            }
            stack.push(kid);
        }
    }
    // Children come after their parent in outline order, so the reverse sees each child first.
    for &node in visited.iter().rev() {
        let h = height.get(node).copied().unwrap_or(0);
        let p = parent.get(node).copied().unwrap_or(0);
        if p != 0
            && let Some(ph) = height.get_mut(p)
        {
            *ph = (*ph).max(h.saturating_add(1));
        }
    }
    Layout { parent, depth, order, height }
}

/// Plan moving the bookmarks at `paths` together (see the module docs). Pure: `shape` is not
/// changed. Refused, with the first offending path, when any of them can't move.
pub fn plan_shift(shape: &OutlineShape, paths: &[Vec<usize>], shift: Shift) -> Result<ShiftPlan> {
    if paths.len() > MAX_SHIFT {
        return Err(OutlineError::TooMany);
    }
    let lay = layout(shape);
    // The picked nodes, each with the path it was asked for.
    let mut picks: Vec<(usize, &Vec<usize>)> = Vec::with_capacity(paths.len());
    let mut picked = HashSet::new();
    for path in paths {
        // A path deeper than bookmarks are listed names nothing the panel shows.
        if path.is_empty() || path.len() > LIST_DEPTH.saturating_add(1) {
            return Err(OutlineError::NoSuchBookmark(path.clone()));
        }
        let node = shape.node(path).ok_or_else(|| OutlineError::NoSuchBookmark(path.clone()))?;
        if picked.insert(node) {
            picks.push((node, path));
        }
    }
    // A bookmark under another picked one moves with it: drop it.
    picks.retain(|&(node, _)| {
        let mut up = lay.parent.get(node).copied().unwrap_or(0);
        // Each step goes a level up, so the walk is no longer than the depth.
        for _ in 0..=lay.depth.get(node).copied().unwrap_or(0) {
            if up == 0 {
                return true;
            }
            if picked.contains(&up) {
                return false;
            }
            up = lay.parent.get(up).copied().unwrap_or(0);
        }
        true
    });
    picks.sort_by_key(|&(node, _)| lay.order.get(node).copied().unwrap_or(usize::MAX));
    if shift == Shift::Out {
        picks.reverse();
    }
    // Replay the single-bookmark rule on copies. A pick's parent, depth and subtree stay as they
    // were: only picks move, and no pick lies under another.
    let mut kids = shape.kids.clone();
    let mut parent = lay.parent.clone();
    let (mut changed, mut opened) = (BTreeSet::new(), BTreeSet::new());
    for &(node, path) in &picks {
        let p = parent.get(node).copied().unwrap_or(0);
        let siblings = kids.get(p).ok_or_else(|| OutlineError::NoSuchBookmark(path.clone()))?;
        let at = siblings.iter().position(|&k| k == node).ok_or_else(|| OutlineError::NoSuchBookmark(path.clone()))?;
        let to = match shift {
            Shift::In => {
                let above = at.checked_sub(1).and_then(|i| siblings.get(i).copied()).ok_or_else(|| OutlineError::CannotIndent(path.clone()))?;
                let deepest = lay.depth.get(node).copied().unwrap_or(0).saturating_add(1).saturating_add(lay.height.get(node).copied().unwrap_or(0));
                if deepest > LIST_DEPTH {
                    return Err(OutlineError::GroupTooDeep(path.clone()));
                }
                if let Some(list) = kids.get_mut(p) {
                    list.remove(at);
                }
                kids.get_mut(above).ok_or_else(|| OutlineError::NoSuchBookmark(path.clone()))?.push(node);
                opened.insert(above);
                above
            }
            Shift::Out => {
                if p == 0 {
                    return Err(OutlineError::CannotOutdent(path.clone()));
                }
                let g = parent.get(p).copied().unwrap_or(0);
                if let Some(list) = kids.get_mut(p) {
                    list.remove(at);
                }
                let list = kids.get_mut(g).ok_or_else(|| OutlineError::NoSuchBookmark(path.clone()))?;
                let after = list.iter().position(|&k| k == p).ok_or_else(|| OutlineError::NoSuchBookmark(path.clone()))?;
                list.insert(after.saturating_add(1), node);
                if g != 0 {
                    opened.insert(g);
                }
                g
            }
        };
        if let Some(slot) = parent.get_mut(node) {
            *slot = to;
        }
        changed.insert(p);
        changed.insert(to);
    }
    // The moved bookmarks' new paths, from one walk of the new shape in outline order.
    let moving: HashSet<usize> = picks.iter().map(|&(node, _)| node).collect();
    let mut new_paths = Vec::with_capacity(moving.len());
    let mut stack: Vec<(usize, Vec<usize>)> =
        kids.first().map(|k| k.iter().enumerate().rev().map(|(i, &n)| (n, vec![i])).collect()).unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some((node, path)) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        if let Some(list) = kids.get(node) {
            for (i, &kid) in list.iter().enumerate().rev() {
                let mut p = path.clone();
                p.push(i);
                stack.push((kid, p));
            }
        }
        if moving.contains(&node) {
            new_paths.push(path);
        }
    }
    let lists = changed.into_iter().map(|n| (n, kids.get(n).cloned().unwrap_or_default())).collect();
    Ok(ShiftPlan { lists, opened: opened.into_iter().collect(), paths: new_paths })
}

/// The outline as a strict tree, with each node's object: the shape [`plan_shift`] works on.
///
/// This walks the whole outline once, from the root, siblings by `/First` and `/Next` as the
/// listing does, with one record of every object it has met. Where the listing quietly ends a run
/// of siblings, this refuses ([`OutlineError::Tangled`]): a bookmark met a second time (linked
/// from two parents, twice in one list, or in a loop back to an ancestor or the root), and a
/// `/First` or `/Next` that isn't a reference to a dictionary. A group move rewrites links, and on
/// such an outline that would either drop data the file has or leave it inconsistent. `null` is
/// the same as no link. Each object is met once, so the walk is no longer than the outline.
fn read_tree(doc: &Document, root: ObjRef) -> Result<(OutlineShape, Vec<ObjRef>)> {
    let mut shape = OutlineShape::new();
    let mut refs = vec![root];
    let mut seen = HashSet::from([root]);
    let mut todo = vec![(0usize, root)];
    // A link: none, a reference, or something else (refused).
    let link = |d: &pdfcraft_cos::Dict, key: &[u8]| -> Result<Option<ObjRef>> {
        match d.get(key) {
            None | Some(Object::Null) => Ok(None),
            Some(Object::Ref(r)) => Ok(Some(*r)),
            Some(_) => Err(OutlineError::Tangled),
        }
    };
    while let Some((node, r)) = todo.pop() {
        let obj = doc.get(r);
        let Some(d) = obj.as_dict() else { return Err(OutlineError::Tangled) };
        let mut next = link(d, b"First")?;
        while let Some(kid) = next {
            if !seen.insert(kid) {
                return Err(OutlineError::Tangled);
            }
            let item = doc.get(kid);
            let Some(kd) = item.as_dict() else { return Err(OutlineError::Tangled) };
            let id = shape.add(node).ok_or(OutlineError::Tangled)?;
            refs.push(kid);
            todo.push((id, kid));
            next = link(kd, b"Next")?;
        }
    }
    Ok((shape, refs))
}

/// Indent or outdent the bookmarks at `paths` together, as one change (see the module docs).
/// Returns their new paths, in outline order. Every check comes before the first write, so a
/// refusal changes nothing.
pub fn shift_bookmarks(doc: &mut Document, paths: &[Vec<usize>], shift: Shift) -> Result<Vec<Vec<usize>>> {
    if paths.len() > MAX_SHIFT {
        return Err(OutlineError::TooMany);
    }
    let Some(root) = outline_root(doc) else {
        return Err(OutlineError::NoSuchBookmark(paths.first().cloned().unwrap_or_default()));
    };
    let (shape, refs) = read_tree(doc, root)?;
    let plan = plan_shift(&shape, paths, shift)?;
    let obj = |n: usize| refs.get(n).copied().ok_or(OutlineError::Tangled);
    // Resolve every object before the first write.
    let lists =
        plan.lists.iter().map(|(n, kids)| Ok((obj(*n)?, kids.iter().map(|&k| obj(k)).collect::<Result<Vec<_>>>()?))).collect::<Result<Vec<_>>>()?;
    let opened = plan.opened.iter().map(|&n| obj(n)).collect::<Result<Vec<_>>>()?;
    for (parent, kids) in &lists {
        relink(doc, *parent, kids)?;
    }
    for item in opened {
        open_item(doc, item)?;
    }
    recount(doc)?;
    Ok(plan.paths)
}

#[cfg(test)]
mod tests;

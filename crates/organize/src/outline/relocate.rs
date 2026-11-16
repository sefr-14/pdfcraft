//! This fork (sefr-14/pdfcraft, BOOKMARK.DND.1): move several bookmarks to one place at once.
//!
//! A group goes to a **target**: a parent (`[]` is the top level) and a position in that parent's
//! list of children **as it is before the move** — "before the child now at `index`"; an index
//! at or past the end appends. Naming the position in the list as it stands keeps it meaningful
//! when some of the moved bookmarks are themselves in that list.
//!
//! - Bookmarks under another picked bookmark move with it and are not moved again.
//! - The group arrives as one run, in outline order, whatever order it was picked in.
//! - Each bookmark keeps its object, and so its title, destination, action, colour, style,
//!   structure element and any key this code doesn't know: only the links between bookmarks and
//!   the `/Count` values are rewritten.
//!
//! The group is all or nothing. Everything is checked before the first write:
//!
//! - the whole outline is a tree (see `read_tree` in the group Indent/Outdent module);
//! - every path names a bookmark, and so does the target parent;
//! - the target isn't one of the moved bookmarks or under one;
//! - a bookmark that goes deeper doesn't end deeper than bookmarks are listed (depth 32, the top
//!   level being 0).
//!
//! A refusal leaves the document untouched, and so does a move that changes nothing.

use std::collections::{BTreeSet, HashSet};

use pdfcraft_cos::Document;

use super::shift::{MAX_SHIFT, OutlineShape, layout, paths_of, picked_roots, read_tree};
use super::{LIST_DEPTH, OutlineError, Result, open_item, outline_root, recount, relink};

/// What a group move does: the new child lists of every node whose children change, the node
/// that must open so the moved bookmarks show, the moved bookmarks' paths afterwards (in outline
/// order), and whether anything changes at all.
#[derive(Clone, Debug, PartialEq)]
pub struct MovePlan {
    pub lists: Vec<(usize, Vec<usize>)>,
    pub opened: Vec<usize>,
    pub paths: Vec<Vec<usize>>,
    /// The bookmarks are already there, in that order: nothing is to be written.
    pub unchanged: bool,
}

/// Plan moving the bookmarks at `paths` to child `index` of `to_parent` (see the module docs).
/// Pure: `shape` is not changed. Refused, naming the offending path, when the group can't go there.
pub fn plan_move(shape: &OutlineShape, paths: &[Vec<usize>], to_parent: &[usize], index: usize) -> Result<MovePlan> {
    if paths.len() > MAX_SHIFT {
        return Err(OutlineError::TooMany);
    }
    let lay = layout(shape);
    let picks = picked_roots(shape, &lay, paths)?;
    if picks.is_empty() {
        return Err(OutlineError::NoSuchBookmark(Vec::new()));
    }
    let moving: HashSet<usize> = picks.iter().map(|&(node, _)| node).collect();
    // The target parent: the root, or a bookmark no deeper than bookmarks are listed.
    let target = if to_parent.is_empty() {
        0
    } else if to_parent.len() > LIST_DEPTH.saturating_add(1) {
        return Err(OutlineError::NoSuchBookmark(to_parent.to_vec()));
    } else {
        shape.node(to_parent).ok_or_else(|| OutlineError::NoSuchBookmark(to_parent.to_vec()))?
    };
    // Not into a moved bookmark or anything under one. Each step goes a level up, so the walk is
    // no longer than the outline.
    let mut up = target;
    for _ in 0..shape.len() {
        if up == 0 {
            break;
        }
        if moving.contains(&up) {
            return Err(OutlineError::IntoItself);
        }
        up = lay.parent.get(up).copied().unwrap_or(0);
    }
    // A bookmark that goes deeper must still be listed, with everything under it.
    let new_depth = if target == 0 { 0 } else { lay.depth.get(target).copied().unwrap_or(0).saturating_add(1) };
    for &(node, path) in &picks {
        let deepest = new_depth.saturating_add(lay.height.get(node).copied().unwrap_or(0));
        if new_depth > lay.depth.get(node).copied().unwrap_or(0) && deepest > LIST_DEPTH {
            return Err(OutlineError::GroupTooDeep(path.clone()));
        }
    }
    // Where the run goes once the moved bookmarks have left the target's list.
    let before = shape.kids.get(target).ok_or_else(|| OutlineError::NoSuchBookmark(to_parent.to_vec()))?;
    let at = index.min(before.len());
    let gone = before.iter().take(at).filter(|k| moving.contains(k)).count();
    let at = at.saturating_sub(gone);
    let mut kids = shape.kids.clone();
    let mut changed = BTreeSet::from([target]);
    let mut arrives = false;
    for &(node, _) in &picks {
        let p = lay.parent.get(node).copied().unwrap_or(0);
        if let Some(list) = kids.get_mut(p) {
            list.retain(|&k| k != node);
        }
        changed.insert(p);
        arrives |= p != target;
    }
    if let Some(list) = kids.get_mut(target) {
        let at = at.min(list.len());
        list.splice(at..at, picks.iter().map(|&(node, _)| node));
    }
    let lists: Vec<(usize, Vec<usize>)> =
        changed.into_iter().filter(|&n| kids.get(n) != shape.kids.get(n)).map(|n| (n, kids.get(n).cloned().unwrap_or_default())).collect();
    let unchanged = lists.is_empty();
    // As a single move does: the new parent opens when a bookmark comes to it from elsewhere.
    let opened = if target != 0 && arrives && !unchanged { vec![target] } else { Vec::new() };
    Ok(MovePlan { lists, opened, paths: paths_of(&kids, &moving), unchanged })
}

/// What [`move_bookmarks`] would do to `doc`, without doing it: the same checks, on the document
/// itself, and the plan. Nothing is written.
pub fn preview_move_bookmarks(doc: &Document, paths: &[Vec<usize>], to_parent: &[usize], index: usize) -> Result<MovePlan> {
    if paths.len() > MAX_SHIFT {
        return Err(OutlineError::TooMany);
    }
    let Some(root) = outline_root(doc) else {
        return Err(OutlineError::NoSuchBookmark(paths.first().cloned().unwrap_or_default()));
    };
    let (shape, _) = read_tree(doc, root)?;
    plan_move(&shape, paths, to_parent, index)
}

/// Move the bookmarks at `paths` together to child `index` of `to_parent`, as one change (see the
/// module docs). Returns their new paths, in outline order. Every check comes before the first
/// write, so a refusal changes nothing; neither does a move to where the bookmarks already are.
pub fn move_bookmarks(doc: &mut Document, paths: &[Vec<usize>], to_parent: &[usize], index: usize) -> Result<Vec<Vec<usize>>> {
    if paths.len() > MAX_SHIFT {
        return Err(OutlineError::TooMany);
    }
    let Some(root) = outline_root(doc) else {
        return Err(OutlineError::NoSuchBookmark(paths.first().cloned().unwrap_or_default()));
    };
    let (shape, refs) = read_tree(doc, root)?;
    let plan = plan_move(&shape, paths, to_parent, index)?;
    if plan.unchanged {
        return Ok(plan.paths);
    }
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

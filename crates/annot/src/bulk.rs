//! This fork (sefr-14/pdfcraft, COMMENTS.1): delete several comments at once.
//!
//! Each target names a `/Annots` slot and what must be in it: the indirect object it finally
//! refers to, or, for an inline dictionary, no object at all. A slot holding anything else means
//! the document changed since the targets were taken, and the whole deletion is refused. Each
//! target goes as [`crate::delete_annotation`] takes one comment: with its pop-up and every
//! reply that hangs off it on its page, Replace Text's strikeout and caret together, and refused
//! while it, or a reply that would go with it, is locked; a group subordinate whose primary goes
//! becomes a comment of its own (upstream's [`crate::release_members`], SYNC.17). Every check comes before the first write; each page's `/Annots` is then written
//! once, keeping everything else in its order.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::AnnotError;
use crate::info::final_ref;
use crate::lookup::{annots, change_annots, page_ref};
use crate::modify::{deleted_index, goes_with, release_members, seed, with_replies_and_popups};

/// The most targets one deletion takes.
pub const MAX_DELETE_TARGETS: usize = 10_000;

/// A comment to delete: its page and position in that page's `/Annots`, and the object that
/// position must hold (`None`: an inline dictionary).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeleteTarget {
    pub page: usize,
    pub index: usize,
    pub object: Option<(u32, u16)>,
}

/// What a deletion removes, worked out without changing anything.
#[derive(Clone, Debug, PartialEq)]
pub struct DeletionSet {
    /// Every entry that goes, as (page, index, whether it is a pop-up), in page and index order.
    pub removed: Vec<(usize, usize, bool)>,
    /// Each affected page, the `/Annots` it keeps, and the objects that go from it.
    kept: Vec<(ObjRef, Vec<Object>, HashSet<ObjRef>)>,
}

/// The slot `target` names holds what it says it does.
fn holds(doc: &Document, entry: &Object, target: &DeleteTarget) -> bool {
    match (entry, target.object) {
        (Object::Ref(r), Some((num, generation))) => {
            let f = final_ref(doc, *r);
            f.num == num && f.generation == generation && doc.get(f).as_dict().is_some()
        }
        (Object::Dict(_), None) => true,
        _ => false,
    }
}

/// Work out what deleting `targets` removes, or why it can't. Changes nothing.
pub fn deletion_set(doc: &Document, targets: &[DeleteTarget]) -> Result<DeletionSet, AnnotError> {
    if targets.is_empty() {
        return Err(AnnotError::Invalid("no comments to delete".into()));
    }
    if targets.len() > MAX_DELETE_TARGETS {
        return Err(AnnotError::Invalid(format!("at most {MAX_DELETE_TARGETS} comments can be deleted at once")));
    }
    let mut by_page: BTreeMap<usize, BTreeSet<DeleteTarget>> = BTreeMap::new();
    for t in targets {
        by_page.entry(t.page).or_default().insert(*t);
    }
    let mut removed = Vec::new();
    let mut kept = Vec::with_capacity(by_page.len());
    for (&page, page_targets) in &by_page {
        let p = page_ref(doc, page)?;
        let list = annots(doc, p);
        let mut seeds: Vec<ObjRef> = Vec::new();
        let mut indices: HashSet<usize> = HashSet::new();
        for t in page_targets {
            let entry = list.get(t.index).ok_or(AnnotError::NoSuchAnnotation { page, index: t.index })?;
            if !holds(doc, entry, t) {
                return Err(AnnotError::Invalid(format!("comment {} on page {} changed; take the comments again", t.index + 1, page + 1)));
            }
            // Replace Text's strikeout deletes from its caret, the pair's primary (upstream).
            let index = deleted_index(doc, page, t.index)?;
            let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
            seeds.extend(seed(doc, entry));
            indices.insert(index);
        }
        let doomed = with_replies_and_popups(doc, &list, seeds);
        let mut keep = Vec::with_capacity(list.len());
        for (i, e) in list.iter().enumerate() {
            if indices.contains(&i) || goes_with(doc, e, &doomed) {
                let popup = doc.resolve(e).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Popup".as_slice()));
                removed.push((page, i, popup));
            } else {
                keep.push(e.clone());
            }
        }
        kept.push((p, keep, doomed));
    }
    Ok(DeletionSet { removed, kept })
}

/// Delete `targets` (see [`deletion_set`]): all of them, or, when any can't go, none.
pub fn delete_annotations(doc: &mut Document, targets: &[DeleteTarget]) -> Result<DeletionSet, AnnotError> {
    let set = deletion_set(doc, targets)?;
    for (page, list, going) in &set.kept {
        let list = release_members(doc, list.clone(), going)?;
        change_annots(doc, *page, list)?;
    }
    Ok(set)
}

#[cfg(test)]
mod tests;

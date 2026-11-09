// INACTIVE — COMMENTS.1 fallback (SYNC.16 trial of upstream #3484). Not compiled: nothing
// declares this module. Original: f3ccab72 (COMMENTS.1c), on 2d1dd6d9 / ff87c233 (COMMENTS.1b, 1a).
// Restore: README.md in this folder, and FORK-CHANGES.md § COMMENTS.1 fallback.
//! This fork (sefr-14/pdfcraft, COMMENTS.1): the cards selected in the Comments panel.
//!
//! A card stands for a comment and its replies. The selection is the panel's own: Ctrl+Click (⌘ on
//! macOS) adds or removes a card, Shift+Click selects the cards shown from the anchor, and
//! neither touches the active comment (`CommentView::selected`, shared with the page), the view
//! or the current page. A plain click, and a comment chosen on the page, make that card the whole
//! selection; the active comment is still set the way it always was.
//!
//! Cards are held by the object their `/Annots` entry names, which stays the same while the
//! document is open, not by their position, which moves when an earlier comment is deleted. An
//! inline comment has no object: it is held by its position for as long as the document stays
//! the version it was selected in, and dropped at the first change.

use std::collections::{BTreeSet, HashSet};

use super::Annotation;

/// A card, as the selection holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommentKey {
    Object((u32, u16)),
    Inline { page: usize, index: usize },
}

impl CommentKey {
    pub fn of(a: &Annotation) -> Self {
        match a.object {
            Some(o) => CommentKey::Object(o),
            None => CommentKey::Inline { page: a.page, index: a.index },
        }
    }
}

/// The selected cards.
#[derive(Clone, Debug, Default)]
pub struct Group {
    selected: BTreeSet<CommentKey>,
    anchor: Option<CommentKey>,
    /// The document version the inline cards in the selection were selected in.
    inline_seen: Option<(u64, usize)>,
    /// The active comment's card when the panel last looked (see [`Group::follow`]).
    followed: Option<CommentKey>,
    /// A card clicked this frame, with the modifiers held, resolved once the list is known.
    pub(crate) clicked: Option<(CommentKey, egui::Modifiers)>,
}

impl Group {
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn contains(&self, key: &CommentKey) -> bool {
        self.selected.contains(key)
    }

    pub fn is_anchor(&self, key: &CommentKey) -> bool {
        self.anchor.as_ref() == Some(key)
    }

    /// Drop the selection; whether there was one.
    pub fn clear(&mut self) -> bool {
        let had = !self.selected.is_empty() || self.anchor.is_some();
        self.selected.clear();
        self.anchor = None;
        self.inline_seen = None;
        had
    }

    fn insert(&mut self, key: CommentKey, now: (u64, usize)) {
        if matches!(key, CommentKey::Inline { .. }) {
            if self.inline_seen != Some(now) {
                self.selected.retain(|k| !matches!(k, CommentKey::Inline { .. }));
            }
            self.inline_seen = Some(now);
        }
        self.selected.insert(key);
    }

    /// Only `key`, which becomes the anchor.
    fn only(&mut self, key: CommentKey, now: (u64, usize)) {
        self.selected.clear();
        self.inline_seen = None;
        self.insert(key, now);
        self.anchor = Some(key);
    }

    /// A click on the card `key` with `modifiers`: Ctrl (⌘) adds or removes it and makes it the
    /// anchor; Shift selects the cards of `shown` from the anchor to it, in place of the
    /// selection, keeping the anchor (with no anchor shown, as a plain click); a plain click
    /// selects only it.
    pub fn click(&mut self, key: CommentKey, modifiers: egui::Modifiers, shown: &[CommentKey], now: (u64, usize)) {
        if modifiers.command {
            if !self.selected.remove(&key) {
                self.insert(key, now);
            }
            self.anchor = Some(key);
        } else if modifiers.shift {
            let at = |k: &CommentKey| shown.iter().position(|s| s == k);
            match (self.anchor.as_ref().and_then(at), at(&key)) {
                (Some(a), Some(b)) => {
                    let anchor = self.anchor;
                    self.selected.clear();
                    self.inline_seen = None;
                    for k in shown.get(a.min(b)..=a.max(b)).into_iter().flatten() {
                        self.insert(*k, now);
                    }
                    self.anchor = anchor;
                }
                _ => self.only(key, now),
            }
        } else {
            self.only(key, now);
        }
    }

    /// The active comment is now `active` (its card, and whether it is a card at all, or `None`).
    /// When it changed since the panel last looked, the selection follows it: a card becomes the
    /// whole selection, none clears it, and a reply leaves it as it is. An unchanged active
    /// comment (one a deletion only moved) leaves the selection alone.
    pub fn follow(&mut self, active: Option<(CommentKey, bool)>, now: (u64, usize)) {
        let key = active.map(|(k, _)| k);
        if key == self.followed {
            return;
        }
        self.followed = key;
        match active {
            Some((k, true)) => self.only(k, now),
            Some((_, false)) => {}
            None => {
                self.clear();
            }
        }
    }

    /// Bring the selection up to date with the document: `cards` are the cards the panel lists
    /// now (its filters and search applied; folded pages count as listed). A card no longer
    /// listed leaves the selection, and so does every inline card once the document is no
    /// longer the version (`now`) it was selected in.
    pub fn reconcile(&mut self, cards: &HashSet<CommentKey>, now: (u64, usize)) {
        if self.inline_seen.is_some_and(|seen| seen != now) {
            self.selected.retain(|k| !matches!(k, CommentKey::Inline { .. }));
            if matches!(self.anchor, Some(CommentKey::Inline { .. })) {
                self.anchor = None;
            }
            self.inline_seen = None;
        }
        self.selected.retain(|k| cards.contains(k));
        if self.anchor.is_some_and(|a| !cards.contains(&a)) {
            self.anchor = None;
        }
    }

    /// The selected cards, as `annotations` lists them now, in list order.
    pub fn listed<'a>(&self, annotations: &'a [Annotation]) -> Vec<&'a Annotation> {
        annotations.iter().filter(|a| a.is_comment() && self.selected.contains(&CommentKey::of(a))).collect()
    }

    /// What Delete selected deletes: each selected card where it is now, with the object it holds.
    pub fn targets(&self, annotations: &[Annotation]) -> Vec<pdfcraft_engine::DeleteTarget> {
        self.listed(annotations).into_iter().map(|a| pdfcraft_engine::DeleteTarget { page: a.page, index: a.index, object: a.object }).collect()
    }

    /// The pages the selected cards are on (each once).
    pub fn pages(&self, annotations: &[Annotation]) -> BTreeSet<usize> {
        self.listed(annotations).into_iter().map(|a| a.page).collect()
    }
}

#[cfg(test)]
mod tests;

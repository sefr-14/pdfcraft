//! This fork (COMMENTS.1): the Comments panel's card selection, without the panel.

use std::collections::HashSet;

use egui::Modifiers;

use super::{CommentKey, Group};

const V1: (u64, usize) = (1, 0);
const V2: (u64, usize) = (2, 0);

fn k(n: u32) -> CommentKey {
    CommentKey::Object((n, 0))
}

fn inline(page: usize, index: usize) -> CommentKey {
    CommentKey::Inline { page, index }
}

fn keys(g: &Group) -> Vec<CommentKey> {
    g.selected.iter().copied().collect()
}

const CTRL: Modifiers = Modifiers::COMMAND;
const SHIFT: Modifiers = Modifiers::SHIFT;

#[test]
fn click_ctrl_click_and_shift_click() {
    let shown = [k(1), k(2), k(3), k(4), k(5)];
    let mut g = Group::default();
    g.click(k(2), Modifiers::NONE, &shown, V1);
    assert_eq!(keys(&g), [k(2)]);
    g.click(k(4), CTRL, &shown, V1);
    assert_eq!(keys(&g), [k(2), k(4)]);
    // The anchor is the last Ctrl+Click; Shift selects from it, in place of the selection.
    g.click(k(1), SHIFT, &shown, V1);
    assert_eq!(keys(&g), [k(1), k(2), k(3), k(4)]);
    assert!(g.is_anchor(&k(4)), "Shift keeps the anchor");
    g.click(k(5), SHIFT, &shown, V1);
    assert_eq!(keys(&g), [k(4), k(5)]);
    // Ctrl+Click takes one out again.
    g.click(k(5), CTRL, &shown, V1);
    assert_eq!(keys(&g), [k(4)]);
}

#[test]
fn a_range_runs_through_the_cards_shown_only() {
    // Card 3 is on a folded page: not shown, so not in the range.
    let shown = [k(1), k(2), k(4)];
    let mut g = Group::default();
    g.click(k(1), Modifiers::NONE, &shown, V1);
    g.click(k(4), SHIFT, &shown, V1);
    assert_eq!(keys(&g), [k(1), k(2), k(4)]);
    // An anchor that isn't shown: Shift selects just the card, which becomes the anchor.
    let mut g = Group::default();
    g.click(k(3), Modifiers::NONE, &[k(3)], V1);
    g.click(k(4), SHIFT, &shown, V1);
    assert_eq!(keys(&g), [k(4)]);
    assert!(g.is_anchor(&k(4)));
}

#[test]
fn the_selection_follows_the_active_comment_only_when_it_changes() {
    let shown = [k(1), k(2), k(3)];
    let mut g = Group::default();
    g.follow(Some((k(1), true)), V1);
    assert_eq!(keys(&g), [k(1)], "a comment chosen on the page becomes the selection");
    g.click(k(2), CTRL, &shown, V1);
    g.click(k(3), CTRL, &shown, V1);
    // The same active comment (a deletion only moved it): the group stays.
    g.follow(Some((k(1), true)), V1);
    assert_eq!(keys(&g), [k(1), k(2), k(3)]);
    // A reply made active leaves the group; a card made active replaces it; none clears it.
    g.follow(Some((k(9), false)), V1);
    assert_eq!(keys(&g), [k(1), k(2), k(3)]);
    g.follow(Some((k(2), true)), V1);
    assert_eq!(keys(&g), [k(2)]);
    g.click(k(3), CTRL, &shown, V1);
    g.follow(None, V1);
    assert!(g.is_empty());
}

#[test]
fn cards_no_longer_listed_leave_the_selection() {
    let shown = [k(1), k(2), k(3)];
    let mut g = Group::default();
    g.click(k(1), Modifiers::NONE, &shown, V1);
    g.click(k(3), SHIFT, &shown, V1);
    // Card 2 is deleted, or filtered out, or has become a reply: it is no longer a listed card.
    g.reconcile(&HashSet::from([k(1), k(3)]), V1);
    assert_eq!(keys(&g), [k(1), k(3)]);
    g.reconcile(&HashSet::from([k(3)]), V1);
    assert_eq!(keys(&g), [k(3)]);
    assert!(!g.is_anchor(&k(1)), "an anchor no longer listed goes too");
}

/// Object keys name the same comment through any change; inline ones only in the version of the
/// document they were selected in.
#[test]
fn inline_cards_drop_at_the_first_change_and_object_cards_stay() {
    let shown = [k(1), inline(0, 4), k(2)];
    let listed: HashSet<CommentKey> = shown.iter().copied().collect();
    let mut g = Group::default();
    g.click(k(1), Modifiers::NONE, &shown, V1);
    g.click(inline(0, 4), CTRL, &shown, V1);
    g.click(k(2), CTRL, &shown, V1);
    g.reconcile(&listed, V1);
    assert_eq!(g.len(), 3, "nothing changed: all stay");
    // Any change of version (an edit, undo, a save, a view toggle, another tab's edit).
    g.reconcile(&listed, V2);
    assert_eq!(keys(&g), [k(1), k(2)]);
    // A range taken in a later version keeps its inline card only in that version.
    g.click(k(1), Modifiers::NONE, &shown, V2);
    g.click(k(2), SHIFT, &shown, V2);
    assert_eq!(g.len(), 3);
    g.reconcile(&listed, V1);
    assert_eq!(keys(&g), [k(1), k(2)]);
}

#[test]
fn clear_says_whether_there_was_a_selection() {
    let mut g = Group::default();
    assert!(!g.clear());
    g.click(k(1), Modifiers::NONE, &[k(1)], V1);
    assert!(g.clear());
    assert!(g.is_empty() && !g.is_anchor(&k(1)));
}

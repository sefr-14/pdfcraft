//! Page pairing for revised drawing sets (the wizard's step 2): inserted, deleted and moved
//! sheets, several changes at once, and re-pairing. Pages are 0-based physical indices here; the
//! comments use the 1-based page numbers a viewer shows.

use pdfcraft_overlay::{Cell, Compare, Effect, Pair, Pairing, PairingError, Side, Slot, Summary};

fn pages(n: usize) -> Vec<usize> {
    (0..n).collect()
}

/// The rows as (A page, B page), 1-based, `None` for a blank.
fn rows(p: &Pairing) -> Vec<(Option<usize>, Option<usize>)> {
    p.rows()
        .iter()
        .map(|r| {
            assert_ne!(r.a, Slot::Unplaced, "{r:?}");
            assert_ne!(r.b, Slot::Unplaced, "{r:?}");
            (r.a.page().map(|x| x + 1), r.b.page().map(|x| x + 1))
        })
        .collect()
}

/// Expected rows built from runs: (first A page, first B page, length), 1-based, 0 for blanks.
fn runs(spec: &[(usize, usize, usize)]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut out = Vec::new();
    for &(a, b, n) in spec {
        for i in 0..n {
            out.push(((a > 0).then(|| a + i), (b > 0).then(|| b + i)));
        }
    }
    out
}

/// Every page of each range is in its column exactly once, and no row is empty.
fn intact(p: &Pairing, na: usize, nb: usize) {
    assert!(p.missing(Side::A, &pages(na)).is_empty() && p.missing(Side::B, &pages(nb)).is_empty(), "{p:?}");
    assert!(p.duplicates(Side::A).is_empty() && p.duplicates(Side::B).is_empty(), "{p:?}");
    assert!(p.empty_rows().is_empty(), "{p:?}");
}

/// Scenario 1: Old has 39 sheets, New 40, with a new sheet at New page 15. Marking New 15 as
/// inserted puts a blank on the Old side there, and Old 15–39 move down to New 16–40.
#[test]
fn an_inserted_sheet() {
    let mut p = Pairing::in_order(&pages(39), &pages(40));
    assert_eq!(p.summary(), Summary { matched: 39, unplaced: 1, ..Summary::default() });
    // Row 15 pairs Old 15 with New 15 until the insertion is marked.
    assert_eq!(p.row(14), Pair { a: Slot::Page(14), b: Slot::Page(14) });
    p.insert_blank(Side::A, 14).unwrap();
    assert_eq!(rows(&p), runs(&[(1, 1, 14), (0, 15, 1), (15, 16, 25)]));
    assert_eq!(p.summary(), Summary { matched: 39, b_only: 1, ..Summary::default() });
    intact(&p, 39, 40);
    // Undo the gap: back to the pairing in order.
    p.remove_blank(Side::A, 14).unwrap();
    assert_eq!(p, Pairing::in_order(&pages(39), &pages(40)));
}

/// Scenario 2: Old has 40 sheets, New 39, with Old page 10 removed. Marking Old 10 as deleted
/// puts a blank on the New side there, and New 10–39 move down to pair with Old 11–40.
#[test]
fn a_deleted_sheet() {
    let mut p = Pairing::in_order(&pages(40), &pages(39));
    p.insert_blank(Side::B, 9).unwrap();
    assert_eq!(rows(&p), runs(&[(1, 1, 9), (10, 0, 1), (11, 10, 30)]));
    assert_eq!(p.summary(), Summary { matched: 39, a_only: 1, ..Summary::default() });
    intact(&p, 40, 39);
}

/// Several insertions and deletions in one set: Old 30 sheets, New 31. Old 5 and Old 20 were
/// deleted, New 9, New 10 and New 26 were inserted.
#[test]
fn several_gaps_in_one_set() {
    let mut p = Pairing::in_order(&pages(30), &pages(31));
    // Old 5 deleted: blank on the New side at row 5.
    p.insert_blank(Side::B, 4).unwrap();
    // New 9 and New 10 inserted: they now sit at rows 10 and 11.
    let r = p.row_of(Side::B, 8).unwrap();
    assert_eq!(r, 9);
    p.insert_blank(Side::A, r).unwrap();
    p.insert_blank(Side::A, r + 1).unwrap();
    // Old 20 deleted.
    let r = p.row_of(Side::A, 19).unwrap();
    p.insert_blank(Side::B, r).unwrap();
    // New 26 inserted.
    let r = p.row_of(Side::B, 25).unwrap();
    p.insert_blank(Side::A, r).unwrap();
    assert_eq!(rows(&p), runs(&[(1, 1, 4), (5, 0, 1), (6, 5, 4), (0, 9, 2), (10, 11, 10), (20, 0, 1), (21, 21, 5), (0, 26, 1), (26, 27, 5)]));
    assert_eq!(p.summary(), Summary { matched: 28, a_only: 2, b_only: 3, ..Summary::default() });
    intact(&p, 30, 31);
}

/// Same page count, different sheets: Old and New both have 40 pages; Old 10 was deleted and a
/// new sheet was inserted at New 25. Paired in order, every sheet from 10 to 24 would be compared
/// with the wrong one; marking both changes gives 41 output pages.
#[test]
fn same_count_with_an_insertion_and_a_deletion() {
    let mut p = Pairing::in_order(&pages(40), &pages(40));
    assert_eq!(p.summary(), Summary { matched: 40, ..Summary::default() });
    p.insert_blank(Side::B, 9).unwrap();
    // New 40 now has no partner until New 25 is marked.
    assert_eq!(p.summary().unplaced, 1);
    let r = p.row_of(Side::B, 24).unwrap();
    p.insert_blank(Side::A, r).unwrap();
    assert_eq!(rows(&p), runs(&[(1, 1, 9), (10, 0, 1), (11, 10, 15), (0, 25, 1), (26, 26, 15)]));
    assert_eq!(p.summary(), Summary { matched: 39, a_only: 1, b_only: 1, ..Summary::default() });
    intact(&p, 40, 40);
}

/// Sheets moved to other positions: pairing a page explicitly swaps it with the page that was
/// there, and moving a page up or down swaps neighbours; every page stays exactly once.
#[test]
fn reordered_sheets() {
    // New moved Old's sheet 3 to position 6.
    let mut p = Pairing::in_order(&pages(8), &pages(8));
    // Pair Old 3 with New 6 by putting New 6 at row 3 …
    p.assign(Side::B, 2, 5).unwrap();
    assert_eq!(p.row(2), Pair { a: Slot::Page(2), b: Slot::Page(5) });
    // … and New 3 where New 6 was.
    assert_eq!(p.row(5), Pair { a: Slot::Page(5), b: Slot::Page(2) });
    intact(&p, 8, 8);
    // A move is undone by moving back.
    p.move_cell(Side::B, 2, true).unwrap();
    assert_eq!(p.row(3), Pair { a: Slot::Page(3), b: Slot::Page(5) });
    p.move_cell(Side::B, 3, false).unwrap();
    assert_eq!(p.row(2), Pair { a: Slot::Page(2), b: Slot::Page(5) });
    assert_eq!(p.move_cell(Side::A, 0, false), Err(PairingError::AtEdge { row: 0 }));
    assert_eq!(p.assign(Side::A, 1, 99), Err(PairingError::NotInColumn { page: 99 }));
    intact(&p, 8, 8);
}

/// Operations never make an output page with no page on either side, and a refused operation
/// changes nothing.
#[test]
fn rows_without_any_page_are_refused() {
    let mut p = Pairing::in_order(&pages(3), &pages(3));
    p.insert_blank(Side::A, 1).unwrap();
    let before = p.clone();
    // A blank opposite a blank.
    assert_eq!(p.insert_blank(Side::B, 1), Err(PairingError::EmptyRow { row: 1 }));
    assert_eq!(p, before);
    assert_eq!(p.remove_blank(Side::B, 1), Err(PairingError::NotBlank { row: 1 }));
    assert_eq!(p.remove_blank(Side::A, 9), Err(PairingError::NoSuchRow { row: 9 }));
    assert_eq!(p, before);
    // A blank past the end, opposite nothing, is dropped rather than kept.
    let q = Pairing::from_columns(vec![Cell::Page(0), Cell::Blank, Cell::Blank], vec![Cell::Page(0)]).unwrap();
    assert_eq!(q.len(), 1);
}

/// Unplaced pages are placed against blanks only on request, and re-pairing returns to the
/// pairing in order. A new range keeps the existing gaps and reorderings.
#[test]
fn placing_unplaced_pages_reset_and_new_ranges() {
    let mut p = Pairing::in_order(&pages(5), &pages(7));
    assert_eq!(p.summary().unplaced, 2);
    p.place_unplaced_against_blanks();
    assert_eq!(p.summary(), Summary { matched: 5, b_only: 2, ..Summary::default() });
    // Reset: the pairing in order again, whatever was changed.
    p.insert_blank(Side::A, 1).unwrap();
    p.assign(Side::B, 3, 6).unwrap();
    assert_ne!(p, Pairing::in_order(&pages(5), &pages(7)));
    p = Pairing::in_order(&pages(5), &pages(7));
    assert_eq!(p.summary().unplaced, 2);

    // A range change keeps manual edits: Old 1–6 with New 3 inserted, then Old's range grows
    // to 1–8 and loses page 1.
    let mut p = Pairing::in_order(&pages(6), &pages(7));
    p.insert_blank(Side::A, 2).unwrap();
    p.set_pages(Side::A, &[1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(p.row(0), Pair { a: Slot::Page(1), b: Slot::Page(0) });
    // The gap opposite New 3 survives, one row up with the rest of Old's column.
    assert_eq!(p.row(1), Pair { a: Slot::Blank, b: Slot::Page(1) });
    assert!(p.missing(Side::A, &[1, 2, 3, 4, 5, 6, 7]).is_empty());
    assert!(p.duplicates(Side::A).is_empty() && p.empty_rows().is_empty());
}

// ── Compare as (the wizard's dropdown), whole-row moves and choosing a partner by hand ──────

use Compare::{Matched, Only};

/// Matched → Old only puts a blank on the New side, and → New only one on the Old side; the
/// displaced sheet stays in the pairing, one row lower, as marking the gap directly does.
#[test]
fn compare_as_one_document_only() {
    let mut p = Pairing::in_order(&pages(40), &pages(40));
    let effects = p.set_compare(9, Only(Side::A)).unwrap();
    assert_eq!(effects, vec![Effect::Shifted { side: Side::B, from: 10, down: true }]);
    assert_eq!(p.row(9), Pair { a: Slot::Page(9), b: Slot::Blank });
    // New 10 is displaced to row 11; New 40 waits for a partner.
    assert_eq!(p.row(10), Pair { a: Slot::Page(10), b: Slot::Page(9) });
    assert_eq!(p.summary(), Summary { matched: 39, a_only: 1, unplaced: 1, ..Summary::default() });
    intact(&p, 40, 40);
    let mut q = Pairing::in_order(&pages(40), &pages(40));
    q.insert_blank(Side::B, 9).unwrap();
    assert_eq!(p, q);

    // Old 39, New 40 with New 15 new: New only on row 15.
    let mut p = Pairing::in_order(&pages(39), &pages(40));
    assert_eq!(p.set_compare(14, Only(Side::B)).unwrap(), vec![Effect::Shifted { side: Side::A, from: 15, down: true }]);
    assert_eq!(rows(&p), runs(&[(1, 1, 14), (0, 15, 1), (15, 16, 25)]));
    assert_eq!(p.row(14).compare(), Some(Only(Side::B)));
    intact(&p, 39, 40);
    // Asking for what the row already is changes nothing.
    let before = p.clone();
    assert_eq!(p.set_compare(14, Only(Side::B)).unwrap(), vec![]);
    assert_eq!(p.set_compare(0, Matched).unwrap(), vec![]);
    assert_eq!(p, before);
}

/// Back to Matched right after a gap: removing it is the exact reverse, in every position and
/// with either document longer.
#[test]
fn back_to_matched_reverses_a_gap() {
    for (na, nb, row, only) in
        [(40, 40, 9, Side::A), (40, 40, 9, Side::B), (39, 40, 14, Side::B), (40, 39, 9, Side::A), (5, 5, 0, Side::A), (5, 5, 4, Side::B)]
    {
        let start = Pairing::in_order(&pages(na), &pages(nb));
        let mut p = start.clone();
        p.set_compare(row, Only(only)).unwrap();
        let back = if only == Side::A { Side::B } else { Side::A };
        let effects = p.set_compare(row, Matched).unwrap();
        assert_eq!(effects, vec![Effect::Shifted { side: back, from: row + 1, down: false }], "{na}/{nb} row {row}");
        assert_eq!(p, start, "{na}/{nb} row {row}");
    }
}

/// A sheet replaced between revisions: Old 6 opposite a blank, directly followed by New 6
/// opposite a blank. Matched on either row pairs them and removes the emptied row; nothing else
/// moves. With a one-sided neighbour on both sides the partner would be a guess: refused.
#[test]
fn back_to_matched_merges_the_neighbouring_one_sided_row() {
    let mut p = Pairing::in_order(&pages(10), &pages(10));
    p.set_compare(5, Only(Side::A)).unwrap();
    p.set_compare(6, Only(Side::B)).unwrap();
    assert_eq!(rows(&p), runs(&[(1, 1, 5), (6, 0, 1), (0, 6, 1), (7, 7, 4)]));
    let mut q = p.clone();
    assert_eq!(q.set_compare(5, Matched).unwrap(), vec![Effect::Merged { side: Side::B, from: 6 }]);
    assert_eq!(q, Pairing::in_order(&pages(10), &pages(10)));
    let mut q = p.clone();
    assert_eq!(q.set_compare(6, Matched).unwrap(), vec![Effect::Merged { side: Side::A, from: 5 }]);
    assert_eq!(q, Pairing::in_order(&pages(10), &pages(10)));

    // A New-only row on each side of an Old-only row: which New sheet is meant is a guess.
    let p = Pairing::from_columns(
        vec![Cell::Page(0), Cell::Blank, Cell::Page(1), Cell::Blank],
        vec![Cell::Page(0), Cell::Page(1), Cell::Blank, Cell::Page(2)],
    )
    .unwrap();
    assert_eq!(p.row(2).compare(), Some(Only(Side::A)));
    let mut q = p.clone();
    assert_eq!(q.set_compare(2, Matched), Err(PairingError::NoCounterpart { row: 2 }));
    assert_eq!(q, p);
}

/// Back to Matched never re-pairs rows set by hand: with a one-sided row, a sheet paired by hand
/// or a row moved further down, removing the gap would shift them, so it is refused and nothing
/// changes; a free sheet further down isn't taken just because it is free.
#[test]
fn back_to_matched_never_re_pairs_rows_set_by_hand() {
    // Another Old only / New only row further down.
    let mut p = Pairing::in_order(&pages(30), &pages(30));
    p.set_compare(4, Only(Side::A)).unwrap();
    p.set_compare(20, Only(Side::B)).unwrap();
    let before = p.clone();
    assert_eq!(p.set_compare(4, Matched), Err(PairingError::NoCounterpart { row: 4 }));
    assert_eq!(p, before);

    // A New sheet paired by hand further down (New 9 with Old 8).
    let mut p = Pairing::in_order(&pages(10), &pages(10));
    p.set_compare(2, Only(Side::A)).unwrap();
    p.take(Side::B, 7, 8).unwrap();
    assert_eq!(p.row(7), Pair { a: Slot::Page(7), b: Slot::Page(8) });
    let before = p.clone();
    assert_eq!(p.set_compare(2, Matched), Err(PairingError::NoCounterpart { row: 2 }));
    assert_eq!(p, before);

    // A row moved by hand further down.
    let mut p = Pairing::in_order(&pages(10), &pages(10));
    p.set_compare(2, Only(Side::A)).unwrap();
    p.move_row(6, true).unwrap();
    let before = p.clone();
    assert_eq!(p.set_compare(2, Matched), Err(PairingError::NoCounterpart { row: 2 }));
    assert_eq!(p, before);

    // New 10, placed against a blank at the end, is free; it isn't taken for Old 3.
    let mut p = Pairing::in_order(&pages(10), &pages(10));
    p.set_compare(2, Only(Side::A)).unwrap();
    p.place_unplaced_against_blanks();
    assert_eq!(p.row(10), Pair { a: Slot::Blank, b: Slot::Page(9) });
    let before = p.clone();
    assert_eq!(p.set_compare(2, Matched), Err(PairingError::NoCounterpart { row: 2 }));
    assert_eq!(p, before);
    intact(&p, 10, 10);
}

/// Choosing a partner by hand: the page and the cell change places; the row the page came from
/// is removed when that leaves it empty, so taking a New-only sheet for an Old-only row works and
/// the rows below keep their pairs. Rows past the first one without a partner can't be reached.
#[test]
fn choosing_a_partner_by_hand() {
    // Old 3 opposite a blank at row 3, New 5 opposite a blank at row 6: not neighbours.
    let a = vec![Cell::Page(0), Cell::Page(1), Cell::Page(2), Cell::Page(3), Cell::Page(4), Cell::Blank, Cell::Page(5)];
    let b = vec![Cell::Page(0), Cell::Page(1), Cell::Blank, Cell::Page(2), Cell::Page(3), Cell::Page(4), Cell::Page(5)];
    let p = Pairing::from_columns(a, b).unwrap();
    let mut q = p.clone();
    // Not neighbours, and the rows below aren't a pushed-down run: Matched is refused …
    assert_eq!(q.set_compare(2, Matched), Err(PairingError::NoCounterpart { row: 2 }));
    // … and New 5 is chosen by hand.
    q.take(Side::B, 2, 4).unwrap();
    assert_eq!(
        rows(&q),
        vec![(Some(1), Some(1)), (Some(2), Some(2)), (Some(3), Some(5)), (Some(4), Some(3)), (Some(5), Some(4)), (Some(6), Some(6))]
    );
    intact(&q, 6, 6);
    // assign alone would leave row 6 empty, so it refuses.
    let mut r = p.clone();
    assert_eq!(r.assign(Side::B, 2, 4), Err(PairingError::EmptyRow { row: 5 }));

    // Old 3, New 6: rows 4–6 have no Old partner yet. Only row 4 can take an Old page.
    let mut p = Pairing::in_order(&pages(3), &pages(6));
    let before = p.clone();
    assert_eq!(p.take(Side::A, 5, 0), Err(PairingError::Unplaced { row: 3 }));
    assert_eq!(p, before);
    p.take(Side::A, 3, 2).unwrap();
    assert_eq!(p.row(2), Pair { a: Slot::Blank, b: Slot::Page(2) });
    assert_eq!(p.row(3), Pair { a: Slot::Page(2), b: Slot::Page(3) });
    assert_eq!(p.row(4), Pair { a: Slot::Unplaced, b: Slot::Page(4) });
    assert!(p.missing(Side::A, &pages(3)).is_empty() && p.duplicates(Side::A).is_empty());
}

/// Rows without a partner yet: one document only goes on the first such row of a column (later
/// ones would need the rows between filled too), and Matched needs a page chosen by hand.
#[test]
fn compare_as_on_rows_without_a_partner() {
    let mut p = Pairing::in_order(&pages(3), &pages(5));
    let before = p.clone();
    assert_eq!(p.set_compare(4, Only(Side::B)), Err(PairingError::Unplaced { row: 3 }));
    assert_eq!(p.set_compare(3, Only(Side::A)), Err(PairingError::Unplaced { row: 3 }));
    assert_eq!(p.set_compare(3, Matched), Err(PairingError::NoCounterpart { row: 3 }));
    assert_eq!(p.set_compare(9, Matched), Err(PairingError::NoSuchRow { row: 9 }));
    assert_eq!(p, before);
    p.set_compare(3, Only(Side::B)).unwrap();
    p.set_compare(4, Only(Side::B)).unwrap();
    assert_eq!(p.summary(), Summary { matched: 3, b_only: 2, ..Summary::default() });
    intact(&p, 3, 5);
}

/// Old only → New only on one row: Matched first (here the reverse of the gap), then the blank
/// on the other side, in one change.
#[test]
fn compare_as_the_other_document_only() {
    let mut p = Pairing::in_order(&pages(39), &pages(40));
    p.set_compare(14, Only(Side::A)).unwrap();
    let before = p.clone();
    p.set_compare(14, Only(Side::B)).unwrap();
    let mut q = Pairing::in_order(&pages(39), &pages(40));
    q.set_compare(14, Only(Side::B)).unwrap();
    assert_eq!(p, q);
    intact(&p, 39, 40);
    // Refused as a whole when Matched is: nothing changes.
    let mut r = Pairing::in_order(&pages(30), &pages(30));
    r.set_compare(4, Only(Side::A)).unwrap();
    r.set_compare(20, Only(Side::B)).unwrap();
    let kept = r.clone();
    assert_eq!(r.set_compare(4, Only(Side::B)), Err(PairingError::NoCounterpart { row: 4 }));
    assert_eq!(r, kept);
    assert_ne!(before, p);
}

/// Several insertions and deletions in one set through Compare as give the same pairing as
/// marking the gaps directly (`several_gaps_in_one_set`).
#[test]
fn several_changes_through_compare_as() {
    let mut p = Pairing::in_order(&pages(30), &pages(31));
    p.set_compare(4, Only(Side::A)).unwrap();
    let r = p.row_of(Side::B, 8).unwrap();
    p.set_compare(r, Only(Side::B)).unwrap();
    p.set_compare(r + 1, Only(Side::B)).unwrap();
    let r = p.row_of(Side::A, 19).unwrap();
    p.set_compare(r, Only(Side::A)).unwrap();
    let r = p.row_of(Side::B, 25).unwrap();
    p.set_compare(r, Only(Side::B)).unwrap();
    assert_eq!(rows(&p), runs(&[(1, 1, 4), (5, 0, 1), (6, 5, 4), (0, 9, 2), (10, 11, 10), (20, 0, 1), (21, 21, 5), (0, 26, 1), (26, 27, 5)]));
    assert_eq!(p.summary(), Summary { matched: 28, a_only: 2, b_only: 3, ..Summary::default() });
    intact(&p, 30, 31);
}

/// A whole output page moves up or down with both its sides; it can't move past the first or
/// last row, nor across the end of a column (a row without a partner yet).
#[test]
fn moving_whole_rows() {
    let mut p = Pairing::in_order(&pages(5), &pages(5));
    p.set_compare(1, Only(Side::A)).unwrap();
    p.place_unplaced_against_blanks();
    let before = p.clone();
    p.move_row(1, true).unwrap();
    assert_eq!(p.row(1), Pair { a: Slot::Page(2), b: Slot::Page(1) });
    assert_eq!(p.row(2), Pair { a: Slot::Page(1), b: Slot::Blank });
    p.move_row(2, false).unwrap();
    assert_eq!(p, before);
    assert_eq!(p.move_row(0, false), Err(PairingError::AtEdge { row: 0 }));
    assert_eq!(p.move_row(5, true), Err(PairingError::AtEdge { row: 5 }));
    assert_eq!(p.move_row(6, true), Err(PairingError::NoSuchRow { row: 6 }));
    // The last row moves up: (blank, New 5) changes places with (Old 5, New 4).
    p.move_row(5, false).unwrap();
    assert_eq!(p.row(4), Pair { a: Slot::Blank, b: Slot::Page(4) });
    assert_eq!(p.row(5), Pair { a: Slot::Page(4), b: Slot::Page(3) });
    intact(&p, 5, 5);

    // Old 5, New 7: rows 6 and 7 have no Old partner; row 5 can't change places with row 6.
    let mut p = Pairing::in_order(&pages(5), &pages(7));
    assert_eq!(p.row_move_target(4, true), Err(PairingError::Unplaced { row: 5 }));
    assert_eq!(p.move_row(5, false), Err(PairingError::Unplaced { row: 5 }));
    // Two rows both without an Old partner do change places.
    p.move_row(5, true).unwrap();
    assert_eq!(p.row(5), Pair { a: Slot::Unplaced, b: Slot::Page(6) });
    assert_eq!(p.row(6), Pair { a: Slot::Unplaced, b: Slot::Page(5) });
}

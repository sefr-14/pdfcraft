//! Page pairing for revised drawing sets (the wizard's step 2): inserted, deleted and moved
//! sheets, several changes at once, and re-pairing. Pages are 0-based physical indices here; the
//! comments use the 1-based page numbers a viewer shows.

use pdfcraft_overlay::{Cell, Pair, Pairing, PairingError, Side, Slot, Summary};

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

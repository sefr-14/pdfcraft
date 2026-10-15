//! Page pairing (the wizard's step 2): which page of A goes with which page of B on each output
//! page, for drawing sets where sheets were inserted, deleted or moved between revisions.
//!
//! A pairing is two columns of cells, one per document. A cell holds a page (its 0-based
//! physical index in the source file, never its label or sheet number) or a blank. Output page
//! `i` pairs A's cell `i` with B's cell `i`:
//!
//! - page + page: matched revisions of a sheet;
//! - page + blank: a sheet only that document has (deleted, or inserted);
//! - where one column is shorter, the other's extra pages are unplaced until the user places them
//!   (main PRD: no page is dropped without a choice).
//!
//! Every operation only moves cells within a column, or adds or removes a blank, so it can never
//! lose or duplicate a page, and the sources are never touched. An operation that would leave a
//! row with no page on either side is refused, and a refused operation changes nothing.
//!
//! The wizard's "Compare as" ([`Pairing::set_compare`]) names what a row holds, Matched, A only
//! or B only, rather than why: a sheet one revision lacks may have been inserted, deleted or just
//! left out. Turning a one-sided row back into a matched one only takes a partner the pairing
//! itself points to (the neighbouring one-sided row, or the run an earlier gap pushed down);
//! anything else is refused, and the partner is chosen by hand ([`Pairing::take`]), so pairings
//! the user made are never re-paired behind their back.

use std::fmt;

use crate::Side;

/// Most output pages a pairing holds.
pub const MAX_ROWS: usize = 10_000;

/// One cell of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cell {
    /// A page of the document, by 0-based physical index.
    Page(usize),
    /// A deliberate blank: the document adds nothing to this output page.
    Blank,
}

/// One side of an output page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Page(usize),
    Blank,
    /// Past the end of this document's column: the other document's page here has no partner yet.
    Unplaced,
}

impl Slot {
    pub fn page(self) -> Option<usize> {
        match self {
            Slot::Page(p) => Some(p),
            _ => None,
        }
    }

    fn from(cell: Option<&Cell>) -> Slot {
        match cell {
            Some(Cell::Page(p)) => Slot::Page(*p),
            Some(Cell::Blank) => Slot::Blank,
            None => Slot::Unplaced,
        }
    }
}

/// One output page: A's side and B's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pair {
    pub a: Slot,
    pub b: Slot,
}

impl Pair {
    /// Both pages, for a matched row.
    pub fn pages(&self) -> Option<(usize, usize)> {
        Some((self.a.page()?, self.b.page()?))
    }

    /// Neither side has a page.
    pub fn is_empty(&self) -> bool {
        self.a.page().is_none() && self.b.page().is_none()
    }

    /// What the row compares; `None` while a side has no partner yet (or the row is empty).
    pub fn compare(&self) -> Option<Compare> {
        match (self.a, self.b) {
            (Slot::Page(_), Slot::Page(_)) => Some(Compare::Matched),
            (Slot::Page(_), Slot::Blank) => Some(Compare::Only(Side::A)),
            (Slot::Blank, Slot::Page(_)) => Some(Compare::Only(Side::B)),
            _ => None,
        }
    }
}

/// What an output page compares: the wizard's "Compare as".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    /// A page from each document.
    Matched,
    /// A page from this document only; the other side is a deliberate blank.
    Only(Side),
}

/// What a [`Pairing::set_compare`] changed besides its own row, so the wizard can say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// `side`'s cells from output page `from` (0-based) on moved one row down, or up.
    Shifted { side: Side, from: usize, down: bool },
    /// `side`'s page came from the neighbouring output page `from` (0-based, before the change),
    /// which held nothing else and was removed.
    Merged { side: Side, from: usize },
}

fn other(side: Side) -> Side {
    match side {
        Side::A => Side::B,
        Side::B => Side::A,
    }
}

/// Why an operation was refused. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingError {
    /// The row (0-based) doesn't exist.
    NoSuchRow { row: usize },
    /// The operation would leave output page `row` (0-based) with no page on either side.
    EmptyRow { row: usize },
    /// The cell isn't a blank, so there is no gap to remove.
    NotBlank { row: usize },
    /// The page isn't in this document's column (outside its range).
    NotInColumn { page: usize },
    /// The cell can't move further that way.
    AtEdge { row: usize },
    /// More than [`MAX_ROWS`] output pages.
    TooManyRows,
    /// Output page `row` (0-based) has no partner yet on one side; that has to be settled first.
    Unplaced { row: usize },
    /// No partner for output page `row` (0-based) can be found without re-pairing pages paired
    /// elsewhere: choose it in the row's cell.
    NoCounterpart { row: usize },
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairingError::NoSuchRow { row } => write!(f, "there is no output page {}", row.saturating_add(1)),
            PairingError::EmptyRow { row } => {
                write!(f, "that would leave output page {} with no page from either document", row.saturating_add(1))
            }
            PairingError::NotBlank { row } => write!(f, "output page {} has no gap on that side", row.saturating_add(1)),
            PairingError::NotInColumn { page } => write!(f, "page {} isn't in this document's range", page.saturating_add(1)),
            PairingError::AtEdge { row } => write!(f, "output page {}'s page can't move further that way", row.saturating_add(1)),
            PairingError::TooManyRows => write!(f, "more than {MAX_ROWS} output pages"),
            PairingError::Unplaced { row } => write!(f, "output page {} has no partner yet; settle that row first", row.saturating_add(1)),
            PairingError::NoCounterpart { row } => write!(
                f,
                "no page can be paired with output page {} without re-pairing other rows; choose it in the row's cell",
                row.saturating_add(1)
            ),
        }
    }
}

impl std::error::Error for PairingError {}

/// Counts of the output pages by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// A page from each document.
    pub matched: usize,
    /// A page from A only (B is blank).
    pub a_only: usize,
    /// A page from B only (A is blank).
    pub b_only: usize,
    /// A page whose partner isn't chosen yet.
    pub unplaced: usize,
    /// No page at all (never made by an operation; refused before generation).
    pub empty: usize,
}

/// The two columns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pairing {
    a: Vec<Cell>,
    b: Vec<Cell>,
}

impl Pairing {
    /// Each document's pages in order, paired row by row (at most [`MAX_ROWS`] each).
    pub fn in_order(pages_a: &[usize], pages_b: &[usize]) -> Self {
        let col = |p: &[usize]| p.iter().take(MAX_ROWS).map(|p| Cell::Page(*p)).collect();
        Pairing { a: col(pages_a), b: col(pages_b) }
    }

    /// A pairing from explicit columns (each page at most once per column).
    pub fn from_columns(a: Vec<Cell>, b: Vec<Cell>) -> Result<Self, PairingError> {
        if a.len() > MAX_ROWS || b.len() > MAX_ROWS {
            return Err(PairingError::TooManyRows);
        }
        let mut p = Pairing { a, b };
        p.trim();
        Ok(p)
    }

    pub fn column(&self, side: Side) -> &[Cell] {
        match side {
            Side::A => &self.a,
            Side::B => &self.b,
        }
    }

    fn column_mut(&mut self, side: Side) -> &mut Vec<Cell> {
        match side {
            Side::A => &mut self.a,
            Side::B => &mut self.b,
        }
    }

    /// The number of output pages.
    pub fn len(&self) -> usize {
        self.a.len().max(self.b.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn slot(&self, side: Side, row: usize) -> Slot {
        Slot::from(self.column(side).get(row))
    }

    pub fn row(&self, row: usize) -> Pair {
        Pair { a: self.slot(Side::A, row), b: self.slot(Side::B, row) }
    }

    /// Every output page, in order.
    pub fn rows(&self) -> Vec<Pair> {
        (0..self.len()).map(|r| self.row(r)).collect()
    }

    /// The row holding `page` in `side`'s column.
    pub fn row_of(&self, side: Side, page: usize) -> Option<usize> {
        self.column(side).iter().position(|c| *c == Cell::Page(page))
    }

    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for r in self.rows() {
            match (r.a, r.b) {
                (Slot::Page(_), Slot::Page(_)) => s.matched += 1,
                (Slot::Unplaced, _) | (_, Slot::Unplaced) if !r.is_empty() => s.unplaced += 1,
                (Slot::Page(_), Slot::Blank) => s.a_only += 1,
                (Slot::Blank, Slot::Page(_)) => s.b_only += 1,
                _ => s.empty += 1,
            }
        }
        s
    }

    /// Rows (0-based) with no page on either side.
    pub fn empty_rows(&self) -> Vec<usize> {
        (0..self.len()).filter(|r| self.row(*r).is_empty()).collect()
    }

    /// Pages of `pages` that no cell of `side`'s column holds.
    pub fn missing(&self, side: Side, pages: &[usize]) -> Vec<usize> {
        pages.iter().copied().filter(|p| self.row_of(side, *p).is_none()).collect()
    }

    /// Pages held by more than one cell of `side`'s column.
    pub fn duplicates(&self, side: Side) -> Vec<usize> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<usize> = self
            .column(side)
            .iter()
            .filter_map(|c| match c {
                Cell::Page(p) if !seen.insert(*p) => Some(*p),
                _ => None,
            })
            .collect();
        out.dedup();
        out
    }

    /// Blanks past the end of the other column pair with nothing: drop them.
    fn trim(&mut self) {
        let (la, lb) = (self.a.len(), self.b.len());
        while self.a.len() > lb && self.a.last() == Some(&Cell::Blank) {
            self.a.pop();
        }
        while self.b.len() > la && self.b.last() == Some(&Cell::Blank) {
            self.b.pop();
        }
    }

    /// Apply `change` to a copy; keep it only if it adds no empty row.
    fn attempt(&mut self, change: impl FnOnce(&mut Pairing) -> Result<(), PairingError>) -> Result<(), PairingError> {
        let mut next = self.clone();
        change(&mut next)?;
        next.trim();
        if next.a.len() > MAX_ROWS || next.b.len() > MAX_ROWS {
            return Err(PairingError::TooManyRows);
        }
        let (before, after) = (self.empty_rows(), next.empty_rows());
        if after.len() > before.len() {
            let row = after.iter().copied().find(|r| !before.contains(r)).or(after.first().copied()).unwrap_or(0);
            return Err(PairingError::EmptyRow { row });
        }
        *self = next;
        Ok(())
    }

    /// Put a blank at output page `row` in `side`'s column; that side's later pages move one
    /// output page down. Marks the other side's page at `row` as one only it has: a sheet
    /// inserted in it, or deleted from `side`.
    pub fn insert_blank(&mut self, side: Side, row: usize) -> Result<(), PairingError> {
        if row > self.len() {
            return Err(PairingError::NoSuchRow { row });
        }
        self.attempt(|p| {
            p.blank_at(side, row);
            Ok(())
        })
    }

    fn blank_at(&mut self, side: Side, row: usize) {
        let col = self.column_mut(side);
        if row > col.len() {
            // Past the end of this column: fill up to the row with blanks first.
            col.resize(row, Cell::Blank);
        }
        // row <= col.len() here, so insert can't panic.
        col.insert(row, Cell::Blank);
    }

    /// Remove the blank at output page `row` in `side`'s column (undo a gap); that side's later
    /// pages move one output page up.
    pub fn remove_blank(&mut self, side: Side, row: usize) -> Result<(), PairingError> {
        match self.column(side).get(row) {
            Some(Cell::Blank) => {}
            Some(Cell::Page(_)) => return Err(PairingError::NotBlank { row }),
            None => return Err(PairingError::NoSuchRow { row }),
        }
        self.attempt(|p| {
            // The cell exists (checked above), so remove can't panic.
            p.column_mut(side).remove(row);
            Ok(())
        })
    }

    /// Swap the cell at `row` in `side`'s column with its neighbour above (`down` false) or below:
    /// a sheet moved to another position.
    pub fn move_cell(&mut self, side: Side, row: usize, down: bool) -> Result<(), PairingError> {
        let len = self.column(side).len();
        if row >= len {
            return Err(PairingError::NoSuchRow { row });
        }
        let other = if down { row.checked_add(1).filter(|r| *r < len) } else { row.checked_sub(1) };
        let Some(other) = other else { return Err(PairingError::AtEdge { row }) };
        self.attempt(|p| {
            p.column_mut(side).swap(row, other);
            Ok(())
        })
    }

    /// Put `page` of `side`'s document at output page `row`: the cell now holding it and the cell
    /// at `row` change places, so every page stays in the column exactly once. A row past the end
    /// of the column is reached through blanks.
    pub fn assign(&mut self, side: Side, row: usize, page: usize) -> Result<(), PairingError> {
        if row >= self.len() {
            return Err(PairingError::NoSuchRow { row });
        }
        let Some(from) = self.row_of(side, page) else { return Err(PairingError::NotInColumn { page }) };
        if from == row {
            return Ok(());
        }
        self.attempt(|p| {
            let col = p.column_mut(side);
            if row >= col.len() {
                col.resize(row + 1, Cell::Blank);
            }
            col.swap(from, row);
            Ok(())
        })
    }

    /// Pair `page` of `side`'s document with output page `row` by hand, as [`Pairing::assign`]
    /// does (the page and the cell at `row` change places), except that the output page the page
    /// came from is removed when that leaves it with no page at all (it held only this page,
    /// opposite a blank). The rows below it keep their pairs, one row up. Only the first row
    /// past the end of `side`'s column can be reached: the rows after it have no partner yet,
    /// and filling them with blanks would change them without a word.
    pub fn take(&mut self, side: Side, row: usize, page: usize) -> Result<(), PairingError> {
        if row >= self.len() {
            return Err(PairingError::NoSuchRow { row });
        }
        let Some(from) = self.row_of(side, page) else { return Err(PairingError::NotInColumn { page }) };
        if from == row {
            return Ok(());
        }
        let end = self.column(side).len();
        if row > end {
            return Err(PairingError::Unplaced { row: end });
        }
        self.attempt(|p| {
            let col = p.column_mut(side);
            if row >= col.len() {
                // row == len here: one blank, so nothing else changes.
                col.push(Cell::Blank);
            }
            // Both are within the column now, so swap can't panic.
            col.swap(from, row);
            if p.row(from).is_empty() {
                p.remove_row(from);
            }
            Ok(())
        })
    }

    /// Remove output page `row` from both columns (where they reach it).
    fn remove_row(&mut self, row: usize) {
        for side in [Side::A, Side::B] {
            let col = self.column_mut(side);
            if row < col.len() {
                col.remove(row);
            }
        }
    }

    /// The row output page `row` changes places with when moved down (or up), if it can: both
    /// rows must be within each column, or both past its end.
    pub fn row_move_target(&self, row: usize, down: bool) -> Result<usize, PairingError> {
        let len = self.len();
        if row >= len {
            return Err(PairingError::NoSuchRow { row });
        }
        let other = if down { row.checked_add(1).filter(|r| *r < len) } else { row.checked_sub(1) };
        let Some(other) = other else { return Err(PairingError::AtEdge { row }) };
        let (lo, hi) = (row.min(other), row.max(other));
        for side in [Side::A, Side::B] {
            let end = self.column(side).len();
            if lo < end && hi >= end {
                return Err(PairingError::Unplaced { row: end });
            }
        }
        Ok(other)
    }

    /// Move the whole output page `row`, both sides, one row down (or up): it and its neighbour
    /// change places, so every pair stays a pair.
    pub fn move_row(&mut self, row: usize, down: bool) -> Result<(), PairingError> {
        let other = self.row_move_target(row, down)?;
        let (lo, hi) = (row.min(other), row.max(other));
        self.attempt(|p| {
            for side in [Side::A, Side::B] {
                let col = p.column_mut(side);
                // Both within the column (row_move_target), so swap can't panic.
                if hi < col.len() {
                    col.swap(lo, hi);
                }
            }
            Ok(())
        })
    }

    /// Make output page `row` compare as `to`, keeping every page:
    ///
    /// - Matched → one document only: a blank goes into the other document's cell, and that
    ///   document's later cells move one row down (the displaced page stays, one row lower).
    /// - One document only → Matched: the partner comes only from where the pairing points to it.
    ///   The row directly above or below holding only the other document's page (not both such
    ///   rows: that would be a guess) merges into this one; or, when every row below is the
    ///   untouched run an earlier gap pushed down (pages on both sides, each column's pages in
    ///   sequence, no blank), the gap is removed and that run moves back up. Otherwise
    ///   [`PairingError::NoCounterpart`]: choose the partner with [`Pairing::take`].
    /// - One document only → the other only: Matched as above, then a blank for the first.
    /// - A row without a partner yet → one document only: a blank in the other's cell, for the
    ///   first such row of that column only.
    ///
    /// Returns what moved besides the row. A refused change changes nothing.
    pub fn set_compare(&mut self, row: usize, to: Compare) -> Result<Vec<Effect>, PairingError> {
        if row >= self.len() {
            return Err(PairingError::NoSuchRow { row });
        }
        let mut effects = Vec::new();
        self.attempt(|p| {
            effects = p.compare_in_place(row, to)?;
            Ok(())
        })?;
        Ok(effects)
    }

    fn compare_in_place(&mut self, row: usize, to: Compare) -> Result<Vec<Effect>, PairingError> {
        let now = self.row(row);
        match (now.compare(), to) {
            (Some(Compare::Matched), Compare::Matched) => Ok(Vec::new()),
            (Some(Compare::Matched), Compare::Only(keep)) => {
                let side = other(keep);
                self.blank_at(side, row);
                Ok(vec![Effect::Shifted { side, from: row.saturating_add(1), down: true }])
            }
            (Some(Compare::Only(have)), Compare::Matched) => Ok(vec![self.match_one_sided(row, have)?]),
            (Some(Compare::Only(have)), Compare::Only(keep)) if keep == have => Ok(Vec::new()),
            (Some(Compare::Only(have)), Compare::Only(_)) => {
                let first = self.match_one_sided(row, have)?;
                // Merging with the row above moves this row up one.
                let row = match first {
                    Effect::Merged { from, .. } if from < row => row.saturating_sub(1),
                    _ => row,
                };
                self.blank_at(have, row);
                Ok(vec![first, Effect::Shifted { side: have, from: row.saturating_add(1), down: true }])
            }
            (None, Compare::Only(keep)) => {
                let side = other(keep);
                let end = self.column(side).len();
                if self.slot(keep, row).page().is_none() || self.slot(side, row) != Slot::Unplaced {
                    return Err(PairingError::Unplaced { row });
                }
                if row != end {
                    // Filling the rows between with blanks would change them too.
                    return Err(PairingError::Unplaced { row: end });
                }
                self.column_mut(side).push(Cell::Blank);
                Ok(Vec::new())
            }
            (None, Compare::Matched) => Err(PairingError::NoCounterpart { row }),
        }
    }

    /// Give the one-sided output page `row` (a page of `have`, a blank for the other document) a
    /// partner, as [`Pairing::set_compare`] describes.
    fn match_one_sided(&mut self, row: usize, have: Side) -> Result<Effect, PairingError> {
        let side = other(have);
        let only_other = |r: usize| self.row(r).compare() == Some(Compare::Only(side));
        let above = row.checked_sub(1).filter(|r| only_other(*r));
        let below = row.checked_add(1).filter(|r| *r < self.len() && only_other(*r));
        match (above, below) {
            (Some(_), Some(_)) => return Err(PairingError::NoCounterpart { row }),
            (Some(n), None) | (None, Some(n)) => {
                let Slot::Page(page) = self.slot(side, n) else { return Err(PairingError::NoCounterpart { row }) };
                let col = self.column_mut(side);
                if let Some(c) = col.get_mut(row) {
                    *c = Cell::Page(page);
                }
                if let Some(c) = col.get_mut(n) {
                    *c = Cell::Blank;
                }
                // Row n now holds a blank on both sides: remove it.
                self.remove_row(n);
                return Ok(Effect::Merged { side, from: n });
            }
            (None, None) => {}
        }
        if self.gap_pushed_run_below(row, have) {
            // The cell is a blank (checked by the caller's match), so this removes the gap.
            if row < self.column(side).len() {
                self.column_mut(side).remove(row);
            }
            return Ok(Effect::Shifted { side, from: row.saturating_add(1), down: false });
        }
        Err(PairingError::NoCounterpart { row })
    }

    /// Below the one-sided output page `row`, is there only the run an earlier gap in the other
    /// document's column pushed down, untouched since? Every row below has a page on both sides
    /// (or is past the end of one column), each column's pages go on in sequence from the row
    /// (from the row below, for the other document), and the page that would move up into the
    /// row follows the other document's nearest page above it. Then removing the gap re-pairs
    /// nothing the user paired by hand.
    fn gap_pushed_run_below(&self, row: usize, have: Side) -> bool {
        let side = other(have);
        let (h, x) = (self.column(have), self.column(side));
        let in_sequence = |cells: &[Cell]| {
            cells.iter().try_fold(None::<usize>, |prev, c| match (prev, c) {
                (_, Cell::Blank) => None,
                (None, Cell::Page(p)) => Some(Some(*p)),
                (Some(q), Cell::Page(p)) => (q.checked_add(1) == Some(*p)).then_some(Some(*p)),
            })
        };
        let (Some(h_run), Some(x_run)) = (h.get(row..), x.get(row.saturating_add(1)..)) else { return false };
        let Some(Cell::Page(first)) = x_run.first() else { return false };
        if in_sequence(h_run).is_none() || in_sequence(x_run).is_none() {
            return false;
        }
        let above = x.get(..row).and_then(|c| c.iter().rev().find_map(|c| if let Cell::Page(p) = c { Some(*p) } else { None }));
        match above {
            Some(p) => p.checked_add(1) == Some(*first),
            None => x.iter().filter_map(|c| if let Cell::Page(p) = c { Some(*p) } else { None }).min() == Some(*first),
        }
    }

    /// The pages left without a partner (past the end of the shorter column) each get a blank:
    /// an explicit choice that they are in one document only.
    pub fn place_unplaced_against_blanks(&mut self) {
        let n = self.len();
        self.a.resize(n, Cell::Blank);
        self.b.resize(n, Cell::Blank);
    }

    /// Change `side`'s pages to `pages` (a new range), keeping the existing pairing where it can:
    /// cells of pages no longer included are removed (the column's later cells move up), pages
    /// newly included go at the end in order, and rows left with no page are dropped (they held
    /// no page).
    pub fn set_pages(&mut self, side: Side, pages: &[usize]) {
        let wanted: std::collections::HashSet<usize> = pages.iter().copied().collect();
        let col = self.column_mut(side);
        col.retain(|c| match c {
            Cell::Page(p) => wanted.contains(p),
            Cell::Blank => true,
        });
        let present: std::collections::HashSet<usize> = col.iter().filter_map(|c| if let Cell::Page(p) = c { Some(*p) } else { None }).collect();
        col.extend(pages.iter().filter(|p| !present.contains(p)).map(|p| Cell::Page(*p)));
        col.truncate(MAX_ROWS);
        // Rows with a blank on both sides hold no page: drop the row from both columns.
        let mut row = 0;
        while row < self.len() {
            if self.row(row).is_empty() {
                if row < self.a.len() {
                    self.a.remove(row);
                }
                if row < self.b.len() {
                    self.b.remove(row);
                }
            } else {
                row += 1;
            }
        }
        self.trim();
    }
}

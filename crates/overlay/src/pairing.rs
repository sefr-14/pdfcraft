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
            let col = p.column_mut(side);
            if row > col.len() {
                // Past the end of this column: fill up to the row with blanks first.
                col.resize(row, Cell::Blank);
            }
            // row <= col.len() here, so insert can't panic.
            col.insert(row, Cell::Blank);
            Ok(())
        })
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

//! Errors and warnings. Each carries the numbers behind it, so the wizard can explain it.

use std::fmt;

/// Something the overlay can't work with. Public functions return these; nothing panics.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayError {
    /// `page` (0-based) is past the end of a document with `count` pages.
    NoSuchPage { page: usize, count: usize },
    /// The visible box has no area, so there is nothing to overlay. `page` is 0-based, when known.
    EmptyBox { page: Option<usize>, width: f64, height: f64 },
    /// A value that must be a finite number isn't (NaN or infinite), or a result overflowed.
    NotFinite { what: &'static str },
    /// A value that must be positive isn't.
    NotPositive { what: &'static str, value: f64 },
    /// A value outside its allowed range, such as a light-tone threshold outside [0.5, 1].
    OutOfRange { what: &'static str, value: f64, min: f64, max: f64 },
    /// A source page couldn't be read or copied. `side` is 'A' or 'B'; `page` is 0-based.
    Source { side: char, page: usize, reason: String },
    /// Neither document has a page here, so there is nothing to compose.
    NoSources,
    /// The output document can't take the page (it has no page tree, for example).
    Output { reason: String },
    /// Both documents were given the same role: one must be `Old` and the other `New`.
    SameRole { role: &'static str },
    /// The finished overlay failed its own safety check (step 4 PRD, Defensive check): it holds
    /// optional content other than its two layers, an annotation, a form, an action or a script.
    /// Nothing is returned, so nothing can be saved.
    Check { found: String },
}

impl OverlayError {
    /// The same error, naming the page it came from.
    pub fn on_page(self, page: usize) -> Self {
        match self {
            OverlayError::EmptyBox { width, height, .. } => OverlayError::EmptyBox { page: Some(page), width, height },
            other => other,
        }
    }
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OverlayError::NoSuchPage { page, count } => write!(f, "page {} doesn't exist; the document has {count} pages", page.saturating_add(1)),
            OverlayError::EmptyBox { page: Some(p), width, height } => {
                write!(f, "page {} has nothing to show: its visible area is {width} × {height} pt", p.saturating_add(1))
            }
            OverlayError::EmptyBox { page: None, width, height } => {
                write!(f, "the visible area is {width} × {height} pt, so there is nothing to show")
            }
            OverlayError::NotFinite { what } => write!(f, "{what} isn't a finite number"),
            OverlayError::NotPositive { what, value } => write!(f, "{what} must be positive, not {value}"),
            OverlayError::OutOfRange { what, value, min, max } => write!(f, "{what} must be between {min} and {max}, not {value}"),
            OverlayError::Source { side, page, reason } => write!(f, "document {side}, page {}: {reason}", page.saturating_add(1)),
            OverlayError::NoSources => write!(f, "neither document has a page here, so there is nothing to overlay"),
            OverlayError::Output { reason } => write!(f, "the overlay can't be written: {reason}"),
            OverlayError::SameRole { role } => write!(f, "documents A and B are both {role}: one must be Old and the other New"),
            OverlayError::Check { found } => write!(f, "the overlay failed its safety check, so nothing was produced: {found}"),
        }
    }
}

impl std::error::Error for OverlayError {}

/// Something the overlay handled the way pdfcraft does, but that you should know about.
/// `page` is 0-based.
#[derive(Clone, Debug, PartialEq)]
pub enum Warning {
    /// The crop box doesn't overlap the media box. pdfcraft shows the media box; so does the overlay.
    CropOutsideMedia { page: usize, crop: [f64; 4], media: [f64; 4] },
    /// The crop box isn't four numbers. pdfcraft ignores it and shows the media box; so does the overlay.
    UnreadableCropBox { page: usize },
    /// The media box is missing or isn't four numbers. pdfcraft shows US Letter (612 × 792 pt);
    /// so does the overlay.
    MissingMediaBox { page: usize },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::CropOutsideMedia { page, .. } => {
                write!(
                    f,
                    "page {}: the crop box lies outside the media box, so the whole media box is used, as pdfcraft shows it",
                    page.saturating_add(1)
                )
            }
            Warning::UnreadableCropBox { page } => {
                write!(f, "page {}: the crop box can't be read, so the media box is used, as pdfcraft shows it", page.saturating_add(1))
            }
            Warning::MissingMediaBox { page } => {
                write!(f, "page {}: there is no usable media box, so US Letter (612 × 792 pt) is assumed, as pdfcraft does", page.saturating_add(1))
            }
        }
    }
}

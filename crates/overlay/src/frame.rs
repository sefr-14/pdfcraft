//! The displayed frame of a page (overlay PRD, Page geometry): its visible box, upright after
//! `/Rotate`, in points after `/UserUnit`, with the visible box's lower-left corner at (0, 0).
//!
//! It inverts pdfcraft's own display matrix (`pdfcraft_model::view_matrix_for`), so the overlay
//! frames a page exactly as the viewer shows it, and the two can't drift apart.

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Document, Object};
use pdfcraft_model::{Page, pages, view_matrix_for};

use crate::{OverlayError, Warning};

/// US Letter, which pdfcraft shows for a page without a usable media box.
pub const DEFAULT_MEDIA: [f64; 4] = [0.0, 0.0, 612.0, 792.0];

/// A page as the viewer shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayFrame {
    /// User space → displayed frame (points, y up, origin at the sheet's lower-left corner).
    pub to_display: Matrix,
    /// Displayed width in points.
    pub width: f64,
    /// Displayed height in points.
    pub height: f64,
    /// The page's `/UserUnit` (1 when it has none).
    pub unit: f64,
    /// The visible box in user space, `[x0 y0 x1 y1]`.
    pub visible: [f64; 4],
    /// Clockwise `/Rotate`: 0, 90, 180 or 270.
    pub rotation: i64,
}

impl DisplayFrame {
    /// The displayed size, (width, height), in points.
    pub fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// The displayed sheet's diagonal in points.
    pub fn diagonal(&self) -> f64 {
        self.width.hypot(self.height)
    }
}

/// The displayed frame of a visible box `[x0 y0 x1 y1]` shown turned clockwise by `rotation`
/// at `user_unit` points per unit. `rotation` is a multiple of 90; other values given here
/// round down to one. Pages don't reach this with such a value: [`frame_of`] passes
/// `pdfcraft_model::Page::rotation`, which reads a `/Rotate` that isn't a multiple of 90 as 0,
/// as pdfcraft draws the page.
pub fn frame(visible: [f64; 4], rotation: i64, user_unit: f64) -> Result<DisplayFrame, OverlayError> {
    if !visible.iter().all(|v| v.is_finite()) {
        return Err(OverlayError::NotFinite { what: "the visible box" });
    }
    if !user_unit.is_finite() {
        return Err(OverlayError::NotFinite { what: "/UserUnit" });
    }
    if user_unit <= 0.0 {
        return Err(OverlayError::NotPositive { what: "/UserUnit", value: user_unit });
    }
    let [a, b, c, d] = visible;
    let visible = [a.min(c), b.min(d), a.max(c), b.max(d)];
    let [x0, y0, x1, y1] = visible;
    let rotation = rotation.rem_euclid(360) / 90 * 90;
    let (w, h) = (x1 - x0, y1 - y0);
    let (w, h) = if rotation % 180 == 0 { (w, h) } else { (h, w) };
    let (width, height) = (w * user_unit, h * user_unit);
    if !(width.is_finite() && height.is_finite()) {
        return Err(OverlayError::NotFinite { what: "the page size" });
    }
    if width <= 0.0 || height <= 0.0 {
        return Err(OverlayError::EmptyBox { page: None, width, height });
    }
    // pdfcraft's matrix takes the displayed frame (before /UserUnit) to user space; invert it,
    // then scale by the unit, as `Page::view_matrix` divides by it.
    let shown = Matrix(view_matrix_for(rotation, visible));
    let back = shown.invert().ok_or(OverlayError::NotFinite { what: "the page matrix" })?;
    let to_display = back.then(&Matrix([user_unit, 0.0, 0.0, user_unit, 0.0, 0.0]));
    if !to_display.0.iter().all(|v| v.is_finite()) {
        return Err(OverlayError::NotFinite { what: "the page matrix" });
    }
    Ok(DisplayFrame { to_display, width, height, unit: user_unit, visible, rotation })
}

/// The displayed frame of page `page` (0-based) of `doc`, with any fallbacks as warnings.
pub fn frame_of_page(doc: &Document, page: usize) -> Result<(DisplayFrame, Vec<Warning>), OverlayError> {
    let all = pages(doc);
    let count = all.len();
    let p = all.get(page).ok_or(OverlayError::NoSuchPage { page, count })?;
    frame_of(doc, p, page)
}

/// [`frame_of_page`] for a page already looked up; `page` is its 0-based index, for messages.
///
/// The visible box is read as pdfcraft reads it (`pdfcraft_model::Page::crop`): the crop box
/// clipped to the media box, the media box when the crop box is missing, unreadable or outside
/// it, and US Letter when there is no usable media box. Each fallback adds a warning.
pub fn frame_of(doc: &Document, p: &Page, page: usize) -> Result<(DisplayFrame, Vec<Warning>), OverlayError> {
    let mut warnings = Vec::new();
    let media = match p.dict.get(b"MediaBox").and_then(|o| rect(doc, o)) {
        Some(m) => m,
        None => {
            warnings.push(Warning::MissingMediaBox { page });
            DEFAULT_MEDIA
        }
    };
    let visible = match p.dict.get(b"CropBox") {
        None => media,
        Some(o) => match rect(doc, o) {
            None => {
                warnings.push(Warning::UnreadableCropBox { page });
                media
            }
            Some(c) => {
                let r = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
                if r[2] > r[0] && r[3] > r[1] {
                    r
                } else {
                    warnings.push(Warning::CropOutsideMedia { page, crop: c, media });
                    media
                }
            }
        },
    };
    let f = frame(visible, p.rotation(doc), p.user_unit(doc)).map_err(|e| e.on_page(page))?;
    Ok((f, warnings))
}

/// A box as pdfcraft reads one: four finite numbers (other entries skipped), normalised.
fn rect(doc: &Document, o: &Object) -> Option<[f64; 4]> {
    let o = doc.resolve(o);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        [a, b, c, d] if v.iter().all(|x| x.is_finite()) => Some([a.min(*c), b.min(*d), a.max(*c), b.max(*d)]),
        _ => None,
    }
}

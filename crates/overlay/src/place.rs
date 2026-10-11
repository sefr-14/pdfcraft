//! Placement (overlay PRD, Page geometry): the output page and the matrices that put A and B on
//! it. A goes through its displayed frame only. B goes through its displayed frame, then the
//! confirmed sheet factor, then the registration. The output page is A's sheet unless it is
//! expanded to hold both; a side longer than the viewer limit gets an output `/UserUnit`.

use pdfcraft_content::Matrix;

use crate::{DisplayFrame, OverlayError, Point, Settings, SheetMatch};

/// Where A and B land on the output page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Output page width and height in points (as displayed).
    pub width: f64,
    pub height: f64,
    /// The output page's `/UserUnit`: 1 unless a side is longer than the viewer limit.
    pub unit: f64,
    /// A's user space → the output page's user space.
    pub a: Matrix,
    /// B's user space → the output page's user space.
    pub b: Matrix,
    /// B's sheet corners on the output page, in points, in the order (0, 0), (W, 0), (W, H),
    /// (0, H) of B's displayed sheet.
    pub b_outline: [Point; 4],
    /// Some of B lies outside the output page and will be clipped. Never true when expanded.
    pub b_clipped: bool,
}

impl Placement {
    /// The output page box `[0 0 w h]` in the output's user-space units.
    pub fn page_box(&self) -> [f64; 4] {
        [0.0, 0.0, self.width / self.unit, self.height / self.unit]
    }
}

fn finite(m: &Matrix) -> bool {
    m.0.iter().all(|v| v.is_finite())
}

/// Place A and B. `registration` maps B's displayed frame (after the sheet factor) onto A's:
/// a fitted or typed `Similarity`/`Affine` matrix, or the identity for none. With `expand`, the
/// output page grows to hold both sheets.
pub fn place(
    frame_a: &DisplayFrame,
    frame_b: &DisplayFrame,
    sheet: SheetMatch,
    registration: &Matrix,
    expand: bool,
    settings: &Settings,
) -> Result<Placement, OverlayError> {
    if !finite(registration) {
        return Err(OverlayError::NotFinite { what: "the registration" });
    }
    let (wa, ha) = frame_a.size();
    let (wb, hb) = frame_b.size();
    // B's displayed frame → A's displayed frame.
    let k = sheet.transform((wa, ha), (wb, hb))?;
    let g = k.then(registration);
    let corners = [Point::new(0.0, 0.0), Point::new(wb, 0.0), Point::new(wb, hb), Point::new(0.0, hb)].map(|c| c.transformed(&g));
    if !corners.iter().all(|c| c.is_finite()) {
        return Err(OverlayError::NotFinite { what: "B's placement" });
    }
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f64, 0.0f64, wa, ha);
    let shift = if expand {
        for c in &corners {
            (x0, y0, x1, y1) = (x0.min(c.x), y0.min(c.y), x1.max(c.x), y1.max(c.y));
        }
        Matrix::translate(-x0, -y0)
    } else {
        Matrix::IDENTITY
    };
    let (width, height) = (x1 - x0, y1 - y0);
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(OverlayError::NotFinite { what: "the output page size" });
    }
    // Rounding at a shared edge (identical sheets) isn't clipping: allow 1e-9 of the sheet.
    let eps = 1e-9 * frame_a.diagonal().max(1.0);
    let b_clipped = !expand && corners.iter().any(|c| c.x < -eps || c.y < -eps || c.x > wa + eps || c.y > ha + eps);
    let b_outline = corners.map(|c| c.transformed(&shift));
    // Over the viewer limit, the output gets a /UserUnit rounded up to 4 decimals, and both
    // matrices scale by its inverse.
    let longest = width.max(height);
    let unit = if longest > settings.max_page_units {
        let mut u = (longest / settings.max_page_units * 1e4).ceil() / 1e4;
        if longest / u > settings.max_page_units {
            u += 1e-4;
        }
        u
    } else {
        1.0
    };
    let to_units = Matrix([1.0 / unit, 0.0, 0.0, 1.0 / unit, 0.0, 0.0]);
    let a = frame_a.to_display.then(&shift).then(&to_units);
    let b = frame_b.to_display.then(&g).then(&shift).then(&to_units);
    if !(finite(&a) && finite(&b) && unit.is_finite()) {
        return Err(OverlayError::NotFinite { what: "the placement" });
    }
    Ok(Placement { width, height, unit, a, b, b_outline, b_clipped })
}

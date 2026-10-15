//! Sheet-size proposals (overlay PRD, Sheet-size matching). Off means 1:1. When Match sheet sizes
//! is on, [`propose`] says which case a page pair is in and where any factor comes from; nothing
//! is applied until it is confirmed as a [`SheetMatch`]. (The PRD had it off by default; the
//! wizard turns it on since OV6.6, where the same sheet is 1:1 with nothing to confirm.)

use pdfcraft_content::Matrix;

use crate::{OverlayError, Settings};

/// How two displayed sheets compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetCase {
    /// Both sides agree within the tolerance: 1:1, nothing to confirm.
    SameSheet,
    /// The same proportions: a uniform factor to confirm.
    SameShape,
    /// Different proportions: choose 1:1, a uniform fit or reference points.
    DifferentShape,
}

/// A proposal for one page pair, from the displayed sheet sizes only.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetProposal {
    pub case: SheetCase,
    /// A's width over B's width.
    pub ratio_w: f64,
    /// A's height over B's height.
    pub ratio_h: f64,
    /// How far apart the two ratios are: `max / min − 1`.
    pub mismatch: f64,
    /// The factor for B: 1 for the same sheet, the geometric mean of the ratios for the same
    /// shape (decision 4), and the uniform fit for a different shape.
    pub factor: f64,
    /// The largest uniform factor that keeps B inside A: `min(ratio_w, ratio_h)`.
    pub fit: f64,
    /// B's sides, swapped, would match A's sheet or shape: B looks turned 90° (decision 5).
    /// Nothing is turned automatically.
    pub turned: bool,
    /// Where the proposal comes from, in words for the wizard.
    pub note: String,
}

impl SheetProposal {
    /// What to apply if the proposal is accepted as it stands. A different shape has no single
    /// answer, so the choice stays with you.
    pub fn suggested(&self) -> Option<SheetMatch> {
        match self.case {
            SheetCase::SameSheet => Some(SheetMatch::Off),
            SheetCase::SameShape => Some(SheetMatch::Scaled { factor: self.factor }),
            SheetCase::DifferentShape => None,
        }
    }
}

/// What has been confirmed for a page pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SheetMatch {
    /// 1:1 with the lower-left corners together and no scaling: Match sheet sizes off (the
    /// default), or the same sheet.
    Off,
    /// B scaled by `factor` and centred on A (decision 3). Reference points correct it afterwards.
    Scaled { factor: f64 },
}

impl SheetMatch {
    /// The transform this applies to B's displayed frame, for sheets of `a` and `b` (width,
    /// height) in points.
    pub fn transform(&self, a: (f64, f64), b: (f64, f64)) -> Result<Matrix, OverlayError> {
        match *self {
            SheetMatch::Off => Ok(Matrix::IDENTITY),
            SheetMatch::Scaled { factor } => {
                check_size(a, "A's sheet")?;
                check_size(b, "B's sheet")?;
                if !factor.is_finite() {
                    return Err(OverlayError::NotFinite { what: "the sheet factor" });
                }
                if factor <= 0.0 {
                    return Err(OverlayError::NotPositive { what: "the sheet factor", value: factor });
                }
                let (tx, ty) = ((a.0 - factor * b.0) / 2.0, (a.1 - factor * b.1) / 2.0);
                let m = Matrix([factor, 0.0, 0.0, factor, tx, ty]);
                if !m.0.iter().all(|v| v.is_finite()) {
                    return Err(OverlayError::NotFinite { what: "the sheet transform" });
                }
                Ok(m)
            }
        }
    }
}

fn check_size((w, h): (f64, f64), what: &'static str) -> Result<(), OverlayError> {
    if !(w.is_finite() && h.is_finite()) {
        return Err(OverlayError::NotFinite { what });
    }
    if w <= 0.0 {
        return Err(OverlayError::NotPositive { what, value: w });
    }
    if h <= 0.0 {
        return Err(OverlayError::NotPositive { what, value: h });
    }
    Ok(())
}

/// Ratios, mismatch and case for sheets `a` and `b`, without the turned check.
fn classify(a: (f64, f64), b: (f64, f64), s: &Settings) -> (SheetCase, f64, f64, f64) {
    let (rw, rh) = (a.0 / b.0, a.1 / b.1);
    let mismatch = rw.max(rh) / rw.min(rh) - 1.0;
    let case = if (a.0 - b.0).abs() <= s.same_sheet_pt && (a.1 - b.1).abs() <= s.same_sheet_pt {
        SheetCase::SameSheet
    } else if mismatch <= s.same_shape {
        SheetCase::SameShape
    } else {
        SheetCase::DifferentShape
    };
    (case, rw, rh, mismatch)
}

/// Compare A's displayed sheet `a` with B's `b`, both (width, height) in points.
pub fn propose(a: (f64, f64), b: (f64, f64), s: &Settings) -> Result<SheetProposal, OverlayError> {
    check_size(a, "A's sheet")?;
    check_size(b, "B's sheet")?;
    let (case, ratio_w, ratio_h, mismatch) = classify(a, b, s);
    if !(ratio_w.is_finite() && ratio_h.is_finite() && mismatch.is_finite() && ratio_w > 0.0 && ratio_h > 0.0) {
        return Err(OverlayError::NotFinite { what: "the sheet ratios" });
    }
    let fit = ratio_w.min(ratio_h);
    let factor = match case {
        SheetCase::SameSheet => 1.0,
        SheetCase::SameShape => (ratio_w * ratio_h).sqrt(),
        SheetCase::DifferentShape => fit,
    };
    let turned = case == SheetCase::DifferentShape && classify(a, (b.1, b.0), s).0 != SheetCase::DifferentShape;
    let pct = |x: f64| x * 100.0;
    let sizes = format!("A {:.1} × {:.1} pt, B {:.1} × {:.1} pt", a.0, a.1, b.0, b.1);
    let mut note = match case {
        SheetCase::SameSheet => format!("Same sheet ({sizes}): 1:1, nothing to confirm."),
        SheetCase::SameShape => format!(
            "Same shape ({sizes}): the width ratio {ratio_w:.4} and height ratio {ratio_h:.4} agree within {:.2} %, so B can be scaled by {factor:.4} and centred on A. \
             A sheet-size factor is a hint, not proof: the smaller sheet may have been plotted at another scale or with other margins, so check it in the preview.",
            pct(s.same_shape)
        ),
        SheetCase::DifferentShape => format!(
            "Different shape ({sizes}): the width ratio {ratio_w:.4} and height ratio {ratio_h:.4} differ by {:.2} %. \
             Choose 1:1, a uniform fit of {fit:.4} (B centred on A), or reference points.",
            pct(mismatch)
        ),
    };
    if turned {
        note.push_str(" B's sheet matches A's turned 90°. Nothing is turned automatically: use reference points, or enter a 90° rotation by hand.");
    }
    Ok(SheetProposal { case, ratio_w, ratio_h, mismatch, factor, fit, turned, note })
}

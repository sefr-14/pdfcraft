//! Every limit the overlay uses, in one place. The defaults are the starting values in the
//! overlay PRD; all of them can be changed.

/// Limits for sheet-size proposals, registration and placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Same sheet: both displayed sides agree within this many points (0.5).
    pub same_sheet_pt: f64,
    /// Same shape: the width and height ratios agree within this fraction (0.005, i.e. 0.5 %).
    pub same_shape: f64,
    /// Similarity fit: the picks on each document must spread (RMS distance from their centroid)
    /// at least this fraction of that document's sheet diagonal (0.05).
    pub min_spread: f64,
    /// Affine fit: B's picks must have a narrow principal spread of at least this fraction of
    /// B's sheet diagonal (0.01).
    pub min_narrow_spread: f64,
    /// Smallest plausible scale, for the similarity scale and both affine singular values (0.05).
    pub min_scale: f64,
    /// Largest plausible scale (20).
    pub max_scale: f64,
    /// Absolute flag: a leave-one-out residual above this many millimetres on A's sheet (0.5).
    pub flag_mm: f64,
    /// The robust flag applies from this many points (5).
    pub robust_from: usize,
    /// Robust flag: a modified z-score above this flags a point (3.5, Iglewicz and Hoaglin).
    pub robust_z: f64,
    /// The robust test is skipped when the MAD is 0 to numerical precision: at or below
    /// `min(mad_floor_diagonal × A's diagonal, mad_floor_tolerance × flag tolerance)`
    /// (decision of 11 Oct: 1e-9 and 0.01, so at most 0.005 mm with the 0.5 mm tolerance).
    pub mad_floor_diagonal: f64,
    pub mad_floor_tolerance: f64,
    /// Identification: an exclusion is plausible when the worst remaining misfit is under
    /// `separation × flag tolerance` (1.5, so 0.75 mm; decision of 11 Oct).
    pub separation: f64,
    /// Largest output page side in user-space units before the output needs a `/UserUnit`
    /// (14,400, the 200-inch limit of Acrobat and most viewers).
    pub max_page_units: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            same_sheet_pt: 0.5,
            same_shape: 0.005,
            min_spread: 0.05,
            min_narrow_spread: 0.01,
            min_scale: 0.05,
            max_scale: 20.0,
            flag_mm: 0.5,
            robust_from: 5,
            robust_z: 3.5,
            mad_floor_diagonal: 1e-9,
            mad_floor_tolerance: 0.01,
            separation: 1.5,
            max_page_units: 14_400.0,
        }
    }
}

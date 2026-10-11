//! Registration from picked point pairs (overlay PRD, Registration): the default similarity fit
//! (shift, rotation, one uniform scale), refusals that say what failed and by how much, and the
//! residual report with leave-one-out flags. No point is ever removed automatically.

use std::fmt;

use pdfcraft_content::Matrix;

use crate::{PT_PER_MM, Settings};

/// A position in a displayed frame, in points.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    pub fn dist(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    pub fn transformed(self, m: &Matrix) -> Point {
        let (x, y) = m.apply(self.x, self.y);
        Point { x, y }
    }
}

/// The same feature picked on both documents: `b` in B's displayed frame (after any confirmed
/// sheet factor) and `a` in A's displayed frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointPair {
    pub b: Point,
    pub a: Point,
}

impl PointPair {
    pub const fn new(b: Point, a: Point) -> Self {
        PointPair { b, a }
    }
}

/// Which document a refusal is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doc {
    A,
    B,
}

impl fmt::Display for Doc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Doc::A => "A",
            Doc::B => "B",
        })
    }
}

/// Why a set of picks can't be fitted. Each carries the numbers the wizard needs to explain it.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// Fewer pairs than the fit needs.
    TooFewPairs { have: usize, need: usize },
    /// Pair `pair` (0-based) has a coordinate that isn't a finite number.
    NotFinite { pair: usize },
    /// The sheet diagonal given for `doc` isn't a finite, positive length.
    BadSheet { doc: Doc, diagonal: f64 },
    /// The picks on `doc` spread less than the minimum (RMS distance from their centroid).
    SpreadTooSmall { doc: Doc, spread: f64, minimum: f64 },
    /// The fitted scale (or an affine singular value) is outside the plausible range.
    ScaleOutOfRange { scale: f64, min: f64, max: f64 },
    /// Affine: B's picks lie too close to one line (narrow principal spread below the minimum).
    NarrowSpreadTooSmall { spread: f64, minimum: f64 },
    /// Affine: the fit would mirror B (determinant zero or negative; decision 6).
    Mirrored { determinant: f64 },
    /// The arithmetic overflowed (coordinates far beyond any sheet).
    Overflow,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::TooFewPairs { have, need } => write!(f, "Pick at least {need} point pairs; there are {have} so far."),
            Refusal::NotFinite { pair } => write!(f, "Point pair {} isn't a usable position.", pair.saturating_add(1)),
            Refusal::BadSheet { doc, diagonal } => write!(f, "The size of sheet {doc} ({diagonal} pt across) can't be used."),
            Refusal::SpreadTooSmall { doc, spread, minimum } => write!(
                f,
                "Your points on {doc} lie within {spread:.1} pt of their centre; spread them over at least {minimum:.1} pt so the fit is reliable."
            ),
            Refusal::ScaleOutOfRange { scale, min, max } => write!(
                f,
                "These picks would scale B by {scale:.4}, outside the plausible range {min} to {max}. Check that each pair picks the same feature on both documents."
            ),
            Refusal::NarrowSpreadTooSmall { spread, minimum } => write!(
                f,
                "Your points on B lie too close to one line for affine mode ({spread:.1} pt across it; at least {minimum:.1} pt needed). Add a point away from that line."
            ),
            Refusal::Mirrored { determinant } => write!(
                f,
                "These picks would mirror B (determinant {determinant:.4}). That usually means a mis-pick, or points picked in a different order on the two documents."
            ),
            Refusal::Overflow => write!(f, "These positions are too far out to compute with."),
        }
    }
}

impl std::error::Error for Refusal {}

/// What a fit checks against: both sheets' diagonals (in the same frames as the picks) and the
/// limits in [`Settings`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// A's displayed sheet diagonal, in points.
    pub diag_a: f64,
    /// B's displayed sheet diagonal after any confirmed sheet factor, in points.
    pub diag_b: f64,
    pub settings: Settings,
}

impl Limits {
    fn check(&self) -> Result<(), Refusal> {
        for (doc, d) in [(Doc::A, self.diag_a), (Doc::B, self.diag_b)] {
            if !(d.is_finite() && d > 0.0) {
                return Err(Refusal::BadSheet { doc, diagonal: d });
            }
        }
        Ok(())
    }
}

/// A transform that can be fitted to point pairs.
pub trait Model: Sized + Clone {
    /// The fewest pairs that determine the transform.
    const MIN_PAIRS: usize;
    /// Least-squares fit, or the reason it is refused.
    fn solve(pairs: &[PointPair], limits: &Limits) -> Result<Self, Refusal>;
    /// B's displayed frame → A's displayed frame.
    fn matrix(&self) -> Matrix;
}

/// Shift, rotation and one uniform scale: a point p of B goes to `scale · R(rotation) · p + (tx, ty)`
/// on A. Shear, reflection and separate X and Y scales can't occur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub scale: f64,
    /// Radians, counter-clockwise, in (−π, π].
    pub rotation: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Similarity {
    pub const IDENTITY: Similarity = Similarity { scale: 1.0, rotation: 0.0, tx: 0.0, ty: 0.0 };

    /// The transform for typed values (the wizard's numeric fields), checked to be finite.
    pub fn new(scale: f64, rotation: f64, tx: f64, ty: f64) -> Option<Self> {
        [scale, rotation, tx, ty].iter().all(|v| v.is_finite()).then_some(Similarity { scale, rotation, tx, ty })
    }
}

impl Model for Similarity {
    const MIN_PAIRS: usize = 2;

    /// The 2D Umeyama solution with points as complex numbers (b on B, a on A):
    /// `z = Σ conj(b − b̄)(a − ā) / Σ |b − b̄|²`, `scale = |z|`, `rotation = arg z`, `t = ā − z b̄`.
    fn solve(pairs: &[PointPair], limits: &Limits) -> Result<Self, Refusal> {
        let c = centred(pairs, Self::MIN_PAIRS, limits)?;
        let s = &limits.settings;
        let (sb, sa) = (c.spread_b(), c.spread_a());
        for (doc, spread, diag) in [(Doc::B, sb, limits.diag_b), (Doc::A, sa, limits.diag_a)] {
            let minimum = s.min_spread * diag;
            if spread < minimum {
                return Err(Refusal::SpreadTooSmall { doc, spread, minimum });
            }
        }
        let (re, im) = (c.sum_ab_re / c.sum_bb, c.sum_ab_im / c.sum_bb);
        let scale = re.hypot(im);
        if !scale.is_finite() {
            return Err(Refusal::Overflow);
        }
        if !in_range(scale, s.min_scale, s.max_scale) {
            return Err(Refusal::ScaleOutOfRange { scale, min: s.min_scale, max: s.max_scale });
        }
        let rotation = im.atan2(re);
        let tx = c.mean_a.x - (re * c.mean_b.x - im * c.mean_b.y);
        let ty = c.mean_a.y - (im * c.mean_b.x + re * c.mean_b.y);
        if !(tx.is_finite() && ty.is_finite() && rotation.is_finite()) {
            return Err(Refusal::Overflow);
        }
        Ok(Similarity { scale, rotation, tx, ty })
    }

    fn matrix(&self) -> Matrix {
        let (sin, cos) = self.rotation.sin_cos();
        let (re, im) = (self.scale * cos, self.scale * sin);
        Matrix([re, im, -im, re, self.tx, self.ty])
    }
}

/// Relative slack at the ends of the plausible scale range: a numerical tolerance only (decision
/// of 11 Oct). A scale or singular value of exactly 0.05 or 20 carries rounding of up to about
/// 2.3e-12 relative after the affine fit; 1e-11 keeps such values inside, while 0.049 and 20.1
/// stay far outside.
pub const RANGE_SLACK: f64 = 1e-11;

/// Is `x` inside `[min, max]`, counting the ends as inside despite rounding ([`RANGE_SLACK`])?
fn in_range(x: f64, min: f64, max: f64) -> bool {
    x >= min * (1.0 - RANGE_SLACK) && x <= max * (1.0 + RANGE_SLACK)
}

/// The advanced affine fit: six parameters, a point (x, y) of B going to
/// `(a x + c y + e, b x + d y + f)` on A (PDF order). It changes the drawing's proportions, so the
/// wizard warns before using it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub Matrix);

impl Affine {
    /// The two singular values of the linear part, largest first: how much the fit stretches
    /// along its most and least stretched directions.
    pub fn singular_values(&self) -> (f64, f64) {
        singular_values(self.0)
    }
}

/// Singular values of the 2 × 2 linear part of `m`, in closed form: with E = (a + d)/2,
/// F = (a − d)/2, G = (b + c)/2, H = (b − c)/2, Q = hypot(E, H) and R = hypot(F, G), they are
/// Q + R and |Q − R|, and the determinant is Q² − R².
fn singular_values(m: Matrix) -> (f64, f64) {
    let [a, b, c, d, ..] = m.0;
    let (e, f, g, h) = ((a + d) / 2.0, (a - d) / 2.0, (b + c) / 2.0, (b - c) / 2.0);
    let (q, r) = (e.hypot(h), f.hypot(g));
    (q + r, (q - r).abs())
}

impl Model for Affine {
    const MIN_PAIRS: usize = 3;

    /// Least squares on centred points (b̃ = b − b̄, ã = a − ā), solved in the principal axes of
    /// B's picks (decision of 11 Oct): rotate b̃ by Rᵀ, where R turns the axes onto the
    /// eigenvectors of Σ b̃ b̃ᵀ, solve `ã ≈ K u` for u = Rᵀ b̃ with each axis divided by its own
    /// sum of squares, then `L = K Rᵀ` and `t = ā − L b̄`. This is the same least-squares
    /// solution as the normal equations `L = (Σ ã b̃ᵀ)(Σ b̃ b̃ᵀ)⁻¹`, without their determinant
    /// `Σb̃x² Σb̃y² − (Σb̃x b̃y)²`, which cancels when the picks lie nearly in a line.
    fn solve(pairs: &[PointPair], limits: &Limits) -> Result<Self, Refusal> {
        let c = centred(pairs, Self::MIN_PAIRS, limits)?;
        let s = &limits.settings;
        let theta = 0.5 * (2.0 * c.bxy).atan2(c.bxx - c.byy);
        let (sn, cs) = theta.sin_cos();
        let (mut u11, mut u22, mut u12) = (0.0, 0.0, 0.0);
        let (mut ax1, mut ax2, mut ay1, mut ay2) = (0.0, 0.0, 0.0, 0.0);
        for p in pairs {
            let (bx, by) = (p.b.x - c.mean_b.x, p.b.y - c.mean_b.y);
            let (u1, u2) = (cs * bx + sn * by, cs * by - sn * bx);
            let (ax, ay) = (p.a.x - c.mean_a.x, p.a.y - c.mean_a.y);
            u11 += u1 * u1;
            u22 += u2 * u2;
            u12 += u1 * u2;
            ax1 += ax * u1;
            ax2 += ax * u2;
            ay1 += ay * u1;
            ay2 += ay * u2;
        }
        // In these axes u12 is rounding-small, so this determinant doesn't cancel. The narrow
        // principal spread is the square root of the smaller eigenvalue of B's covariance,
        // computed as det / λ_max.
        let det = u11 * u22 - u12 * u12;
        let lambda_max = (u11 + u22) / 2.0 + ((u11 - u22) / 2.0).hypot(u12);
        let narrow = if lambda_max > 0.0 { (det / lambda_max / c.n).max(0.0).sqrt() } else { 0.0 };
        let minimum = s.min_narrow_spread * limits.diag_b;
        if !narrow.is_finite() {
            return Err(Refusal::Overflow);
        }
        if narrow < minimum {
            return Err(Refusal::NarrowSpreadTooSmall { spread: narrow, minimum });
        }
        let (k11, k12) = ((ax1 * u22 - ax2 * u12) / det, (ax2 * u11 - ax1 * u12) / det);
        let (k21, k22) = ((ay1 * u22 - ay2 * u12) / det, (ay2 * u11 - ay1 * u12) / det);
        let (a11, a12) = (k11 * cs - k12 * sn, k11 * sn + k12 * cs);
        let (a21, a22) = (k21 * cs - k22 * sn, k21 * sn + k22 * cs);
        let e = c.mean_a.x - (a11 * c.mean_b.x + a12 * c.mean_b.y);
        let f = c.mean_a.y - (a21 * c.mean_b.x + a22 * c.mean_b.y);
        let m = Matrix([a11, a21, a12, a22, e, f]);
        if !m.0.iter().all(|v| v.is_finite()) {
            return Err(Refusal::Overflow);
        }
        let determinant = a11 * a22 - a12 * a21;
        if determinant <= 0.0 {
            return Err(Refusal::Mirrored { determinant });
        }
        let (big, small) = singular_values(m);
        for scale in [small, big] {
            if !in_range(scale, s.min_scale, s.max_scale) {
                return Err(Refusal::ScaleOutOfRange { scale, min: s.min_scale, max: s.max_scale });
            }
        }
        Ok(Affine(m))
    }

    fn matrix(&self) -> Matrix {
        self.0
    }
}

/// Centroids and centred second moments of a set of pairs.
pub(crate) struct Centred {
    pub n: f64,
    pub mean_b: Point,
    pub mean_a: Point,
    /// Σ |b̃|²
    pub sum_bb: f64,
    /// Σ |ã|²
    pub sum_aa: f64,
    /// Σ Re(conj(b̃) ã) = Σ (b̃x ãx + b̃y ãy)
    pub sum_ab_re: f64,
    /// Σ Im(conj(b̃) ã) = Σ (b̃x ãy − b̃y ãx)
    pub sum_ab_im: f64,
    /// B's covariance sums Σ b̃x², Σ b̃y², Σ b̃x b̃y
    pub bxx: f64,
    pub byy: f64,
    pub bxy: f64,
}

impl Centred {
    pub fn spread_b(&self) -> f64 {
        (self.sum_bb / self.n).sqrt()
    }
    pub fn spread_a(&self) -> f64 {
        (self.sum_aa / self.n).sqrt()
    }
}

/// Checks shared by every fit, then the centred sums.
pub(crate) fn centred(pairs: &[PointPair], need: usize, limits: &Limits) -> Result<Centred, Refusal> {
    if pairs.len() < need {
        return Err(Refusal::TooFewPairs { have: pairs.len(), need });
    }
    if let Some(i) = pairs.iter().position(|p| !(p.a.is_finite() && p.b.is_finite())) {
        return Err(Refusal::NotFinite { pair: i });
    }
    limits.check()?;
    let n = pairs.len() as f64;
    let mean = |f: fn(&PointPair) -> Point| {
        let (sx, sy) = pairs.iter().fold((0.0, 0.0), |(sx, sy), p| (sx + f(p).x, sy + f(p).y));
        Point::new(sx / n, sy / n)
    };
    let (mean_b, mean_a) = (mean(|p| p.b), mean(|p| p.a));
    let mut c = Centred { n, mean_b, mean_a, sum_bb: 0.0, sum_aa: 0.0, sum_ab_re: 0.0, sum_ab_im: 0.0, bxx: 0.0, byy: 0.0, bxy: 0.0 };
    for p in pairs {
        let (bx, by) = (p.b.x - mean_b.x, p.b.y - mean_b.y);
        let (ax, ay) = (p.a.x - mean_a.x, p.a.y - mean_a.y);
        c.bxx += bx * bx;
        c.byy += by * by;
        c.bxy += bx * by;
        c.sum_aa += ax * ax + ay * ay;
        c.sum_ab_re += bx * ax + by * ay;
        c.sum_ab_im += bx * ay - by * ax;
    }
    c.sum_bb = c.bxx + c.byy;
    let sums = [c.mean_a.x, c.mean_a.y, c.mean_b.x, c.mean_b.y, c.sum_bb, c.sum_aa, c.sum_ab_re, c.sum_ab_im, c.bxy];
    if !sums.iter().all(|v| v.is_finite()) {
        return Err(Refusal::Overflow);
    }
    Ok(c)
}

/// Why a point is flagged. Neither flag removes it from the fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flag {
    /// Its leave-one-out residual exceeds the absolute tolerance on A's sheet.
    Absolute { residual_mm: f64, limit_mm: f64 },
    /// Its modified z-score over the leave-one-out residuals exceeds the limit (from 5 points).
    Robust { z: f64, limit: f64 },
}

/// One pick's line in the residual report.
#[derive(Clone, Debug, PartialEq)]
pub struct PointReport {
    /// Distance on A's sheet between where the fit puts `b` and where `a` was picked, in points.
    pub residual: f64,
    /// The same in millimetres.
    pub residual_mm: f64,
    /// Distance from where a fit without this pair puts it, in points. `None` when the other
    /// pairs can't be fitted on their own ("no check possible").
    pub leave_one_out: Option<f64>,
    /// RMS of the fit without this pair, over the other pairs, in points.
    pub rms_without: Option<f64>,
    pub flags: Vec<Flag>,
}

impl PointReport {
    pub fn flagged(&self) -> bool {
        !self.flags.is_empty()
    }
}

/// A fitted transform with its residual report. Every pair stays in the fit.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit<M> {
    pub transform: M,
    pub points: Vec<PointReport>,
    /// RMS residual over all pairs, in points (and millimetres).
    pub rms: f64,
    pub rms_mm: f64,
    /// Largest residual, in points (and millimetres).
    pub max: f64,
    pub max_mm: f64,
    /// Exactly as many pairs as the fit needs: it passes through them, so no residual check is
    /// possible. The wizard suggests another point.
    pub exact: bool,
}

/// Residuals of `m` over `pairs`: each, the RMS and the maximum, in points.
pub fn residuals(m: &Matrix, pairs: &[PointPair]) -> (Vec<f64>, f64, f64) {
    let each: Vec<f64> = pairs.iter().map(|p| p.b.transformed(m).dist(p.a)).collect();
    let n = each.len().max(1) as f64;
    let rms = (each.iter().map(|r| r * r).sum::<f64>() / n).sqrt();
    let max = each.iter().copied().fold(0.0, f64::max);
    (each, rms, max)
}

/// Fit `M` to `pairs` and build the residual report with leave-one-out flags.
pub fn fit<M: Model>(pairs: &[PointPair], limits: &Limits) -> Result<Fit<M>, Refusal> {
    let transform = M::solve(pairs, limits)?;
    Ok(report(transform, pairs, limits))
}

/// The residual report for a given transform, fitted or typed (the wizard's final nudge): the
/// residuals use `transform`, and the leave-one-out checks use fits of the picks themselves.
pub fn report<M: Model>(transform: M, pairs: &[PointPair], limits: &Limits) -> Fit<M> {
    let s = &limits.settings;
    let m = transform.matrix();
    let (each, rms, max) = residuals(&m, pairs);
    let n = pairs.len();
    // Leave-one-out needs the other pairs to determine a fit on their own.
    let loo: Vec<(Option<f64>, Option<f64>)> = if n > M::MIN_PAIRS {
        (0..n)
            .map(|i| {
                let others: Vec<PointPair> = pairs.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, p)| *p).collect();
                match (M::solve(&others, limits), pairs.get(i)) {
                    (Ok(t), Some(p)) => {
                        let mi = t.matrix();
                        (Some(p.b.transformed(&mi).dist(p.a)), Some(residuals(&mi, &others).1))
                    }
                    _ => (None, None),
                }
            })
            .collect()
    } else {
        vec![(None, None); n]
    };
    let limit_pt = s.flag_mm * PT_PER_MM;
    let available: Vec<f64> = loo.iter().filter_map(|(l, _)| *l).collect();
    let robust = if available.len() >= s.robust_from {
        let med = median(&available);
        let mad = median(&available.iter().map(|l| (l - med).abs()).collect::<Vec<_>>());
        (mad > 0.0 && mad.is_finite()).then_some((med, mad))
    } else {
        None
    };
    let points = each
        .iter()
        .zip(loo.iter())
        .map(|(&residual, &(leave_one_out, rms_without))| {
            let mut flags = Vec::new();
            if let Some(l) = leave_one_out {
                if l > limit_pt {
                    flags.push(Flag::Absolute { residual_mm: l / PT_PER_MM, limit_mm: s.flag_mm });
                }
                if let Some((med, mad)) = robust {
                    let z = 0.6745 * (l - med) / mad;
                    if z > s.robust_z {
                        flags.push(Flag::Robust { z, limit: s.robust_z });
                    }
                }
            }
            PointReport { residual, residual_mm: residual / PT_PER_MM, leave_one_out, rms_without, flags }
        })
        .collect();
    Fit { transform, points, rms, rms_mm: rms / PT_PER_MM, max, max_mm: max / PT_PER_MM, exact: n == M::MIN_PAIRS }
}

/// Median of a non-empty slice (0 when empty). NaN never reaches it: residuals are finite.
fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    match (s.get(n / 2), n % 2) {
        (Some(&hi), 1) => hi,
        (Some(&hi), _) => s.get(n / 2 - 1).map_or(hi, |&lo| (lo + hi) / 2.0),
        (None, _) => 0.0,
    }
}

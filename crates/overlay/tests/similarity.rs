//! The similarity fit (OV2.3): acceptance tests 7, 8, 9, 12, 13 and 14, large coordinates for
//! check B, and the registration part of test 16.

mod common;

use std::f64::consts::PI;

use common::{Rng, assert_close};
use pdfcraft_overlay::register::Model;
use pdfcraft_overlay::{Doc, Flag, Limits, PT_PER_MM, Point, PointPair, Refusal, Settings, Similarity, fit, report};

const SHEET: (f64, f64) = (2592.0, 1728.0); // ARCH D landscape

fn diag((w, h): (f64, f64)) -> f64 {
    w.hypot(h)
}

fn limits(diag_a: f64, diag_b: f64) -> Limits {
    Limits { diag_a, diag_b, settings: Settings::default() }
}

/// The true transform as (scale, rotation, tx, ty), applied to a point.
fn apply(t: (f64, f64, f64, f64), p: Point) -> Point {
    let (s, r, tx, ty) = t;
    let (sin, cos) = r.sin_cos();
    Point::new(s * (cos * p.x - sin * p.y) + tx, s * (sin * p.x + cos * p.y) + ty)
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(2.0 * PI);
    if d > PI { d - 2.0 * PI } else { d }
}

/// `n` points spread over a sheet of `size`, or along one line across it when `line`.
fn points(rng: &mut Rng, n: usize, size: (f64, f64), line: bool) -> Vec<Point> {
    loop {
        let pts: Vec<Point> = if line {
            let (x0, y0, x1, y1) =
                (rng.range(0.0, 0.2) * size.0, rng.range(0.0, 1.0) * size.1, rng.range(0.8, 1.0) * size.0, rng.range(0.0, 1.0) * size.1);
            (0..n).map(|_| rng.unit()).map(|t| Point::new(x0 + t * (x1 - x0), y0 + t * (y1 - y0))).collect()
        } else {
            (0..n).map(|_| Point::new(rng.range(0.0, size.0), rng.range(0.0, size.1))).collect()
        };
        // Keep sets that clear the minimum spread with room to spare.
        let c = Point::new(pts.iter().map(|p| p.x).sum::<f64>() / n as f64, pts.iter().map(|p| p.y).sum::<f64>() / n as f64);
        let spread = (pts.iter().map(|p| p.dist(c).powi(2)).sum::<f64>() / n as f64).sqrt();
        if spread > 0.1 * diag(size) {
            return pts;
        }
    }
}

/// Test 7: recovery of 50 known transforms from exact pairs, 2, 3, 10 and 50 of them, spread out
/// or along one line. Shift within 1e-6 pt, rotation within 1e-9 rad, scale within 1e-9.
#[test]
fn similarity_recovers_known_transforms() {
    let mut rng = Rng(7);
    let (mut worst_shift, mut worst_rot, mut worst_scale) = (0.0f64, 0.0f64, 0.0f64);
    for k in 0..50 {
        let scale = (rng.range(0.1f64.ln(), 10f64.ln())).exp();
        let rotation = match k {
            0 => PI,
            1 => -PI + 1e-12,
            2 => 0.0,
            _ => rng.range(-PI, PI),
        };
        let truth = (scale, rotation, rng.range(-5000.0, 5000.0), rng.range(-5000.0, 5000.0));
        for n in [2, 3, 10, 50] {
            for line in [false, true] {
                let pairs: Vec<PointPair> = points(&mut rng, n, SHEET, line).into_iter().map(|b| PointPair::new(b, apply(truth, b))).collect();
                let f =
                    fit::<Similarity>(&pairs, &limits(scale * diag(SHEET), diag(SHEET))).unwrap_or_else(|e| panic!("transform {k}, {n} pairs: {e}"));
                let t = f.transform;
                let what = format!("transform {k}, {n} pairs{}", if line { " in a line" } else { "" });
                let shift = (t.tx - truth.2).abs().max((t.ty - truth.3).abs());
                let rot = angle_diff(t.rotation, truth.1).abs();
                let sc = (t.scale - truth.0).abs();
                assert!(shift <= 1e-6, "{what}: shift off by {shift}");
                assert!(rot <= 1e-9, "{what}: rotation off by {rot}");
                assert!(sc <= 1e-9, "{what}: scale off by {sc}");
                assert_eq!(f.exact, n == 2, "{what}");
                worst_shift = worst_shift.max(shift);
                worst_rot = worst_rot.max(rot);
                worst_scale = worst_scale.max(sc);
            }
        }
    }
    println!("WORST test7 shift={worst_shift:e} pt rotation={worst_rot:e} rad scale={worst_scale:e}");
}

/// Test 8: the fit has one scale, no shear and no mirror, even for mirrored or sheared picks.
#[test]
fn similarity_never_shears_or_mirrors() {
    let mut rng = Rng(8);
    let bs = points(&mut rng, 8, SHEET, false);
    let mirror = |p: Point| Point::new(SHEET.0 - p.x, p.y);
    let shear = |p: Point| Point::new(p.x + 0.3 * p.y, p.y);
    for (name, f) in [("mirrored", &mirror as &dyn Fn(Point) -> Point), ("sheared", &shear)] {
        let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, f(b))).collect();
        let fitted = fit::<Similarity>(&pairs, &limits(diag(SHEET), diag(SHEET))).unwrap_or_else(|e| panic!("{name}: {e}"));
        let [a, b, c, d, ..] = fitted.transform.matrix().0;
        assert_close(a, d, 1e-12, &format!("{name}: one scale"));
        assert_close(b, -c, 1e-12, &format!("{name}: no shear"));
        assert!(a * d - b * c > 0.0, "{name}: mirrored");
        assert!(fitted.max > 1.0, "{name}: such picks can't fit a similarity, so residuals show it ({})", fitted.max);
    }
}

/// Test 9: refusals, each with its reason, and acceptance just inside each limit.
#[test]
fn similarity_refusals() {
    let l = limits(1000.0, 1000.0);
    let p = |bx, by, ax, ay| PointPair::new(Point::new(bx, by), Point::new(ax, ay));
    assert_eq!(fit::<Similarity>(&[], &l).err(), Some(Refusal::TooFewPairs { have: 0, need: 2 }));
    assert_eq!(fit::<Similarity>(&[p(0.0, 0.0, 0.0, 0.0)], &l).err(), Some(Refusal::TooFewPairs { have: 1, need: 2 }));
    for bad in [f64::NAN, f64::INFINITY] {
        let r = fit::<Similarity>(&[p(0.0, 0.0, 0.0, 0.0), p(bad, 0.0, 100.0, 0.0)], &l);
        assert_eq!(r.err(), Some(Refusal::NotFinite { pair: 1 }), "{bad}");
    }
    // Two points ±r from their centre have spread r. The minimum is 5 % of 1000 = 50 pt.
    let two = |r: f64, s: f64| [p(500.0 - r, 500.0, 500.0 - s * r, 500.0), p(500.0 + r, 500.0, 500.0 + s * r, 500.0)];
    let below = 50.0 * (1.0 - 1e-9);
    let above = 50.0 * (1.0 + 1e-9);
    assert!(matches!(fit::<Similarity>(&two(below, 1.0), &l), Err(Refusal::SpreadTooSmall { doc: Doc::B, .. })));
    assert!(fit::<Similarity>(&two(above, 1.0), &l).is_ok());
    // A's spread: B spreads 100 pt, A the same at scale 1, against A's minimum of 5 % of A's diagonal.
    assert!(matches!(fit::<Similarity>(&two(100.0, 1.0), &limits(2000.0 * (1.0 + 1e-9), 1000.0)), Err(Refusal::SpreadTooSmall { doc: Doc::A, .. })));
    assert!(fit::<Similarity>(&two(100.0, 1.0), &limits(2000.0 * (1.0 - 1e-9), 1000.0)).is_ok());
    // Scale: 0.049 and 20.1 are refused, 0.051 and 19.9 accepted. A's diagonal follows the scale,
    // so only the scale decides.
    for (s, ok) in [(0.049, false), (0.051, true), (19.9, true), (20.1, false)] {
        let r = fit::<Similarity>(&two(100.0, s), &limits(1000.0 * s, 1000.0));
        assert_eq!(r.is_ok(), ok, "scale {s}: {r:?}");
        if !ok {
            assert!(matches!(r, Err(Refusal::ScaleOutOfRange { .. })), "scale {s}");
        }
    }
    // A sheet size that can't be used.
    assert!(matches!(fit::<Similarity>(&two(100.0, 1.0), &limits(f64::NAN, 1000.0)), Err(Refusal::BadSheet { doc: Doc::A, .. })));
    // Every refusal reads as a sentence.
    assert!(Refusal::SpreadTooSmall { doc: Doc::B, spread: 12.3, minimum: 61.2 }.to_string().contains("within 12.3 pt"));
}

/// Test 12: residuals, RMS and maximum match a case worked out by hand.
///
/// B: (0, 0), (100, 0), (100, 100), (0, 100). A: the same, but the third point picked 2 pt to the
/// right, at (102, 100). With complex numbers: b̄ = 50 + 50i, ā = 50.5 + 50i, Σ|b̃|² = 20000,
/// Σ conj(b̃)ã = 20100 − 100i, so z = 1.005 − 0.005i and t = ā − z b̄ = 0.
/// z·b = 0, 100.5 − 0.5i, 101 + 100i, 0.5 + 100.5i, so the residuals are 0, √0.5, 1 and √0.5,
/// the RMS is √(2/4) = √0.5 and the maximum is 1.
#[test]
fn residuals_match_a_hand_computation() {
    let b = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    let a = [(0.0, 0.0), (100.0, 0.0), (102.0, 100.0), (0.0, 100.0)];
    let pairs: Vec<PointPair> = b.iter().zip(a.iter()).map(|(&(bx, by), &(ax, ay))| PointPair::new(Point::new(bx, by), Point::new(ax, ay))).collect();
    let f = fit::<Similarity>(&pairs, &limits(200.0, 200.0)).expect("fit");
    let m = f.transform.matrix().0;
    for (got, want) in m.iter().zip([1.005, -0.005, 0.005, 1.005, 0.0, 0.0]) {
        assert_close(*got, want, 1e-12, "matrix");
    }
    let half = 0.5f64.sqrt();
    for (r, want) in f.points.iter().map(|p| p.residual).zip([0.0, half, 1.0, half]) {
        assert_close(r, want, 1e-12, "residual");
    }
    assert_close(f.rms, half, 1e-12, "RMS");
    assert_close(f.max, 1.0, 1e-12, "max");
    assert_close(f.max_mm, 25.4 / 72.0, 1e-12, "max in mm");
}

/// Gaussian noise with standard deviation `sd` per axis (Box–Muller).
fn noise(rng: &mut Rng, sd: f64) -> Point {
    let (u, v) = (rng.unit().max(1e-300), rng.unit());
    let r = sd * (-2.0 * u.ln()).sqrt();
    Point::new(r * (2.0 * PI * v).cos(), r * (2.0 * PI * v).sin())
}

/// `n` noisy pairs under a random similarity, with pair `bad` picked `off_mm` away on A.
fn planted(rng: &mut Rng, n: usize, bad: usize, off_mm: f64) -> (Vec<PointPair>, Limits) {
    let truth = (rng.range(0.5, 2.0), rng.range(-PI, PI), rng.range(-500.0, 500.0), rng.range(-500.0, 500.0));
    let bs = points(rng, n, SHEET, false);
    let dir = rng.range(-PI, PI);
    let pairs = bs
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let mut a = apply(truth, b);
            let e = noise(rng, 0.1 * PT_PER_MM);
            a = Point::new(a.x + e.x, a.y + e.y);
            if i == bad {
                a = Point::new(a.x + off_mm * PT_PER_MM * dir.cos(), a.y + off_mm * PT_PER_MM * dir.sin());
            }
            PointPair::new(b, a)
        })
        .collect();
    (pairs, limits(truth.0 * diag(SHEET), diag(SHEET)))
}

/// Test 13, the parts that hold under the approved flag rules: with 3, 5 and 10 points, 0.1 mm
/// noise and one pick 2 mm off, every point stays in the fit and is reported. With 5 and 10
/// points the bad pick is flagged in every one of 500 seeded trials, with the RMS without it
/// reported.
///
/// OPEN (step 2 PRD, row 13, decision pending): "exactly that pick flagged" is not met by the
/// approved rules (good picks beside a bad one are flagged too), and with 3 points no rule can
/// single the bad pick out. With 3 points, a bad pick whose two partners lie close together
/// gets "no check possible". Measurements are in the OV2 results.
#[test]
fn a_planted_bad_pick_is_flagged_and_kept() {
    let mut rng = Rng(13);
    for n in [3, 5, 10] {
        for trial in 0..500 {
            let bad = trial % n;
            let (pairs, l) = planted(&mut rng, n, bad, 2.0);
            let f = fit::<Similarity>(&pairs, &l).expect("fit");
            assert_eq!(f.points.len(), n, "every point reported");
            assert_eq!(f.transform, Similarity::solve(&pairs, &l).expect("solve"), "every point stays in the fit");
            if n >= 5 {
                let p = &f.points[bad];
                assert!(p.flagged(), "{n} points, trial {trial}: the bad pick isn't flagged: {p:?}");
                assert!(p.rms_without.is_some(), "{n} points: RMS without the bad pick");
            }
        }
    }
}

/// Test 14: flag edge cases.
#[test]
fn flag_edge_cases() {
    let l = limits(diag(SHEET), diag(SHEET));
    let mut rng = Rng(14);
    // Exact picks: every leave-one-out residual is 0, the MAD is 0, the robust test is skipped,
    // and nothing is flagged.
    let bs = points(&mut rng, 6, SHEET, false);
    let exact: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b)).collect();
    let f = fit::<Similarity>(&exact, &l).expect("fit");
    assert!(f.points.iter().all(|p| p.flags.is_empty()), "{f:?}");
    // Four points: the robust test isn't applied, only the absolute one.
    let (pairs, l4) = planted(&mut rng, 4, 0, 2.0);
    let f = fit::<Similarity>(&pairs, &l4).expect("fit");
    assert!(f.points.iter().flat_map(|p| p.flags.iter()).all(|fl| matches!(fl, Flag::Absolute { .. })), "{f:?}");
    // Two points: the fit passes through them; no residual check is possible.
    let f = fit::<Similarity>(&exact[..2], &l).expect("fit");
    assert!(f.exact && f.points.iter().all(|p| p.leave_one_out.is_none()));
    // A reduced set that can't be fitted: without the far point, the other two lie 1 pt apart,
    // below the minimum spread. That point gets "no check possible", the others still a number.
    let close = [Point::new(1000.0, 800.0), Point::new(1001.0, 800.0), Point::new(2500.0, 1600.0)];
    let pairs: Vec<PointPair> = close.iter().map(|&b| PointPair::new(b, b)).collect();
    let f = fit::<Similarity>(&pairs, &l).expect("fit");
    assert!(f.points[2].leave_one_out.is_none(), "{f:?}");
    assert!(f.points[0].leave_one_out.is_some() && f.points[1].leave_one_out.is_some(), "{f:?}");
}

/// Typed values (the wizard's final nudge) get residuals too.
#[test]
fn typed_values_get_residuals() {
    let pairs = [PointPair::new(Point::new(0.0, 0.0), Point::new(10.0, 0.0)), PointPair::new(Point::new(1000.0, 0.0), Point::new(1010.0, 0.0))];
    let typed = Similarity::new(1.0, 0.0, 9.0, 0.0).expect("finite");
    let f = report(typed, &pairs, &limits(1000.0, 1000.0));
    assert_close(f.rms, 1.0, 1e-12, "1 pt short");
    assert!(Similarity::new(f64::NAN, 0.0, 0.0, 0.0).is_none());
}

/// Check B for the similarity fit: large coordinates. Sheets up to 1,000,000 pt: shift within
/// 1e-6 pt and every sheet corner within 1e-6 pt of its exact image. Larger sheets, up to the
/// 1.08e9 pt /UserUnit allows: within 1e-12 of the sheet diagonal.
#[test]
fn similarity_is_stable_at_large_coordinates() {
    let mut rng = Rng(0xB1);
    let mut worst = Vec::new();
    for side in [14_400.0, 100_000.0, 1_000_000.0, 1.0e7, 1.08e9] {
        let size = (side, side * 0.7);
        let mut worst_corner = 0.0f64;
        for _ in 0..20 {
            let truth = (rng.range(0.5, 2.0), rng.range(-PI, PI), rng.range(-1.0, 1.0) * side, rng.range(-1.0, 1.0) * side);
            let n = rng.pick(&[2, 3, 10]);
            let pairs: Vec<PointPair> = points(&mut rng, n, size, false).into_iter().map(|b| PointPair::new(b, apply(truth, b))).collect();
            let t = fit::<Similarity>(&pairs, &limits(truth.0 * diag(size), diag(size))).expect("fit").transform;
            let m = t.matrix();
            for c in [Point::new(0.0, 0.0), Point::new(size.0, 0.0), Point::new(size.0, size.1), Point::new(0.0, size.1)] {
                worst_corner = worst_corner.max(c.transformed(&m).dist(apply(truth, c)));
            }
            if side <= 1_000_000.0 {
                let shift = (t.tx - truth.2).abs().max((t.ty - truth.3).abs());
                assert!(shift <= 1e-6, "side {side}: shift off by {shift}");
            }
        }
        let mark = if side <= 1_000_000.0 { 1e-6 } else { 1e-12 * diag(size) };
        assert!(worst_corner <= mark, "side {side}: a corner is off by {worst_corner} (mark {mark})");
        worst.push(format!("{side:e}:{worst_corner:e}"));
    }
    println!("WORST checkB-similarity corner error by side: {}", worst.join(" "));
}

/// Test 16, registration part: 10,000 seeded sets, many hostile. Nothing panics.
#[test]
fn similarity_never_panics() {
    let mut rng = Rng(0x16);
    for _ in 0..10_000 {
        let n = rng.pick(&[0, 1, 2, 3, 5, 12]);
        let pairs: Vec<PointPair> = (0..n)
            .map(|_| {
                if rng.unit() < 0.5 {
                    let b = Point::new(rng.range(0.0, 3000.0), rng.range(0.0, 3000.0));
                    PointPair::new(b, Point::new(b.x + rng.range(-5.0, 5.0), b.y))
                } else {
                    PointPair::new(Point::new(rng.wild(), rng.wild()), Point::new(rng.wild(), rng.wild()))
                }
            })
            .collect();
        let l = limits(rng.pick(&[3000.0, 0.0, f64::NAN, 1e300, -5.0]), rng.pick(&[3000.0, f64::INFINITY, 1e-300]));
        if let Ok(f) = fit::<Similarity>(&pairs, &l) {
            assert!(f.rms.is_finite() || pairs.iter().any(|p| p.a.x.abs() > 1e150), "{f:?}");
        }
    }
}

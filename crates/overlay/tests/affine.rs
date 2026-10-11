//! The affine fit (OV2.4): acceptance tests 10 and 11, check B (numerical stability: nearly
//! collinear picks, extreme scales, large coordinates) and the affine part of test 16.
//!
//! Marks, fixed in the step 2 PRD before this ran: coefficients within 1e-9 of the largest
//! coefficient, shift within 1e-6 pt, every sheet corner within 1e-6 pt of its exact image for
//! sheets up to 1,000,000 pt; beyond that, corners within 1e-12 of the sheet diagonal.

mod common;

use std::f64::consts::PI;

use common::Rng;
use pdfcraft_content::Matrix;
use pdfcraft_overlay::register::Model;
use pdfcraft_overlay::{Affine, Limits, Point, PointPair, Refusal, Settings, fit};

const SHEET: (f64, f64) = (2592.0, 1728.0);

fn diag((w, h): (f64, f64)) -> f64 {
    w.hypot(h)
}

fn limits(diag_b: f64) -> Limits {
    Limits { diag_a: diag_b, diag_b, settings: Settings::default() }
}

/// L = R(θ1) · diag(s1, s2) · R(θ2) plus a shift, as a PDF matrix.
fn affine(s1: f64, s2: f64, t1: f64, t2: f64, e: f64, f: f64) -> Matrix {
    let r = |t: f64| {
        let (s, c) = t.sin_cos();
        [[c, -s], [s, c]]
    };
    let (r1, r2) = (r(t1), r(t2));
    let mid = [[s1 * r2[0][0], s1 * r2[0][1]], [s2 * r2[1][0], s2 * r2[1][1]]];
    let l = [
        [r1[0][0] * mid[0][0] + r1[0][1] * mid[1][0], r1[0][0] * mid[0][1] + r1[0][1] * mid[1][1]],
        [r1[1][0] * mid[0][0] + r1[1][1] * mid[1][0], r1[1][0] * mid[0][1] + r1[1][1] * mid[1][1]],
    ];
    Matrix([l[0][0], l[1][0], l[0][1], l[1][1], e, f])
}

/// How far `got` is from `truth`: (coefficient error relative to the largest coefficient,
/// shift error in pt, worst sheet-corner error in pt).
fn errors(got: Matrix, truth: Matrix, size: (f64, f64)) -> (f64, f64, f64) {
    let [a, b, c, d, e, f] = got.0;
    let [ta, tb, tc, td, te, tf] = truth.0;
    let big = [ta, tb, tc, td].iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let coef = [a - ta, b - tb, c - tc, d - td].iter().fold(0.0f64, |m, v| m.max(v.abs())) / big;
    let shift = (e - te).abs().max((f - tf).abs());
    let corner = [(0.0, 0.0), (size.0, 0.0), (size.0, size.1), (0.0, size.1)]
        .iter()
        .map(|&(x, y)| Point::new(x, y).transformed(&got).dist(Point::new(x, y).transformed(&truth)))
        .fold(0.0, f64::max);
    (coef, shift, corner)
}

/// Picks along one line across a sheet with a narrow principal spread of exactly `narrow`:
/// positions ±t along the line, offsets ±narrow across it, arranged so the two don't correlate.
fn near_line(size: (f64, f64), narrow: f64, n_pairs_of_points: usize, angle: f64) -> Vec<Point> {
    let (cx, cy) = (size.0 / 2.0, size.1 / 2.0);
    let half = 0.45 * diag(size);
    let (s, c) = angle.sin_cos();
    let mut pts = Vec::new();
    for k in 0..n_pairs_of_points {
        let t = half * (k as f64 + 1.0) / n_pairs_of_points as f64;
        let o = if k % 2 == 0 { narrow } else { -narrow };
        for (tt, oo) in [(t, o), (-t, o)] {
            pts.push(Point::new(cx + tt * c - oo * s, cy + tt * s + oo * c));
        }
    }
    pts
}

/// Test 10: sheared and stretched transforms recovered from 3, 4 and 10 exact pairs.
#[test]
fn affine_recovers_known_transforms() {
    let mut rng = Rng(10);
    let (mut wc, mut ws) = (0.0f64, 0.0f64);
    for k in 0..50 {
        let truth = affine(
            rng.range(0.3, 3.0),
            rng.range(0.3, 3.0),
            rng.range(-PI, PI),
            rng.range(-PI, PI),
            rng.range(-5000.0, 5000.0),
            rng.range(-5000.0, 5000.0),
        );
        for n in [3, 4, 10] {
            let bs: Vec<Point> = loop {
                let p: Vec<Point> = (0..n).map(|_| Point::new(rng.range(0.0, SHEET.0), rng.range(0.0, SHEET.1))).collect();
                if Affine::solve(&p.iter().map(|&b| PointPair::new(b, b)).collect::<Vec<_>>(), &limits(diag(SHEET))).is_ok() {
                    break p;
                }
            };
            let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b.transformed(&truth))).collect();
            let f = fit::<Affine>(&pairs, &limits(diag(SHEET))).unwrap_or_else(|e| panic!("transform {k}, {n} pairs: {e}"));
            let (coef, shift, _) = errors(f.transform.0, truth, SHEET);
            assert!(coef <= 1e-9, "transform {k}, {n} pairs: coefficients off by {coef}");
            assert!(shift <= 1e-6, "transform {k}, {n} pairs: shift off by {shift}");
            assert_eq!(f.exact, n == 3);
            // Leave-one-out needs 4 or more pairs, and then exists exactly when the other pairs
            // can be fitted on their own ("no check possible" otherwise).
            for (i, p) in f.points.iter().enumerate() {
                let others: Vec<PointPair> = pairs.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, q)| *q).collect();
                let fittable = n >= 4 && Affine::solve(&others, &limits(diag(SHEET))).is_ok();
                assert_eq!(p.leave_one_out.is_some(), fittable, "transform {k}, {n} pairs, pair {i}");
            }
            wc = wc.max(coef);
            ws = ws.max(shift);
        }
    }
    println!("WORST test10 coefficients={wc:e} shift={ws:e} pt");
}

/// Test 11: refusals, each with its reason.
#[test]
fn affine_refusals() {
    let l = limits(diag(SHEET));
    let p = |x: f64, y: f64| Point::new(x, y);
    let id = |pts: &[Point]| pts.iter().map(|&b| PointPair::new(b, b)).collect::<Vec<_>>();
    assert_eq!(fit::<Affine>(&id(&[p(0.0, 0.0), p(100.0, 0.0)]), &l).err(), Some(Refusal::TooFewPairs { have: 2, need: 3 }));
    // Three pairs in a line.
    assert!(matches!(fit::<Affine>(&id(&[p(0.0, 0.0), p(1000.0, 500.0), p(2000.0, 1000.0)]), &l), Err(Refusal::NarrowSpreadTooSmall { .. })));
    // A mirror image (decision 6).
    let tri = [p(100.0, 100.0), p(2400.0, 200.0), p(1300.0, 1600.0), p(500.0, 1200.0)];
    let mirrored: Vec<PointPair> = tri.iter().map(|&b| PointPair::new(b, Point::new(SHEET.0 - b.x, b.y))).collect();
    assert!(matches!(fit::<Affine>(&mirrored, &l), Err(Refusal::Mirrored { .. })));
    // A singular value out of range (decision 7), either end; the ends themselves are accepted.
    for (s1, s2, ok) in [(0.049, 1.0, false), (1.0, 20.1, false), (0.05, 1.0, true), (1.0, 20.0, true)] {
        let t = affine(s1, s2, 0.4, -1.1, 0.0, 0.0);
        let pairs: Vec<PointPair> = tri.iter().map(|&b| PointPair::new(b, b.transformed(&t))).collect();
        let r = fit::<Affine>(&pairs, &l);
        assert_eq!(r.is_ok(), ok, "singular values {s1}, {s2}: {r:?}");
        if !ok {
            assert!(matches!(r, Err(Refusal::ScaleOutOfRange { .. })));
        }
    }
    assert!(Refusal::Mirrored { determinant: -1.0 }.to_string().contains("mirror"));
}

/// Check B: nearly collinear picks at 0.9999, 1.0001, 2 and 10 times the 1 % limit. The first
/// is refused; the others recover the transform within the marks.
#[test]
fn check_b_nearly_collinear() {
    let mut rng = Rng(0xB2);
    let minimum = 0.01 * diag(SHEET);
    let mut worst = (0.0f64, 0.0f64, 0.0f64);
    for k in [0.9999, 1.0001, 2.0, 10.0] {
        for pairs_of_points in [2, 4] {
            for _ in 0..25 {
                let angle = rng.range(-PI, PI);
                let bs = near_line(SHEET, k * minimum, pairs_of_points, angle);
                let truth = affine(
                    rng.range(0.3, 3.0),
                    rng.range(0.3, 3.0),
                    rng.range(-PI, PI),
                    rng.range(-PI, PI),
                    rng.range(-5000.0, 5000.0),
                    rng.range(-5000.0, 5000.0),
                );
                let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b.transformed(&truth))).collect();
                let r = Affine::solve(&pairs, &limits(diag(SHEET)));
                if k < 1.0 {
                    assert!(matches!(r, Err(Refusal::NarrowSpreadTooSmall { .. })), "{k}× the limit must be refused: {r:?}");
                    continue;
                }
                let got = r.unwrap_or_else(|e| panic!("{k}× the limit must be accepted: {e}"));
                let (coef, shift, corner) = errors(got.0, truth, SHEET);
                assert!(coef <= 1e-9 && shift <= 1e-6 && corner <= 1e-6, "{k}×: coefficients {coef:e}, shift {shift:e}, corner {corner:e}");
                worst = (worst.0.max(coef), worst.1.max(shift), worst.2.max(corner));
            }
        }
    }
    println!("WORST checkB-collinear coefficients={:e} shift={:e} corner={:e}", worst.0, worst.1, worst.2);
}

/// Check B: extreme scales, singular values at the ends of the plausible range, with shear and
/// rotation from random turns on both sides.
#[test]
fn check_b_extreme_scales() {
    let mut rng = Rng(0xB3);
    let mut worst = (0.0f64, 0.0f64, 0.0f64);
    for (s1, s2) in [(0.05, 0.05), (20.0, 20.0), (0.05, 20.0), (20.0, 0.05)] {
        for _ in 0..50 {
            let truth = affine(s1, s2, rng.range(-PI, PI), rng.range(-PI, PI), rng.range(-5000.0, 5000.0), rng.range(-5000.0, 5000.0));
            let n = rng.pick(&[3, 4, 10]);
            let bs: Vec<Point> = (0..n).map(|_| Point::new(rng.range(0.0, SHEET.0), rng.range(0.0, SHEET.1))).collect();
            let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b.transformed(&truth))).collect();
            let Ok(got) = Affine::solve(&pairs, &limits(diag(SHEET))) else {
                // Random points can land too close to a line; that refusal is test 11's business.
                continue;
            };
            let (coef, shift, corner) = errors(got.0, truth, SHEET);
            assert!(coef <= 1e-9 && shift <= 1e-6 && corner <= 1e-6, "({s1}, {s2}): coefficients {coef:e}, shift {shift:e}, corner {corner:e}");
            worst = (worst.0.max(coef), worst.1.max(shift), worst.2.max(corner));
        }
    }
    println!("WORST checkB-scales coefficients={:e} shift={:e} corner={:e}", worst.0, worst.1, worst.2);
}

/// Check B: large coordinates, alone and combined with the worst cases above: nearly collinear
/// picks at 1.0001 × the limit and singular values 0.05 and 20.
#[test]
fn check_b_large_coordinates() {
    let mut rng = Rng(0xB4);
    let mut report = Vec::new();
    for side in [14_400.0, 100_000.0, 1_000_000.0, 1.0e7, 1.08e9] {
        let size = (side, side * 0.7);
        let near = side <= 1_000_000.0;
        let corner_mark = if near { 1e-6 } else { 1e-12 * diag(size) };
        let mut worst = (0.0f64, 0.0f64, 0.0f64);
        for case in 0..60 {
            let (s1, s2) = if case % 3 == 0 { (0.05, 20.0) } else { (rng.range(0.5, 2.0), rng.range(0.5, 2.0)) };
            let truth = affine(s1, s2, rng.range(-PI, PI), rng.range(-PI, PI), rng.range(-1.0, 1.0) * side, rng.range(-1.0, 1.0) * side);
            let bs = if case % 2 == 0 {
                near_line(size, 1.0001 * 0.01 * diag(size), rng.pick(&[2, 4]), rng.range(-PI, PI))
            } else {
                (0..rng.pick(&[4, 10])).map(|_| Point::new(rng.range(0.0, size.0), rng.range(0.0, size.1))).collect()
            };
            let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b.transformed(&truth))).collect();
            let Ok(got) = Affine::solve(&pairs, &limits(diag(size))) else { continue };
            let (coef, shift, corner) = errors(got.0, truth, size);
            assert!(coef <= 1e-9, "side {side}, case {case}: coefficients off by {coef:e}");
            if near {
                assert!(shift <= 1e-6, "side {side}, case {case} ({s1}, {s2}): shift off by {shift:e}");
            }
            assert!(corner <= corner_mark, "side {side}, case {case} ({s1}, {s2}): a corner is off by {corner:e} (mark {corner_mark:e})");
            worst = (worst.0.max(coef), worst.1.max(shift), worst.2.max(corner));
        }
        report.push(format!("{side:e}: coef {:e} shift {:e} corner {:e} (mark {corner_mark:e})", worst.0, worst.1, worst.2));
    }
    println!("WORST checkB-large {}", report.join(" | "));
}

/// Test 16, affine part: 10,000 seeded sets, many hostile. Nothing panics.
#[test]
fn affine_never_panics() {
    let mut rng = Rng(0xA16);
    for _ in 0..10_000 {
        let n = rng.pick(&[0, 2, 3, 4, 7]);
        let pairs: Vec<PointPair> = (0..n)
            .map(|_| {
                if rng.unit() < 0.5 {
                    let b = Point::new(rng.range(0.0, 3000.0), rng.range(0.0, 3000.0));
                    PointPair::new(b, Point::new(b.x * rng.range(-2.0, 2.0), b.y + rng.wild().clamp(-1e6, 1e6)))
                } else {
                    PointPair::new(Point::new(rng.wild(), rng.wild()), Point::new(rng.wild(), rng.wild()))
                }
            })
            .collect();
        let l = Limits { diag_a: rng.pick(&[3000.0, f64::NAN]), diag_b: rng.pick(&[3000.0, 0.0, 1e300]), settings: Settings::default() };
        let _ = fit::<Affine>(&pairs, &l);
    }
}

/// The old method (normal equations), kept here only as the reference for the documented
/// comparison of 11 Oct. The crate solves in B's principal axes instead.
fn normal_equations(pairs: &[PointPair]) -> Matrix {
    let n = pairs.len() as f64;
    let mb = Point::new(pairs.iter().map(|p| p.b.x).sum::<f64>() / n, pairs.iter().map(|p| p.b.y).sum::<f64>() / n);
    let ma = Point::new(pairs.iter().map(|p| p.a.x).sum::<f64>() / n, pairs.iter().map(|p| p.a.y).sum::<f64>() / n);
    let (mut bxx, mut byy, mut bxy, mut axbx, mut axby, mut aybx, mut ayby) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pairs {
        let (bx, by, ax, ay) = (p.b.x - mb.x, p.b.y - mb.y, p.a.x - ma.x, p.a.y - ma.y);
        bxx += bx * bx;
        byy += by * by;
        bxy += bx * by;
        axbx += ax * bx;
        axby += ax * by;
        aybx += ay * bx;
        ayby += ay * by;
    }
    let det = bxx * byy - bxy * bxy;
    let (a11, a12) = ((axbx * byy - axby * bxy) / det, (axby * bxx - axbx * bxy) / det);
    let (a21, a22) = ((aybx * byy - ayby * bxy) / det, (ayby * bxx - aybx * bxy) / det);
    Matrix([a11, a21, a12, a22, ma.x - (a11 * mb.x + a12 * mb.y), ma.y - (a21 * mb.x + a22 * mb.y)])
}

/// Relative distance of the singular values of `m` from the range ends 0.05 and 20.
fn end_error(m: Matrix) -> f64 {
    let [a, b, c, d, ..] = m.0;
    let (e, f, g, h) = ((a + d) / 2.0, (a - d) / 2.0, (b + c) / 2.0, (b - c) / 2.0);
    let (q, r) = (e.hypot(h), f.hypot(g));
    let (small, big) = ((q - r).abs(), q + r);
    ((small - 0.05) / 0.05).abs().max(((big - 20.0) / 20.0).abs())
}

/// Check B's worst case, with the generator and seeds of the 11 Oct stress run: picks at
/// 1.0001 × the narrow-spread limit, singular values exactly 0.05 and 20, sheets from 14,400 pt
/// to 1.08e9 pt, 20,000 cases each. The crate (principal axes) must accept every case (the ends
/// of the range are inside) and meet every mark. The old normal equations are measured on the
/// same cases and printed for the record.
#[test]
fn check_b_worst_case_principal_axes_vs_normal_equations() {
    let mut lines = Vec::new();
    for side in [14_400.0f64, 1_000_000.0, 1.0e7, 1.08e9] {
        let size = (side, side * 0.7);
        let d = size.0.hypot(size.1);
        let near = side <= 1_000_000.0;
        let mark = if near { 1e-6 } else { 1e-12 * d };
        let mut rng = Rng(side as u64);
        let (mut pa_worst, mut pa_coef, mut pa_shift, mut pa_end) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let (mut ne_worst, mut ne_over, mut ne_shift_over, mut ne_end, mut ne_end_refused) = (0.0f64, 0, 0, 0.0f64, 0);
        let cases = 20_000;
        for case in 0..cases {
            let (s1, s2) = if case % 2 == 0 { (0.05, 20.0) } else { (20.0, 0.05) };
            let truth = affine(s1, s2, rng.range(-PI, PI), rng.range(-PI, PI), rng.range(-1.0, 1.0) * side, rng.range(-1.0, 1.0) * side);
            let bs = near_line(size, 1.0001 * 0.01 * d, rng.pick(&[2, 4]), rng.range(-PI, PI));
            let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b.transformed(&truth))).collect();

            let got = Affine::solve(&pairs, &limits(d)).unwrap_or_else(|e| panic!("side {side:e}, case {case}: refused: {e}"));
            let (coef, shift, corner) = errors(got.0, truth, size);
            assert!(coef <= 1e-9, "side {side:e}, case {case}: coefficients off by {coef:e}");
            if near {
                assert!(shift <= 1e-6, "side {side:e}, case {case}: shift off by {shift:e}");
            }
            assert!(corner <= mark, "side {side:e}, case {case}: corner off by {corner:e} (mark {mark:e})");
            pa_worst = pa_worst.max(corner);
            pa_coef = pa_coef.max(coef);
            pa_shift = pa_shift.max(shift);
            pa_end = pa_end.max(end_error(got.0));

            let old = normal_equations(&pairs);
            let (_, old_shift, old_corner) = errors(old, truth, size);
            ne_worst = ne_worst.max(old_corner);
            if old_corner > mark {
                ne_over += 1;
            }
            if near && old_shift > 1e-6 {
                ne_shift_over += 1;
            }
            let e = end_error(old);
            ne_end = ne_end.max(e);
            if e > pdfcraft_overlay::RANGE_SLACK {
                ne_end_refused += 1;
            }
        }
        lines.push(format!(
            "side {side:e} (mark {mark:e}): principal axes worst corner {pa_worst:e} ({:.2}× mark), coefficients {pa_coef:e}, shift {pa_shift:e}, end error {pa_end:e}, 0 over, 0 refused | normal equations worst corner {ne_worst:e} ({:.2}× mark), {ne_over}/{cases} over the corner mark, {ne_shift_over} over the shift mark, end error {ne_end:e}, {ne_end_refused} would be refused at the ends",
            pa_worst / mark,
            ne_worst / mark
        ));
    }
    println!("COMPARISON\n{}", lines.join("\n"));
}

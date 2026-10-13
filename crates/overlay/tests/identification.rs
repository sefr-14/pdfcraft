//! Row 13 as revised and approved on 11 Oct (inconsistency warning, separate from identifying a
//! probable wrong pick), and row 14's bounded MAD floor.
//!
//! The study reuses the original scenarios exactly: the generator of the first flag experiment,
//! seeds 1000 + n, 1,000 trials per size, 0.1 mm noise, one pick 2 mm off (pick t mod n in trial
//! t). Clean sets use the same generator without a bad pick: seeds 2000 + n with 0.1 mm noise,
//! 3000 + n without noise. Nothing is chosen after seeing results.

mod common;

use std::f64::consts::PI;

use common::Rng;
use pdfcraft_overlay::register::Model;
use pdfcraft_overlay::{Affine, Fit, Flag, Limits, PT_PER_MM, Point, PointPair, Settings, Similarity, Verdict, fit};

const SHEET: (f64, f64) = (2592.0, 1728.0);
const TAU: f64 = 0.5 * PT_PER_MM;

fn noise(rng: &mut Rng, sd: f64) -> Point {
    let (u, v) = (rng.unit().max(1e-300), rng.unit());
    let r = sd * (-2.0 * u.ln()).sqrt();
    Point::new(r * (2.0 * PI * v).cos(), r * (2.0 * PI * v).sin())
}

/// The original scenario generator, unchanged: a similarity of scale 0.5–2 and any rotation,
/// n picks spread over the sheet, noise on every pick, and pick `bad` moved 2 mm.
fn scenario(rng: &mut Rng, n: usize, bad: Option<usize>, noise_mm: f64) -> (Vec<PointPair>, f64) {
    let d = SHEET.0.hypot(SHEET.1);
    let (s, r) = (rng.range(0.5, 2.0), rng.range(-PI, PI));
    let bs: Vec<Point> = loop {
        let pts: Vec<Point> = (0..n).map(|_| Point::new(rng.range(0.0, SHEET.0), rng.range(0.0, SHEET.1))).collect();
        let c = Point::new(pts.iter().map(|p| p.x).sum::<f64>() / n as f64, pts.iter().map(|p| p.y).sum::<f64>() / n as f64);
        let spread = (pts.iter().map(|p| p.dist(c).powi(2)).sum::<f64>() / n as f64).sqrt();
        if spread > 0.1 * d {
            break pts;
        }
    };
    let dir = rng.range(-PI, PI);
    let pairs = bs
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let (sin, cos) = r.sin_cos();
            let mut a = Point::new(s * (cos * b.x - sin * b.y), s * (sin * b.x + cos * b.y));
            let e = noise(rng, noise_mm * PT_PER_MM);
            a = Point::new(a.x + e.x, a.y + e.y);
            if Some(i) == bad {
                a = Point::new(a.x + 2.0 * PT_PER_MM * dir.cos(), a.y + 2.0 * PT_PER_MM * dir.sin());
            }
            PointPair::new(b, a)
        })
        .collect();
    (pairs, s)
}

fn limits(scale: f64, separation: f64) -> Limits {
    let d = SHEET.0.hypot(SHEET.1);
    Limits { diag_a: scale * d, diag_b: d, settings: Settings { separation, ..Settings::default() } }
}

/// e₍ⱼ₎ for every pick, recomputed here independently of the crate's report.
fn exclusion_misfits<M: Model>(pairs: &[PointPair], l: &Limits) -> Vec<Option<f64>> {
    (0..pairs.len())
        .map(|j| {
            let others: Vec<PointPair> = pairs.iter().enumerate().filter(|(k, _)| *k != j).map(|(_, p)| *p).collect();
            M::solve(&others, l).ok().map(|t| {
                let m = t.matrix();
                others.iter().map(|p| p.b.transformed(&m).dist(p.a)).fold(0.0, f64::max)
            })
        })
        .collect()
}

/// G1, G2, G4 and G5 on one fit (G3 is checked on exact sets by the callers).
#[track_caller]
fn guarantees<M: Model + PartialEq + std::fmt::Debug>(f: &Fit<M>, pairs: &[PointPair], l: &Limits, what: &str) {
    // G1: every pick stays, unchanged, in the fit and the report.
    assert_eq!(f.points.len(), pairs.len(), "{what}: G1");
    assert_eq!(f.transform, M::solve(pairs, l).expect("solve"), "{what}: G1, the fit uses every pick");
    // G2: with m0 + 1 picks, never a probable wrong pick.
    if pairs.len() == M::MIN_PAIRS + 1 {
        assert!(!matches!(f.verdict, Verdict::ProbablyWrong { .. }), "{what}: G2 {:?}", f.verdict);
    }
    let s = l.settings.separation;
    match &f.verdict {
        // G4: the definition holds, recomputed independently.
        Verdict::ProbablyWrong { pair } => {
            let e = exclusion_misfits::<M>(pairs, l);
            assert!(e[*pair].is_some_and(|x| x <= TAU), "{what}: G4, e of the named pick {:?}", e[*pair]);
            for (j, ej) in e.iter().enumerate() {
                if j != *pair {
                    assert!(ej.is_none_or(|x| x >= s * TAU), "{what}: G4, pick {j} is plausible too ({ej:?})");
                }
            }
        }
        // G5: every plausible exclusion is listed.
        Verdict::Ambiguous { candidates } => {
            let e = exclusion_misfits::<M>(pairs, l);
            let plausible: Vec<usize> = e.iter().enumerate().filter(|(_, x)| x.is_some_and(|x| x < s * TAU)).map(|(j, _)| j).collect();
            assert_eq!(candidates, &plausible, "{what}: G5");
            assert!(candidates.len() >= 2, "{what}: G5");
        }
        _ => {}
    }
}

#[derive(Default, Debug)]
struct Tally {
    consistent: usize,
    add_pick: usize,
    right: usize,
    wrong: usize,
    ambiguous: usize,
    unexplained: usize,
    // Approved rule (flags), for the record.
    bad_flagged: usize,
    only_bad_flagged: usize,
    // Which approved flag fired anywhere in the set.
    any_absolute: usize,
    any_robust: usize,
}

impl Tally {
    fn flags<M>(&mut self, f: &Fit<M>) {
        if f.points.iter().any(|p| p.flags.iter().any(|x| matches!(x, Flag::Absolute { .. }))) {
            self.any_absolute += 1;
        }
        if f.points.iter().any(|p| p.flags.iter().any(|x| matches!(x, Flag::Robust { .. }))) {
            self.any_robust += 1;
        }
    }
    fn add(&mut self, v: &Verdict, bad: Option<usize>) {
        match v {
            Verdict::Consistent => self.consistent += 1,
            Verdict::AddPick => self.add_pick += 1,
            Verdict::ProbablyWrong { pair } if Some(*pair) == bad => self.right += 1,
            Verdict::ProbablyWrong { .. } => self.wrong += 1,
            Verdict::Ambiguous { .. } => self.ambiguous += 1,
            Verdict::Unexplained => self.unexplained += 1,
        }
    }
    fn warned(&self) -> usize {
        self.add_pick + self.right + self.wrong + self.ambiguous + self.unexplained
    }
}

const TRIALS: usize = 1000;

/// Run one model over the planted, clean-noisy and clean-exact scenarios for one n and one s.
fn study<M: Model + PartialEq + std::fmt::Debug>(name: &str, n: usize, s: f64) -> (Tally, Tally, Tally) {
    let (mut planted, mut clean, mut exact) = (Tally::default(), Tally::default(), Tally::default());
    let mut rng = Rng(1000 + n as u64);
    for t in 0..TRIALS {
        let bad = t % n;
        let (pairs, scale) = scenario(&mut rng, n, Some(bad), 0.1);
        let l = limits(scale, s);
        let Ok(f) = fit::<M>(&pairs, &l) else { continue };
        guarantees(&f, &pairs, &l, &format!("{name} planted n={n} s={s} trial {t}"));
        planted.add(&f.verdict, Some(bad));
        planted.flags(&f);
        let flagged: Vec<usize> = f.points.iter().enumerate().filter(|(_, p)| p.flagged()).map(|(i, _)| i).collect();
        if flagged.contains(&bad) {
            planted.bad_flagged += 1;
        }
        if flagged == [bad] {
            planted.only_bad_flagged += 1;
        }
    }
    let mut rng = Rng(2000 + n as u64);
    for t in 0..TRIALS {
        let (pairs, scale) = scenario(&mut rng, n, None, 0.1);
        let l = limits(scale, s);
        let Ok(f) = fit::<M>(&pairs, &l) else { continue };
        guarantees(&f, &pairs, &l, &format!("{name} clean n={n} s={s} trial {t}"));
        clean.add(&f.verdict, None);
        clean.flags(&f);
    }
    let mut rng = Rng(3000 + n as u64);
    for t in 0..TRIALS {
        let (pairs, scale) = scenario(&mut rng, n, None, 0.0);
        let l = limits(scale, s);
        let Ok(f) = fit::<M>(&pairs, &l) else { continue };
        guarantees(&f, &pairs, &l, &format!("{name} exact n={n} s={s} trial {t}"));
        // G3: exact picks are always consistent, with no flag at all.
        assert_eq!(f.verdict, Verdict::Consistent, "{name} exact n={n} trial {t}: G3");
        assert!(f.points.iter().all(|p| p.flags.is_empty()), "{name} exact n={n} trial {t}: G3 flags");
        exact.add(&f.verdict, None);
    }
    (planted, clean, exact)
}

/// The approved-rule results documented on 11 Oct (bad pick flagged, only the bad pick
/// flagged), per n, for similarity. Reproducing them shows the scenarios and seeds are unchanged.
const DOCUMENTED: [(usize, usize, usize); 6] = [(3, 963, 13), (4, 999, 5), (5, 1000, 0), (6, 1000, 0), (8, 1000, 15), (10, 1000, 153)];

/// Row 13 as revised: the study on the original scenarios, with G1–G5 asserted on every case,
/// the documented approved-rule results reproduced, and the agreed target checked for
/// similarity at s = 1.5: a good pick named as the wrong one in at most 1 % of the sets of each
/// scenario, planted and clean. The results are printed for the record (s = 1.25, 1.5 and 2.0;
/// affine for information).
#[test]
fn row13_study_original_scenarios() {
    let mut out = Vec::new();
    let mut misses = Vec::new();
    for (name, sizes) in [("similarity", vec![3, 4, 5, 6, 8, 10]), ("affine", vec![4, 5, 6, 8, 10])] {
        for &n in &sizes {
            for s in [1.25, 1.5, 2.0] {
                let (p, c, e) = if name == "similarity" { study::<Similarity>(name, n, s) } else { study::<Affine>(name, n, s) };
                if name == "similarity" {
                    if let Some(&(_, flagged, only)) = DOCUMENTED.iter().find(|d| d.0 == n) {
                        assert_eq!((p.bad_flagged, p.only_bad_flagged), (flagged, only), "n={n}: documented approved-rule results not reproduced");
                    }
                    if s == 1.5 && (p.wrong > TRIALS / 100 || c.wrong > TRIALS / 100) {
                        misses.push(format!("n={n}: planted {} / clean {} wrong identifications", p.wrong, c.wrong));
                    }
                }
                out.push(format!(
                    "ROW13 {name} n={n} s={s}: planted detect {} right {} wrong {} ambiguous {} unexplained {} add-pick {} missed {} | approved-rule bad-flagged {} only-bad {} | clean(0.1mm) warned {} (absolute {} robust {}) wrong-id {} ambiguous {} unexplained {} add-pick {} | exact warned {}",
                    p.warned(),
                    p.right,
                    p.wrong,
                    p.ambiguous,
                    p.unexplained,
                    p.add_pick,
                    p.consistent,
                    p.bad_flagged,
                    p.only_bad_flagged,
                    c.warned(),
                    c.any_absolute,
                    c.any_robust,
                    c.wrong,
                    c.ambiguous,
                    c.unexplained,
                    c.add_pick,
                    e.warned()
                ));
            }
        }
    }
    println!("{}", out.join("\n"));
    assert!(misses.is_empty(), "the 1 % target is missed: {misses:?}");
}

/// The alternative floor kept for future comparison only (review of 11 Oct): the same rule with
/// a cap of 1e-4 × τ instead of 0.01 × τ. The crate uses the approved bounded floor.
fn alternative_floor(diag_a: f64) -> f64 {
    (1e-9 * diag_a).min(1e-4 * TAU)
}

/// Row 14 (1) and (2): exact picks under transforms other than the identity raise no flag, on
/// ordinary and very large sheets, at extreme scales, for both fits. Prints how far the
/// rounding-driven MAD stays below the floor.
#[test]
fn row14_exact_picks_raise_no_rounding_flags() {
    let mut rng = Rng(0x14);
    let (mut worst_ratio, mut worst_alt_ratio) = (0.0f64, 0.0f64);
    let mut runs = 0;
    for side in [2592.0f64, 14_400.0, 1_000_000.0, 1.08e9] {
        let size = (side, side * 2.0 / 3.0);
        let d = size.0.hypot(size.1);
        for _ in 0..300 {
            let n = rng.pick(&[5usize, 6, 8, 10, 20]);
            let bs: Vec<Point> = (0..n).map(|_| Point::new(rng.range(0.0, size.0), rng.range(0.0, size.1))).collect();
            // Similarity at scales 0.05, 1 and 20.
            let scale = rng.pick(&[0.05, 1.0, 20.0]);
            let (sin, cos) = rng.range(-PI, PI).sin_cos();
            let (tx, ty) = (rng.range(-1.0, 1.0) * side, rng.range(-1.0, 1.0) * side);
            let pairs: Vec<PointPair> = bs
                .iter()
                .map(|&b| PointPair::new(b, Point::new(scale * (cos * b.x - sin * b.y) + tx, scale * (sin * b.x + cos * b.y) + ty)))
                .collect();
            let l = Limits { diag_a: scale * d, diag_b: d, settings: Settings::default() };
            if let Ok(f) = fit::<Similarity>(&pairs, &l) {
                assert!(
                    f.points.iter().all(|p| p.flags.is_empty()) && f.verdict == Verdict::Consistent,
                    "similarity side {side} scale {scale}: {f:?}"
                );
                if let Some(m) = f.mad {
                    worst_ratio = worst_ratio.max(m / f.mad_floor);
                    worst_alt_ratio = worst_alt_ratio.max(m / alternative_floor(l.diag_a));
                }
                runs += 1;
            }
            // Affine with singular values 0.05 and 20.
            let (s1, s2) = if rng.unit() < 0.5 { (0.05, 20.0) } else { (20.0, 0.05) };
            let (a1, a2) = (rng.range(-PI, PI), rng.range(-PI, PI));
            let (c1, n1, c2, n2) = (a1.cos(), a1.sin(), a2.cos(), a2.sin());
            // L = R(a1) diag(s1, s2) R(a2)
            let l11 = c1 * s1 * c2 - n1 * s2 * n2;
            let l12 = -c1 * s1 * n2 - n1 * s2 * c2;
            let l21 = n1 * s1 * c2 + c1 * s2 * n2;
            let l22 = -n1 * s1 * n2 + c1 * s2 * c2;
            let pairs: Vec<PointPair> =
                bs.iter().map(|&b| PointPair::new(b, Point::new(l11 * b.x + l12 * b.y + tx, l21 * b.x + l22 * b.y + ty))).collect();
            let l = Limits { diag_a: 20.0 * d, diag_b: d, settings: Settings::default() };
            if let Ok(f) = fit::<Affine>(&pairs, &l) {
                assert!(f.points.iter().all(|p| p.flags.is_empty()) && f.verdict == Verdict::Consistent, "affine side {side}: {f:?}");
                if let Some(m) = f.mad {
                    worst_ratio = worst_ratio.max(m / f.mad_floor);
                    worst_alt_ratio = worst_alt_ratio.max(m / alternative_floor(l.diag_a));
                }
                runs += 1;
            }
        }
    }
    println!(
        "ROW14 exact sets: {runs} fits, no flag; largest rounding MAD / floor = {worst_ratio:e}; for comparison, / alternative floor (cap 1e-4 τ) = {worst_alt_ratio:e}"
    );
}

/// Row 14 (3): a real deviation near the tolerance is not hidden by the floor. Exact picks
/// except one moved d × τ, on an ordinary sheet and on a 1.08e9 pt sheet. Reports how often the
/// robust test ran, how often the moved pick was flagged, and, for comparison, how often an
/// unbounded floor (1e-9 × A's diagonal) or the alternative floor (cap 1e-4 × τ) would have
/// skipped the robust test.
#[test]
fn row14_real_deviations_near_the_tolerance_are_not_hidden() {
    let mut out = Vec::new();
    for side in [2592.0f64, 1.08e9] {
        let size = (side, side * 2.0 / 3.0);
        let d = size.0.hypot(size.1);
        for frac in [0.5, 0.8, 0.9, 1.0, 1.1, 1.2] {
            let mut rng = Rng(0x14_00 + (frac * 10.0) as u64);
            let (mut cases, mut applied, mut flagged, mut unbounded_skips, mut min_mad_ratio, mut floor_skips) = (0, 0, 0, 0, f64::INFINITY, 0);
            let mut alternative_skips = 0;
            for t in 0..400 {
                let n = [5usize, 6, 8, 10][t % 4];
                let bad = t % n;
                let bs: Vec<Point> = (0..n).map(|_| Point::new(rng.range(0.0, size.0), rng.range(0.0, size.1))).collect();
                let (sin, cos) = rng.range(-PI, PI).sin_cos();
                let dir = rng.range(-PI, PI);
                let pairs: Vec<PointPair> = bs
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| {
                        let mut a = Point::new(cos * b.x - sin * b.y, sin * b.x + cos * b.y);
                        if i == bad {
                            a = Point::new(a.x + frac * TAU * dir.cos(), a.y + frac * TAU * dir.sin());
                        }
                        PointPair::new(b, a)
                    })
                    .collect();
                let l = Limits { diag_a: d, diag_b: d, settings: Settings::default() };
                let Ok(f) = fit::<Similarity>(&pairs, &l) else { continue };
                cases += 1;
                if f.robust_applied {
                    applied += 1;
                }
                if f.points[bad].flagged() {
                    flagged += 1;
                }
                if f.mad.is_some_and(|m| m <= 1e-9 * d) {
                    unbounded_skips += 1;
                }
                if f.mad.is_some_and(|m| m <= f.mad_floor) {
                    floor_skips += 1;
                }
                if f.mad.is_some_and(|m| m <= alternative_floor(d)) {
                    alternative_skips += 1;
                }
                // A deviation past the tolerance is always flagged, whatever the floor does.
                if frac >= 1.1 {
                    assert!(f.points[bad].flagged(), "side {side}, {frac}τ, set {t}: the moved pick isn't flagged");
                }
                if let Some(m) = f.mad {
                    min_mad_ratio = min_mad_ratio.min(m / f.mad_floor);
                }
            }
            // On sheets of ordinary size the floor never decides anything.
            if side <= 1_000_000.0 {
                assert_eq!(floor_skips, 0, "side {side}, {frac}τ: the floor skipped the robust test");
            }
            out.push(format!(
                "ROW14 deviation {frac}τ side {side:e}: {cases} sets, robust test ran {applied}, skipped by the bounded floor {floor_skips}, moved pick flagged {flagged}, an unbounded floor would skip the robust test in {unbounded_skips}, the alternative floor (cap 1e-4 τ) in {alternative_skips}; smallest MAD / bounded floor {min_mad_ratio:.3e}"
            ));
        }
    }
    println!("{}", out.join("\n"));
}

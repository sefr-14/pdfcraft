//! Sheet-size proposals (OV2.2): acceptance tests 5 and 6, and the sheet part of test 16.

mod common;

use common::{Rng, assert_close};
use pdfcraft_content::Matrix;
use pdfcraft_overlay::{OverlayError, Settings, SheetCase, SheetMatch, propose};

const ARCH_D: (f64, f64) = (1728.0, 2592.0); // 24 × 36 in
const ARCH_B: (f64, f64) = (864.0, 1296.0); // 12 × 18 in
const ANSI_D: (f64, f64) = (1584.0, 2448.0); // 22 × 34 in

fn s() -> Settings {
    Settings::default()
}

/// Test 5: the same-sheet tolerance, 0.5 pt per side, decides at 0.49 and 0.51 pt.
#[test]
fn same_sheet_within_half_a_point() {
    let p = propose(ARCH_D, (ARCH_D.0 + 0.49, ARCH_D.1 - 0.49), &s()).expect("proposal");
    assert_eq!(p.case, SheetCase::SameSheet);
    assert_eq!(p.factor, 1.0);
    assert_eq!(p.suggested(), Some(SheetMatch::Off));
    let p = propose(ARCH_D, (ARCH_D.0 + 0.51, ARCH_D.1), &s()).expect("proposal");
    assert_ne!(p.case, SheetCase::SameSheet, "0.51 pt is outside the same-sheet tolerance");
}

/// Test 5: the PRD's examples.
#[test]
fn prd_examples() {
    // ARCH D against ARCH B: same shape, 2.000.
    let p = propose(ARCH_D, ARCH_B, &s()).expect("proposal");
    assert_eq!(p.case, SheetCase::SameShape);
    assert_eq!((p.ratio_w, p.ratio_h, p.factor), (2.0, 2.0, 2.0));
    assert_eq!(p.suggested(), Some(SheetMatch::Scaled { factor: 2.0 }));
    assert!(!p.turned);
    // And the other way round: 0.5.
    assert_eq!(propose(ARCH_B, ARCH_D, &s()).expect("proposal").factor, 0.5);

    // ARCH D against ANSI D: different shape, ratios 1.091 and 1.059, uniform fit 1.059.
    let p = propose(ARCH_D, ANSI_D, &s()).expect("proposal");
    assert_eq!(p.case, SheetCase::DifferentShape);
    assert_close(p.ratio_w, 1728.0 / 1584.0, 1e-15, "width ratio");
    assert_close(p.ratio_h, 2592.0 / 2448.0, 1e-15, "height ratio");
    assert_eq!(format!("{:.3} {:.3}", p.ratio_w, p.ratio_h), "1.091 1.059");
    assert_close(p.fit, 2592.0 / 2448.0, 1e-15, "uniform fit");
    assert_eq!(p.factor, p.fit);
    assert_eq!(p.suggested(), None, "a different shape leaves the choice to you");
    assert!(p.note.contains("Choose 1:1"), "{}", p.note);
}

/// Test 5: the same-shape tolerance, 0.5 %, decides at 0.49 % and 0.51 %, whichever document
/// is A, with the geometric mean as the factor (decision 4).
#[test]
fn same_shape_within_half_a_percent() {
    let a = (1000.0, 1000.0);
    for (apart, case) in [(0.0049, SheetCase::SameShape), (0.0051, SheetCase::DifferentShape)] {
        // Width ratio 2, height ratio 2 × (1 + apart).
        let b = (500.0, 500.0 / (1.0 + apart));
        for (x, y) in [(a, b), (b, a)] {
            let p = propose(x, y, &s()).expect("proposal");
            assert_eq!(p.case, case, "{apart}");
            assert_close(p.mismatch, apart, 1e-12, "mismatch");
            if case == SheetCase::SameShape {
                assert_close(p.factor, (p.ratio_w * p.ratio_h).sqrt(), 1e-15, "geometric mean");
            }
        }
    }
}

/// Test 5: a sheet turned 90° is reported, and nothing is turned (decision 5).
#[test]
fn turned_sheets_are_noted_not_turned() {
    for b in [(ARCH_D.1, ARCH_D.0), (ARCH_B.1, ARCH_B.0)] {
        let p = propose(ARCH_D, b, &s()).expect("proposal");
        assert_eq!(p.case, SheetCase::DifferentShape, "{b:?}");
        assert!(p.turned, "{b:?}");
        assert!(p.note.contains("turned 90°"), "{}", p.note);
        assert_eq!(p.suggested(), None);
    }
    // ANSI D against ARCH D landscape is a different shape either way: not reported as turned.
    assert!(!propose(ANSI_D, (ARCH_D.1, ARCH_D.0), &s()).expect("proposal").turned);
    // Same sheets are never "turned", square ones included.
    assert!(!propose((500.0, 500.0), (500.0, 500.0), &s()).expect("proposal").turned);
}

/// Test 6: off means 1:1. No scaling and no shift, whatever the sheets.
#[test]
fn off_means_one_to_one() {
    for (a, b) in [(ARCH_D, ARCH_B), (ARCH_D, ANSI_D), ((1.0, 1.0), (14_400.0, 3.0))] {
        assert_eq!(SheetMatch::Off.transform(a, b), Ok(Matrix::IDENTITY));
    }
}

/// A confirmed factor scales B and centres it on A (decision 3).
#[test]
fn a_confirmed_factor_centres_b_on_a() {
    // ARCH B at 2.000 covers ARCH D exactly.
    let m = SheetMatch::Scaled { factor: 2.0 }.transform(ARCH_D, ARCH_B).expect("transform");
    assert_eq!(m.apply(0.0, 0.0), (0.0, 0.0));
    assert_eq!(m.apply(ARCH_B.0, ARCH_B.1), ARCH_D);
    // ANSI D fitted into ARCH D: equal margins left and right, none top and bottom.
    let fit = propose(ARCH_D, ANSI_D, &s()).expect("proposal").fit;
    let m = SheetMatch::Scaled { factor: fit }.transform(ARCH_D, ANSI_D).expect("transform");
    let (x0, y0) = m.apply(0.0, 0.0);
    let (x1, y1) = m.apply(ANSI_D.0, ANSI_D.1);
    assert_close(x0, ARCH_D.0 - x1, 1e-9, "left margin = right margin");
    assert!(x0 > 0.0);
    assert_close(y0, 0.0, 1e-9, "bottom");
    assert_close(y1, ARCH_D.1, 1e-9, "top");
}

#[test]
fn bad_sizes_and_factors_are_errors() {
    for bad in [(f64::NAN, 1.0), (1.0, f64::INFINITY), (0.0, 1.0), (1.0, -2.0)] {
        assert!(propose(bad, ARCH_D, &s()).is_err(), "{bad:?}");
        assert!(propose(ARCH_D, bad, &s()).is_err(), "{bad:?}");
        assert!(SheetMatch::Scaled { factor: 1.0 }.transform(bad, ARCH_D).is_err(), "{bad:?}");
    }
    for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(SheetMatch::Scaled { factor }.transform(ARCH_D, ARCH_B).is_err(), "{factor}");
    }
    assert!(matches!(propose((1e300, 1.0), (1e-300, 1.0), &s()), Err(OverlayError::NotFinite { .. })));
}

/// Test 16, sheet part: 10,000 seeded inputs, many hostile, never panic.
#[test]
fn sheet_functions_never_panic() {
    let mut rng = Rng(0x5_4ee7);
    for _ in 0..10_000 {
        let (a, b) = ((rng.wild(), rng.wild()), (rng.wild(), rng.wild()));
        if let Ok(p) = propose(a, b, &s()) {
            assert!(p.factor.is_finite() && p.factor > 0.0, "{p:?}");
            if let Some(m) = p.suggested() {
                let _ = m.transform(a, b);
            }
        }
        let _ = SheetMatch::Scaled { factor: rng.wild() }.transform(a, b);
    }
}

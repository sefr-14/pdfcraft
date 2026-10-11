//! Placement (OV2.5): acceptance tests 4, 6 (placement part) and 15, and the full test 16 across
//! every public function.

mod common;

use common::{Rng, assert_close, doc};
use pdfcraft_content::Matrix;
use pdfcraft_overlay::register::Model;
use pdfcraft_overlay::{Affine, Limits, Point, PointPair, Settings, SheetMatch, Similarity, fit, frame, frame_of_page, place, propose};

const TOL: f64 = 1e-9;

fn s() -> Settings {
    Settings::default()
}

#[track_caller]
fn assert_matrix_close(a: &Matrix, b: &Matrix, what: &str) {
    for (k, (x, y)) in a.0.iter().zip(b.0.iter()).enumerate() {
        assert_close(*x, *y, TOL, &format!("{what}: coefficient {k}"));
    }
}

/// Test 4: identical pages overlay with no offset. Same frame for A and B, no sheet factor, no
/// registration: B's placement equals A's within 1e-9 pt, for every rotation and unit.
#[test]
fn identical_pages_overlay_with_no_offset() {
    for rotation in [0, 90, 180, 270] {
        for unit in [1.0, 2.0] {
            let d = doc(&format!("/MediaBox [100 200 900 1300] /Rotate {rotation}"), &[&format!("/CropBox [150 260 650 1060] /UserUnit {unit}")]);
            let (fa, _) = frame_of_page(&d, 0).expect("frame");
            let (fb, _) = frame_of_page(&d, 0).expect("frame");
            let p = place(&fa, &fb, SheetMatch::Off, &Matrix::IDENTITY, false, &s()).expect("place");
            let what = format!("/Rotate {rotation}, /UserUnit {unit}");
            assert_matrix_close(&p.b, &p.a, &what);
            for (x, y) in [(150.0, 260.0), (650.0, 260.0), (650.0, 1060.0), (150.0, 1060.0), (321.5, 777.25)] {
                let (pa, pb) = (p.a.apply(x, y), p.b.apply(x, y));
                assert_close(pa.0, pb.0, TOL, &what);
                assert_close(pa.1, pb.1, TOL, &what);
            }
            assert!(!p.b_clipped, "{what}");
            assert_eq!((p.width, p.height, p.unit), (fa.width, fa.height, 1.0), "{what}");
        }
    }
}

/// The same drawing stored upright on A and turned on B (`/Rotate 90` over a landscape media
/// box) lands in the same place: the composition respects each page's own rotation.
#[test]
fn a_turned_page_lands_where_it_is_shown() {
    let a = doc("/MediaBox [0 0 600 800]", &[""]);
    let b = doc("/MediaBox [0 0 800 600] /Rotate 90", &[""]);
    let (fa, _) = frame_of_page(&a, 0).expect("frame");
    let (fb, _) = frame_of_page(&b, 0).expect("frame");
    assert_eq!(fa.size(), fb.size());
    let p = place(&fa, &fb, SheetMatch::Off, &Matrix::IDENTITY, false, &s()).expect("place");
    // B's user-space origin is shown at the top-left of a page turned 90° clockwise, which is
    // A's top-left corner, (0, 800) in A's user space and on the output page.
    let (x, y) = p.b.apply(0.0, 0.0);
    assert_close(x, 0.0, TOL, "x");
    assert_close(y, 800.0, TOL, "y");
    let (x, y) = p.a.apply(0.0, 800.0);
    assert_close(x, 0.0, TOL, "A x");
    assert_close(y, 800.0, TOL, "A y");
}

/// Test 6, placement part: with Match sheet sizes off, B keeps its own size and its lower-left
/// corner sits on A's, whatever the sheets.
#[test]
fn off_places_b_one_to_one() {
    let fa = frame([0.0, 0.0, 1728.0, 2592.0], 0, 1.0).expect("A");
    let fb = frame([0.0, 0.0, 864.0, 1296.0], 0, 1.0).expect("B");
    let p = place(&fa, &fb, SheetMatch::Off, &Matrix::IDENTITY, false, &s()).expect("place");
    let want = [(0.0, 0.0), (864.0, 0.0), (864.0, 1296.0), (0.0, 1296.0)];
    for (c, (x, y)) in p.b_outline.iter().zip(want) {
        assert_eq!((c.x, c.y), (x, y));
    }
    // Confirmed at 2.000, ARCH B covers ARCH D exactly.
    let scaled = propose(fa.size(), fb.size(), &s()).expect("proposal").suggested().expect("same shape");
    let p = place(&fa, &fb, scaled, &Matrix::IDENTITY, false, &s()).expect("place");
    let want = [(0.0, 0.0), (1728.0, 0.0), (1728.0, 2592.0), (0.0, 2592.0)];
    for (c, (x, y)) in p.b_outline.iter().zip(want) {
        assert_close(c.x, x, TOL, "x");
        assert_close(c.y, y, TOL, "y");
    }
    assert!(!p.b_clipped);
}

/// Test 15: the output is A's sheet; expansion takes the bounding box of both sheets; clipping
/// is reported only without expansion; a side over 14,400 gets a /UserUnit.
#[test]
fn output_page_expansion_clipping_and_unit() {
    let fa = frame([0.0, 0.0, 600.0, 800.0], 0, 1.0).expect("A");
    let fb = frame([0.0, 0.0, 600.0, 800.0], 0, 1.0).expect("B");
    let shift = Matrix::translate(-100.0, 50.0);

    let p = place(&fa, &fb, SheetMatch::Off, &shift, false, &s()).expect("place");
    assert_eq!((p.width, p.height, p.unit), (600.0, 800.0, 1.0));
    assert_eq!(p.page_box(), [0.0, 0.0, 600.0, 800.0]);
    assert!(p.b_clipped, "B sticks out left and top");
    assert_eq!(p.a.apply(0.0, 0.0), (0.0, 0.0));

    let p = place(&fa, &fb, SheetMatch::Off, &shift, true, &s()).expect("place");
    assert_eq!((p.width, p.height), (700.0, 850.0), "bounding box of [0, 600] × [0, 800] and [−100, 500] × [50, 850]");
    assert!(!p.b_clipped, "nothing is clipped once expanded");
    assert_eq!(p.a.apply(0.0, 0.0), (100.0, 0.0), "A's lower-left moves right by 100");
    assert_eq!(p.b.apply(0.0, 0.0), (0.0, 50.0), "B's lower-left is the new left edge");
    assert_eq!((p.b_outline[0].x, p.b_outline[0].y), (0.0, 50.0));

    // B inside A: not clipped.
    let small = frame([0.0, 0.0, 300.0, 400.0], 0, 1.0).expect("small B");
    let p = place(&fa, &small, SheetMatch::Off, &Matrix::translate(10.0, 10.0), false, &s()).expect("place");
    assert!(!p.b_clipped);

    // A 15,000 pt side: /UserUnit 1.0417, so the page box stays within 14,400 units.
    let long = frame([0.0, 0.0, 15_000.0, 1_000.0], 0, 1.0).expect("long A");
    let p = place(&long, &long, SheetMatch::Off, &Matrix::IDENTITY, false, &s()).expect("place");
    assert_eq!(p.unit, 1.0417);
    assert!(p.page_box()[2] <= 14_400.0, "{:?}", p.page_box());
    let (x, y) = p.a.apply(15_000.0, 1_000.0);
    assert_close(x, 15_000.0 / 1.0417, TOL, "x in output units");
    assert_close(y, 1_000.0 / 1.0417, TOL, "y in output units");
    // Exactly at the limit: no unit needed.
    let edge = frame([0.0, 0.0, 14_400.0, 1_000.0], 0, 1.0).expect("edge A");
    assert_eq!(place(&edge, &edge, SheetMatch::Off, &Matrix::IDENTITY, false, &s()).expect("place").unit, 1.0);
}

/// A fitted registration goes straight into the placement: a pick on B lands on its partner on A.
#[test]
fn a_fitted_registration_places_picks_on_their_partners() {
    let fa = frame([0.0, 0.0, 2592.0, 1728.0], 0, 1.0).expect("A");
    let fb = frame([0.0, 0.0, 2592.0, 1728.0], 0, 1.0).expect("B");
    let truth = Matrix([0.999, 0.02, -0.02, 0.999, 12.0, -7.0]);
    let bs = [Point::new(100.0, 100.0), Point::new(2400.0, 150.0), Point::new(2300.0, 1600.0), Point::new(200.0, 1500.0)];
    let pairs: Vec<PointPair> = bs.iter().map(|&b| PointPair::new(b, b.transformed(&truth))).collect();
    let l = Limits { diag_a: fa.diagonal(), diag_b: fb.diagonal(), settings: s() };
    let t = fit::<Similarity>(&pairs, &l).expect("fit").transform;
    let p = place(&fa, &fb, SheetMatch::Off, &t.matrix(), true, &s()).expect("place");
    for pair in &pairs {
        let on_b = pair.b.transformed(&p.b);
        let on_a = pair.a.transformed(&p.a);
        assert!(on_b.dist(on_a) < 1e-9, "{on_b:?} vs {on_a:?}");
    }
}

/// Test 16, complete: 10,000 seeded rounds through every public function with hostile inputs.
/// Nothing panics, and every result that comes back is finite.
#[test]
fn nothing_panics_anywhere() {
    let mut rng = Rng(0x1616);
    let rots = [0, 90, 180, 270, -90, 45, i64::MAX];
    for _ in 0..10_000 {
        let fa = frame([rng.wild(), rng.wild(), rng.wild(), rng.wild()], rng.pick(&rots), rng.wild());
        let fb = frame([rng.wild(), rng.wild(), rng.wild(), rng.wild()], rng.pick(&rots), rng.wild());
        let (Ok(fa), Ok(fb)) = (
            fa.or_else(|_| frame([0.0, 0.0, 600.0, 800.0], 0, 1.0)),
            fb.or_else(|_| frame([0.0, 0.0, rng.range(1.0, 5e4), rng.range(1.0, 5e4)], 90, 1.0)),
        ) else {
            continue;
        };
        let sheet = match propose(fa.size(), fb.size(), &s()) {
            Ok(p) => p.suggested().unwrap_or(SheetMatch::Scaled { factor: p.fit }),
            Err(_) => SheetMatch::Scaled { factor: rng.wild() },
        };
        let n = rng.pick(&[0, 2, 3, 5]);
        let pairs: Vec<PointPair> = (0..n).map(|_| PointPair::new(Point::new(rng.wild(), rng.wild()), Point::new(rng.wild(), rng.wild()))).collect();
        let l = Limits { diag_a: fa.diagonal(), diag_b: fb.diagonal(), settings: s() };
        let registration = match (rng.next_u64() % 4, fit::<Similarity>(&pairs, &l), Affine::solve(&pairs, &l)) {
            (0, Ok(f), _) => f.transform.matrix(),
            (1, _, Ok(a)) => a.matrix(),
            (2, _, _) => Matrix([rng.wild(), rng.wild(), rng.wild(), rng.wild(), rng.wild(), rng.wild()]),
            _ => Matrix::IDENTITY,
        };
        if let Ok(p) = place(&fa, &fb, sheet, &registration, rng.unit() < 0.5, &s()) {
            assert!(p.width.is_finite() && p.height.is_finite() && p.unit.is_finite() && p.unit >= 1.0, "{p:?}");
            assert!(p.a.0.iter().chain(p.b.0.iter()).all(|v| v.is_finite()), "{p:?}");
            assert!(p.page_box()[2] <= 14_400.0 * (1.0 + 1e-12) && p.page_box()[3] <= 14_400.0 * (1.0 + 1e-12), "{p:?}");
        }
    }
}

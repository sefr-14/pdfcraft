//! The displayed frame (OV2.1): acceptance tests 1 to 3, the frame part of test 16, and check A
//! (coordinate conventions) from the step 2 PRD.

mod common;

use common::{Rng, assert_close, doc};
use pdfcraft_content::Matrix;
use pdfcraft_model::pages;
use pdfcraft_overlay::frame::DEFAULT_MEDIA;
use pdfcraft_overlay::{OverlayError, Warning, frame, frame_of_page};

const MEDIA: [f64; 4] = [100.0, 200.0, 900.0, 1300.0];
/// Smaller than the media box, with a nonzero origin: 500 × 800 in user space.
const CROP: [f64; 4] = [150.0, 260.0, 650.0, 1060.0];
const TOL: f64 = 1e-9;

/// Check A's table, written from ISO 32000-2 ("the number of degrees by which the page shall be
/// rotated clockwise when displayed"), not from pdfcraft's code: where each corner of the
/// visible box lands in the displayed frame, for the user-space corners
/// (x0, y0), (x1, y0), (x1, y1), (x0, y1). `w` and `h` are the displayed width and height.
fn expected_corners(rotation: i64, w: f64, h: f64) -> [(f64, f64); 4] {
    match rotation {
        0 => [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)],
        90 => [(0.0, h), (0.0, 0.0), (w, 0.0), (w, h)],
        180 => [(w, h), (0.0, h), (0.0, 0.0), (w, 0.0)],
        270 => [(w, 0.0), (w, h), (0.0, h), (0.0, 0.0)],
        _ => panic!("not a quarter turn: {rotation}"),
    }
}

/// Where user space's "up" points on screen, as a unit vector.
fn expected_up(rotation: i64) -> (f64, f64) {
    match rotation {
        0 => (0.0, 1.0),
        90 => (1.0, 0.0),
        180 => (0.0, -1.0),
        270 => (-1.0, 0.0),
        _ => panic!("not a quarter turn: {rotation}"),
    }
}

fn corners(b: [f64; 4]) -> [(f64, f64); 4] {
    [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])]
}

/// Check A on one frame: every corner, the size, orientation and no mirror.
#[track_caller]
fn check_a(f: &pdfcraft_overlay::DisplayFrame, visible: [f64; 4], rotation: i64, unit: f64, what: &str) {
    let (uw, uh) = (visible[2] - visible[0], visible[3] - visible[1]);
    let (w, h) = if rotation % 180 == 0 { (uw * unit, uh * unit) } else { (uh * unit, uw * unit) };
    assert_close(f.width, w, TOL, &format!("{what}: width"));
    assert_close(f.height, h, TOL, &format!("{what}: height"));
    for (i, (&(x, y), &(ex, ey))) in corners(visible).iter().zip(expected_corners(rotation, w, h).iter()).enumerate() {
        let (dx, dy) = f.to_display.apply(x, y);
        assert_close(dx, ex, TOL, &format!("{what}: corner {i} x"));
        assert_close(dy, ey, TOL, &format!("{what}: corner {i} y"));
    }
    let [a, b, c, d, ..] = f.to_display.0;
    assert!(a * d - b * c > 0.0, "{what}: the frame mirrors the page (determinant {})", a * d - b * c);
    // The linear part applied to user space's up vector (0, 1) is (c, d).
    let (ux, uy) = expected_up(rotation);
    assert_close(c, ux * unit, TOL, &format!("{what}: up x"));
    assert_close(d, uy * unit, TOL, &format!("{what}: up y"));
}

#[test]
fn check_a_corner_table_for_every_rotation_and_unit() {
    for rotation in [0, 90, 180, 270] {
        for unit in [1.0, 2.0] {
            let f = frame(CROP, rotation, unit).expect("frame");
            check_a(&f, CROP, rotation, unit, &format!("/Rotate {rotation}, /UserUnit {unit}"));
        }
    }
}

/// Test 1 and check A through real page objects: inherited media box and rotation, a crop box
/// on the page, `/UserUnit` on the page. The frame must agree with pdfcraft's own reading.
#[test]
fn frame_of_page_agrees_with_pdfcraft() {
    for rotation in [0, 90, 180, 270] {
        for unit in [1.0, 2.0] {
            let tree = format!("/MediaBox [{} {} {} {}] /Rotate {rotation}", MEDIA[0], MEDIA[1], MEDIA[2], MEDIA[3]);
            let page = format!("/CropBox [{} {} {} {}] /UserUnit {unit}", CROP[0], CROP[1], CROP[2], CROP[3]);
            let d = doc(&tree, &[&page]);
            let what = format!("page /Rotate {rotation}, /UserUnit {unit}");
            let (f, warnings) = frame_of_page(&d, 0).expect("frame");
            assert!(warnings.is_empty(), "{what}: {warnings:?}");
            check_a(&f, CROP, rotation, unit, &what);

            let p = &pages(&d)[0];
            assert_eq!(f.visible, p.crop(&d), "{what}: visible box");
            assert_eq!(f.rotation, p.rotation(&d), "{what}: rotation");
            let (pw, ph) = p.display_size(&d);
            assert_close(f.width, pw, TOL, &format!("{what}: width vs display_size"));
            assert_close(f.height, ph, TOL, &format!("{what}: height vs display_size"));
            // The overlay's frame followed by pdfcraft's display → user matrix is the identity.
            let round = f.to_display.then(&Matrix(p.view_matrix(&d)));
            for (k, (got, want)) in round.0.iter().zip(Matrix::IDENTITY.0.iter()).enumerate() {
                assert_close(*got, *want, TOL, &format!("{what}: frame then view_matrix, coefficient {k}"));
            }
        }
    }
}

/// `/Rotate` values that aren't 0, 90, 180 or 270 are read as pdfcraft reads them.
#[test]
fn unusual_rotate_values_match_pdfcraft() {
    for rotate in [450, -90, -450, 45, 135, 359, 360, 720] {
        let d = doc("/MediaBox [0 0 600 800]", &[&format!("/Rotate {rotate}")]);
        let (f, _) = frame_of_page(&d, 0).expect("frame");
        let p = &pages(&d)[0];
        assert_eq!(f.rotation, p.rotation(&d), "/Rotate {rotate}");
        assert_eq!(f.size(), p.display_size(&d), "/Rotate {rotate}");
        check_a(&f, [0.0, 0.0, 600.0, 800.0], f.rotation, 1.0, &format!("/Rotate {rotate}"));
    }
}

/// `/UserUnit` is read as pdfcraft reads it, including its cap at 75,000.
#[test]
fn user_unit_matches_pdfcraft_including_the_cap() {
    for unit in ["0.5", "1", "2.5", "1000000", "/Foo"] {
        let d = doc("/MediaBox [0 0 10 20]", &[&format!("/UserUnit {unit}")]);
        let (f, _) = frame_of_page(&d, 0).expect("frame");
        let p = &pages(&d)[0];
        assert_eq!(f.unit, p.user_unit(&d), "/UserUnit {unit}");
        assert_eq!(f.size(), p.display_size(&d), "/UserUnit {unit}");
    }
}

/// The composition order the overlay relies on: `a.then(b)` applies `a` first.
#[test]
fn matrix_then_applies_its_left_matrix_first() {
    let shift = Matrix::translate(10.0, 0.0);
    let double = Matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    assert_eq!(shift.then(&double).apply(1.0, 0.0), (22.0, 0.0));
    assert_eq!(double.then(&shift).apply(1.0, 0.0), (12.0, 0.0));
}

/// Test 3: broken boxes are handled as pdfcraft handles them, with a warning naming the page,
/// and a page with nothing to show is an error naming it.
#[test]
fn broken_boxes_warn_or_fail_naming_the_page() {
    // The second page carries the problem, so the warning must name page index 1.
    let ok = "/CropBox [0 0 600 800]";
    let cases: [(&str, &str, Option<Warning>); 4] = [
        (
            "/MediaBox [0 0 600 800]",
            "/CropBox [2000 2000 2100 2100]",
            Some(Warning::CropOutsideMedia { page: 1, crop: [2000.0, 2000.0, 2100.0, 2100.0], media: [0.0, 0.0, 600.0, 800.0] }),
        ),
        ("/MediaBox [0 0 600 800]", "/CropBox [0 0 /Foo]", Some(Warning::UnreadableCropBox { page: 1 })),
        ("/MediaBox [0 0 600 800]", "/CropBox 7", Some(Warning::UnreadableCropBox { page: 1 })),
        ("", "", Some(Warning::MissingMediaBox { page: 1 })),
    ];
    for (tree, page, warning) in cases {
        let d = doc(tree, &[ok, page]);
        let (f, warnings) = frame_of_page(&d, 1).expect("frame");
        assert_eq!(warnings.first(), warning.as_ref(), "{tree} / {page}");
        // Whatever the fallback, the overlay shows the page exactly as pdfcraft does.
        let p = &pages(&d)[1];
        assert_eq!(f.visible, p.crop(&d), "{tree} / {page}");
        assert_eq!(f.size(), p.display_size(&d), "{tree} / {page}");
    }
    let d = doc("", &[""]);
    let (f, _) = frame_of_page(&d, 0).expect("frame");
    assert_eq!(f.visible, DEFAULT_MEDIA);

    // A zero-size media box leaves nothing to show.
    let d = doc("/MediaBox [0 0 600 800]", &[ok, "/MediaBox [5 5 5 5]"]);
    assert!(matches!(frame_of_page(&d, 1), Err(OverlayError::EmptyBox { page: Some(1), .. })));
    assert!(frame_of_page(&d, 1).err().is_some_and(|e| e.to_string().contains("page 2")));

    // Past the last page.
    assert_eq!(frame_of_page(&d, 5).err(), Some(OverlayError::NoSuchPage { page: 5, count: 2 }));
}

#[test]
fn frame_rejects_bad_numbers() {
    assert_eq!(frame([0.0, 0.0, f64::NAN, 1.0], 0, 1.0).err(), Some(OverlayError::NotFinite { what: "the visible box" }));
    assert_eq!(frame([0.0, 0.0, 1.0, 1.0], 0, f64::INFINITY).err(), Some(OverlayError::NotFinite { what: "/UserUnit" }));
    assert!(matches!(frame([0.0, 0.0, 1.0, 1.0], 0, 0.0), Err(OverlayError::NotPositive { .. })));
    assert!(matches!(frame([-1e308, 0.0, 1e308, 1.0], 0, 1.0), Err(OverlayError::NotFinite { .. })));
    assert!(matches!(frame([3.0, 0.0, 3.0, 1.0], 90, 1.0), Err(OverlayError::EmptyBox { page: None, .. })));
}

/// Test 16, frame part: 10,000 seeded inputs, many hostile. Nothing panics, and every frame that
/// comes back has a finite, positive size and finite matrix.
#[test]
fn frame_never_panics() {
    let mut rng = Rng(0x0f_2a_11);
    for _ in 0..10_000 {
        let visible = [rng.wild(), rng.wild(), rng.wild(), rng.wild()];
        let rotation = rng.pick(&[0, 90, 180, 270, -90, 45, i64::MIN, i64::MAX, 7_000_000_000]);
        let unit = rng.wild();
        if let Ok(f) = frame(visible, rotation, unit) {
            assert!(f.width.is_finite() && f.width > 0.0 && f.height.is_finite() && f.height > 0.0, "{f:?}");
            assert!(f.to_display.0.iter().all(|v| v.is_finite()), "{f:?}");
            assert!([0, 90, 180, 270].contains(&f.rotation));
        }
    }
}

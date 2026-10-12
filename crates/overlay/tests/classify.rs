//! Ground truth for the pixel classifier (step 3 PRD, decision D1, approved with conditions).
//!
//! The truth here is defined without the classifier: pixel colours come from the overlay's
//! model with known coverages, and coverages come from exact box-filter areas of lines on a
//! pixel grid. The classifier must find every real difference, never report rounding or
//! inversion noise as one, and say "ambiguous" rather than guess near its threshold or where the
//! colours can't be separated.

mod common;

use common::measure::{Class, Image, K_SIGMA, Pair, classify, nearest_colour, profile, recover};

const PAIRS: [Pair; 2] = [Pair::RED_CYAN, Pair::GREEN_MAGENTA];

/// The colour the model gives for coverages (αA, αB), rounded to 8 bits as a renderer stores it.
fn pixel(pair: Pair, aa: f64, ab: f64) -> [u8; 3] {
    pair.predict(aa, ab).map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// The share of pixel column `i` (spanning [i, i + 1)) covered by the band [lo, hi].
fn coverage(lo: f64, hi: f64, i: i64) -> f64 {
    let (a, b) = (lo.max(i as f64), hi.min(i as f64 + 1.0));
    (b - a).max(0.0)
}

/// The decision rule applied to the true coverages: what the pixel really shows.
fn truth(pair: Pair, aa: f64, ab: f64) -> Class {
    let (da, db) = (Pair::visible_coverage(pair.a), Pair::visible_coverage(pair.b));
    if aa - ab >= da {
        Class::A
    } else if ab - aa >= db {
        Class::B
    } else if aa >= da || ab >= db {
        Class::Shared
    } else {
        Class::Paper
    }
}

/// Only the difference decision matters to the criteria: A, B, or no difference.
fn difference(c: Class) -> Option<Class> {
    matches!(c, Class::A | Class::B).then_some(c)
}

/// Widths in pixels: 0.13, 0.18, 0.25 and 0.35 mm at 150 and 300 dpi.
fn widths_px() -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    for dpi in [150.0, 300.0] {
        for mm in [0.13, 0.18, 0.25, 0.35] {
            out.push((dpi, mm, mm / 25.4 * dpi));
        }
    }
    out
}

/// Identical lines at zero displacement: whatever the coverage, and with a level of noise in
/// any channel, no pixel is a difference. The original nearest-colour rule fails this.
#[test]
fn identical_lines_never_show_a_difference() {
    for pair in PAIRS {
        let mut old_rule_fails = 0;
        for i in 0..=1000 {
            let c = f64::from(i) / 1000.0;
            let base = pixel(pair, c, c);
            for dr in -1i16..=1 {
                for dg in -1i16..=1 {
                    for db in -1i16..=1 {
                        let p = [
                            (i16::from(base[0]) + dr).clamp(0, 255) as u8,
                            (i16::from(base[1]) + dg).clamp(0, 255) as u8,
                            (i16::from(base[2]) + db).clamp(0, 255) as u8,
                        ];
                        let (class, r) = classify(p, pair);
                        assert!(!matches!(class, Class::A | Class::B), "{pair:?} coverage {c} pixel {p:?}: {class:?} {r:?}");
                    }
                }
            }
            if matches!(nearest_colour(base, pair), Class::A | Class::B) {
                old_rule_fails += 1;
            }
        }
        assert!(old_rule_fails > 0, "the nearest-colour rule calls some shared edges a difference (R1)");
    }
}

/// Antialiased edges of identical lines, at every width and eight sub-pixel positions.
#[test]
fn antialiased_edges_of_identical_lines_are_not_differences() {
    for pair in PAIRS {
        for (_, _, w) in widths_px() {
            for k in 0..8 {
                let lo = 10.0 + f64::from(k) / 8.0;
                for i in 8..16 {
                    let c = coverage(lo, lo + w, i);
                    let (class, _) = classify(pixel(pair, c, c), pair);
                    assert!(!matches!(class, Class::A | Class::B), "{pair:?} width {w} px offset {k}/8 column {i}: {class:?}");
                }
            }
        }
    }
}

/// A line in one document only is found at every width, offset and resolution, and its edges
/// never show the other document.
#[test]
fn lines_in_one_document_are_found() {
    for pair in PAIRS {
        for (dpi, mm, w) in widths_px() {
            for k in 0..8 {
                let lo = 10.0 + f64::from(k) / 8.0;
                for side in [Class::A, Class::B] {
                    let mut found = false;
                    for i in 8..16 {
                        let c = coverage(lo, lo + w, i);
                        let (aa, ab) = if side == Class::A { (c, 0.0) } else { (0.0, c) };
                        let (class, _) = classify(pixel(pair, aa, ab), pair);
                        let other = if side == Class::A { Class::B } else { Class::A };
                        assert_ne!(class, other, "{pair:?} {mm} mm at {dpi} dpi");
                        let want = truth(pair, aa, ab);
                        if difference(class) != difference(want) {
                            assert_eq!(class, Class::NearThreshold, "{pair:?} {mm} mm at {dpi} dpi, column {i}: {class:?}, truth {want:?}");
                        }
                        found |= class == side;
                    }
                    assert!(found, "{side:?}-only line of {mm} mm at {dpi} dpi, offset {k}/8: no pixel found");
                }
            }
        }
    }
}

/// Lines displaced by each step of the study, down to the smallest: the classifier agrees with
/// the truth pixel by pixel (or says near-threshold), so it never invents a difference and never
/// misses a clear one. The smallest steps are below one visible coverage and show no difference;
/// the largest is found everywhere.
#[test]
fn displaced_lines_agree_with_the_truth() {
    let steps_mm = [0.025, 0.05, 0.1, 0.15, 0.25, 0.5];
    for pair in PAIRS {
        for (dpi, mm, w) in widths_px() {
            for d_mm in steps_mm {
                let d = d_mm / 25.4 * dpi;
                let mut both_everywhere = true;
                let mut any_difference = false;
                for k in 0..8 {
                    let lo = 10.0 + f64::from(k) / 8.0;
                    let (mut a_found, mut b_found) = (false, false);
                    for i in 6..20 {
                        let (aa, ab) = (coverage(lo, lo + w, i), coverage(lo + d, lo + d + w, i));
                        let (class, r) = classify(pixel(pair, aa, ab), pair);
                        let want = truth(pair, aa, ab);
                        if difference(class) != difference(want) {
                            assert_eq!(
                                class,
                                Class::NearThreshold,
                                "{pair:?} {mm} mm, {d_mm} mm at {dpi} dpi, offset {k}, column {i}: {class:?} vs {want:?} ({r:?})"
                            );
                        }
                        a_found |= class == Class::A;
                        b_found |= class == Class::B;
                    }
                    any_difference |= a_found || b_found;
                    both_everywhere &= a_found && b_found;
                }
                // Coverage can differ by at most d per pixel, so below one visible coverage there is
                // no difference to find at all.
                if d < Pair::visible_coverage(pair.a).min(Pair::visible_coverage(pair.b)) {
                    assert!(!any_difference, "{pair:?} {mm} mm, {d_mm} mm at {dpi} dpi: a difference below what coverage allows");
                }
                if d_mm == 0.5 {
                    assert!(both_everywhere, "{pair:?} {mm} mm at {dpi} dpi: 0.5 mm must show both colours at every offset");
                }
            }
        }
    }
}

/// Coverage within noise of the threshold is reported as near-threshold, not forced either way;
/// well clear of it, the decision is made.
#[test]
fn borderline_coverage_is_reported_not_forced() {
    for pair in PAIRS {
        let da = Pair::visible_coverage(pair.a);
        let sigma = recover(pixel(pair, da, 0.0), pair).sigma_diff;
        assert!(sigma.is_finite() && sigma > 0.0 && sigma < 0.02, "{pair:?}: σ {sigma}");
        for f in [-0.6, -0.2, 0.2, 0.6] {
            let aa = da + f * K_SIGMA * sigma;
            let (class, _) = classify(pixel(pair, aa, 0.0), pair);
            assert_eq!(class, Class::NearThreshold, "{pair:?} αA = δ {f:+} · {K_SIGMA}σ");
        }
        // Clear of the threshold by more than the rounding can move it.
        let (above, _) = classify(pixel(pair, da + 0.05, 0.0), pair);
        let (below, _) = classify(pixel(pair, da - 0.05, 0.0), pair);
        assert_eq!(above, Class::A);
        assert_eq!(below, Class::Paper);
    }
}

/// Colours the two layer colours can't produce, and pairs whose coverages can't be separated,
/// are reported as such.
#[test]
fn unexplained_and_underdetermined_pixels_are_reported() {
    for p in [[0, 255, 0], [255, 255, 0], [0, 0, 255], [255, 0, 255]] {
        assert_eq!(classify(p, Pair::RED_CYAN).0, Class::Unexplained, "{p:?}");
    }
    let grays = Pair { a: [128, 128, 128], b: [64, 64, 64] };
    for p in [[200, 200, 200], [100, 100, 100], [40, 40, 40]] {
        assert!(classify(p, grays).0.is_ambiguous(), "{p:?} with two grays");
        assert_eq!(classify(p, grays).0, Class::Underdetermined, "{p:?}");
    }
}

/// Recovery from 8-bit colours is accurate to well within the threshold's margin.
#[test]
fn recovery_is_accurate_on_model_colours() {
    for pair in PAIRS {
        for i in 0..=20 {
            for j in 0..=20 {
                let (aa, ab) = (f64::from(i) / 20.0, f64::from(j) / 20.0);
                let r = recover(pixel(pair, aa, ab), pair);
                assert!((r.alpha_a - aa).abs() < 0.02 && (r.alpha_b - ab).abs() < 0.02, "{pair:?} ({aa}, {ab}) → {r:?}");
                assert!(r.residual < 1.0, "{pair:?} ({aa}, {ab}) residual {}", r.residual);
            }
        }
    }
}

/// The profile used across lines contains every pixel it passes through: consecutive pixels
/// share an edge (no diagonal step can skip the pixel a line lies on), and a profile through a
/// pixel corner contains all four pixels there.
#[test]
fn profiles_include_every_pixel_they_cross() {
    let img = Image { width: 64, height: 64, rgb: vec![255; 64 * 64 * 3] };
    for deg in [0.0_f64, 15.0, 30.0, 45.0, 60.0, 90.0, 135.0, 200.0, 315.0] {
        for (cx, cy) in [(32.0, 32.0), (32.5, 32.5), (32.37, 31.81)] {
            let (nx, ny) = (deg.to_radians().cos(), deg.to_radians().sin());
            let p = profile(&img, (cx - 6.0 * nx, cy - 6.0 * ny), (cx + 6.0 * nx, cy + 6.0 * ny));
            assert!(p.len() >= 12, "{deg}° at ({cx}, {cy}): {p:?}");
            let mut seen = std::collections::HashSet::new();
            for w in p.windows(2) {
                let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                assert!(seen.insert(w[0]), "{deg}°: {:?} twice", w[0]);
                let gap = x0.abs_diff(x1) + y0.abs_diff(y1);
                // Through a corner the four pixels there come in some order; elsewhere each step
                // crosses one edge.
                assert!(
                    gap == 1 || p.iter().any(|q| q.0.abs_diff(x0) + q.1.abs_diff(y0) == 1 && q.0.abs_diff(x1) + q.1.abs_diff(y1) == 1),
                    "{deg}° at ({cx}, {cy}): {:?} to {:?} skips a pixel",
                    w[0],
                    w[1]
                );
            }
            let centre = (cx.floor() as usize, cy.floor() as usize);
            assert!(p.contains(&centre), "{deg}° at ({cx}, {cy}): misses the centre pixel");
        }
    }
    // Through the corner (32, 32) at 45°: all four pixels around it.
    let p = profile(&img, (26.0, 26.0), (38.0, 38.0));
    for q in [(31, 31), (32, 31), (31, 32), (32, 32)] {
        assert!(p.contains(&q), "{q:?} missing from {p:?}");
    }
}

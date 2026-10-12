//! In-process pixel tests (OV3.3): a fast subset of the proof, rendered by PdfCraft only, so it
//! also runs in `cargo test` on Windows. Poppler and PDFium are compared by the proof example.
//! Step 3 PRD criteria 1, 3, 4, 6, 8 and 9b for PdfCraft, and the source-controlled compositing
//! check that R-a and R-b were accepted on (12 Oct).

mod common;

use std::path::Path;

use common::evaluate::{LineRow, PatchRow, Render, Renderer, TargetRow, control, measure_lines, measure_patches, measure_targets, render};
use common::families::{self, Case, LineKind, Set};

fn draw(set: &Set, case: &Case, dpi: f64) -> Render {
    render(Renderer::PdfCraft, &set.overlay, Path::new(""), case.page, dpi, case, Path::new("")).expect("PdfCraft renders")
}

fn patches(set: &Set, dpi: f64) -> Vec<PatchRow> {
    let mut rows = Vec::new();
    for case in &set.cases {
        measure_patches(set.name, case, &draw(set, case, dpi), Renderer::PdfCraft, &mut rows);
    }
    rows
}

#[track_caller]
fn assert_all_pass(rows: &[PatchRow]) {
    let failed: Vec<String> = rows
        .iter()
        .filter(|r| r.pass == Some(false))
        .map(|r| {
            format!(
                "{} / {} at {} dpi: got {:?}, expected {:?} (mask {:?}), error {:.2} > {}",
                r.case, r.patch, r.dpi, r.value, r.expected, r.mask, r.error, r.tol
            )
        })
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(rows.iter().any(|r| r.pass == Some(true)), "nothing was checked");
}

/// Criterion 1: overlap A × B / 255, exclusive colours and white paper, for the four pairs.
#[test]
fn overlap_and_exclusive_colours() {
    let set = families::overlap();
    for dpi in [150.0, 300.0] {
        assert_all_pass(&patches(&set, dpi));
    }
}

/// Criterion 6: the light-tone function recovered from the ramps within 1 level.
#[test]
fn light_tones_match_the_function() {
    assert_all_pass(&patches(&families::light_tones(), 150.0));
}

/// Criteria 2 and 9b: coloured fills print at 1 − PDF luminosity (with the D2 renderer fix).
#[test]
fn coloured_fills_print_at_pdf_luminosity() {
    assert_all_pass(&patches(&families::colours(), 150.0));
}

/// Criterion 8: targets land where OV2 puts them, and A and B coincide, within 0.5 px at 300 dpi.
#[test]
fn rotated_and_offset_pages_land_where_ov2_puts_them() {
    let set = families::rotation();
    let tol_pt = 0.5 * 72.0 / 300.0;
    // Every case of the family, not a selection.
    for case in &set.cases {
        let r = draw(&set, case, 300.0);
        let mut rows: Vec<TargetRow> = Vec::new();
        measure_targets(set.name, case, &r, Renderer::PdfCraft, &mut rows);
        assert!(rows.iter().any(|t| t.on_page), "{}: no target on the page", case.id);
        for t in rows.iter().filter(|t| t.on_page) {
            let e = t.error_pt.unwrap_or_else(|| panic!("{} {} not found", case.id, t.target));
            assert!(e <= tol_pt, "{} {}: {e:.4} pt off", case.id, t.target);
            if let Some(p) = rows.iter().find(|u| u.on_page && Some(&u.target) == t.partner.as_ref()) {
                let (m, n) = (t.measured.expect("measured"), p.measured.expect("measured"));
                let d = ((m.0 - n.0).powi(2) + (m.1 - n.1).powi(2)).sqrt();
                assert!(d <= tol_pt, "{} {}/{}: {d:.4} pt apart", case.id, t.target, p.target);
            }
        }
    }
}

/// Criteria 3 and 4 on every page of the study (four widths, nine kinds, eight offsets, both
/// pairs, all orientations) at 150 dpi: identical lines show no difference and one-document lines
/// are visible at every offset. Two checks on the measure itself, not criteria: every profile of
/// an identical pair crosses the line (so "no difference" can't pass by missing it), and a 0.5 mm
/// displacement shows both colours (criterion 5 only reports).
#[test]
fn line_study_at_150_dpi() {
    let set = families::lines();
    assert!(!set.cases.is_empty());
    for case in &set.cases {
        let r = draw(&set, case, 150.0);
        let mut rows: Vec<LineRow> = Vec::new();
        measure_lines(case, &r, Renderer::PdfCraft, &mut rows);
        assert!(!rows.is_empty(), "{}: no lines measured", case.id);
        let id = &case.id;
        for w in families::WIDTHS_MM {
            let of = |kind: LineKind| rows.iter().filter(move |l| l.width_mm == w && l.kind == kind);
            for l in of(LineKind::Shared) {
                assert_eq!(l.d1.a + l.d1.b, 0, "{id} {w} mm, {}°, offset {}: identical lines show a difference: {:?}", l.orientation, l.offset, l.d1);
                assert!(l.d1.shared > 0, "{id} {w} mm, {}°, offset {}: the profile missed the line {:?}", l.orientation, l.offset, l.d1);
            }
            for l in of(LineKind::AOnly) {
                assert!(l.d1.a > 0, "{id} {w} mm A-only, {}°, offset {}: not visible {:?}", l.orientation, l.offset, l.d1);
            }
            for l in of(LineKind::BOnly) {
                assert!(l.d1.b > 0, "{id} {w} mm B-only, {}°, offset {}: not visible {:?}", l.orientation, l.offset, l.d1);
            }
            for l in of(LineKind::Displaced(0.5)) {
                assert!(l.d1.a > 0 && l.d1.b > 0, "{id} {w} mm displaced 0.5 mm, {}°, offset {}: {:?}", l.orientation, l.offset, l.d1);
            }
        }
    }
}

/// The source-controlled compositing check (OV3 acceptance, 12 Oct). R-a and R-b were accepted
/// because the overlay reproduces what the renderer draws for each source page on its own: the
/// source rendered alone, put through the model, is the control, and the overlay must match it
/// within 2 levels in every channel. This keeps a compositing regression detectable even where
/// the original criteria 2 and 6 fail for renderer reasons (the 0.1 mm hatching at 150 dpi, the
/// light tones at t < 1); those original measurements are reported by the proof, not here.
#[test]
fn compositing_matches_the_source_control() {
    let tol = 2.0;
    let mut compared = 0;
    let mut hatching = 0;
    let mut failed = Vec::new();
    for set in [families::overlap(), families::colours(), families::tones(), families::transparency(), families::images(), families::light_tones()] {
        for case in &set.cases {
            let r = draw(&set, case, 150.0);
            let c = control(&set, case, Renderer::PdfCraft, &r, Path::new(""), Path::new(""))
                .unwrap_or_else(|| panic!("{}: no control (placement not the identity, or renders not aligned)", case.id));
            let (mut got, mut want) = (Vec::new(), Vec::new());
            measure_patches(set.name, case, &r, Renderer::PdfCraft, &mut got);
            measure_patches(set.name, case, &c, Renderer::PdfCraft, &mut want);
            assert_eq!(got.len(), want.len(), "{}", case.id);
            for (g, w) in got.iter().zip(&want) {
                let d = (0..3).map(|i| (g.value[i] - w.value[i]).abs()).fold(0.0, f64::max);
                compared += 1;
                hatching += usize::from(g.patch.starts_with("hatching"));
                if d.is_nan() || d > tol {
                    failed.push(format!("{} / {}: overlay {:?}, control {:?}, {d:.2} levels", g.case, g.patch, g.value, w.value));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    // 150 patches at 150 dpi: every patch of these families (the proof also runs them at 300).
    assert!(compared >= 150, "only {compared} patches compared");
    assert!(hatching >= 4, "the R-a hatching patches must be among those compared");
}

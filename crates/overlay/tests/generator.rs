//! The synthetic suite (OV3.2): every family builds, every file opens, the page boxes are the
//! placements', and two generations are byte-identical (step 3 PRD criteria 2, 9a and 13).

mod common;

use common::families;
use common::synth;

#[test]
fn the_suite_builds_and_is_deterministic() {
    let first = families::all();
    let second = families::all();
    assert_eq!(first.len(), 11);
    let mut pages = 0;
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.name, b.name);
        assert!(a.a == b.a && a.b == b.b && a.overlay == b.overlay, "{}: two generations differ", a.name);
        let overlay = synth::open(&a.overlay);
        assert_eq!(pdfcraft_model::pages(&overlay).len(), a.cases.len(), "{}", a.name);
        for case in &a.cases {
            for (what, ok, detail) in &case.checks {
                assert!(ok, "{} {what}: {detail}", case.id);
            }
            assert!(!case.patches.is_empty() || !case.targets.is_empty() || !case.lines.is_empty(), "{} measures nothing", case.id);
        }
        pages += a.cases.len();
    }
    assert!(pages >= 40, "{pages} overlay pages");
}

/// The line study covers every condition of the PRD: 4 angles × 4 widths × (7 displacements,
/// A only, B only) × 8 offsets, for two colour pairs.
#[test]
fn the_line_study_covers_every_condition() {
    let set = families::lines();
    assert_eq!(set.cases.len(), 8);
    for case in &set.cases {
        assert_eq!(case.lines.len(), 4 * 9 * 8, "{}", case.id);
    }
}

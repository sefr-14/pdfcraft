//! A whole overlay from two documents (`job::generate`, used by the wizard): pairs with blanks,
//! registration notes, clipping, and every unresolved item of both documents in one error.

mod common;

use common::synth::{self, Page};
use pdfcraft_content::Matrix;
use pdfcraft_cos::Document;
use pdfcraft_overlay::{
    LayerStyle, OverlayError, PairPlan, Plan, RegistrationKind, RegistrationNote, Roles, Rule, Settings, SheetMatch, Visibility, check_output,
    generate,
};

fn pages(n: usize) -> Document {
    let p: Vec<Page> = (0..n).map(|i| Page::new(200.0, 100.0, format!("0 g {} 10 20 20 re f", 10 + 30 * i))).collect();
    synth::open(&synth::pdf(&p))
}

fn plan(pairs: Vec<PairPlan>) -> Plan {
    Plan { pairs, style: LayerStyle::default(), roles: Roles::default(), expand: false, settings: Settings::default() }
}

#[test]
fn pairs_with_blanks_make_one_page_each() {
    let (a, b) = (pages(3), pages(2));
    let v = Visibility::default();
    let shift = Matrix::translate(30.0, 0.0);
    let fitted = RegistrationNote { kind: RegistrationKind::Similarity, rms_mm: Some(0.1), max_mm: Some(0.2) };
    let pairs = vec![
        PairPlan { a: Some(0), b: Some(1), sheet: SheetMatch::Off, registration: shift, note: fitted },
        PairPlan::placed(Some(1), None),
        PairPlan::placed(None, Some(0)),
        PairPlan::placed(Some(2), Some(0)),
    ];
    let out = generate(&a, &b, &v, &v, &plan(pairs)).expect("generates");
    assert_eq!(pdfcraft_model::pages(&out.doc).len(), 4);
    check_output(&out.doc).expect("only Old and New");
    assert_eq!(out.report.pages.len(), 4);
    assert_eq!(out.report.pages[0].registration, fitted);
    // A blank side: no registration applies.
    assert_eq!(out.report.pages[1].registration, RegistrationNote::PLACED);
    assert!(out.report.pages[1].b.is_none() && out.report.pages[2].a.is_none());
    // The shift pushes part of B off A's sheet.
    assert_eq!(out.clipped, [0]);
    // The sources are only read.
    assert_eq!(pdfcraft_model::pages(&a).len(), 3);
}

#[test]
fn nothing_to_compose_is_an_error() {
    let (a, b) = (pages(1), pages(1));
    let v = Visibility::default();
    assert_eq!(generate(&a, &b, &v, &v, &plan(vec![])).err(), Some(OverlayError::NoSources));
    assert_eq!(generate(&a, &b, &v, &v, &plan(vec![PairPlan::placed(None, None)])).err(), Some(OverlayError::NoSources));
    assert!(matches!(generate(&a, &b, &v, &v, &plan(vec![PairPlan::placed(Some(4), Some(0))])), Err(OverlayError::NoSuchPage { .. })));
}

/// One error lists every unresolved item of every page of both documents (O12), and a decision
/// on each settles it.
#[test]
fn every_unresolved_item_of_both_documents_comes_at_once() {
    let layered = |ocmd: &str| {
        synth::open(&synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R 7 0 R] /D << /OFF [7 0 R] >> >> >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 9 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Properties << /M 8 0 R >> >> >>".to_string(),
            synth::raw_stream("", "0 g 0 0 9 9 re f /OC /M BDC 0 g 50 50 9 9 re f EMC"),
            synth::raw_stream("", "0 g 0 0 9 9 re f"),
            "<< /Type /OCG /Name (on) >>".to_string(),
            "<< /Type /OCG /Name (off) >>".to_string(),
            ocmd.to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 5 0 R >>".to_string(),
        ]))
    };
    // A: /VE and /P disagree (OV4.7). B: a deleted member that changes the result.
    let a = layered("<< /Type /OCMD /OCGs [7 0 R] /P /AllOn /VE [/Or 6 0 R] >>");
    let b = layered("<< /Type /OCMD /OCGs [99 0 R] /P /AllOff >>");
    let none = Visibility::default();
    let p = plan(vec![PairPlan::placed(Some(0), Some(0)), PairPlan::placed(Some(1), Some(1))]);
    match generate(&a, &b, &none, &none, &p) {
        Err(OverlayError::Unresolved { a: ua, b: ub, roles }) => {
            assert_eq!(roles, Roles::default());
            assert_eq!(ua.iter().map(|u| (u.page, u.rule, u.settleable)).collect::<Vec<_>>(), [(0, Rule::ExpressionDisagrees, true)]);
            assert_eq!(ub.iter().map(|u| (u.page, u.rule, u.settleable)).collect::<Vec<_>>(), [(0, Rule::DeletedMember, true)]);
        }
        other => panic!("expected Unresolved, got {other:?}"),
    }
    let decide = |on| Visibility { decisions: vec![(pdfcraft_cos::ObjRef::new(8, 0), on)], ..Visibility::default() };
    let out = generate(&a, &b, &decide(false), &decide(true), &p).expect("decided");
    assert_eq!(pdfcraft_model::pages(&out.doc).len(), 2);
}

/// The tab's layer states become decisions only where they differ from the file's default, and
/// a layer the default can't resolve is left for the wizard to ask about.
#[test]
fn snapshot_decisions_follow_the_tab() {
    let doc = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [4 0 R 5 0 R 6 0 R] /D << /OFF [5 0 R] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".to_string(),
        "<< /Type /OCG /Name (on) >>".to_string(),
        "<< /Type /OCG /Name (off) >>".to_string(),
        "<< /Type /OCG /Name (design) /Intent /Design >>".to_string(),
    ]));
    let r = |n| pdfcraft_cos::ObjRef::new(n, 0);
    assert_eq!(pdfcraft_overlay::snapshot_decisions(&doc, &[(r(4), true), (r(5), false), (r(6), true)]), []);
    assert_eq!(pdfcraft_overlay::snapshot_decisions(&doc, &[(r(4), false), (r(5), true), (r(6), false)]), [(r(4), false), (r(5), true)]);
    // Not a layer of this file: ignored.
    assert_eq!(pdfcraft_overlay::snapshot_decisions(&doc, &[(r(3), true)]), []);
}

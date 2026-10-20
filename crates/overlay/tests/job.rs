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

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Three pages that each draw a block in an undeclared layer named "0" (object 6) and, on page
/// 3, one in another undeclared layer also named "0" (object 7). The configuration's `/OFF` names
/// both, so viewers disagree (OV4.10, U4) and each needs a decision. `x` moves the blocks, so two
/// documents with the same objects can be told apart in the output.
fn repeated_layers(x: u32) -> Document {
    let page = |contents: u32| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Resources << /Properties << /L 6 0 R /M 7 0 R >> >> /Contents {contents} 0 R >>"
        )
    };
    let first = format!("/OC /L BDC 1 0 0 rg {x} 11 21 21 re f EMC");
    let third = format!("{first} /OC /M BDC 0 0 1 rg {} 11 23 23 re f EMC", x + 40);
    synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [] /D << /OFF [6 0 R 7 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".into(),
        page(8),
        page(9),
        page(10),
        "<< /Type /OCG /Name (0) >>".into(),
        "<< /Type /OCG /Name (0) >>".into(),
        synth::raw_stream("", &first),
        synth::raw_stream("", &first),
        synth::raw_stream("", &third),
    ]))
}

/// OV4.10: a layer used on several pages is one question per document, whatever the pages, and
/// one decision settles all of them. Two layers with the same name are two questions, decided
/// apart, and the same object number in documents A and B is decided per document.
#[test]
fn one_layer_on_several_pages_is_one_question() {
    let r = |n| pdfcraft_cos::ObjRef::new(n, 0);
    let (a, b) = (repeated_layers(41), repeated_layers(141));
    let pairs = || (0..3).map(|i| PairPlan::placed(Some(i), Some(i))).collect::<Vec<_>>();
    let none = Visibility::default();
    let Err(OverlayError::Unresolved { a: ua, b: ub, .. }) = generate(&a, &b, &none, &none, &plan(pairs())) else {
        panic!("expected the undeclared layers to be unresolved")
    };
    // One item per page and layer: layer 6 on pages 1-3, layer 7 on page 3.
    assert_eq!(ua.len(), 4);
    for items in [&ua, &ub] {
        let groups = pdfcraft_overlay::oc::group_unresolved(items);
        let summary: Vec<_> = groups.iter().map(|g| (g.objects.clone(), g.rule, g.pages.clone())).collect();
        assert_eq!(
            summary,
            [(vec![(r(6), Some("0".to_string()))], Rule::NotInOcgs, vec![0, 1, 2]), (vec![(r(7), Some("0".to_string()))], Rule::NotInOcgs, vec![2])]
        );
        assert_eq!(
            groups[0].to_string(),
            "pages 1–3: a layer isn't declared in the document's layer list, and viewers disagree on whether it shows [\"0\" (6 0 R)]; an explicit layer decision settles it"
        );
        assert!(groups[1].to_string().starts_with("page 3: "), "{}", groups[1]);
    }
    // A shows its layer 6 and hides 7; B the other way round. One decision per layer settles
    // every page of that document, and neither document's decisions reach the other.
    let va = Visibility { decisions: vec![(r(6), true), (r(7), false)], ..Visibility::default() };
    let vb = Visibility { decisions: vec![(r(6), false), (r(7), true)], ..Visibility::default() };
    let out = generate(&a, &b, &va, &vb, &plan(pairs())).expect("generates");
    check_output(&out.doc).expect("only Old and New");
    let mut found = [false; 4];
    for (_, o) in out.doc.scan_objects() {
        let pdfcraft_cos::Object::Stream(s) = &*o else { continue };
        let Ok(d) = s.decoded() else { continue };
        for (i, needle) in ["rg 41 11 21 21 re", "rg 81 11 23 23 re", "rg 141 11 21 21 re", "rg 181 11 23 23 re"].iter().enumerate() {
            found[i] |= contains(&d, needle.as_bytes());
        }
    }
    assert_eq!(found, [true, false, false, true], "A's layer 6 and B's layer 7 only");
}

/// Unmatched rows (a page from one document only) each make one output page that keeps that
/// page's vector content, inside the layer of that document's role, with roles as given.
#[test]
fn unmatched_rows_keep_their_vectors_in_their_documents_layer() {
    let a = synth::open(&synth::pdf(&[Page::new(200.0, 100.0, "0 g 13 17 29 31 re f"), Page::new(200.0, 100.0, "0 g 1 2 3 4 re f")]));
    let b = synth::open(&synth::pdf(&[Page::new(300.0, 150.0, "0 g 41 43 47 53 re f")]));
    let v = Visibility::default();
    for (roles, a_layer, b_layer) in [(Roles::default(), "/PCOld", "/PCNew"), (Roles::SWAPPED, "/PCNew", "/PCOld")] {
        // Old-only (deleted), New-only (inserted), then a matched row.
        let pairs = vec![PairPlan::placed(Some(0), None), PairPlan::placed(None, Some(0)), PairPlan::placed(Some(1), Some(0))];
        let out = generate(&a, &b, &v, &v, &Plan { roles, ..plan(pairs) }).expect("generates");
        let doc = &out.doc;
        assert_eq!(pdfcraft_model::pages(doc).len(), 3, "one output page per row");
        check_output(doc).expect("only Old and New");
        let (p0, p1, p2) = (synth::page_content(doc, 0), synth::page_content(doc, 1), synth::page_content(doc, 2));
        assert!(contains(&p0, a_layer.as_bytes()) && !contains(&p0, b_layer.as_bytes()), "A-only row in A's layer");
        assert!(contains(&p1, b_layer.as_bytes()) && !contains(&p1, a_layer.as_bytes()), "B-only row in B's layer");
        assert!(contains(&p2, b"/PCOld") && contains(&p2, b"/PCNew"));
        // A blank side takes the other document's sheet: B's 300 × 150 page.
        let box1 = pdfcraft_model::pages(doc)[1].dict.get(b"MediaBox").map(|o| format!("{:?}", doc.resolve(o))).unwrap_or_default();
        assert!(box1.contains("300") && box1.contains("150"), "{box1}");
        // The sources' drawing operators are copied as vectors, and nothing is rasterized.
        let mut found = (false, false);
        for (_, o) in doc.scan_objects() {
            let pdfcraft_cos::Object::Stream(s) = &*o else { continue };
            assert_ne!(s.dict.name(b"Subtype"), Some(&b"Image"[..]), "no image in the output");
            let Ok(d) = s.decoded() else { continue };
            found.0 |= contains(&d, b"13 17 29 31 re f");
            found.1 |= contains(&d, b"41 43 47 53 re f");
        }
        assert_eq!(found, (true, true));
        assert_eq!(out.report.pages[0].b, None);
        assert_eq!(out.report.pages[1].a, None);
    }
}

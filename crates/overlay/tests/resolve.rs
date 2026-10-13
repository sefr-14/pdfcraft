//! Source preparation (OV4.3): each source's own optional content is resolved before it is
//! composed. Step 4 PRD, T4 and T5:
//!
//! - L5: for every layered case the prepared page's operators equal its twin's, the overlay
//!   renders as the twin's overlay does, and no output carries the canary of hidden content;
//!   every unresolvable case is refused with its page, objects and rule, and nothing is written.
//! - L6: the default configuration with no decisions; explicit decisions give their twins; the
//!   same decisions twice give the same file; a decision that can't apply is an error; a decision
//!   settles exactly the items it names.
//! - L11: the sources are only read.

mod common;

use std::sync::Arc;

use common::measure::Image;
use common::synth;
use common::twins::{self, CANARY_OPERANDS, CANARY_RGB, CANARY_TEXT, Case, Node, XObject};
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_overlay::oc::Note;
use pdfcraft_overlay::{
    LayerStyle, LayeredOutput, OverlayError, Placement, Prepared, RegistrationNote, Roles, Rule, Settings, SheetMatch, Visibility, check_output,
    compose_layered, frame_of_page, place, prepare,
};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

use Node::{Begin, Canary, Do, End, Layer, Live, LiveClip, Ops, Tag};

// ── Helpers ──────────────────────────────────────────────────────────────────────────────────

fn r(n: u32) -> ObjRef {
    ObjRef::new(n, 0)
}

fn visibility(case: &Case, built: &twins::Built) -> Visibility {
    Visibility { decisions: case.decisions.iter().map(|(name, on)| (r(built.numbers[name]), *on)).collect(), ..Visibility::default() }
}

fn placement(doc: &Document) -> Placement {
    let (f, _) = frame_of_page(doc, 0).expect("frame");
    place(&f, &f, SheetMatch::Off, &Matrix::IDENTITY, false, &Settings::default()).expect("placement")
}

fn ops(bytes: &[u8]) -> Vec<(Vec<u8>, Vec<Object>)> {
    let p = parse(bytes);
    assert_eq!(p.skipped, 0);
    p.ops.into_iter().map(|o| (o.op, o.operands)).collect()
}

fn stream_data(doc: &Document, o: &Object) -> Vec<u8> {
    match &*doc.resolve(o) {
        Object::Stream(s) => s.decoded().expect("decodes"),
        other => panic!("not a stream: {other:?}"),
    }
}

/// The XObject `name` drawn from page 0's resources.
fn xobject(doc: &Document, name: &str) -> Option<Object> {
    let page = pdfcraft_model::pages(doc).first().expect("page").dict.clone();
    let res = doc.dict(page.get(b"Resources")?)?;
    doc.dict(res.get(b"XObject")?)?.get(name.as_bytes()).cloned()
}

fn render(pdf: &Arc<Vec<u8>>, dpi: f64) -> Image {
    let mut r = PageRenderer::new(pdf.clone(), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, scale: (dpi / 72.0) as f32, ..RenderRequest::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba)
}

fn max_diff(x: &Image, y: &Image) -> u8 {
    assert_eq!((x.width, x.height), (y.width, y.height));
    x.rgb.iter().zip(&y.rgb).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// L5's leak check: no decoded stream holds the canary's text or colour, and no image its pixels.
fn no_canary(doc: &Document, what: &str) {
    for (r, o) in doc.scan_objects() {
        let Object::Stream(s) = &*o else { continue };
        let Ok(d) = s.decoded() else { continue };
        assert!(!contains(&d, CANARY_TEXT.as_bytes()), "{what}: the canary text is in object {}", r.num);
        assert!(!contains(&d, CANARY_OPERANDS.as_bytes()), "{what}: the canary colour is in object {}", r.num);
        if s.dict.name(b"Subtype") == Some(&b"Image"[..]) {
            assert!(!contains(&d, &CANARY_RGB), "{what}: a canary image is in object {}", r.num);
        }
    }
}

fn no_canary_pixel(img: &Image, what: &str) {
    for px in img.rgb.chunks(3) {
        let near = px.iter().zip(CANARY_RGB).all(|(a, b)| a.abs_diff(b) <= 12);
        assert!(!near, "{what}: a canary-coloured pixel");
    }
}

fn layered_one(prepared: &Prepared, p: &Placement) -> Result<Document, OverlayError> {
    compose_layered(Some(prepared.source(0)?), None, p, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED)
}

/// L5 for one case: prepared under its decisions, the page's operators (and each drawn form's)
/// equal the twin's; the overlay renders as the twin's overlay; no canary anywhere.
fn check_twin(case: &Case) -> Prepared {
    let built = twins::build(case);
    let (src, twin) = (synth::open(&built.source), synth::open(&built.twin));
    let prepared = prepare(&src, &[0], &visibility(case, &built)).expect("prepares");
    let report = &prepared.reports()[0];
    assert!(report.unresolved.is_empty(), "{}: {:?}", case.name, report.unresolved);
    assert!(report.optional, "{}: the case has layers", case.name);
    assert!(!src.is_modified(), "{}: the source is only read", case.name);

    no_canary(&twin, &format!("{}'s twin (every canary must be hidden)", case.name));
    let copy = prepared.doc();
    assert_eq!(ops(&synth::page_content(copy, 0)), ops(&synth::page_content(&twin, 0)), "{}: page operators", case.name);
    let twin_page = synth::page_content(&twin, 0);
    for x in &case.xobjects {
        if !x.shown() {
            assert!(xobject(copy, x.name).is_none(), "{}: hidden {} is still a resource", case.name, x.name);
        }
        let drawn = contains(&twin_page, format!("/{} Do", x.name).as_bytes());
        if drawn && matches!(x.kind, twins::Kind::Form(_)) {
            let t = xobject(&twin, x.name).expect("the twin's form");
            let c = xobject(copy, x.name).unwrap_or_else(|| panic!("{}: {} is drawn but missing", case.name, x.name));
            assert_eq!(ops(&stream_data(copy, &c)), ops(&stream_data(&twin, &t)), "{}: {}'s operators", case.name, x.name);
            assert!(copy.dict(&c).is_some_and(|d| !d.contains(b"OC")), "{}: {} keeps no /OC", case.name, x.name);
        }
    }

    let p = placement(&src);
    let out = layered_one(&prepared, &p).unwrap_or_else(|e| panic!("{}: {e}", case.name));
    let twin_prepared = prepare(&twin, &[0], &Visibility::default()).expect("twin prepares");
    assert!(!twin_prepared.touched(), "the twin has no layers");
    let twin_out = layered_one(&twin_prepared, &p).expect("twin composes");
    let (o, t) = (synth::save(&out, true), synth::save(&twin_out, true));
    let back = synth::open(&o);
    assert!(check_output(&back).is_ok(), "{}", case.name);
    no_canary(&back, case.name);
    let (ro, rt) = (render(&o, 72.0), render(&t, 72.0));
    assert!(max_diff(&ro, &rt) <= 1, "{}: the overlay differs from the twin's by {}", case.name, max_diff(&ro, &rt));
    no_canary_pixel(&render(&synth::save(copy, true), 72.0), case.name);
    prepared
}

fn group(name: &str) -> String {
    format!("<< /Type /OCG /Name ({name}) >>")
}

fn groups3() -> Vec<(&'static str, String)> {
    vec![("G1", group("Base")), ("G2", group("Notes")), ("G3", group("Hatch"))]
}

fn props3() -> Vec<(&'static str, &'static str)> {
    vec![("L1", "G1"), ("L2", "G2"), ("L3", "G3")]
}

/// A visible drawing in an ordinary colour, so the overlay has something to show.
const BASE: Node = Ops("0.2 g 20 20 100 40 re f");
const MORE: Node = Ops("0 0 1 RG 3 w 30 200 m 250 200 l S");

// ── L5: resolved cases (T4) ──────────────────────────────────────────────────────────────────

#[test]
fn default_configuration_lists() {
    // /BaseState ON (absent) with /OFF.
    check_twin(&Case {
        name: "base-on-off-list",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Layer("L1", true, vec![MORE]), Layer("L2", false, vec![Canary]), Ops("0.5 g 150 20 40 40 re f")]],
        ..Case::default()
    });
    // /BaseState OFF with /ON: followed as viewers do, with a note.
    let p = check_twin(&Case {
        name: "base-off-on-list",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /BaseState /OFF /ON [{G1}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Layer("L1", true, vec![MORE]), Layer("L2", false, vec![Canary]), Layer("L3", false, vec![Canary])]],
        ..Case::default()
    });
    assert!(p.notes().contains(&Note::BaseStateOff));
    // /ON and /OFF lists.
    check_twin(&Case {
        name: "on-off-lists",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /BaseState /ON /ON [{G1}] /OFF [{G2} {G3}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![Layer("L3", false, vec![Canary]), BASE, Layer("L1", true, vec![MORE]), Layer("L2", false, vec![Canary])]],
        ..Case::default()
    });
}

#[test]
fn nested_blocks_three_deep() {
    let p = check_twin(&Case {
        name: "nested-3-deep",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G3}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer(
                "L1",
                true,
                vec![
                    Ops("1 0 0 rg 10 150 30 30 re f"),
                    Layer("L2", true, vec![MORE, Layer("L3", false, vec![Canary]), Ops("0 1 0 rg 60 150 30 30 re f")]),
                ],
            ),
            // Visible layers inside a hidden one stay hidden.
            Layer("L3", false, vec![Layer("L1", true, vec![Canary, Layer("L2", true, vec![Canary])])]),
        ]],
        ..Case::default()
    });
    let rep = &p.reports()[0];
    assert_eq!((rep.hidden_blocks, rep.markers_removed), (2, 2), "{rep:?}");
}

#[test]
fn membership_policies() {
    let mut objects = groups3();
    for (name, body) in [
        ("M1", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AnyOn >>"),
        ("M2", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AllOn >>"),
        ("M3", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AnyOff >>"),
        ("M4", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AllOff >>"),
        ("M5", "<< /Type /OCMD /OCGs {G1} /P /AllOn >>"),
        ("M6", "<< /Type /OCMD /OCGs {G2} /P /AllOff >>"),
        ("M7", "<< /Type /OCMD /OCGs [{G1} {G2}] >>"),
    ] {
        objects.push((name, body.to_string()));
    }
    check_twin(&Case {
        name: "ocmd-policies",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: vec![("A1", "M1"), ("A2", "M2"), ("A3", "M3"), ("A4", "M4"), ("A5", "M5"), ("A6", "M6"), ("A7", "M7")],
        contents: vec![vec![
            BASE,
            Layer("A1", true, vec![Ops("0.1 g 10 150 20 20 re f")]),
            Layer("A2", false, vec![Canary]),
            Layer("A3", true, vec![Ops("0.3 g 40 150 20 20 re f")]),
            Layer("A4", false, vec![Canary]),
            Layer("A5", true, vec![Ops("0.5 g 70 150 20 20 re f")]),
            Layer("A6", true, vec![Ops("0.6 g 100 150 20 20 re f")]),
            Layer("A7", true, vec![Ops("0.7 g 130 150 20 20 re f")]),
        ]],
        ..Case::default()
    });
}

#[test]
fn visibility_expressions_take_precedence() {
    let mut objects = groups3();
    for (name, body) in [
        ("E1", "<< /Type /OCMD /VE [/And {G1} [/Not {G2}]] >>"),
        ("E2", "<< /Type /OCMD /VE [/Or {G2} [/And {G1} {G2}]] >>"),
        // Three deep: Not(Or(And(G1, G2), Not(G1))) = Not(Or(false, false)) = true.
        ("E3", "<< /Type /OCMD /VE [/Not [/Or [/And {G1} {G2}] [/Not {G1}]]] >>"),
        // /VE and /P disagree: /VE is used in preference (PDF Reference 1.7, Table 4.49).
        ("E4", "<< /Type /OCMD /OCGs [{G2}] /P /AllOn /VE [/Or {G1}] >>"),
    ] {
        objects.push((name, body.to_string()));
    }
    let p = check_twin(&Case {
        name: "visibility-expressions",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: vec![("X1", "E1"), ("X2", "E2"), ("X3", "E3"), ("X4", "E4")],
        contents: vec![vec![
            BASE,
            Layer("X1", true, vec![Ops("0.1 g 10 150 20 20 re f")]),
            Layer("X2", false, vec![Canary]),
            Layer("X3", true, vec![Ops("0.3 g 40 150 20 20 re f")]),
            Layer("X4", true, vec![Ops("0.4 g 70 150 20 20 re f")]),
        ]],
        ..Case::default()
    });
    assert!(p.notes().iter().any(|n| matches!(n, Note::ExpressionIgnoredByDisplay { display_differs: Some(true), .. })), "{:?}", p.notes());
}

#[test]
fn optional_content_on_forms_and_images() {
    let p = check_twin(&Case {
        name: "oc-on-form-and-image",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Do("F1"), Do("F2"), Ops("q 40 0 0 40 150 150 cm"), Do("I1"), Ops("Q q 40 0 0 40 200 150 cm"), Do("I2"), Ops("Q")]],
        xobjects: vec![
            XObject::form("F1", vec![Ops("0 0.5 0 rg 10 100 30 30 re f")]).with_oc("G1", true),
            XObject::form("F2", vec![Canary]).with_oc("G2", false),
            XObject::image("I1").with_oc("G1", true),
            XObject::image("I2").with_oc("G2", false),
        ],
        ..Case::default()
    });
    assert_eq!(p.reports()[0].hidden_xobjects, 2);
}

#[test]
fn one_form_drawn_in_a_visible_and_a_hidden_block() {
    check_twin(&Case {
        name: "form-in-visible-and-hidden",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![Do("F3")]),
            Layer("L2", false, vec![Ops("q 1 0 0 1 50 50 cm"), Do("F3"), Ops("Q"), Canary]),
        ]],
        xobjects: vec![XObject::form("F3", vec![Ops("0 0 0.5 rg 100 100 40 40 re f")])],
        ..Case::default()
    });
}

#[test]
fn markers_split_across_content_streams() {
    check_twin(&Case {
        name: "split-across-streams",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Begin("L2", false)], vec![Canary, End, Begin("L1", true), MORE], vec![Ops("0.4 g 200 20 30 30 re f"), End]],
        ..Case::default()
    });
}

#[test]
fn tagged_content_inside_layers() {
    check_twin(&Case {
        name: "tagged-text",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![Tag("/P << /MCID 0 >> BDC", vec![Ops("BT /FC 12 Tf 20 260 Td (Visible) Tj ET")])]),
            Layer("L2", false, vec![Tag("/P << /MCID 1 >> BDC", vec![Canary])]),
            Tag("/Span << /ActualText (x) >> BDC", vec![Layer("L1", true, vec![MORE])]),
        ]],
        ..Case::default()
    });
}

#[test]
fn intents_matched_with_the_configuration() {
    let objects = vec![
        ("G1", "<< /Type /OCG /Name (Both) /Intent [/View /Design] >>".to_string()),
        ("G2", group("Plain")),
        ("G3", "<< /Type /OCG /Name (Design) /Intent /Design >>".to_string()),
    ];
    check_twin(&Case {
        name: "intent-all",
        objects: objects.clone(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /Intent /All /OFF [{G3}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![MORE]),
            Layer("L2", true, vec![Ops("0.6 g 200 20 30 30 re f")]),
            Layer("L3", false, vec![Canary]),
        ]],
        ..Case::default()
    });
    check_twin(&Case {
        name: "intent-view-design",
        objects,
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /Intent [/View /Design] /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L1", true, vec![MORE]),
            Layer("L2", false, vec![Canary]),
            Layer("L3", true, vec![Ops("0.6 g 200 20 30 30 re f")]),
        ]],
        ..Case::default()
    });
}

#[test]
fn layers_inside_forms_patterns_and_soft_masks() {
    let mask = XObject { group: true, ..XObject::form("SMG", vec![Ops("1 g 0 0 300 300 re f"), Layer("L2", false, vec![Canary])]) };
    check_twin(&Case {
        name: "inside-forms",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![BASE, Do("F4"), Ops("q /Pattern cs /P1 scn 150 150 100 100 re f Q"), Ops("q /SM gs 0 0 0.6 rg 150 20 60 60 re f Q")]],
        xobjects: vec![
            XObject::form("F4", vec![Layer("L1", true, vec![Ops("0.3 0 0 rg 10 100 40 40 re f")]), Layer("L2", false, vec![Canary])]),
            mask,
        ],
        patterns: vec![("P1", vec![Ops("0 0.4 0 rg 0 0 10 10 re f"), Layer("L2", false, vec![Canary])])],
        soft_masks: vec![("SM", "SMG")],
        ..Case::default()
    });
}

/// Graphics-state changes in hidden content that later content uses are kept (viewers apply
/// them); the rest goes with the block.
#[test]
fn live_state_in_hidden_blocks_is_kept() {
    let p = check_twin(&Case {
        name: "live-state",
        objects: groups3(),
        oc_properties: Some("<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>".into()),
        properties: props3(),
        contents: vec![vec![
            BASE,
            Layer("L2", false, vec![Ops("0.9 g"), Live("0.2 0.4 0.6 rg"), Canary, Live("1 0 0 1 10 0 cm"), Live("4 w")]),
            Ops("150 20 40 40 re f"),
            // Overwritten before use: the hidden block's colour isn't kept.
            Layer("L2", false, vec![Ops("0.8 0.1 0.1 rg"), Canary]),
            Ops("0 0 0 rg 0 0 1 RG 20 150 m 120 150 l S"),
            // A clipping path later content is drawn through.
            Layer("L2", false, vec![LiveClip("100 100 100 100"), Canary]),
            Ops("0.5 g 0 0 300 300 re f"),
        ]],
        ..Case::default()
    });
    let rep = &p.reports()[0];
    assert_eq!((rep.kept_state, rep.kept_clips), (3, 1), "{rep:?}");
}

// ── L5: unresolved cases (T4) ────────────────────────────────────────────────────────────────

/// Prepare `case` and return its page's unresolved items as (rule, settleable, object names).
fn unresolved(case: &Case) -> Vec<(Rule, bool, Vec<String>)> {
    let built = twins::build(case);
    let src = synth::open(&built.source);
    let prepared = prepare(&src, &[0], &visibility(case, &built)).expect("prepares");
    let items = prepared.reports()[0].unresolved.clone();
    // Refused, and nothing is written.
    let p = placement(&src);
    let mut out = LayeredOutput::new().expect("output");
    match out.add_page(Some(prepared.source(0).expect("page")), None, &p, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED) {
        Err(OverlayError::Unresolved { a, b, roles }) => {
            assert_eq!(roles, Roles::default());
            assert_eq!(a, items, "{}", case.name);
            assert!(b.is_empty());
            assert!(a.iter().all(|u| u.page == 0));
        }
        other => panic!("{}: expected Unresolved, got {other:?}", case.name),
    }
    assert_eq!(out.page_count(), 0);
    assert_eq!(out.finish().err(), Some(OverlayError::NoSources));
    let mut v: Vec<(Rule, bool, Vec<String>)> =
        items.into_iter().map(|u| (u.rule, u.settleable, u.objects.into_iter().map(|(_, n)| n.unwrap_or_default()).collect())).collect();
    v.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
    v
}

fn simple(name: &'static str, objects: Vec<(&'static str, String)>, props: &str, content: Vec<Node>) -> Case {
    let properties =
        [("L1", "G1"), ("L2", "G2"), ("L3", "G3"), ("M", "M")].into_iter().filter(|(_, o)| objects.iter().any(|(n, _)| n == o)).collect();
    Case { name, objects, oc_properties: Some(props.to_string()), properties, contents: vec![content], ..Case::default() }
}

fn with_ocmd(body: &str) -> Vec<(&'static str, String)> {
    let mut o = groups3();
    o.push(("M", body.to_string()));
    o
}

const D: &str = "<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>";

#[test]
fn configuration_and_group_rules_are_unresolved() {
    let one = |rule, settleable: bool, names: &[&str]| vec![(rule, settleable, names.iter().map(|s| s.to_string()).collect::<Vec<_>>())];
    // A group missing from /OCGs.
    assert_eq!(
        unresolved(&simple("not-in-ocgs", groups3(), "<< /OCGs [{G1} {G2}] /D << >> >>", vec![BASE, Layer("L3", false, vec![Canary])])),
        one(Rule::NotInOcgs, true, &["Hatch"])
    );
    // A Design group under the default configuration (View).
    let mut design = groups3();
    design[2].1 = "<< /Type /OCG /Name (Hatch) /Intent /Design >>".into();
    assert_eq!(
        unresolved(&simple("intent-design", design, D, vec![BASE, Layer("L3", false, vec![Canary])])),
        one(Rule::IntentMismatch, true, &["Hatch"])
    );
    // /BaseState Unchanged: no decision repairs the configuration.
    assert_eq!(
        unresolved(&simple(
            "base-unchanged",
            groups3(),
            "<< /OCGs [{G1} {G2} {G3}] /D << /BaseState /Unchanged >> >>",
            vec![BASE, Layer("L1", false, vec![Canary])]
        )),
        one(Rule::BaseState, false, &[])
    );
    // Auto-states: Zoom and Language depend on the viewer; a malformed one; a View state that
    // would change a group.
    let usage = |what: &str| {
        let mut g = groups3();
        g[1].1 = format!("<< /Type /OCG /Name (Notes) /Usage << {what} >> >>");
        g
    };
    let auto = |cat: &str| format!("<< /OCGs [{{G1}} {{G2}} {{G3}}] /D << /AS [<< /Event /View /Category [/{cat}] /OCGs [{{G2}}] >>] >> >>");
    assert_eq!(
        unresolved(&simple("as-zoom", usage("/Zoom << /min 2 >>"), &auto("Zoom"), vec![BASE, Layer("L2", false, vec![Canary])])),
        one(Rule::AutoStateEnvironment, true, &["Notes"])
    );
    assert_eq!(
        unresolved(&simple("as-language", usage("/Language << /Lang (fr) >>"), &auto("Language"), vec![BASE, Layer("L2", false, vec![Canary])])),
        one(Rule::AutoStateEnvironment, true, &["Notes"])
    );
    assert_eq!(
        unresolved(&simple(
            "as-malformed",
            groups3(),
            "<< /OCGs [{G1} {G2} {G3}] /D << /AS [<< /Event /Hover /Category [/View] /OCGs [{G2}] >>] >> >>",
            vec![BASE, Layer("L2", false, vec![Canary])]
        )),
        one(Rule::MalformedAutoState, true, &["Notes"])
    );
    assert_eq!(
        unresolved(&simple("as-view-changes", usage("/View << /ViewState /OFF >>"), &auto("View"), vec![BASE, Layer("L2", false, vec![Canary])])),
        one(Rule::AutoStateView, true, &["Notes"])
    );
}

#[test]
fn auto_states_that_change_nothing_are_noted() {
    let mut g = groups3();
    g[1].1 = "<< /Type /OCG /Name (Notes) /Usage << /View << /ViewState /ON >> /Print << /PrintState /ON >> >> >>".into();
    let p = check_twin(&simple(
        "as-view-no-change-and-print",
        g,
        "<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G3}] /AS [<< /Event /View /Category [/View] /OCGs [{G2}] >> << /Event /Print /Category [/Print] /OCGs [{G2}] >>] >> >>",
        vec![BASE, Layer("L2", true, vec![MORE]), Layer("L3", false, vec![Canary])],
    ));
    assert!(p.notes().iter().any(|n| matches!(n, Note::AutoStateNoChange { .. })));
    assert!(p.notes().iter().any(|n| matches!(n, Note::PrintExportNotApplied { .. })));
}

#[test]
fn membership_rules_are_unresolved() {
    let case = |name, body: &str| simple(name, with_ocmd(body), D, vec![BASE, Layer("M", false, vec![Canary])]);
    for (name, body) in [
        ("ve-unknown-operator", "<< /Type /OCMD /VE [/Xor {G1} {G2}] >>"),
        ("ve-not-two", "<< /Type /OCMD /VE [/Not {G1} {G2}] >>"),
        ("ve-leaf-not-group", "<< /Type /OCMD /VE [/And {G1} 7] >>"),
    ] {
        let got = unresolved(&case(name, body));
        assert_eq!(got.len(), 1, "{name}: {got:?}");
        assert_eq!((got[0].0, got[0].1), (Rule::MalformedExpression, true), "{name}");
    }
    // A cycle, and nesting 33 deep.
    let mut cyc = groups3();
    cyc.push(("M", "<< /Type /OCMD /VE {V} >>".into()));
    cyc.push(("V", "[/And {G1} {W}]".into()));
    cyc.push(("W", "[/Or {V}]".into()));
    let got = unresolved(&simple("ve-cycle", cyc, D, vec![BASE, Layer("M", false, vec![Canary])]));
    assert_eq!((got[0].0, got.len()), (Rule::MalformedExpression, 1));
    // 33 levels below the top expression (32 are allowed).
    let deep = format!("{}{{G1}}{}", "[/Not ".repeat(34), "]".repeat(34));
    let got = unresolved(&case("ve-depth-33", &format!("<< /Type /OCMD /VE {deep} >>")));
    assert_eq!((got[0].0, got.len()), (Rule::MalformedExpression, 1));
    // An unknown /P.
    let got = unresolved(&case("unknown-policy", "<< /Type /OCMD /OCGs [{G1}] /P /SomeOn >>"));
    assert_eq!(got.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), [(Rule::UnknownPolicy, true)]);
}

#[test]
fn structural_conditions_are_unresolved() {
    let rules = |case: Case| unresolved(&case).into_iter().map(|(r, s, _)| (r, s)).collect::<Vec<_>>();
    let c = |name, content: Vec<Node>| simple(name, groups3(), D, content);
    // An inline dictionary after /OC: nothing can name it.
    assert_eq!(rules(c("inline", vec![BASE, Ops("/OC << /Type /OCMD /OCGs [] >> BDC"), Canary, Ops("EMC")])), [(Rule::Inline, false)]);
    // A marker naming a property the resources don't have.
    assert_eq!(rules(c("unknown-property", vec![BASE, Ops("/OC /Nope BDC"), Canary, Ops("EMC")])), [(Rule::UnknownProperty, false)]);
    // An end without a beginning, and a beginning never ended, around a layer.
    assert_eq!(rules(c("unbalanced-emc", vec![BASE, Layer("L2", false, vec![Canary]), Ops("EMC")])), [(Rule::Unbalanced, false)]);
    assert_eq!(rules(c("unclosed-bdc", vec![BASE, Ops("/OC /L2 BDC"), Canary])), [(Rule::Unbalanced, false)]);
    // An unreadable inline image before a layer: skipped bytes.
    assert_eq!(
        rules(c("unreadable-inline-image", vec![Ops("BI /W 1 /H ) ID x EI"), BASE, Layer("L2", false, vec![Canary])])),
        [(Rule::Unreadable, false)]
    );
    // A hidden block that a save/restore, a text object or a path crosses.
    assert_eq!(rules(c("crosses-q", vec![BASE, Ops("q"), Layer("L2", false, vec![Ops("Q"), Canary])])), [(Rule::HiddenBlockCrossesStructure, false)]);
    assert_eq!(
        rules(c("crosses-text", vec![Ops("BT /FC 12 Tf 10 10 Td"), Layer("L2", false, vec![Ops("(x) Tj ET"), Canary])])),
        [(Rule::HiddenBlockCrossesStructure, false)]
    );
    assert_eq!(
        rules(c("crosses-path", vec![Ops("10 10 m 50 50 l"), Layer("L2", false, vec![Ops("S"), Canary])])),
        [(Rule::HiddenBlockCrossesStructure, false)]
    );
    // Hidden text that visible text in the same line continues from.
    assert_eq!(
        rules(c(
            "hidden-text-in-line",
            vec![Ops("BT /FC 12 Tf 10 250 Td (Seen) Tj"), Layer("L2", false, vec![Ops("(CANARY-7Q4) Tj")]), Ops("(After) Tj ET")]
        )),
        [(Rule::HiddenTextMovesVisibleText, false)]
    );
    // Hidden `"` sets spacing that later text uses; hidden TD sets leading that T* uses.
    assert_eq!(
        rules(c(
            "hidden-quote-spacing",
            vec![Layer("L2", false, vec![Ops("BT /FC 12 Tf 10 10 Td 3 1 (CANARY-7Q4) \" ET")]), Ops("BT /FC 12 Tf 10 200 Td (Seen) Tj ET")]
        )),
        [(Rule::HiddenTextMovesVisibleText, false)]
    );
    assert_eq!(
        rules(c("hidden-td-leading", vec![Layer("L2", false, vec![Ops("BT 0 -20 TD ET"), Canary]), Ops("BT /FC 12 Tf 10 200 Td T* (Seen) Tj ET")])),
        [(Rule::HiddenTextMovesVisibleText, false)]
    );
    // Hidden text in a clipping render mode, with content drawn after it.
    assert_eq!(
        rules(c(
            "hidden-clipping-text",
            vec![Ops("7 Tr"), Layer("L2", false, vec![Ops("BT /FC 40 Tf 10 10 Td (CANARY-7Q4) Tj ET")]), Ops("0 Tr"), BASE]
        )),
        [(Rule::HiddenClippingText, false)]
    );
}

#[test]
fn forms_that_cant_be_resolved() {
    // A form that draws itself.
    let case = Case {
        name: "form-cycle",
        objects: groups3(),
        oc_properties: Some(D.into()),
        properties: props3(),
        contents: vec![vec![BASE, Do("F5")]],
        xobjects: vec![XObject::form("F5", vec![Layer("L2", false, vec![Canary]), Do("F5")])],
        ..Case::default()
    };
    assert_eq!(unresolved(&case).into_iter().map(|x| x.0).collect::<Vec<_>>(), [Rule::TooDeep]);

    // Optional content in data the overlay copies but doesn't draw.
    let built = twins::build(&Case {
        name: "in-resources",
        objects: groups3(),
        oc_properties: Some(D.into()),
        properties: props3(),
        contents: vec![vec![BASE, Do("F6")]],
        xobjects: vec![XObject::form("F6", vec![MORE])],
        ..Case::default()
    });
    let mut src = synth::open(&built.source);
    let f6 = r(built.numbers["F6"]);
    let g3 = r(built.numbers["G3"]);
    src.update_dict(f6, |d| {
        let mut private = pdfcraft_cos::Dict::new();
        private.set(b"Private".to_vec(), Object::Ref(g3));
        let mut piece = pdfcraft_cos::Dict::new();
        piece.set(b"App".to_vec(), Object::Dict(private));
        d.set(b"PieceInfo".to_vec(), Object::Dict(piece));
    })
    .expect("form");
    let prepared = prepare(&src, &[0], &Visibility::default()).expect("prepares");
    assert_eq!(prepared.reports()[0].unresolved.iter().map(|u| u.rule).collect::<Vec<_>>(), [Rule::InResources]);

    // A form without resources drawn from two places whose layer properties differ.
    let doc = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R 8 0 R] /D << /OFF [8 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Properties << /L 7 0 R >> /XObject << /F 6 0 R >> >> /Contents 5 0 R >>"
            .into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Properties << /L 8 0 R >> /XObject << /F 6 0 R >> >> /Contents 5 0 R >>"
            .into(),
        synth::raw_stream("", "/F Do"),
        synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100]", "/OC /L BDC 0 g 0 0 50 50 re f EMC"),
        "<< /Type /OCG /Name (On) >>".into(),
        "<< /Type /OCG /Name (Off) >>".into(),
    ]));
    let prepared = prepare(&doc, &[0, 1], &Visibility::default()).expect("prepares");
    assert!(prepared.reports()[0].unresolved.is_empty());
    assert_eq!(prepared.reports()[1].unresolved.iter().map(|u| u.rule).collect::<Vec<_>>(), [Rule::AmbiguousResources]);

    // A form that can't be decoded, in a document with layers.
    let doc = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /F 5 0 R >> >> /Contents 4 0 R >>".into(),
        synth::raw_stream("", "/F Do"),
        synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Filter /Bogus", "x"),
        "<< /Type /OCG /Name (On) >>".into(),
    ]));
    let prepared = prepare(&doc, &[0], &Visibility::default()).expect("prepares");
    assert_eq!(prepared.reports()[0].unresolved.iter().map(|u| u.rule).collect::<Vec<_>>(), [Rule::Unreadable]);
}

/// Both sides' items come back in one error, each with its page, objects and rule.
#[test]
fn both_documents_items_are_listed_at_once() {
    let a = twins::build(&simple("a", groups3(), "<< /OCGs [{G1} {G2}] /D << >> >>", vec![BASE, Layer("L3", false, vec![Canary])]));
    let b = twins::build(&simple("b", with_ocmd("<< /Type /OCMD /OCGs [{G1}] /P /Most >>"), D, vec![BASE, Layer("M", false, vec![Canary])]));
    let (da, db) = (synth::open(&a.source), synth::open(&b.source));
    let (pa, pb) = (prepare(&da, &[0], &Visibility::default()).expect("A"), prepare(&db, &[0], &Visibility::default()).expect("B"));
    let p = placement(&da);
    match compose_layered(
        Some(pa.source(0).expect("A")),
        Some(pb.source(0).expect("B")),
        &p,
        &LayerStyle::default(),
        Roles::SWAPPED,
        RegistrationNote::PLACED,
    ) {
        Err(OverlayError::Unresolved { a: ua, b: ub, roles }) => {
            assert_eq!(roles, Roles::SWAPPED);
            let e = OverlayError::Unresolved { a: ua.clone(), b: ub.clone(), roles };
            assert_eq!(
                ua.iter().map(|u| (u.rule, u.objects.clone())).collect::<Vec<_>>(),
                [(Rule::NotInOcgs, vec![(r(a.numbers["G3"]), Some("Hatch".into()))])]
            );
            assert_eq!(ub.iter().map(|u| u.rule).collect::<Vec<_>>(), [Rule::UnknownPolicy]);
            let text = e.to_string();
            assert!(text.contains("document A (New), page 1") && text.contains("\"Hatch\"") && text.contains("document B (Old), page 1"), "{text}");
        }
        other => panic!("expected Unresolved: {other:?}"),
    }
}

// ── L6: visibility state (T5) ────────────────────────────────────────────────────────────────

fn t5(name: &'static str, decisions: Vec<(&'static str, bool)>, g1: bool, g2: bool, m: bool) -> Case {
    let mut objects = groups3();
    objects.push(("M", "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AllOn >>".into()));
    let node = |shown: bool, colour: &'static str| if shown { Ops(colour) } else { Canary };
    Case {
        name,
        objects,
        oc_properties: Some(D.into()),
        properties: vec![("L1", "G1"), ("L2", "G2"), ("M", "M")],
        contents: vec![vec![
            BASE,
            Layer("L1", g1, vec![node(g1, "0.1 g 10 150 30 30 re f")]),
            Layer("L2", g2, vec![node(g2, "0.4 g 60 150 30 30 re f")]),
            Layer("M", m, vec![node(m, "0.7 g 110 150 30 30 re f")]),
        ]],
        decisions,
        ..Case::default()
    }
}

#[test]
fn explicit_states_give_their_twins() {
    // The default configuration: G1 on, G2 off, so M (AllOn) is off.
    check_twin(&t5("default", vec![], true, false, false));
    // Two explicit states.
    check_twin(&t5("g2-on", vec![("G2", true)], true, true, true));
    check_twin(&t5("g1-off", vec![("G1", false)], false, false, false));
    // A decision on the membership dictionary itself.
    check_twin(&t5("m-on", vec![("M", true)], true, false, true));
}

#[test]
fn the_same_state_twice_gives_the_same_file() {
    let case = t5("g2-on", vec![("G2", true)], true, true, true);
    let files: Vec<Arc<Vec<u8>>> = (0..2)
        .map(|_| {
            let built = twins::build(&case);
            let src = synth::open(&built.source);
            let prepared = prepare(&src, &[0], &visibility(&case, &built)).expect("prepares");
            synth::save(&layered_one(&prepared, &placement(&src)).expect("composes"), true)
        })
        .collect();
    assert_eq!(files[0], files[1]);
}

#[test]
fn decisions_that_cant_apply_are_errors() {
    let case = t5("errors", vec![], true, false, false);
    let built = twins::build(&case);
    let src = synth::open(&built.source);
    let g1 = r(built.numbers["G1"]);
    for (decisions, what) in [
        (vec![(r(3), true)], "the page"),
        (vec![(r(4), false)], "the font"),
        (vec![(r(999), true)], "an object that isn't in the file"),
        (vec![(g1, true), (g1, false)], "both on and off"),
    ] {
        let e = prepare(&src, &[0], &Visibility { decisions, ..Visibility::default() }).err();
        assert!(matches!(e, Some(OverlayError::Decision { .. })), "{what}: {e:?}");
    }
    // The same decision twice is the same decision.
    assert!(prepare(&src, &[0], &Visibility { decisions: vec![(g1, true), (g1, true)], ..Visibility::default() }).is_ok());
}

#[test]
fn a_decision_settles_exactly_what_it_names() {
    let mut objects = groups3();
    objects.push(("U1", group("Loose 1")));
    objects.push(("U2", group("Loose 2")));
    let case = |decisions| Case {
        name: "settles",
        objects: objects.clone(),
        oc_properties: Some(D.into()),
        properties: vec![("U1", "U1"), ("U2", "U2")],
        contents: vec![vec![BASE, Layer("U1", false, vec![Canary]), Layer("U2", false, vec![Canary])]],
        decisions,
        ..Case::default()
    };
    let names = |c: &Case| unresolved(c).into_iter().map(|(rule, _, n)| (rule, n)).collect::<Vec<_>>();
    assert_eq!(names(&case(vec![])), [(Rule::NotInOcgs, vec!["Loose 1".to_string()]), (Rule::NotInOcgs, vec!["Loose 2".to_string()])]);
    assert_eq!(names(&case(vec![("U1", false)])), [(Rule::NotInOcgs, vec!["Loose 2".to_string()])]);
    check_twin(&case(vec![("U1", false), ("U2", false)]));
}

// ── Pages without layers, rewriting, sources ─────────────────────────────────────────────────

/// A page with no optional content keeps its content byte for byte; only its resources are
/// reduced to what it draws, so another page's hidden content can't come along.
#[test]
fn pages_without_layers_keep_their_content() {
    let doc = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [7 0 R] /D << /OFF [7 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << /XObject << /H 6 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 5 0 R >>".into(),
        synth::raw_stream("", "0 g  10 10 20 20 re f % a comment\n"),
        synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /OC 7 0 R", &format!("q {CANARY_OPERANDS} rg 0 0 9 9 re f Q")),
        "<< /Type /OCG /Name (Hidden) >>".into(),
    ]));
    let prepared = prepare(&doc, &[0], &Visibility::default()).expect("prepares");
    let rep = &prepared.reports()[0];
    assert!(rep.unresolved.is_empty() && !rep.optional && rep.streams_rewritten == 0, "{rep:?}");
    assert_eq!(rep.resources_removed, 1);
    assert_eq!(synth::page_content(prepared.doc(), 0), synth::page_content(&doc, 0), "byte for byte");
    let out = layered_one(&prepared, &placement(&doc)).expect("composes");
    no_canary(&synth::open(&synth::save(&out, true)), "page without layers");
}

/// Rewriting removes the hidden block's bytes, comments included, and nothing else; the rest of
/// the stream is byte for byte what it was.
#[test]
fn rewriting_keeps_every_other_byte() {
    let doc = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Properties << /L 5 0 R >> >> /Contents 4 0 R >>".into(),
        synth::raw_stream("", "0 g  10 10 20 20 re f % kept\n/OC /L BDC % CANARY-7Q4\nq 1 0 0 rg 0 0 9 9 re f Q EMC\n  0.5 g 1 1 2 2 re f"),
        "<< /Type /OCG /Name (Hidden) >>".into(),
    ]));
    let prepared = prepare(&doc, &[0], &Visibility::default()).expect("prepares");
    let data = synth::page_content(prepared.doc(), 0);
    let text = String::from_utf8(data).expect("text");
    assert!(text.starts_with("0 g  10 10 20 20 re f % kept\n"), "{text:?}");
    assert!(text.trim_end().ends_with("\n  0.5 g 1 1 2 2 re f"), "{text:?}");
    assert!(!text.contains("CANARY") && !text.contains("1 0 0 rg"), "{text:?}");
}

/// L11: the source bytes and the in-memory documents are unchanged, for layered sources and their
/// overlays (the proof hashes the files with SHA-256 as well).
#[test]
fn the_sources_are_not_modified() {
    let case = t5("sources", vec![("G2", true)], true, true, true);
    let built = twins::build(&case);
    let before: Vec<u8> = built.source.as_slice().to_vec();
    let src = synth::open(&built.source);
    let prepared = prepare(&src, &[0], &visibility(&case, &built)).expect("prepares");
    let out = layered_one(&prepared, &placement(&src)).expect("composes");
    let _ = synth::save(&out, true);
    let _ = synth::save(prepared.doc(), true);
    assert!(!src.is_modified());
    assert_eq!(src.bytes().as_slice(), before.as_slice());
    assert_eq!(built.source.as_slice(), before.as_slice());
}

// ── No panics ────────────────────────────────────────────────────────────────────────────────

/// Seeded hostile sources through preparation: random content from layer markers, unbalanced
/// structure, text, paths, clips, forms that draw themselves, inline images and unreadable bytes,
/// under random configurations and decisions. Never a panic, each case well under 2 s; a page that
/// resolves composes into an output that passes the defensive check. (OV4.5 extends this to the
/// full 10,000-case sweep with annotations.)
#[test]
fn hostile_sources_never_panic() {
    const PIECES: &[&str] = &[
        "/OC /L1 BDC",
        "/OC /L2 BDC",
        "/OC /L3 BDC",
        "/OC /M BDC",
        "/OC /Nope BDC",
        "/OC << /Type /OCMD >> BDC",
        "EMC",
        "EMC",
        "/P << /MCID 0 >> BDC",
        "/Span BMC",
        "q",
        "Q",
        "BT /FC 12 Tf 10 10 Td",
        "ET",
        "(x) Tj",
        "2 0 TD",
        "T*",
        "1 2 (y) \"",
        "7 Tr",
        "0 Tr",
        "10 10 m",
        "20 20 l",
        "S",
        "f",
        "W n",
        "0 0 9 9 re",
        "0.5 g",
        "0 0 1 rg",
        "1 0 0 1 5 5 cm",
        "3 w",
        "/F1 Do",
        "/F2 Do",
        "/I1 Do",
        "/SM gs",
        "/Pattern cs /P1 scn",
        "BI /W 1 /H 1 /CS /G /BPC 8 ID x EI",
        "BI /W ) ID x EI",
        ")",
        "% comment",
        "/X",
    ];
    let mut rng = common::Rng(0x0154_0003);
    let configs = [
        "<< /OCGs [{G1} {G2} {G3}] /D << /OFF [{G2}] >> >>",
        "<< /OCGs [{G1} {G2} {G3}] /D << /BaseState /OFF /ON [{G1}] >> >>",
        "<< /OCGs [{G1} {G2}] /D << /BaseState /Unchanged >> >>",
        "<< /OCGs [{G1} {G2} {G3}] /D << /Intent /All /OFF [{G3}] /AS [<< /Event /View /Category [/Zoom] /OCGs [{G1}] >>] >> >>",
        "<< /OCGs {G1} /D 5 >>",
        "[]",
    ];
    let memberships = [
        "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AnyOff >>",
        "<< /Type /OCMD /VE [/Not [/And {G1} {G2}]] >>",
        "<< /Type /OCMD /VE [/Or {M}] >>",
        "<< /Type /OCMD /OCGs [] >>",
        "<< /Type /OCMD /OCGs 7 /P /Odd >>",
    ];
    let content = |rng: &mut common::Rng| {
        let n = (rng.next_u64() % 24) as usize;
        let mut s = String::from("0.2 g 20 20 100 40 re f\n");
        for _ in 0..n {
            s.push_str(PIECES[(rng.next_u64() % PIECES.len() as u64) as usize]);
            s.push('\n');
        }
        s
    };
    let (mut composed, mut unresolved_pages, mut refused) = (0, 0, 0);
    for i in 0..2_000 {
        let started = std::time::Instant::now();
        let mut objects = groups3();
        objects.push(("M", memberships[(rng.next_u64() % memberships.len() as u64) as usize].to_string()));
        let props = configs[(rng.next_u64() % configs.len() as u64) as usize];
        let page = content(&mut rng);
        let f1 = content(&mut rng);
        let f2 = content(&mut rng);
        let p1 = content(&mut rng);
        // Built by hand so the content is exactly the random text.
        let names: Vec<&str> = ["G1", "G2", "G3", "M", "F1", "F2", "I1", "P1", "SMG"].to_vec();
        let num = |n: &str| 6 + names.iter().position(|x| *x == n).expect("named") as u32;
        let refs = |s: &str| {
            let mut out = s.to_string();
            for n in &names {
                out = out.replace(&format!("{{{n}}}"), &format!("{} 0 R", num(n)));
            }
            out
        };
        let res = refs(
            "<< /Properties << /L1 {G1} /L2 {G2} /L3 {G3} /M {M} >> /Font << /FC 4 0 R >> /XObject << /F1 {F1} /F2 {F2} /I1 {I1} >> \
             /Pattern << /P1 {P1} >> /ExtGState << /SM << /SMask << /S /Luminosity /G {SMG} >> >> >> >>",
        );
        let f1_oc = if rng.next_u64().is_multiple_of(2) { refs("/OC {G2} ") } else { String::new() };
        let bodies = vec![
            refs(&format!("<< /Type /Catalog /Pages 2 0 R /OCProperties {props} >>")),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources {res} /Contents 5 0 R >>"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            synth::raw_stream("", &page),
            group("Base"),
            group("Notes"),
            group("Hatch"),
            refs(&objects[3].1),
            synth::raw_stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources {res} {f1_oc}"), &f1),
            synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200]", &f2),
            synth::raw_stream(&refs("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OC {M}"), "x"),
            synth::raw_stream(
                &format!("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 9 9] /XStep 9 /YStep 9 /Resources {res}"),
                &p1,
            ),
            synth::raw_stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group << /S /Transparency >>",
                "1 g 0 0 200 200 re f /OC /L2 BDC 0 g EMC",
            ),
        ];
        let bytes = synth::raw_pdf(&bodies);
        let Ok(doc) = Document::open(bytes.clone()) else { continue };
        let decisions = match rng.next_u64() % 4 {
            0 => vec![(r(num("G2")), true)],
            1 => vec![(r(num("M")), false)],
            _ => Vec::new(),
        };
        match prepare(&doc, &[0], &Visibility { decisions, ..Visibility::default() }) {
            Err(_) => refused += 1,
            Ok(prepared) => {
                if prepared.reports()[0].unresolved.is_empty() {
                    match layered_one(&prepared, &placement(&doc)) {
                        Ok(out) => {
                            composed += 1;
                            assert!(check_output(&out).is_ok(), "case {i}");
                            let _ = pdfcraft_cos::write_full(&out, &pdfcraft_cos::SaveOptions::default());
                        }
                        Err(_) => refused += 1,
                    }
                } else {
                    unresolved_pages += 1;
                }
            }
        }
        assert!(!doc.is_modified(), "case {i}: the source is only read");
        assert!(started.elapsed().as_secs_f64() < 2.0, "case {i} took {:?}", started.elapsed());
    }
    assert!(composed > 100 && unresolved_pages > 100, "both outcomes: {composed} composed, {unresolved_pages} unresolved, {refused} refused");
}

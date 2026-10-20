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

use common::catalogue::{self, BASE, D, MORE, group, groups3, props3, settles, simple, t5, with_ocmd};
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

use Node::{Canary, Do, Layer, Ops};

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

// ── L5: resolved cases (T4) ──────────────────────────────────────────────────────────────────

#[test]
fn default_configuration_lists() {
    check_twin(&catalogue::get("base-on-off-list"));
    // /BaseState OFF with /ON: followed as viewers do, with a note.
    let p = check_twin(&catalogue::get("base-off-on-list"));
    assert!(p.notes().contains(&Note::BaseStateOff));
    check_twin(&catalogue::get("on-off-lists"));
}

#[test]
fn nested_blocks_three_deep() {
    let p = check_twin(&catalogue::get("nested-3-deep"));
    let rep = &p.reports()[0];
    assert_eq!((rep.hidden_blocks, rep.markers_removed), (2, 2), "{rep:?}");
}

#[test]
fn membership_policies() {
    check_twin(&catalogue::get("ocmd-policies"));
}

#[test]
fn visibility_expressions() {
    let p = check_twin(&catalogue::get("visibility-expressions"));
    assert!(
        p.notes().iter().any(|n| matches!(n, Note::ExpressionIgnoredByDisplay { with_policy: false, display_differs: Some(true), .. })),
        "{:?}",
        p.notes()
    );
    assert!(p.notes().iter().any(|n| matches!(n, Note::ExpressionIgnoredByDisplay { with_policy: true, .. })), "{:?}", p.notes());
}

/// OV4.7, F1: `/VE` and `/P` that disagree are unresolved, in both directions, and nothing is
/// written; a decision on the dictionary settles it either way (twins).
#[test]
fn an_expression_and_a_policy_that_disagree_are_unresolved() {
    let case = |name, body: &str| simple(name, with_ocmd(body), D, vec![BASE, Layer("M", false, vec![Canary])]);
    for (name, body) in [
        // The review's E4: /VE shows the content, /P (and pdfcraft's display) hides it.
        ("ve-shows-p-hides", "<< /Type /OCMD /OCGs [{G2}] /P /AllOn /VE [/Or {G1}] >>"),
        ("ve-hides-p-shows", "<< /Type /OCMD /OCGs [{G1}] /P /AnyOn /VE [/Not {G1}] >>"),
    ] {
        let got = unresolved(&case(name, body));
        assert_eq!(got.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), [(Rule::ExpressionDisagrees, true)], "{name}");
    }
    // A valid expression beside an unknown policy: no fallback to the expression.
    let got = unresolved(&case("ve-with-unknown-policy", "<< /Type /OCMD /OCGs [{G1}] /P /Most /VE [/Or {G1}] >>"));
    assert_eq!(got.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), [(Rule::UnknownPolicy, true)]);
    check_twin(&catalogue::get("ve-p-disagree-shown"));
    check_twin(&catalogue::get("ve-p-disagree-hidden"));
}

/// OV4.7, F2: deleted members of `/OCGs` that change the result are unresolved.
#[test]
fn deleted_members_that_change_the_result_are_unresolved() {
    let case = |name, body: &str| simple(name, with_ocmd(body), D, vec![BASE, Layer("M", false, vec![Canary])]);
    let got = unresolved(&case("deleted-all-off", "<< /Type /OCMD /OCGs [999 0 R] /P /AllOff >>"));
    assert_eq!(got.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(), [(Rule::DeletedMember, true)]);
    // Agreeing readings stand: G2 is off, so AllOn is off whether the deleted member counts or not.
    check_twin(&simple(
        "deleted-agree",
        with_ocmd("<< /Type /OCMD /OCGs [{G2} 999 0 R] /P /AllOn >>"),
        D,
        vec![BASE, Layer("M", false, vec![Canary])],
    ));
}

#[test]
fn optional_content_on_forms_and_images() {
    let p = check_twin(&catalogue::get("oc-on-form-and-image"));
    assert_eq!(p.reports()[0].hidden_xobjects, 2);
}

#[test]
fn one_form_drawn_in_a_visible_and_a_hidden_block() {
    check_twin(&catalogue::get("form-in-visible-and-hidden"));
}

#[test]
fn markers_split_across_content_streams() {
    check_twin(&catalogue::get("split-across-streams"));
}

#[test]
fn tagged_content_inside_layers() {
    check_twin(&catalogue::get("tagged-text"));
}

#[test]
fn intents_matched_with_the_configuration() {
    check_twin(&catalogue::get("intent-all"));
    check_twin(&catalogue::get("intent-view-design"));
}

#[test]
fn layers_inside_forms_patterns_and_soft_masks() {
    check_twin(&catalogue::get("inside-forms"));
}

/// Graphics-state changes in hidden content that later content uses are kept (viewers apply
/// them); the rest goes with the block.
#[test]
fn live_state_in_hidden_blocks_is_kept() {
    let p = check_twin(&catalogue::get("live-state"));
    let rep = &p.reports()[0];
    assert_eq!((rep.kept_state, rep.kept_clips), (3, 1), "{rep:?}");
}

/// Every case of the catalogue (the proof renders the same ones).
#[test]
fn the_whole_catalogue_matches_its_twins() {
    for case in catalogue::resolved() {
        check_twin(&case);
    }
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

#[test]
fn configuration_and_group_rules_are_unresolved() {
    let one = |rule, settleable: bool, names: &[&str]| vec![(rule, settleable, names.iter().map(|s| s.to_string()).collect::<Vec<_>>())];
    // A group missing from /OCGs that /OFF names (OV4.10, U4: PdfCraft hides it, Poppler shows it).
    assert_eq!(
        unresolved(&simple("not-in-ocgs", groups3(), "<< /OCGs [{G1} {G2}] /D << /OFF [{G3}] >> >>", vec![BASE, Layer("L3", false, vec![Canary])])),
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
    let p = check_twin(&catalogue::get("as-view-no-change-and-print"));
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
    // A marker whose name the resources don't define isn't optional content (ISO 32000-1
    // §8.11.3.2): its content is shown, as viewers show it, with a note (real Bluebeam sets).
    let undefined = c("undefined-marker", vec![BASE, Ops("/OC /Nope BDC"), MORE, Ops("EMC")]);
    let built = twins::build(&undefined);
    let src = synth::open(&built.source);
    let p = prepare(&src, &[0], &visibility(&undefined, &built)).expect("prepares");
    assert!(p.reports()[0].unresolved.is_empty(), "{:?}", p.reports()[0].unresolved);
    assert_eq!(p.reports()[0].markers_removed, 1);
    assert!(p.notes().contains(&Note::UndefinedMarker));
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

// ── OV4.9: marker names a stream's own resources don't define ───────────────────────────────

/// A fill that has to stay visible, and its colour operator as it is copied.
const KEEP: &str = "0.2 0.4 0.6 rg 50 50 20 20 re f";
const KEEP_COLOUR: &[u8] = b"0.2 0.4 0.6 rg";
/// A catalog whose default configuration hides the group, object 4.
const HIDES_4: &str = "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [4 0 R] /D << /OFF [4 0 R] >> >> >>";
/// A catalog whose default configuration shows it.
const SHOWS_4: &str = "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [4 0 R] /D << >> >> >>";
const GROUP: &str = "<< /Type /OCG /Name (Layer) >>";
const ONE_PAGE: &str = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";

/// Fills a box in the canary colour.
fn canary() -> String {
    format!("{CANARY_OPERANDS} rg 10 10 30 30 re f")
}

/// Page object 3: a 100 × 100 page with `entries` (its resources, annotations) drawing object 5.
fn page3(entries: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] {entries} /Contents 5 0 R >>")
}

fn form(dict: &str, content: &str) -> String {
    synth::raw_stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 100 100] {dict}"), content)
}

fn drawn(doc: &Document, needle: &[u8]) -> bool {
    doc.scan_objects().any(|(_, o)| matches!(&*o, Object::Stream(s) if s.decoded().is_ok_and(|d| contains(&d, needle))))
}

fn prepare_raw(objects: &[String], pages: &[usize]) -> (Document, Prepared) {
    let doc = synth::open(&synth::raw_pdf(objects));
    let prepared = prepare(&doc, pages, &Visibility::default()).expect("prepares");
    assert!(!doc.is_modified(), "the source is only read");
    (doc, prepared)
}

fn layered_page(prepared: &Prepared, page: usize, p: &Placement) -> Result<Document, OverlayError> {
    compose_layered(Some(prepared.source(page)?), None, p, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED)
}

/// Page `page` resolves, and its overlay keeps the visible fill and carries no canary: not in the
/// overlay's streams, and not in the pixels of the prepared page.
fn resolves(what: &str, prepared: &Prepared, doc: &Document, page: usize) {
    let report = prepared.reports().iter().find(|r| r.page == page).expect("report");
    assert!(report.unresolved.is_empty(), "{what}: {:?}", report.unresolved);
    let out = layered_page(prepared, page, &placement(doc)).unwrap_or_else(|e| panic!("{what}: {e}"));
    let bytes = synth::save(&out, true);
    let back = synth::open(&bytes);
    assert!(check_output(&back).is_ok(), "{what}");
    no_canary(&back, what);
    assert!(drawn(&back, KEEP_COLOUR), "{what}: the visible fill is missing");
    if page == 0 {
        no_canary_pixel(&render(&synth::save(prepared.doc(), true), 72.0), what);
    }
}

/// Page `page` is unresolved with `UnknownProperty` alone, naming `object`, which no decision
/// settles, and no overlay is made from it.
fn refused(what: &str, prepared: &Prepared, doc: &Document, page: usize, object: ObjRef) {
    let report = prepared.reports().iter().find(|r| r.page == page).expect("report");
    let u = &report.unresolved;
    assert_eq!(u.iter().map(|u| (u.rule, u.settleable)).collect::<Vec<_>>(), [(Rule::UnknownProperty, false)], "{what}: {u:?}");
    assert_eq!(u[0].objects.iter().map(|o| o.0).collect::<Vec<_>>(), [object], "{what}");
    assert!(layered_page(prepared, page, &placement(doc)).is_err(), "{what}: an overlay was made");
}

/// The targeted check for OV4.8's relaxation (content under an `/OC` name the stream's own
/// resources don't define is shown, as PdfCraft's display shows it): no layer that is hidden
/// through a valid, resolvable optional-content relationship becomes visible through it, and a
/// name a surrounding page or drawing defines as a hidden or undecidable layer (OV4.9) is
/// unresolved, because viewers disagree there.
///
/// Limits of this check:
/// - It checks the resolver on small synthetic files, in the overlay's own output and in
///   PdfCraft's rendering of the prepared page; it is not a cross-renderer proof.
/// - The viewer disagreement behind OV4.9 rests on one black-box observation of Poppler 24.02
///   (it looks marker names up through the forms and the page that draw a stream; PdfCraft's
///   display doesn't). pdf.js, PDFium and Acrobat weren't observed for these files, and no viewer
///   is taken as correct: the refusal follows from the disagreement and the conservative
///   source-visibility policy, not from the surrounding dictionary being authoritative.
/// - The surrounding dictionaries read are those of the streams and page drawing a stream and of
///   the page's ancestors in the page tree; names reachable any other way aren't looked for.
/// - Inline property lists after `/OC` stay unresolved (`Rule::Inline`), and markers whose
///   resources define the name are decided by the evaluator as before (its own tests).
/// - Judging by the vendored renderer's code (not rendered here), PdfCraft's display also shows
///   content in a form without resources, or on a page without resources, whose inherited
///   `/Properties` hide it; the overlay hides it, as the standard says, so there the output shows
///   less than the source's tab.
#[test]
fn undefined_marker_names_never_show_a_hidden_layer() {
    let c = canary();
    let page_hides = "/Resources << /Properties << /MC0 4 0 R >> >>";
    let simple = |entries: &str, content: String| -> Vec<String> {
        vec![HIDES_4.into(), ONE_PAGE.into(), page3(entries), GROUP.into(), synth::raw_stream("", &content)]
    };

    // Hidden through valid relationships, beside or around undefined names.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("an undefined name beside a hidden layer", simple(page_hides, format!("/OC /MC0 BDC {c} EMC /OC /Nope BDC {KEEP} EMC"))),
        ("an undefined marker around a hidden one", simple(page_hides, format!("/OC /Nope BDC /OC /MC0 BDC {c} EMC {KEEP} EMC"))),
        ("a hidden marker around an undefined one", simple(page_hides, format!("/OC /MC0 BDC /OC /Nope BDC {c} EMC EMC {KEEP}"))),
        (
            "a page inheriting all its resources",
            vec![
                HIDES_4.into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /Properties << /MC0 4 0 R >> >> >>".into(),
                page3(""),
                GROUP.into(),
                synth::raw_stream("", &format!("/OC /MC0 BDC {c} EMC {KEEP}")),
            ],
        ),
        (
            "a form without resources, inheriting the page's",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /Properties << /MC0 4 0 R >> /XObject << /F 6 0 R >> >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/F Do {KEEP}")),
                form("", &format!("/OC /MC0 BDC {c} EMC")),
            ],
        ),
        (
            "a form's own layer, beside an undefined name",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /XObject << /F 6 0 R >> >>"),
                GROUP.into(),
                synth::raw_stream("", "/F Do"),
                form("/Resources << /Properties << /MC0 4 0 R >> >>", &format!("/OC /MC0 BDC {c} EMC /OC /Nope BDC {KEEP} EMC")),
            ],
        ),
        (
            "a hidden form with an undefined marker inside",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /XObject << /F 6 0 R >> >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/F Do {KEEP}")),
                form("/OC 4 0 R /Resources << >>", &format!("/OC /Nope BDC {c} EMC")),
            ],
        ),
        (
            "properties given by reference",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /Properties 6 0 R >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/OC /MC0 BDC {c} EMC /OC /Nope BDC {KEEP} EMC")),
                "<< /MC0 4 0 R >>".into(),
            ],
        ),
    ];
    for (what, objects) in &cases {
        let (doc, prepared) = prepare_raw(objects, &[0]);
        resolves(what, &prepared, &doc, 0);
    }

    // A name the stream's own resources don't define, which a surrounding page or drawing defines
    // as a hidden or undecidable layer: viewers disagree, so it is unresolved.
    let cases: Vec<(&str, Vec<String>, u32)> = vec![
        (
            "a form with its own resources on a page that hides the name",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /Properties << /MC0 4 0 R >> /XObject << /F 6 0 R >> >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/F Do {KEEP}")),
                form("/Resources << /ProcSet [/PDF] >>", &format!("/OC /MC0 BDC {c} EMC")),
            ],
            4,
        ),
        (
            "a form in a form on a page that hides the name",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /Properties << /MC0 4 0 R >> /XObject << /F 6 0 R >> >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/F Do {KEEP}")),
                form("/Resources << /XObject << /G 7 0 R >> >>", "/G Do"),
                form("/Resources << >>", &format!("/OC /MC0 BDC {c} EMC")),
            ],
            4,
        ),
        (
            "a page with its own resources whose ancestor hides the name",
            vec![
                HIDES_4.into(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /Properties << /MC0 4 0 R >> >> >>".into(),
                page3("/Resources << /ProcSet [/PDF] >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/OC /MC0 BDC {c} EMC {KEEP}")),
            ],
            4,
        ),
        (
            "an annotation's appearance on a page that hides the name",
            vec![
                HIDES_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /Properties << /MC0 4 0 R >> >> /Annots [6 0 R]"),
                GROUP.into(),
                synth::raw_stream("", KEEP),
                "<< /Type /Annot /Subtype /Square /Rect [10 10 60 60] /F 4 /AP << /N 7 0 R >> >>".into(),
                synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 50 50] /Resources << >>", &format!("/OC /MC0 BDC {c} EMC")),
            ],
            4,
        ),
        (
            "a page that defines the name as a membership no one can decide",
            vec![
                SHOWS_4.into(),
                ONE_PAGE.into(),
                page3("/Resources << /Properties << /MC0 6 0 R >> /XObject << /F 7 0 R >> >>"),
                GROUP.into(),
                synth::raw_stream("", &format!("/F Do {KEEP}")),
                "<< /Type /OCMD /OCGs [4 0 R] /P /Bogus >>".into(),
                form("/Resources << >>", &format!("/OC /MC0 BDC {c} EMC")),
            ],
            6,
        ),
    ];
    for (what, objects, object) in &cases {
        let (doc, prepared) = prepare_raw(objects, &[0]);
        refused(what, &prepared, &doc, 0, r(*object));
    }

    // Shown, as viewers agree: a name defined nowhere (OV4.8, with its note), one a surrounding
    // page defines as a layer that is on, and one in a file without layers.
    let on_page = |catalog: &str| -> Vec<String> {
        vec![
            catalog.into(),
            ONE_PAGE.into(),
            page3("/Resources << /Properties << /MC0 4 0 R >> /XObject << /F 6 0 R >> >>"),
            GROUP.into(),
            synth::raw_stream("", "/F Do"),
            form("/Resources << >>", &format!("/OC /MC0 BDC {KEEP} EMC")),
        ]
    };
    let nowhere: Vec<String> = vec![
        HIDES_4.into(),
        ONE_PAGE.into(),
        page3("/Resources << /XObject << /F 6 0 R >> >>"),
        GROUP.into(),
        synth::raw_stream("", "/F Do"),
        form("/Resources << >>", &format!("/OC /MC0 BDC {KEEP} EMC")),
    ];
    for (what, objects, note) in
        [("a name defined nowhere", nowhere, true), ("a name a surrounding page defines as a layer that is on", on_page(SHOWS_4), false)]
    {
        let (doc, prepared) = prepare_raw(&objects, &[0]);
        resolves(what, &prepared, &doc, 0);
        assert_eq!(prepared.notes().contains(&Note::UndefinedMarker), note, "{what}: {:?}", prepared.notes());
    }
    // Without `/OCProperties` the surroundings aren't read: the page resolves with the note. (No
    // overlay is made of this file, but not because of the marker: nothing is pruned in a file
    // without layers, so the page's resources still carry the group, and the defensive check
    // refuses an output with any layer but Old and New.)
    let (_, prepared) = prepare_raw(&on_page("<< /Type /Catalog /Pages 2 0 R >>"), &[0]);
    assert!(prepared.reports()[0].unresolved.is_empty(), "{:?}", prepared.reports()[0].unresolved);
    assert!(prepared.notes().contains(&Note::UndefinedMarker));
    assert!(drawn(prepared.doc(), KEEP_COLOUR));
}

/// A form is resolved once and reused wherever it is drawn, but whether content under a name its
/// own resources don't define may be shown depends on where it is drawn (OV4.9). The same form
/// drawn under different surrounding resources is decided in each place, whichever comes first.
#[test]
fn a_reused_form_is_decided_where_it_is_drawn() {
    let shared = form("/Resources << /ProcSet [/PDF] >>", &format!("/OC /MC0 BDC {KEEP} EMC"));
    // Two pages draw the form: object 3 doesn't define the name, object 8 hides it.
    for kids in ["[3 0 R 8 0 R]", "[8 0 R 3 0 R]"] {
        let objects = vec![
            HIDES_4.into(),
            format!("<< /Type /Pages /Kids {kids} /Count 2 >>"),
            page3("/Resources << /XObject << /F 6 0 R >> >>"),
            GROUP.into(),
            synth::raw_stream("", "/F Do"),
            shared.clone(),
            synth::raw_stream("", "/F Do"),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Properties << /MC0 4 0 R >> /XObject << /F 6 0 R >> >> /Contents 7 0 R >>"
                .into(),
        ];
        let (plain, hiding) = if kids.starts_with("[3") { (0, 1) } else { (1, 0) };
        let what = format!("pages {kids}");
        for pages in [vec![0, 1], vec![plain], vec![hiding]] {
            let (doc, prepared) = prepare_raw(&objects, &pages);
            if pages.contains(&plain) {
                resolves(&what, &prepared, &doc, plain);
            }
            if pages.contains(&hiding) {
                refused(&what, &prepared, &doc, hiding, r(4));
            }
        }
    }

    // On one page, the form drawn directly and through a form whose resources hide the name, in
    // both orders (forms are followed in name order).
    for (direct, wrapper) in [("A", "B"), ("B", "A")] {
        let objects = vec![
            HIDES_4.into(),
            ONE_PAGE.into(),
            page3(&format!("/Resources << /XObject << /{direct} 6 0 R /{wrapper} 7 0 R >> >>")),
            GROUP.into(),
            synth::raw_stream("", &format!("/{direct} Do /{wrapper} Do")),
            shared.clone(),
            form("/Resources << /Properties << /MC0 4 0 R >> /XObject << /F 6 0 R >> >>", "/F Do"),
        ];
        let (doc, prepared) = prepare_raw(&objects, &[0]);
        refused(&format!("direct /{direct}, wrapped /{wrapper}"), &prepared, &doc, 0, r(4));
    }
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
    let a = twins::build(&simple("a", groups3(), "<< /OCGs [{G1} {G2}] /D << /OFF [{G3}] >> >>", vec![BASE, Layer("L3", false, vec![Canary])]));
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

#[test]
fn explicit_states_give_their_twins() {
    // The default configuration: G1 on, G2 off, so M (AllOn) is off.
    check_twin(&catalogue::get("default"));
    // Two explicit states.
    check_twin(&catalogue::get("g2-on"));
    check_twin(&catalogue::get("g1-off"));
    // A decision on the membership dictionary itself.
    check_twin(&catalogue::get("m-on"));
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

/// OV4.10: a group `/OCGs` doesn't list, in a configuration that doesn't name it in `/OFF`, or
/// in a file without `/OCProperties`, is shown as the source shows it. Its content is drawn, its
/// marker removed, and nothing is asked, including under `/BaseState /OFF`; the report notes it.
#[test]
fn undeclared_groups_are_shown_as_the_source_shows_them() {
    for (name, props) in [
        ("undeclared", Some("<< /OCGs [{G1} {G2}] /D << /OFF [{G2}] >> >>")),
        ("undeclared-base-off", Some("<< /OCGs [{G1} {G2}] /D << /BaseState /OFF /ON [{G1}] >> >>")),
        ("undeclared-no-properties", None),
    ] {
        let mut case = simple(name, groups3(), "", vec![BASE, Layer("L1", true, vec![MORE]), Layer("L3", true, vec![MORE])]);
        case.oc_properties = props.map(str::to_string);
        if props.is_none() {
            // Without a configuration every group shows.
            case.contents = vec![vec![BASE, Layer("L1", true, vec![MORE]), Layer("L2", true, vec![MORE]), Layer("L3", true, vec![MORE])]];
        }
        let prepared = check_twin(&case);
        assert!(prepared.notes().iter().any(|n| matches!(n, Note::UndeclaredShown { .. })), "{name}: {:?}", prepared.notes());
    }
}

/// OV4.10 inside forms: a form whose `/OC` is an undeclared group, drawing a second form whose
/// stream marks content with it; both are shown, their markers and `/OC` removed.
#[test]
fn undeclared_groups_inside_forms_are_shown() {
    let mut case =
        simple("undeclared-forms", groups3(), "<< /OCGs [{G1} {G2}] /D << /OFF [{G2}] >> >>", vec![BASE, Do("F5"), Layer("L2", false, vec![Canary])]);
    case.xobjects = vec![
        XObject::form("F5", vec![Ops("0 0.5 0 rg 10 100 30 30 re f"), Do("F6")]).with_oc("G3", true),
        XObject::form("F6", vec![Layer("L3", true, vec![MORE])]),
    ];
    check_twin(&case);
}

#[test]
fn a_decision_settles_exactly_what_it_names() {
    let names = |c: &Case| unresolved(c).into_iter().map(|(rule, _, n)| (rule, n)).collect::<Vec<_>>();
    assert_eq!(names(&settles(vec![])), [(Rule::NotInOcgs, vec!["Loose 1".to_string()]), (Rule::NotInOcgs, vec!["Loose 2".to_string()])]);
    assert_eq!(names(&settles(vec![("U1", false)])), [(Rule::NotInOcgs, vec!["Loose 2".to_string()])]);
    check_twin(&settles(vec![("U1", false), ("U2", false)]));
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

/// T10 (the proof renders it in full): the drawing-like pair prepares cleanly, with HVAC hidden by
/// default and shown when decided on; its annotations are drawn.
#[test]
fn the_drawing_pair_resolves() {
    use common::drawing::{self, HVAC, HVAC_OPERANDS, Rev};
    for rev in [Rev::Old, Rev::New] {
        let doc = synth::open(&drawing::sheet(rev));
        for (decisions, shown) in [(Vec::new(), false), (vec![(r(HVAC), true)], true)] {
            let p = prepare(&doc, &[0], &Visibility { decisions, ..Visibility::default() }).expect("prepares");
            let rep = &p.reports()[0];
            assert!(rep.unresolved.is_empty(), "{rev:?}: {:?}", rep.unresolved);
            assert!(rep.annotations.iter().all(|a| matches!(a.outcome, pdfcraft_overlay::Outcome::Drawn(_))), "{:?}", rep.annotations);
            let out = layered_one(&p, &placement(&doc)).expect("composes");
            let back = synth::open(&synth::save(&out, true));
            let has_hvac = back
                .scan_objects()
                .any(|(_, o)| matches!(&*o, Object::Stream(s) if s.decoded().is_ok_and(|d| contains(&d, HVAC_OPERANDS.as_bytes()))));
            assert_eq!(has_hvac, shown, "{rev:?}: HVAC shown {shown}");
        }
    }
}

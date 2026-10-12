//! The output's two layers (OV4.1): structure (L1), toggling in PdfCraft through the Layers
//! panel's own render overrides (L2, in process), OV3's page unchanged with both layers on (L4,
//! structure and pixels), roles and colours (O1), metadata (O2), the defensive check, and no
//! panics. Step 4 PRD.

mod common;

use std::sync::Arc;

use common::families::{self, Case, Set};
use common::measure::Image;
use common::synth::{self, Page};
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_overlay::{
    CYAN, LayerStyle, LayeredOutput, OverlayError, Placement, RED, RegistrationKind, RegistrationNote, Revision, Roles, Settings, SheetMatch, Source,
    Style, check_output, compose, compose_layered, frame_of_page, place,
};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

fn src(page: Page) -> Document {
    synth::open(&synth::pdf(&[page]))
}

fn placement(a: &Document, b: &Document) -> Placement {
    let (fa, _) = frame_of_page(a, 0).expect("frame A");
    let (fb, _) = frame_of_page(b, 0).expect("frame B");
    place(&fa, &fb, SheetMatch::Off, &Matrix::IDENTITY, false, &Settings::default()).expect("placement")
}

fn two_pages() -> (Document, Document) {
    (src(Page::new(612.0, 792.0, "0 g 72 72 144 144 re f")), src(Page::new(612.0, 792.0, "0 g 100 100 144 144 re f")))
}

fn written(doc: &Document) -> Document {
    synth::open(&synth::save(doc, true))
}

fn dict(doc: &Document, o: &Object) -> Dict {
    doc.dict(o).unwrap_or_else(|| panic!("not a dictionary: {o:?}"))
}

fn data(doc: &Document, o: &Object) -> Vec<u8> {
    match &*doc.resolve(o) {
        Object::Stream(s) => s.decoded().expect("decodes"),
        other => panic!("not a stream: {other:?}"),
    }
}

fn page_dict(doc: &Document, i: usize) -> Dict {
    pdfcraft_model::pages(doc).get(i).expect("page").dict.clone()
}

fn refs(doc: &Document, o: Option<&Object>) -> Vec<ObjRef> {
    doc.resolve(o.expect("present")).as_array().expect("array").iter().map(|x| x.as_ref().expect("a reference")).collect()
}

fn text(doc: &Document, o: Option<&Object>) -> String {
    doc.resolve(o.expect("present")).as_string().expect("a string").to_text()
}

/// (operator, operands as written) of a content stream.
fn ops(bytes: &[u8]) -> Vec<(String, Vec<Object>)> {
    parse(bytes).ops.into_iter().map(|o| (String::from_utf8_lossy(&o.op).into_owned(), o.operands)).collect()
}

fn layered(a: Option<Source<'_>>, b: Option<Source<'_>>, p: &Placement, style: &LayerStyle, roles: Roles) -> Document {
    compose_layered(a, b, p, style, roles, RegistrationNote::PLACED).expect("composes")
}

/// L1: exactly two layers, `Old` and `New`, both on, listed Old then New; nothing else.
#[test]
fn the_output_has_exactly_two_layers() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    let out = written(&layered(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &LayerStyle::default(), Roles::default()));
    let catalog = dict(&out, &Object::Ref(out.root().expect("catalog")));
    let props = dict(&out, catalog.get(b"OCProperties").expect("optional content properties"));
    let ocgs = refs(&out, props.get(b"OCGs"));
    assert_eq!(ocgs.len(), 2);
    for (r, name) in ocgs.iter().zip(["Old", "New"]) {
        let g = dict(&out, &Object::Ref(*r));
        assert_eq!(g.name(b"Type"), Some(&b"OCG"[..]));
        assert_eq!(text(&out, g.get(b"Name")), name);
        assert_eq!(g.len(), 2, "a plain group: /Type and /Name only");
    }
    let d = dict(&out, props.get(b"D").expect("default configuration"));
    assert_eq!(d.name(b"BaseState"), Some(&b"ON"[..]));
    assert_eq!(refs(&out, d.get(b"ON")), ocgs);
    assert!(refs(&out, d.get(b"OFF")).is_empty());
    assert_eq!(refs(&out, d.get(b"Order")), ocgs);
    for key in [&b"Locked"[..], b"RBGroups", b"AS", b"Intent"] {
        assert!(!d.contains(key));
    }
    assert!(!props.contains(b"Configs"));

    // No other group or membership dictionary anywhere, and no /OC entry.
    let mut groups = 0;
    for (_, o) in out.scan_objects() {
        if let Some(d) = o.as_dict() {
            assert_ne!(d.name(b"Type"), Some(&b"OCMD"[..]));
            assert!(!d.contains(b"OC"));
            if d.name(b"Type") == Some(&b"OCG"[..]) {
                groups += 1;
            }
        }
    }
    assert_eq!(groups, 2);
    assert_eq!(check_output(&out).map(|g| [g.old, g.new]), Ok([ocgs[0], ocgs[1]]), "the saved file passes the check too");

    // The page maps its two markers to the two groups and wraps each fill in one.
    let page = page_dict(&out, 0);
    let resources = dict(&out, page.get(b"Resources").expect("resources"));
    let properties = dict(&out, resources.get(b"Properties").expect("properties"));
    assert_eq!(properties.reference(b"PCOld"), Some(ocgs[0]));
    assert_eq!(properties.reference(b"PCNew"), Some(ocgs[1]));
    assert_eq!(properties.len(), 2);
    let names: Vec<String> = ops(&data(&out, page.get(b"Contents").expect("contents"))).into_iter().map(|(o, _)| o).collect();
    assert_eq!(names, ["BDC", "q", "gs", "rg", "re", "f", "Q", "EMC", "BDC", "q", "gs", "rg", "re", "f", "Q", "EMC"]);
}

/// L4 (structure): with both layers on, the page is OV3's page plus the two marker pairs. The
/// graphics states, masks, forms and transfer function are written exactly as OV3 writes them.
#[test]
fn with_both_layers_on_the_page_is_ov3s_page_plus_the_markers() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    let (sa, sb) = (Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }));
    let ov3 = written(&compose(sa, sb, &p, &Style::default()).expect("composes"));
    let ov4 = written(&layered(sa, sb, &p, &LayerStyle::default(), Roles::default()));
    let (p3, p4) = (page_dict(&ov3, 0), page_dict(&ov4, 0));
    let c3 = ops(&data(&ov3, p3.get(b"Contents").expect("contents")));
    let c4 = ops(&data(&ov4, p4.get(b"Contents").expect("contents")));
    let without_markers: Vec<_> = c4.iter().filter(|(o, _)| o != "BDC" && o != "EMC").cloned().collect();
    assert_eq!(without_markers, c3, "every operator and operand of OV3's page, in order");
    let markers: Vec<_> = c4.iter().filter(|(o, _)| o == "BDC").map(|(_, v)| v.clone()).collect();
    assert_eq!(markers, vec![vec![Object::name("OC"), Object::name("PCOld")], vec![Object::name("OC"), Object::name("PCNew")]]);

    // Everything the page draws with is the same: the same object graph under /ExtGState.
    let states = |doc: &Document, page: &Dict| dict(doc, dict(doc, page.get(b"Resources").expect("resources")).get(b"ExtGState").expect("states"));
    for name in [&b"GA"[..], b"GB"] {
        let (g3, g4) = (dict(&ov3, states(&ov3, &p3).get(name).expect("state")), dict(&ov4, states(&ov4, &p4).get(name).expect("state")));
        let (m3, m4) = (dict(&ov3, g3.get(b"SMask").expect("mask")), dict(&ov4, g4.get(b"SMask").expect("mask")));
        assert_eq!(data(&ov3, m3.get(b"G").expect("group")), data(&ov4, m4.get(b"G").expect("group")));
        let form = |doc: &Document, m: &Dict| {
            let g = dict(doc, m.get(b"G").expect("group"));
            data(
                doc,
                dict(doc, g.get(b"Resources").expect("resources"))
                    .get(b"XObject")
                    .and_then(|x| dict(doc, x).get(b"S").cloned())
                    .as_ref()
                    .expect("form"),
            )
        };
        assert_eq!(form(&ov3, &m3), form(&ov4, &m4), "the source form, byte for byte");
        assert_eq!(dict(&ov3, m3.get(b"TR").expect("tr")), dict(&ov4, m4.get(b"TR").expect("tr")));
    }
}

/// O1: A is Old and B is New by default; swapped, A is New. Colours follow the role.
#[test]
fn roles_can_be_swapped_and_colours_follow_the_role() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    let rgb = |c: [u8; 3]| c.iter().map(|v| Object::Real(f64::from(*v) / 255.0)).collect::<Vec<_>>();
    for (roles, first, second) in [(Roles::default(), ("PCOld", RED), ("PCNew", CYAN)), (Roles::SWAPPED, ("PCNew", CYAN), ("PCOld", RED))] {
        let out = written(&layered(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &LayerStyle::default(), roles));
        let page = page_dict(&out, 0);
        let c = ops(&data(&out, page.get(b"Contents").expect("contents")));
        // A is drawn first (graphics state GA), as in OV3.
        assert_eq!(c[0].1, vec![Object::name("OC"), Object::name(first.0)]);
        assert_eq!(c[2].1, vec![Object::name("GA")]);
        assert_eq!(c[3].1, rgb(first.1));
        assert_eq!(c[8].1, vec![Object::name("OC"), Object::name(second.0)]);
        assert_eq!(c[10].1, vec![Object::name("GB")]);
        assert_eq!(c[11].1, rgb(second.1));
    }
    // Colours chosen by the caller follow the role too.
    let style = LayerStyle { old: [10, 20, 30], new: [200, 210, 220], threshold: 1.0 };
    let out = written(&layered(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &style, Roles::SWAPPED));
    let c = ops(&data(&out, page_dict(&out, 0).get(b"Contents").expect("contents")));
    assert_eq!(c[3].1, rgb([200, 210, 220]), "A is New");
    assert_eq!(c[11].1, rgb([10, 20, 30]), "B is Old");

    for role in [Revision::Old, Revision::New] {
        let r = compose_layered(
            Some(Source { doc: &a, page: 0 }),
            Some(Source { doc: &b, page: 0 }),
            &p,
            &LayerStyle::default(),
            Roles { a: role, b: role },
            RegistrationNote::PLACED,
        );
        assert_eq!(r.err(), Some(OverlayError::SameRole { role: role.name() }));
    }
}

/// A blank side gets no marker and no property; the other layer is still declared.
#[test]
fn a_blank_side_has_no_marker() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    for (one_a, marker) in [(true, "PCOld"), (false, "PCNew")] {
        let (sa, sb) = if one_a { (Some(Source { doc: &a, page: 0 }), None) } else { (None, Some(Source { doc: &b, page: 0 })) };
        let out = written(&layered(sa, sb, &p, &LayerStyle::default(), Roles::default()));
        let page = page_dict(&out, 0);
        let properties = dict(&out, dict(&out, page.get(b"Resources").expect("resources")).get(b"Properties").expect("properties"));
        assert_eq!(properties.len(), 1);
        assert!(properties.get(marker.as_bytes()).is_some());
        let c = ops(&data(&out, page.get(b"Contents").expect("contents")));
        assert_eq!(c.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(), ["BDC", "q", "gs", "rg", "re", "f", "Q", "EMC"]);
        assert_eq!(c[0].1, vec![Object::name("OC"), Object::name(marker)]);
        assert!(check_output(&out).is_ok());
    }
    assert_eq!(
        compose_layered(None, None, &p, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED).err(),
        Some(OverlayError::NoSources)
    );
}

/// O2: the document information's /Subject and the XMP dc:description state each page's
/// registration and residual; no file name goes in.
#[test]
fn the_metadata_states_the_registration() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    let mut out = LayeredOutput::new().expect("output");
    let notes = [
        RegistrationNote { kind: RegistrationKind::Affine, rms_mm: Some(0.1234), max_mm: Some(0.25) },
        RegistrationNote { kind: RegistrationKind::Similarity, rms_mm: Some(0.0), max_mm: Some(0.0) },
        RegistrationNote::PLACED,
        RegistrationNote { kind: RegistrationKind::Matrix, rms_mm: None, max_mm: None },
    ];
    for n in notes {
        out.add_page(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &LayerStyle::default(), Roles::default(), n)
            .expect("page");
    }
    let doc = written(&out.finish().expect("finishes"));
    let info = dict(&doc, doc.trailer().get(b"Info").expect("document information"));
    let subject = text(&doc, info.get(b"Subject"));
    assert_eq!(
        subject,
        "Overlay comparison with two layers, Old (previous revision) and New (updated revision). \
         Page 1: affine registration, RMS residual 0.123 mm, largest residual 0.250 mm. \
         Page 2: similarity registration, RMS residual 0.000 mm, largest residual 0.000 mm. \
         Page 3: no registration (placed by the sheet match). \
         Page 4: registration matrix given."
    );
    let catalog = dict(&doc, &Object::Ref(doc.root().expect("catalog")));
    let meta = catalog.get(b"Metadata").expect("XMP metadata");
    let xmp = String::from_utf8(data(&doc, meta)).expect("UTF-8");
    assert!(xmp.contains(&format!("<rdf:li xml:lang=\"x-default\">{subject}</rdf:li>")), "{xmp}");
    assert_eq!(pdfcraft_model::pages(&doc).len(), 4);

    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        let note = RegistrationNote { kind: RegistrationKind::Affine, rms_mm: Some(bad), max_mm: None };
        let r = compose_layered(Some(Source { doc: &a, page: 0 }), None, &p, &LayerStyle::default(), Roles::default(), note);
        assert!(matches!(r, Err(OverlayError::NotFinite { .. })), "{bad}");
    }
}

/// All or nothing: a page that fails leaves the output as it was.
#[test]
fn a_failed_page_leaves_the_output_unchanged() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    let mut out = LayeredOutput::new().expect("output");
    out.add_page(
        Some(Source { doc: &a, page: 0 }),
        Some(Source { doc: &b, page: 0 }),
        &p,
        &LayerStyle::default(),
        Roles::default(),
        RegistrationNote::PLACED,
    )
    .expect("first page");
    let mut bad = p;
    bad.b = Matrix([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert!(
        out.add_page(
            Some(Source { doc: &a, page: 0 }),
            Some(Source { doc: &b, page: 0 }),
            &bad,
            &LayerStyle::default(),
            Roles::default(),
            RegistrationNote::PLACED
        )
        .is_err()
    );
    assert!(out.add_page(Some(Source { doc: &a, page: 7 }), None, &p, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED).is_err());
    assert_eq!(out.page_count(), 1);
    let before = synth::save(&out.finish().expect("finishes"), false);
    let mut again = LayeredOutput::new().expect("output");
    again
        .add_page(
            Some(Source { doc: &a, page: 0 }),
            Some(Source { doc: &b, page: 0 }),
            &p,
            &LayerStyle::default(),
            Roles::default(),
            RegistrationNote::PLACED,
        )
        .expect("first page");
    assert_eq!(before, synth::save(&again.finish().expect("finishes"), false), "the same file as without the failed pages");
}

/// OV4.1 has no source preparation yet: a source with optional content or annotations is refused,
/// never composed with its layers or annotations lost. A group the catalog doesn't declare is
/// caught by the defensive check.
#[test]
fn unprepared_layers_and_annotations_are_refused() {
    let b = src(Page::new(300.0, 300.0, "0 g 10 10 50 50 re f"));
    let layered_src = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R] /D << /OFF [5 0 R] >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Properties << /L 5 0 R >> >> /Contents 4 0 R >>".into(),
        synth::raw_stream("", "/OC /L BDC 1 0 0 rg 0 0 300 300 re f EMC 0 g 10 10 20 20 re f"),
        "<< /Type /OCG /Name (Hidden) >>".into(),
    ]));
    let annotated = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [5 0 R] /Contents 4 0 R >>".into(),
        synth::raw_stream("", "0 g 10 10 20 20 re f"),
        "<< /Type /Annot /Subtype /Square /Rect [50 50 100 100] /C [1 0 0] >>".into(),
    ]));
    let undeclared = synth::open(&synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Properties << /L 5 0 R >> >> /Contents 4 0 R >>".into(),
        synth::raw_stream("", "/OC /L BDC 1 0 0 rg 0 0 300 300 re f EMC"),
        "<< /Type /OCG /Name (Undeclared) >>".into(),
    ]));
    for (doc, expect) in [(&layered_src, "optional content"), (&annotated, "annotations")] {
        let p = placement(doc, &b);
        match compose_layered(
            Some(Source { doc, page: 0 }),
            Some(Source { doc: &b, page: 0 }),
            &p,
            &LayerStyle::default(),
            Roles::default(),
            RegistrationNote::PLACED,
        ) {
            Err(OverlayError::Source { side: 'A', page: 0, reason }) => assert!(reason.contains(expect), "{reason}"),
            other => panic!("expected a refusal: {other:?}"),
        }
    }
    let p = placement(&undeclared, &b);
    let r = compose_layered(Some(Source { doc: &undeclared, page: 0 }), None, &p, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED);
    assert!(matches!(r, Err(OverlayError::Check { .. })), "{r:?}");
}

/// The defensive check turns down every kind of leak or interactive content, each on its own.
#[test]
fn the_defensive_check_finds_every_leak() {
    let (a, b) = two_pages();
    let p = placement(&a, &b);
    let good = layered(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &LayerStyle::default(), Roles::default());
    let groups = check_output(&good).expect("the good document passes");
    let page_ref = pdfcraft_model::pages(&good).first().expect("page").obj;
    let root = good.root().expect("catalog");
    let form_ref = good
        .scan_objects()
        .find(|(_, o)| matches!(&**o, Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Form"[..]) && s.dict.get(b"Group").is_none()))
        .map(|(r, _)| r)
        .expect("a source form");
    let contents_ref = page_dict(&good, 0).reference(b"Contents").expect("contents");

    type Edit = Box<dyn Fn(&mut Document)>;
    let set_contents = move |s: &'static str| -> Edit {
        Box::new(move |d: &mut Document| d.set(contents_ref, Object::Stream(Stream::flate(Dict::new(), s.as_bytes()))))
    };
    let cases: Vec<(&str, Edit)> = vec![
        (
            "another group",
            Box::new(move |d: &mut Document| {
                let mut g = Dict::new();
                g.set(b"Type".to_vec(), Object::name("OCG"));
                g.set(b"Name".to_vec(), Object::String(PdfString::text("Extra")));
                let r = d.add(Object::Dict(g));
                let mut props = Dict::new();
                props.set(b"X".to_vec(), Object::Ref(r));
                let mut res = Dict::new();
                res.set(b"Properties".to_vec(), Object::Dict(props));
                d.update_dict(form_ref, |f| f.set(b"Resources".to_vec(), Object::Dict(res))).expect("form");
            }),
        ),
        (
            "a membership dictionary in a form's properties",
            Box::new(move |d: &mut Document| {
                let mut m = Dict::new();
                m.set(b"Type".to_vec(), Object::name("OCMD"));
                m.set(b"OCGs".to_vec(), Object::Ref(groups.old));
                let mut props = Dict::new();
                props.set(b"M".to_vec(), Object::Dict(m));
                let mut res = Dict::new();
                res.set(b"Properties".to_vec(), Object::Dict(props));
                d.update_dict(form_ref, |f| f.set(b"Resources".to_vec(), Object::Dict(res))).expect("form");
            }),
        ),
        (
            "/OC on a form",
            Box::new(move |d: &mut Document| d.update_dict(form_ref, |f| f.set(b"OC".to_vec(), Object::Ref(groups.new))).expect("form")),
        ),
        (
            "a marker in a form",
            Box::new(move |d: &mut Document| {
                let mut dd = d.dict(&Object::Ref(form_ref)).expect("form");
                dd.remove(b"Filter");
                d.set(form_ref, Object::Stream(Stream::from_raw(dd, b"/OC /PCOld BDC 0 g 0 0 1 1 re f EMC".to_vec())));
            }),
        ),
        ("an inline dictionary after /OC", set_contents("/OC << /Type /OCMD >> BDC q /GA gs 0 0 1 1 re f Q EMC")),
        ("a marker naming a third property", set_contents("/OC /PCX BDC q /GA gs 0 0 1 1 re f Q EMC")),
        ("an unclosed marker", set_contents("/OC /PCOld BDC q /GA gs 0 0 1 1 re f Q")),
        ("an extra end", set_contents("q Q EMC")),
        ("a nested marker", set_contents("/Span BMC /OC /PCOld BDC q Q EMC EMC")),
        ("unreadable bytes", set_contents("q ) 1 0 0 rg Q")),
        (
            "an annotation",
            Box::new(move |d: &mut Document| {
                let mut an = Dict::new();
                an.set(b"Type".to_vec(), Object::name("Annot"));
                an.set(b"Subtype".to_vec(), Object::name("Square"));
                let r = d.add(Object::Dict(an));
                d.update_dict(page_ref, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(r)]))).expect("page");
            }),
        ),
        (
            "a form",
            Box::new(move |d: &mut Document| d.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(Dict::new()))).expect("catalog")),
        ),
        (
            "an open action",
            Box::new(move |d: &mut Document| d.update_dict(root, |c| c.set(b"OpenAction".to_vec(), Object::Array(Vec::new()))).expect("catalog")),
        ),
        (
            "JavaScript",
            Box::new(move |d: &mut Document| {
                let mut js = Dict::new();
                js.set(b"S".to_vec(), Object::name("JavaScript"));
                js.set(b"JS".to_vec(), Object::String(PdfString::text("app.alert(1)")));
                let r = d.add(Object::Dict(js));
                d.update_dict(form_ref, |f| f.set(b"PieceInfo".to_vec(), Object::Ref(r))).expect("form");
            }),
        ),
        (
            "a layer off by default",
            Box::new(move |d: &mut Document| {
                let mut props = dict(d, d.dict(&Object::Ref(root)).expect("catalog").get(b"OCProperties").expect("props"));
                let mut cfg = dict(d, props.get(b"D").expect("D"));
                cfg.set(b"OFF".to_vec(), Object::Array(vec![Object::Ref(groups.new)]));
                props.set(b"D".to_vec(), Object::Dict(cfg));
                d.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props))).expect("catalog");
            }),
        ),
        (
            "locked layers",
            Box::new(move |d: &mut Document| {
                let mut props = dict(d, d.dict(&Object::Ref(root)).expect("catalog").get(b"OCProperties").expect("props"));
                let mut cfg = dict(d, props.get(b"D").expect("D"));
                cfg.set(b"Locked".to_vec(), Object::Array(vec![Object::Ref(groups.old)]));
                props.set(b"D".to_vec(), Object::Dict(cfg));
                d.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props))).expect("catalog");
            }),
        ),
        (
            "a renamed layer",
            Box::new(move |d: &mut Document| {
                d.update_dict(groups.old, |g| g.set(b"Name".to_vec(), Object::String(PdfString::text("A: Old")))).expect("group");
            }),
        ),
    ];
    for (what, edit) in cases {
        let mut doc = good.clone();
        edit(&mut doc);
        assert!(matches!(check_output(&doc), Err(OverlayError::Check { .. })), "{what}: {:?}", check_output(&doc));
        // And as another reader sees the file.
        let back = written(&doc);
        assert!(matches!(check_output(&back), Err(OverlayError::Check { .. })), "{what} (saved)");
    }
}

/// L11 (OV4.1 part): the sources are only read.
#[test]
fn the_sources_are_not_modified() {
    let (bytes_a, bytes_b) =
        (synth::pdf(&[Page::new(612.0, 792.0, "0 g 1 1 10 10 re f")]), synth::pdf(&[Page::new(500.0, 700.0, "0 g 2 2 10 10 re f")]));
    let (a, b) = (synth::open(&bytes_a), synth::open(&bytes_b));
    let p = placement(&a, &b);
    let out = layered(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &LayerStyle::default(), Roles::SWAPPED);
    let _ = synth::save(&out, true);
    assert!(!a.is_modified() && !b.is_modified());
    assert_eq!(**a.bytes(), *bytes_a);
    assert_eq!(**b.bytes(), *bytes_b);
}

// ── L2 and L4 in pixels, PdfCraft in process ────────────────────────────────────────────────

/// Render page `page` of `pdf` at `dpi` with optional-content overrides, as the Layers panel sets
/// them (`RenderConfig.layers`).
fn draw(pdf: &Arc<Vec<u8>>, page: usize, dpi: f64, layers: Vec<(i32, i32, bool)>) -> Image {
    let mut r = PageRenderer::new(pdf.clone(), RenderConfig { layers: Arc::new(layers), ..RenderConfig::default() });
    let out = r.render(RenderRequest { page, scale: (dpi / 72.0) as f32, ..RenderRequest::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba)
}

/// The largest channel difference between two renders of the same size.
fn max_diff(x: &Image, y: &Image) -> u8 {
    assert_eq!((x.width, x.height), (y.width, y.height));
    x.rgb.iter().zip(&y.rgb).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)
}

/// One case of an OV3 family, recomposed: its OV3 page, its layered page, and the layered pages
/// with A blank and with B blank. Saved as another reader sees them.
struct Composed {
    ov3: Arc<Vec<u8>>,
    layered: Arc<Vec<u8>>,
    a_blank: Option<Arc<Vec<u8>>>,
    b_blank: Option<Arc<Vec<u8>>>,
    /// (Old, New) object numbers in `layered`.
    old: (i32, i32),
    new: (i32, i32),
}

fn recompose(set: &Set, case: &Case, roles: Roles) -> Composed {
    let (a_doc, b_doc) = (synth::open(&set.a), synth::open(&set.b));
    let sa = case.a_page.map(|page| Source { doc: &a_doc, page });
    let sb = case.b_page.map(|page| Source { doc: &b_doc, page });
    let style = LayerStyle {
        old: if roles.a == Revision::Old { case.style.colour_a } else { case.style.colour_b },
        new: if roles.a == Revision::Old { case.style.colour_b } else { case.style.colour_a },
        threshold: case.style.threshold,
    };
    let save = |d: Document| synth::save(&d, true);
    let ov3 = save(compose(sa, sb, &case.placement, &case.style).expect("OV3 composes"));
    let layered_bytes = save(compose_layered(sa, sb, &case.placement, &style, roles, RegistrationNote::PLACED).expect("layered"));
    let one = |a: Option<Source<'_>>, b: Option<Source<'_>>| {
        (a.is_some() || b.is_some()).then(|| save(compose_layered(a, b, &case.placement, &style, roles, RegistrationNote::PLACED).expect("one side")))
    };
    let back = synth::open(&layered_bytes);
    let g = check_output(&back).expect("passes the check");
    let n = |r: ObjRef| (r.num as i32, i32::from(r.generation));
    Composed { ov3, a_blank: one(None, sb), b_blank: one(sa, None), layered: layered_bytes, old: n(g.old), new: n(g.new) }
}

/// L2 and L4 for one family: with both layers on the layered page renders exactly as OV3's; with
/// one layer off it renders as the page composed without that document (every channel within 1
/// level); with both off it is paper.
fn toggles(set: &Set, roles: Roles, dpi: f64) {
    for case in &set.cases {
        let c = recompose(set, case, roles);
        let both = draw(&c.layered, 0, dpi, Vec::new());
        let ov3 = draw(&c.ov3, 0, dpi, Vec::new());
        assert_eq!(max_diff(&both, &ov3), 0, "{}: both layers on differs from OV3", case.id);
        let explicit_on = draw(&c.layered, 0, dpi, vec![(c.old.0, c.old.1, true), (c.new.0, c.new.1, true)]);
        assert_eq!(max_diff(&explicit_on, &ov3), 0, "{}: both layers switched on", case.id);

        // A's layer is roles.a; switching it off leaves B alone.
        let (a_layer, b_layer) = if roles.a == Revision::Old { (c.old, c.new) } else { (c.new, c.old) };
        for (off, alone, what) in [(a_layer, &c.a_blank, "A's layer off"), (b_layer, &c.b_blank, "B's layer off")] {
            let img = draw(&c.layered, 0, dpi, vec![(off.0, off.1, false)]);
            match alone {
                Some(reference) => {
                    let d = max_diff(&img, &draw(reference, 0, dpi, Vec::new()));
                    assert!(d <= 1, "{}: {what}: {d} levels from the page without that document", case.id);
                }
                None => assert!(img.rgb.iter().all(|v| *v == 255), "{}: {what}, and the other side is blank: paper", case.id),
            }
        }
        let none = draw(&c.layered, 0, dpi, vec![(c.old.0, c.old.1, false), (c.new.0, c.new.1, false)]);
        assert!(none.rgb.iter().all(|v| *v == 255), "{}: both layers off is paper", case.id);
    }
}

/// L2 and L4: overlap pairs (four colour pairs, both roles).
#[test]
fn toggling_overlap_pairs() {
    let set = families::overlap();
    toggles(&set, Roles::default(), 150.0);
    toggles(&set, Roles::SWAPPED, 150.0);
}

/// L2 and L4: coloured fills (D2's luminosity).
#[test]
fn toggling_coloured_fills() {
    toggles(&families::colours(), Roles::default(), 150.0);
}

/// L2 and L4: rotated and offset pages.
#[test]
fn toggling_rotated_pages() {
    toggles(&families::rotation(), Roles::default(), 150.0);
}

/// L2 and L4: the line study, including the 0.1 mm hatching. At 100 dpi here, to keep `cargo test`
/// fast in debug builds; the proof (OV4.6) toggles it at 150 and 300 dpi in all three renderers.
#[test]
fn toggling_the_line_study() {
    toggles(&families::lines(), Roles::default(), 100.0);
}

/// Hostile sources and placements through the layered path: errors or documents that pass the
/// check, never a panic (L14 has the full sweep in OV4.5).
#[test]
fn hostile_layered_input_never_panics() {
    let pool: Vec<Document> = [
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /X 5 0 R >> >> /Contents 4 0 R >>".into(),
            synth::raw_stream("", "/X Do"),
            synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /OC 6 0 R", "0 g 0 0 1 1 re f"),
            "<< /Type /OCMD /OCGs [] >>".into(),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".into(),
            synth::raw_stream("", "/OC << /Type /OCMD >> BDC 0 g 0 0 9 9 re f EMC"),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".into(),
            synth::raw_stream("/Filter /Bogus", "x"),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".into(),
            synth::raw_stream("", "q 0 g ) 0 0 9 9 re f"),
        ]),
    ]
    .iter()
    .map(synth::open)
    .chain([src(Page::new(100.0, 100.0, "0 g 1 1 10 10 re f"))])
    .collect();
    let mut rng = common::Rng(0x0154_0001);
    let (mut ok, mut err) = (0, 0);
    for _ in 0..2_000 {
        // The well-formed source half the time; a hostile one, a missing page or a blank otherwise.
        let pick = |rng: &mut common::Rng| match rng.next_u64() % 12 {
            0 => None,
            k @ 1..=4 => Some(Source { doc: &pool[k as usize - 1], page: 0 }),
            5 => Some(Source { doc: &pool[4], page: 1 }),
            _ => Some(Source { doc: &pool[4], page: 0 }),
        };
        let (a, b) = (pick(&mut rng), pick(&mut rng));
        let reference = &pool[4];
        let mut p = placement(reference, reference);
        if rng.next_u64().is_multiple_of(4) {
            p.a = Matrix([rng.wild(), rng.wild(), rng.wild(), rng.wild(), rng.wild(), rng.wild()]);
        }
        let roles = match rng.next_u64() % 8 {
            0 => Roles { a: Revision::Old, b: Revision::Old },
            1 => Roles::SWAPPED,
            _ => Roles::default(),
        };
        let rms = if rng.next_u64().is_multiple_of(5) { rng.wild() } else { rng.range(0.0, 1.0) };
        let note = RegistrationNote { kind: RegistrationKind::Affine, rms_mm: Some(rms), max_mm: None };
        match compose_layered(a, b, &p, &LayerStyle::default(), roles, note) {
            Ok(doc) => {
                ok += 1;
                assert!(check_output(&doc).is_ok());
                let _ = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default());
            }
            Err(_) => err += 1,
        }
    }
    assert!(ok > 50 && err > 50, "both outcomes are exercised: {ok} composed, {err} refused");
}

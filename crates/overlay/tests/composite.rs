//! Compositing structure (OV3.1): the objects the overlay page is made of, the transfer
//! function, full-precision matrices, vector preservation, untouched sources, the seam with the
//! OV2 geometry, and no panics on hostile input. Step 3 PRD criteria 7, 10 to 12 and 14.

mod common;

use common::Rng;
use common::synth::{self, Page, Res};
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_overlay::{
    CYAN, OverlayError, Placement, Point, RED, Settings, SheetMatch, Source, Style, compose, frame_of_page, light_tone, place, transfer_function,
};

fn src(page: Page) -> Document {
    synth::open(&synth::pdf(&[page]))
}

fn placement(a: &Document, b: &Document, registration: &Matrix, expand: bool) -> Placement {
    let (fa, _) = frame_of_page(a, 0).expect("frame A");
    let (fb, _) = frame_of_page(b, 0).expect("frame B");
    place(&fa, &fb, SheetMatch::Off, registration, expand, &Settings::default()).expect("placement")
}

/// The document as another reader sees it: saved as PdfCraft saves, then opened again.
fn written(doc: &Document) -> Document {
    synth::open(&synth::save(doc, true))
}

fn dict(doc: &Document, o: &Object) -> Dict {
    match &*doc.resolve(o) {
        Object::Dict(d) => d.clone(),
        Object::Stream(s) => s.dict.clone(),
        other => panic!("not a dictionary: {other:?}"),
    }
}

fn data(doc: &Document, o: &Object) -> Vec<u8> {
    match &*doc.resolve(o) {
        Object::Stream(s) => s.decoded().expect("decodes"),
        other => panic!("not a stream: {other:?}"),
    }
}

fn nums(doc: &Document, o: &Object) -> Vec<f64> {
    doc.resolve(o).as_array().expect("an array").iter().map(|x| doc.resolve(x).as_f64().expect("a number")).collect()
}

fn ops(bytes: &[u8]) -> Vec<String> {
    parse(bytes).ops.iter().map(|o| String::from_utf8_lossy(&o.op).into_owned()).collect()
}

fn page_dict(doc: &Document) -> Dict {
    pdfcraft_model::pages(doc).first().expect("one page").dict.clone()
}

fn two_pages() -> (Document, Document) {
    (src(Page::new(612.0, 792.0, "0 g 72 72 144 144 re f")), src(Page::new(612.0, 792.0, "0 g 100 100 144 144 re f")))
}

/// The page, its graphics states, soft masks, groups and source forms are exactly the overlay
/// PRD's structure (Compositing, Structure of an output page).
#[test]
fn the_page_has_the_approved_structure() {
    let (a, b) = two_pages();
    let p = placement(&a, &b, &Matrix::IDENTITY, false);
    let out = written(&compose(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &Style::default()).expect("composes"));
    let page = page_dict(&out);

    let group = dict(&out, page.get(b"Group").expect("page group"));
    assert_eq!(group.name(b"S"), Some(&b"Transparency"[..]));
    assert_eq!(group.name(b"CS"), Some(&b"DeviceRGB"[..]));
    assert!(group.get(b"I").is_none() && group.get(b"K").is_none(), "the page group is neither isolated nor knockout");
    assert_eq!(nums(&out, page.get(b"MediaBox").expect("media box")), p.page_box().to_vec());
    assert!(page.get(b"UserUnit").is_none());

    let states = dict(&out, dict(&out, page.get(b"Resources").expect("resources")).get(b"ExtGState").expect("states"));
    let mut transfer = Vec::new();
    for (name, source, matrix) in [(&b"GA"[..], &a, p.a), (&b"GB"[..], &b, p.b)] {
        let gs = dict(&out, states.get(name).expect("graphics state"));
        assert_eq!(gs.name(b"BM"), Some(&b"Multiply"[..]));
        assert_eq!(out.resolve(gs.get(b"CA").expect("CA")).as_f64(), Some(1.0));
        assert_eq!(out.resolve(gs.get(b"ca").expect("ca")).as_f64(), Some(1.0));
        let mask = dict(&out, gs.get(b"SMask").expect("soft mask"));
        assert_eq!(mask.name(b"Type"), Some(&b"Mask"[..]));
        assert_eq!(mask.name(b"S"), Some(&b"Luminosity"[..]));
        assert_eq!(nums(&out, mask.get(b"BC").expect("backdrop")), vec![1.0, 1.0, 1.0]);
        transfer.push(mask.reference(b"TR").expect("the transfer function is an object"));

        // The mask's group: isolated, DeviceRGB, the page box, drawing the source under the placement.
        let g = mask.get(b"G").expect("group");
        let gd = dict(&out, g);
        assert_eq!(gd.name(b"Subtype"), Some(&b"Form"[..]));
        assert_eq!(nums(&out, gd.get(b"BBox").expect("bbox")), p.page_box().to_vec());
        let gg = dict(&out, gd.get(b"Group").expect("group attributes"));
        assert_eq!((gg.name(b"S"), gg.name(b"CS")), (Some(&b"Transparency"[..]), Some(&b"DeviceRGB"[..])));
        assert_eq!(out.resolve(gg.get(b"I").expect("isolated")).as_ref(), &Object::Bool(true));
        assert!(gg.get(b"K").is_none());
        let content = parse(&data(&out, g));
        let names: Vec<String> = content.ops.iter().map(|o| String::from_utf8_lossy(&o.op).into_owned()).collect();
        assert_eq!(names, ["q", "cm", "Do", "Q"]);
        let cm: Vec<f64> = content.ops[1].operands.iter().map(|o| o.as_f64().expect("number")).collect();
        assert_eq!(cm, matrix.0.to_vec());
        assert_eq!(content.ops[2].operands, vec![Object::name("S")]);

        // The source form: OV2's visible box, identity matrix, the page's content unchanged.
        let form = dict(&out, gd.get(b"Resources").expect("group resources")).get(b"XObject").map(|x| dict(&out, x)).expect("xobjects");
        let s = form.get(b"S").expect("source form");
        let sd = dict(&out, s);
        let (frame, _) = frame_of_page(source, 0).expect("frame");
        assert_eq!(nums(&out, sd.get(b"BBox").expect("form bbox")), frame.visible.to_vec());
        assert_eq!(nums(&out, sd.get(b"Matrix").expect("form matrix")), vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(data(&out, s), synth::page_content(source, 0));
    }
    assert_eq!(transfer[0], transfer[1], "both documents share one transfer function");
    let f = dict(&out, &Object::Ref(transfer[0]));
    assert_eq!(f.int(b"FunctionType"), Some(2));
    assert_eq!(nums(&out, f.get(b"C1").expect("C1")), vec![0.0]);

    // The page draws two rectangles in the layer colours, nothing else.
    let content = parse(&data(&out, page.get(b"Contents").expect("contents")));
    let names: Vec<String> = content.ops.iter().map(|o| String::from_utf8_lossy(&o.op).into_owned()).collect();
    assert_eq!(names, ["q", "gs", "rg", "re", "f", "Q", "q", "gs", "rg", "re", "f", "Q"]);
    for (i, colour) in [(2, RED), (8, CYAN)] {
        let rg: Vec<f64> = content.ops[i].operands.iter().map(|o| o.as_f64().expect("number")).collect();
        assert_eq!(rg, colour.iter().map(|c| f64::from(*c) / 255.0).collect::<Vec<_>>());
    }
    let re: Vec<f64> = content.ops[3].operands.iter().map(|o| o.as_f64().expect("number")).collect();
    assert_eq!(re, vec![0.0, 0.0, p.page_box()[2], p.page_box()[3]]);
}

/// The transfer function is the approved Type 2 function, with C1 = 1 − 1/t to the bit after
/// saving and reopening (criterion 7).
#[test]
fn the_transfer_function_is_exact() {
    let (a, b) = two_pages();
    let p = placement(&a, &b, &Matrix::IDENTITY, false);
    for t in [0.5, 0.8, 0.83, 0.97, 1.0] {
        assert!(transfer_function(t).is_ok());
        let style = Style { threshold: t, ..Style::default() };
        let back = written(&compose(Some(Source { doc: &a, page: 0 }), None, &p, &style).expect("composes"));
        let page = page_dict(&back);
        let states = dict(&back, dict(&back, page.get(b"Resources").expect("resources")).get(b"ExtGState").expect("states"));
        let mask = dict(&back, dict(&back, states.get(b"GA").expect("state")).get(b"SMask").expect("mask"));
        let f = dict(&back, mask.get(b"TR").expect("transfer function"));
        assert_eq!(f.int(b"FunctionType"), Some(2));
        assert_eq!(nums(&back, f.get(b"Domain").expect("domain")), vec![0.0, 1.0]);
        assert_eq!(nums(&back, f.get(b"Range").expect("range")), vec![0.0, 1.0]);
        assert_eq!(nums(&back, f.get(b"C0").expect("C0")), vec![1.0]);
        let c1 = nums(&back, f.get(b"C1").expect("C1"));
        assert_eq!(c1.len(), 1);
        assert_eq!(c1[0].to_bits(), (1.0 - 1.0 / t).to_bits(), "t = {t}");
        assert_eq!(f.int(b"N"), Some(1));
    }
}

#[test]
fn thresholds_outside_the_approved_range_are_refused() {
    let (a, b) = two_pages();
    let p = placement(&a, &b, &Matrix::IDENTITY, false);
    for t in [0.499_999, 1.000_001, 0.0, -1.0, f64::NAN, f64::INFINITY] {
        let style = Style { threshold: t, ..Style::default() };
        assert!(matches!(compose(Some(Source { doc: &a, page: 0 }), None, &p, &style), Err(OverlayError::OutOfRange { .. })), "{t}");
        assert!(transfer_function(t).is_err(), "{t}");
        assert_eq!(light_tone(t, 0.5), None, "{t}");
    }
    for t in [0.5, 1.0] {
        assert!(compose(Some(Source { doc: &a, page: 0 }), None, &p, &Style { threshold: t, ..Style::default() }).is_ok(), "{t}");
    }
}

/// m_t(L) as the overlay PRD states it (Light tones).
#[test]
fn the_light_tone_function_has_the_approved_shape() {
    for t in [0.5, 0.8, 1.0] {
        assert_eq!(light_tone(t, 0.0), Some(1.0), "black ink prints at full colour at every t");
        for l in [t, (t + 1.0) / 2.0, 1.0] {
            assert_eq!(light_tone(t, l), Some(0.0), "tones at or lighter than t print as paper: t {t}, L {l}");
        }
    }
    assert_eq!(light_tone(0.8, 0.5), Some(0.375), "the PRD's example");
    for l in [0.0, 0.25, 0.5, 0.75, 1.0] {
        assert_eq!(light_tone(1.0, l), Some(1.0 - l), "plain inversion at t = 1");
    }
}

#[test]
fn either_side_may_be_blank_but_not_both() {
    let (a, b) = two_pages();
    let p = placement(&a, &b, &Matrix::IDENTITY, false);
    for (one_a, name) in [(true, &b"GA"[..]), (false, &b"GB"[..])] {
        let (sa, sb) = if one_a { (Some(Source { doc: &a, page: 0 }), None) } else { (None, Some(Source { doc: &b, page: 0 })) };
        let out = written(&compose(sa, sb, &p, &Style::default()).expect("composes"));
        let page = page_dict(&out);
        let states = dict(&out, dict(&out, page.get(b"Resources").expect("resources")).get(b"ExtGState").expect("states"));
        assert_eq!(states.len(), 1);
        assert!(states.get(name).is_some());
        assert_eq!(ops(&data(&out, page.get(b"Contents").expect("contents"))), ["q", "gs", "rg", "re", "f", "Q"]);
    }
    assert_eq!(compose(None, None, &p, &Style::default()).err(), Some(OverlayError::NoSources));
}

/// Criterion 10: the source content stays as its own operators inside the form, nothing is
/// turned into an image, and the page itself draws only fills, graphics states and forms.
#[test]
fn vector_content_is_kept_as_it_is() {
    let mut image = synth::image_dict(2, 2, "DeviceGray");
    image.set(b"Decode".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(1)]));
    let mut nested = Dict::new();
    nested.set(b"Type".to_vec(), Object::name("XObject"));
    nested.set(b"Subtype".to_vec(), Object::name("Form"));
    nested.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(50), Object::Int(50)]));
    let mut page = Page::new(400.0, 300.0, "0.5 w 0 G 10 10 m 390 290 l S");
    page.contents.push("q 50 0 0 50 100 100 cm /Im Do Q /Fx Do".into());
    page.contents.push("BT /F1 12 Tf 20 20 Td (Vector text) Tj ET".into());
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    let page = page
        .with_resource("XObject", "Im", Res::Stream(image, vec![0, 64, 128, 255]))
        .with_resource("XObject", "Fx", Res::Stream(nested, b"0 g 0 0 50 50 re f".to_vec()))
        .with_resource("Font", "F1", Res::Dict(font));
    let a = src(page);
    let b = src(Page::new(400.0, 300.0, "1 0 0 RG 2 w 10 290 m 390 10 l S"));
    let p = placement(&a, &b, &Matrix::IDENTITY, false);
    let out = written(&compose(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &Style::default()).expect("composes"));

    let images = |doc: &Document| {
        doc.scan_objects().filter(|(_, o)| matches!(&**o, Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Image"[..]))).count()
    };
    assert_eq!(images(&out), images(&a) + images(&b), "no image is added");
    let forms: Vec<Vec<u8>> = out
        .scan_objects()
        .filter_map(|(_, o)| match &*o {
            Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Form"[..]) => s.decoded().ok(),
            _ => None,
        })
        .collect();
    for source in [&a, &b] {
        let content = synth::page_content(source, 0);
        assert!(forms.contains(&content), "the source content is inside a form, byte for byte");
    }
    assert!(forms.iter().any(|f| f == b"0 g 0 0 50 50 re f"), "the source's own nested form is copied as it is");
    let page = page_dict(&out);
    for op in ops(&data(&out, page.get(b"Contents").expect("contents"))) {
        assert!(["q", "Q", "gs", "rg", "re", "f"].contains(&op.as_str()), "the page draws {op}");
    }
}

/// Criterion 11: the sources are only read.
#[test]
fn the_sources_are_not_modified() {
    let (bytes_a, bytes_b) =
        (synth::pdf(&[Page::new(612.0, 792.0, "0 g 1 1 10 10 re f")]), synth::pdf(&[Page::new(500.0, 700.0, "0 g 2 2 10 10 re f")]));
    let (a, b) = (synth::open(&bytes_a), synth::open(&bytes_b));
    let (copy_a, copy_b) = ((*bytes_a).clone(), (*bytes_b).clone());
    let p = placement(&a, &b, &Matrix::IDENTITY, true);
    let out = compose(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &Style::default()).expect("composes");
    let _ = synth::save(&out, true);
    assert!(!a.is_modified() && !b.is_modified());
    assert_eq!(**a.bytes(), copy_a);
    assert_eq!(**b.bytes(), copy_b);
}

/// The placement survives saving to the bit: a 0.001° rotation on a 3,000 pt sheet, where
/// rounding to 4 decimals would move content by up to 0.15 pt.
#[test]
fn matrices_are_written_at_full_precision() {
    let a = src(Page::new(3000.0, 2000.0, "0 g 10 10 100 100 re f"));
    let b = src(Page::new(3000.0, 2000.0, "0 g 20 20 100 100 re f"));
    let th = 0.001_f64.to_radians();
    let s = 1.000_000_3;
    let reg = Matrix([s * th.cos(), s * th.sin(), -s * th.sin(), s * th.cos(), 0.123_456_789_012_3, 7.000_000_000_1]);
    let p = placement(&a, &b, &reg, false);
    let out = written(&compose(Some(Source { doc: &a, page: 0 }), Some(Source { doc: &b, page: 0 }), &p, &Style::default()).expect("composes"));
    let page = page_dict(&out);
    let states = dict(&out, dict(&out, page.get(b"Resources").expect("resources")).get(b"ExtGState").expect("states"));
    let gs = dict(&out, states.get(b"GB").expect("B's state"));
    let g = dict(&out, gs.get(b"SMask").expect("mask")).get(b"G").cloned().expect("group");
    let content = parse(&data(&out, &g));
    let cm: Vec<u64> = content.ops[1].operands.iter().map(|o| o.as_f64().expect("number").to_bits()).collect();
    assert_eq!(cm, p.b.0.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
}

/// Seam with OV2: where `page_as_form` handles a page correctly (crop box inside the media box,
/// unit 1, `/Rotate` a multiple of 90), its own matrix equals OV2's displayed frame.
#[test]
fn page_as_form_agrees_with_the_ov2_frame() {
    for rotate in [0, 90, 180, 270] {
        for (ox, oy) in [(0.0, 0.0), (-100.0, 50.0), (37.5, -12.25), (500.0, 300.0)] {
            let mut page = Page::new(400.0, 300.0, "0 g 0 0 1 1 re f");
            page.media = [ox, oy, ox + 400.0, oy + 300.0];
            page.crop = Some([ox + 10.0, oy + 20.0, ox + 390.0, oy + 280.0]);
            page.rotate = rotate;
            let source = src(page);
            let mut dst = Document::new_empty();
            let (form, _) = pdfcraft_organize::page_as_form(&mut dst, &source, 0).expect("form");
            let d = dict(&dst, &Object::Ref(form));
            let m = nums(&dst, d.get(b"Matrix").expect("matrix"));
            let (frame, _) = frame_of_page(&source, 0).expect("frame");
            for (x, y) in m.iter().zip(frame.to_display.0) {
                assert!((x - y).abs() <= 1e-12, "/Rotate {rotate}, origin ({ox}, {oy}): {m:?} vs {:?}", frame.to_display.0);
            }
            assert_eq!(nums(&dst, d.get(b"BBox").expect("bbox")), frame.visible.to_vec());
        }
    }
}

/// Where `page_as_form` differs from OV2 (a crop box partly outside the media box, `/UserUnit`,
/// a `/Rotate` that isn't a multiple of 90), the overlay's rewrite gives OV2's frame.
#[test]
fn the_rewrite_gives_the_ov2_frame_where_page_as_form_differs() {
    type Edit = Box<dyn Fn(&mut Page)>;
    let cases: [(&str, Edit); 3] = [
        ("crop partly outside media", Box::new(|p: &mut Page| p.crop = Some([-50.0, 20.0, 300.0, 280.0]))),
        ("user unit 2", Box::new(|p: &mut Page| p.user_unit = Some(2.0))),
        ("rotate 100", Box::new(|p: &mut Page| p.rotate = 100)),
    ];
    for (what, edit) in cases {
        let mut page = Page::new(400.0, 300.0, "0 g 10 10 50 50 re f");
        edit(&mut page);
        let a = src(page);
        let b = src(Page::new(400.0, 300.0, "0 g 10 10 50 50 re f"));
        let (frame, _) = frame_of_page(&a, 0).expect("frame");

        // page_as_form on its own disagrees with OV2 here.
        let mut dst = Document::new_empty();
        let (raw, _) = pdfcraft_organize::page_as_form(&mut dst, &a, 0).expect("form");
        let raw = dict(&dst, &Object::Ref(raw));
        let differs = nums(&dst, raw.get(b"BBox").expect("bbox")) != frame.visible.to_vec()
            || nums(&dst, raw.get(b"Matrix").expect("matrix")) != frame.to_display.0.to_vec();
        assert!(differs, "{what}: page_as_form alone already matches OV2, so this case tests nothing");

        // The overlay's form uses OV2's visible box, and its group draws it under OV2's matrix.
        let p = placement(&a, &b, &Matrix::IDENTITY, false);
        assert_eq!(p.a, frame.to_display, "{what}");
        let out = written(&compose(Some(Source { doc: &a, page: 0 }), None, &p, &Style::default()).expect("composes"));
        let page = page_dict(&out);
        let states = dict(&out, dict(&out, page.get(b"Resources").expect("resources")).get(b"ExtGState").expect("states"));
        let g = dict(&out, dict(&out, states.get(b"GA").expect("state")).get(b"SMask").expect("mask")).get(b"G").cloned().expect("group");
        let gd = dict(&out, &g);
        let form =
            dict(&out, dict(&out, gd.get(b"Resources").expect("resources")).get(b"XObject").expect("xobjects")).get(b"S").cloned().expect("form");
        assert_eq!(nums(&out, dict(&out, &form).get(b"BBox").expect("bbox")), frame.visible.to_vec(), "{what}");
        assert_eq!(nums(&out, dict(&out, &form).get(b"Matrix").expect("matrix")), vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0], "{what}");
        let cm: Vec<f64> = parse(&data(&out, &g)).ops[1].operands.iter().map(|o| o.as_f64().expect("number")).collect();
        assert_eq!(cm, p.a.0.to_vec(), "{what}");
    }
}

/// The overlay copies the page pdfcraft-model and OV2 mean, even in a nested page tree with
/// inherited boxes and rotation.
#[test]
fn the_same_page_is_used_in_a_nested_tree() {
    let stream = |n: usize| synth::raw_stream("", &format!("% page {n}\n0 g {n} {n} 20 20 re f"));
    let bytes = synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 3 /MediaBox [0 0 300 200] /Rotate 90 >>".into(),
        "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [-20 -10 400 250] >>".into(),
        "<< /Type /Page /Parent 3 0 R /Contents 7 0 R >>".into(),
        "<< /Type /Page /Parent 3 0 R /Contents 8 0 R /CropBox [10 10 200 100] /Rotate 180 >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 9 0 R >>".into(),
        stream(1),
        stream(2),
        stream(3),
    ]);
    let doc = synth::open(&bytes);
    assert_eq!(pdfcraft_model::pages(&doc).len(), 3);
    let other = src(Page::new(300.0, 200.0, "0 g 0 0 1 1 re f"));
    for i in 0..3 {
        let (fa, _) = frame_of_page(&doc, i).expect("frame");
        let (fb, _) = frame_of_page(&other, 0).expect("frame");
        let p = place(&fa, &fb, SheetMatch::Off, &Matrix::IDENTITY, true, &Settings::default()).expect("placement");
        let out = written(&compose(Some(Source { doc: &doc, page: i }), None, &p, &Style::default()).expect("composes"));
        let content = synth::page_content(&doc, i);
        assert!(String::from_utf8_lossy(&content).contains(&format!("% page {}", i + 1)));
        let found = out.scan_objects().any(|(_, o)| matches!(&*o, Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Form"[..]) && s.decoded().ok().as_deref() == Some(&content[..])));
        assert!(found, "page {i}");
    }
}

/// Criterion 12: 10,000 seeded hostile cases return an error or a page, and never panic, through
/// composing and saving.
#[test]
fn hostile_input_never_panics() {
    let stream = |d: &str, s: &str| synth::raw_stream(d, s);
    let pool: Vec<Document> = [
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 0 0] >>".into(),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 99 0 R >>".into(),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".into(),
            stream("/Filter /Bogus", "garbage"),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /X 50 0 R >> /ExtGState 3 0 R >> /Contents 4 0 R >>"
                .into(),
            stream("", "/X Do"),
        ]),
        synth::raw_pdf(&["<< /Type /Catalog /Pages 2 0 R >>".into(), "<< /Type /Pages /Kids [2 0 R] /Count 1 >>".into()]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 4294967295 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1e308 1e308] /Rotate 45 /UserUnit 1e9 /Contents [4 0 R 1 0 R 77 0 R] >>".into(),
            stream("", "q 1e308 0 0 1e308 0 0 cm 0 g 0 0 1 1 re f Q"),
        ]),
        synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [5 5 1 1] /CropBox (x) >>".into(),
        ]),
    ]
    .iter()
    .map(synth::open)
    .chain([src(Page::new(612.0, 792.0, "0 g 1 1 10 10 re f"))])
    .collect();
    let mut rng = Rng(0x0153_0003);
    let (mut ok, mut err) = (0, 0);
    for _ in 0..10_000 {
        let pick = |rng: &mut Rng| match rng.next_u64() % 9 {
            0 => None,
            k => Some(Source { doc: &pool[(k as usize - 1) % pool.len()], page: (rng.next_u64() % 3) as usize }),
        };
        let (a, b) = (pick(&mut rng), pick(&mut rng));
        let placement = if rng.next_u64().is_multiple_of(2) {
            let m = |rng: &mut Rng| Matrix([rng.wild(), rng.wild(), rng.wild(), rng.wild(), rng.wild(), rng.wild()]);
            Placement {
                width: rng.wild(),
                height: rng.wild(),
                unit: rng.wild(),
                a: m(&mut rng),
                b: m(&mut rng),
                b_outline: [Point::new(0.0, 0.0); 4],
                b_clipped: false,
            }
        } else {
            let a = &pool[7];
            let reg = Matrix([1.0 + rng.range(-0.1, 0.1), rng.range(-0.1, 0.1), rng.range(-0.1, 0.1), 1.0, rng.wild(), rng.range(-50.0, 50.0)]);
            match placement_or_none(a, a, &reg, rng.next_u64().is_multiple_of(2)) {
                Some(p) => p,
                None => continue,
            }
        };
        let style = Style {
            colour_a: [rng.next_u64() as u8, rng.next_u64() as u8, rng.next_u64() as u8],
            colour_b: [rng.next_u64() as u8, rng.next_u64() as u8, rng.next_u64() as u8],
            threshold: if rng.next_u64().is_multiple_of(3) { rng.wild() } else { rng.range(0.5, 1.0) },
        };
        match compose(a, b, &placement, &style) {
            Ok(doc) => {
                ok += 1;
                let _ = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default());
            }
            Err(_) => err += 1,
        }
    }
    assert!(ok > 100 && err > 100, "both outcomes are exercised: {ok} composed, {err} refused");
}

fn placement_or_none(a: &Document, b: &Document, reg: &Matrix, expand: bool) -> Option<Placement> {
    let (fa, _) = frame_of_page(a, 0).ok()?;
    let (fb, _) = frame_of_page(b, 0).ok()?;
    place(&fa, &fb, SheetMatch::Off, reg, expand, &Settings::default()).ok()
}

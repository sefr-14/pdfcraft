//! L14 (OV4.5): 10,000 seeded hostile sources through preparation and the layered output (step 4
//! PRD, T11). Each case mixes hostile layer configurations (base states the standard doesn't
//! allow, intents, auto-states, groups that aren't dictionaries, `/OCGs` arrays past the limit,
//! cyclic and deep visibility expressions), hostile content (unbalanced markers and structure,
//! unreadable bytes, forms that draw themselves) and hostile annotations (broken appearance
//! streams, rectangles and matrices that aren't finite, unknown kinds, marks), with hidden
//! content carrying the canary in every place it can hide: marked blocks, forms and images with
//! `/OC`, membership dictionaries, annotations and their appearances.
//!
//! Every case returns an error or a result, never a panic, each within 2 s. Every output passes
//! the defensive check and the L5 leak check: no decoded stream holds the canary's text, colour or
//! pixels. The sources are only read.

mod common;

use std::time::Instant;

use common::synth;
use common::twins::{CANARY_OPERANDS, CANARY_RGB, CANARY_TEXT};
use pdfcraft_content::Matrix;
use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_overlay::{
    LayerStyle, LayeredOutput, RegistrationNote, Roles, Settings, SheetMatch, Visibility, check_output, frame_of_page, place, prepare,
};

const CASES: usize = 10_000;

/// Named objects in a fixed order, so references can be written before the bodies.
const NAMES: [&str; 16] = ["C", "G1", "G2", "MC", "M1", "M2", "V1", "FC", "FS", "F1", "IC", "AP", "APC", "A1", "A2", "AC"];

fn num(name: &str) -> u32 {
    6 + NAMES.iter().position(|n| *n == name).unwrap_or(0) as u32
}

fn refs(s: &str) -> String {
    let mut out = s.to_string();
    for n in NAMES {
        out = out.replace(&format!("{{{n}}}"), &format!("{} 0 R", num(n)));
    }
    out
}

/// Hidden content: the canary's colour and text, saved and restored.
fn canary() -> String {
    format!("q {CANARY_OPERANDS} rg 0 0 50 50 re f BT /FC 12 Tf 5 60 Td ({CANARY_TEXT}) Tj ET Q")
}

fn pick<'a>(rng: &mut common::Rng, from: &[&'a str]) -> &'a str {
    from[(rng.next_u64() % from.len() as u64) as usize]
}

fn huge() -> String {
    format!("1{}", "0".repeat(400))
}

/// One hostile source and the decisions to prepare it with (never on the canary's layer).
fn case(rng: &mut common::Rng) -> (Vec<u8>, Visibility) {
    let configs = [
        "<< /OCGs [{C} {G1} {G2}] /D << /OFF [{C} {G2}] >> >>",
        "<< /OCGs [{C} {G1} {G2}] /D << /BaseState /OFF /ON [{G1}] >> >>",
        "<< /OCGs [{C} {G1} {G2}] /D << /BaseState /Unchanged /OFF [{C}] >> >>",
        "<< /OCGs [{C} {G1} {G2}] /D << /Intent /All /OFF [{C}] /AS [<< /Event /View /Category [/Zoom] /OCGs [{G1}] >> << /Event /Print /Category [/Print] /OCGs [{G2}] >>] >> >>",
        "<< /OCGs [{C} {G1} {G2}] /D << /Intent /Design /OFF [{C}] >> >>",
        "<< /OCGs [{C} {G1} 5] /D << /OFF [{C}] >> >>",
        "<< /OCGs [{C} {G1}] /D << /OFF [{C}] /AS [<< /Event /Hover >>] >> >>",
        "<< /OCGs {C} /D 5 >>",
        "HUGE",
    ];
    let mut config = pick(rng, &configs).to_string();
    if config == "HUGE" {
        // Past the 65,536-entry limit: the whole configuration is unresolved.
        config = format!("<< /OCGs [{{C}} {}] /D << /OFF [{{C}}] >> >>", "{G1} ".repeat(70_000));
    }
    let memberships = [
        "<< /Type /OCMD /OCGs [{G1} {G2}] /P /AnyOff >>",
        "<< /Type /OCMD /VE [/Not [/And {G1} {G2}]] >>",
        "<< /Type /OCMD /VE {V1} >>",
        "<< /Type /OCMD /OCGs [] >>",
        "<< /Type /OCMD /OCGs 7 /P /Odd >>",
        "DEEP",
    ];
    let mut m1 = pick(rng, &memberships).to_string();
    if m1 == "DEEP" {
        m1 = format!("<< /Type /OCMD /VE {}{{G1}}{} >>", "[/Not ".repeat(40), "]".repeat(40));
    }
    let pieces = [
        "/OC /L1 BDC",
        "/OC /L2 BDC",
        "/OC /M1 BDC",
        "/OC /Nope BDC",
        "/OC << /Type /OCMD >> BDC",
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
        "/F1 Do",
        "/FS Do",
        "BI /W 1 /H 1 /CS /G /BPC 8 ID x EI",
        "BI /W ) ID x EI",
        ")",
        "% comment",
    ];
    let content = |rng: &mut common::Rng| {
        let mut s = String::from("0.2 g 20 20 100 40 re f\n");
        for _ in 0..(rng.next_u64() % 20) {
            // The canary's own blocks are whole: the canary is hidden wherever they land.
            match rng.next_u64() % 10 {
                0 => s.push_str(&format!("/OC /LC BDC {} EMC", canary())),
                1 => s.push_str(&format!("/OC /MC BDC {} EMC", canary())),
                2 => s.push_str("/FC Do"),
                3 => s.push_str("q 9 0 0 9 50 50 cm /IC Do Q"),
                _ => s.push_str(pick(rng, &pieces)),
            }
            s.push('\n');
        }
        s
    };
    let page = content(rng);
    let f1 = content(rng);
    let fs = format!("{}\n/FS Do", content(rng));
    let ap = content(rng);
    let f1_oc = pick(rng, &["", "/OC {G2} ", "/OC {M1} ", "/OC {G1} "]);
    let bbox = pick(rng, &["[0 0 20 20]", "[0 0 0 0]", "[5 5]", "BIG"]).replace("BIG", &format!("[0 0 {} 1]", huge()));
    let matrix = pick(rng, &["", "/Matrix [0 1 -1 0 0 0]", "/Matrix [1 0 0 1 (x) 0]", "/Matrix [0 0 0 0 0 0]", "BIG"])
        .replace("BIG", &format!("/Matrix [{} 0 0 1 0 0]", huge()));
    let rect = |rng: &mut common::Rng| {
        pick(rng, &["[10 10 60 40]", "[60 40 10 10]", "[0 0 0 0]", "[1 2 3]", "(x)", "BIG"]).replace("BIG", &format!("[0 0 {} 9]", huge()))
    };
    let kinds = ["Square", "FreeText", "Text", "Ink", "Widget", "Link", "Popup", "Doodle", "Sound", "Stamp", "Redact"];
    let extras =
        ["", "/CA 0.5", "/CA -3", "/OC {G1}", "/OC {M1}", "/PCUndrawn (x)", "/Contents <FEFF2192>", "/FT /Tx /V (v)", "/InkList [[10 10 20 20]]"];
    let annot = |rng: &mut common::Rng| {
        format!(
            "<< /Type /Annot /Subtype /{} /F {} /Rect {} {} {} >>",
            pick(rng, &kinds),
            pick(rng, &["0", "2", "4", "16", "32", "5"]),
            rect(rng),
            pick(rng, &["", "/AP << /N {AP} >>", "/AP << /N 5 >>", "/AP << /N << /On {AP} >> /AS /On >>", "/AP << /N {APC} >>"]),
            pick(rng, &extras)
        )
    };
    let properties = "/Properties << /LC {C} /MC {MC} /L1 {G1} /L2 {G2} /M1 {M1} >>";
    let res = refs(&format!("<< {properties} /Font << /FC 4 0 R >> /XObject << /FC {{FC}} /FS {{FS}} /F1 {{F1}} /IC {{IC}} >> >>"));
    let bodies = vec![
        refs(&format!("<< /Type /Catalog /Pages 2 0 R /OCProperties {config} >>")),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        refs(&format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Rotate {} /Resources {res} /Contents 5 0 R /Annots [{{A1}} {{A2}} {{AC}}] >>",
            pick(rng, &["0", "90", "180", "270", "45"])
        )),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        synth::raw_stream("", &page),
        // C: the canary's layer, off in every configuration that resolves.
        "<< /Type /OCG /Name (Canary) >>".into(),
        "<< /Type /OCG /Name (G1) >>".into(),
        if rng.next_u64().is_multiple_of(4) { "5".into() } else { "<< /Type /OCG /Name (G2) >>".into() },
        // MC: hidden whenever C is.
        refs("<< /Type /OCMD /OCGs [{C}] /P /AllOn >>"),
        refs(&m1),
        refs("<< /Type /OCMD /VE [/And {G1} {G2}] >>"),
        // V1: a cyclic expression.
        refs("[/Or {G1} [/And {V1}]]"),
        synth::raw_stream(&refs(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /OC {{C}} /Resources {res}")), &canary()),
        synth::raw_stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources {res}"), &fs),
        synth::raw_stream(&refs(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 200] {f1_oc}/Resources {res}")), &f1),
        synth::raw_stream(
            &refs("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode /OC {MC}"),
            "E82191>",
        ),
        synth::raw_stream(&format!("/Type /XObject /Subtype /Form /BBox {bbox} {matrix} /Resources {res}"), &ap),
        synth::raw_stream(&refs(&format!("/Type /XObject /Subtype /Form /BBox [0 0 20 20] /OC {{C}} /Resources {res}")), &canary()),
        refs(&annot(rng)),
        refs(&annot(rng)),
        // AC: a canary annotation, hidden by its layer.
        refs(&format!("<< /Type /Annot /Subtype /Square /F 4 /Rect [100 100 150 150] /OC {{{}}} /AP << /N {{APC}} >> >>", pick(rng, &["C", "MC"]))),
    ];
    let decisions = match rng.next_u64() % 5 {
        0 => vec![(ObjRef::new(num("G2"), 0), true)],
        1 => vec![(ObjRef::new(num("M1"), 0), false)],
        2 => vec![(ObjRef::new(num("G1"), 0), false)],
        _ => Vec::new(),
    };
    let omit = if rng.next_u64().is_multiple_of(3) { vec![ObjRef::new(num("A1"), 0)] } else { Vec::new() };
    (synth::raw_pdf(&bodies).to_vec(), Visibility { decisions, omit })
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// L5's leak check on one output.
fn leaks(doc: &Document) -> Option<String> {
    for (r, o) in doc.scan_objects() {
        let Object::Stream(s) = &*o else { continue };
        let Ok(d) = s.decoded() else { continue };
        if contains(&d, CANARY_TEXT.as_bytes()) || contains(&d, CANARY_OPERANDS.as_bytes()) {
            return Some(format!("the canary in object {}", r.num));
        }
        if s.dict.name(b"Subtype") == Some(&b"Image"[..]) && contains(&d, &CANARY_RGB) {
            return Some(format!("a canary image in object {}", r.num));
        }
    }
    None
}

#[test]
fn ten_thousand_hostile_sources() {
    let mut rng = common::Rng(0x0154_0005);
    let (mut composed, mut unresolved, mut refused) = (0usize, 0usize, 0usize);
    let mut slowest = 0.0f64;
    for i in 0..CASES {
        let (bytes, visibility) = case(&mut rng);
        let started = Instant::now();
        let Ok(doc) = Document::open(std::sync::Arc::new(bytes.clone())) else {
            refused += 1;
            continue;
        };
        match prepare(&doc, &[0], &visibility) {
            Err(_) => refused += 1,
            Ok(p) => {
                let report = &p.reports()[0];
                assert_eq!(report.annotations.len(), 3, "case {i}: every annotation accounted for");
                if report.unresolved.is_empty() {
                    let placement =
                        frame_of_page(&doc, 0).and_then(|(f, _)| place(&f, &f, SheetMatch::Off, &Matrix::IDENTITY, false, &Settings::default()));
                    let result = placement.and_then(|placement| {
                        let mut out = LayeredOutput::new()?;
                        out.add_page(Some(p.source(0)?), None, &placement, &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED)?;
                        out.finish()
                    });
                    match result {
                        Ok(out) => {
                            composed += 1;
                            assert!(check_output(&out).is_ok(), "case {i}");
                            if let Some(leak) = leaks(&out) {
                                panic!("case {i}: {leak}");
                            }
                            let saved = pdfcraft_cos::write_full(&out, &pdfcraft_cos::SaveOptions::default()).expect("saves");
                            let back = Document::open(std::sync::Arc::new(saved)).expect("opens");
                            assert!(leaks(&back).is_none() && check_output(&back).is_ok(), "case {i}, saved");
                        }
                        Err(_) => refused += 1,
                    }
                } else {
                    unresolved += 1;
                }
            }
        }
        assert!(!doc.is_modified(), "case {i}: the source is only read");
        assert_eq!(**doc.bytes(), bytes, "case {i}");
        let t = started.elapsed().as_secs_f64();
        slowest = slowest.max(t);
        assert!(t < 2.0, "case {i} took {t:.2} s");
    }
    eprintln!("{CASES} cases: {composed} composed, {unresolved} unresolved, {refused} refused; slowest {slowest:.3} s");
    assert!(composed > 500 && unresolved > 500, "both outcomes: {composed} composed, {unresolved} unresolved, {refused} refused");
}

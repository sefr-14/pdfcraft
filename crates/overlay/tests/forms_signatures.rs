//! Form fields, signatures and redaction marks (OV4.5). Step 4 PRD, T8 and T9:
//!
//! - L9 for forms (O8): a field is redrawn from its value only when its appearance is missing or
//!   the form sets `/NeedAppearances`; a redraw that would show `?` falls back to a clean stored
//!   appearance, else the field is unresolved; check boxes and radio buttons show their `/AS`
//!   state, drawn when missing. Each renders as the control (the field filled by PdfCraft's own
//!   form code, put through the model).
//! - L10: a signed source gives the signed warning, its appearance is drawn as plain graphics, and
//!   the output has no `/AcroForm`, `/Annots`, `/Sig` or `/ByteRange` (`sign::list` finds no
//!   signature in it); unapplied redaction marks give a warning naming the page and count and are
//!   drawn as they appear. Neither the report nor the output's metadata says the output is signed
//!   or that redactions were applied.
//! - L11: the signed and marked sources are only read.

mod common;

use std::sync::Arc;

use common::annots::{form, signed_source};
use common::measure::{Image, Pair};
use common::synth;
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Document, Object};
use pdfcraft_overlay::{
    AppearanceSource, LayerStyle, LayeredOutput, Outcome, Placement, Prepared, RegistrationNote, Roles, Rule, Settings, SheetMatch, SourceWarning,
    Visibility, check_output, frame_of_page, light_tone, luminosity, place, prepare,
};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

fn placement(doc: &Document) -> Placement {
    let (f, _) = frame_of_page(doc, 0).expect("frame");
    place(&f, &f, SheetMatch::Off, &Matrix::IDENTITY, false, &Settings::default()).expect("placement")
}

fn prep(doc: &Document) -> Prepared {
    prepare(doc, &[0], &Visibility::default()).expect("prepares")
}

/// The overlay of page 0 with A only, saved, and its report.
fn overlay(doc: &Document, p: &Prepared) -> (Arc<Vec<u8>>, pdfcraft_overlay::OverlayReport) {
    let mut out = LayeredOutput::new().expect("output");
    out.add_page(Some(p.source(0).expect("page")), None, &placement(doc), &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED)
        .expect("composes");
    let (doc, report) = out.finish_with_report().expect("finishes");
    let bytes = synth::save(&doc, true);
    assert!(check_output(&synth::open(&bytes)).is_ok());
    (bytes, report)
}

fn render(pdf: &Arc<Vec<u8>>, dpi: f64) -> Image {
    let mut r = PageRenderer::new(pdf.clone(), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, scale: (dpi / 72.0) as f32, ..RenderRequest::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba)
}

/// The compositing control for side A alone (Old, red).
fn control(source: &Arc<Vec<u8>>, dpi: f64) -> Image {
    let img = render(source, dpi);
    let pair = Pair { a: [204, 51, 51], b: [51, 204, 204] };
    let mut rgb = Vec::with_capacity(img.rgb.len());
    for p in img.rgb.chunks(3) {
        let alpha = light_tone(1.0, luminosity([p[0], p[1], p[2]].map(|v| f64::from(v) / 255.0))).unwrap_or(0.0);
        rgb.extend(pair.predict(alpha, 0.0).map(|v| v.round().clamp(0.0, 255.0) as u8));
    }
    Image { width: img.width, height: img.height, rgb }
}

fn max_diff(x: &Image, y: &Image) -> u8 {
    assert_eq!((x.width, x.height), (y.width, y.height));
    x.rgb.iter().zip(&y.rgb).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)
}

/// Every string shown by `Tj`/`TJ` in the document's decoded streams.
fn shown_text(doc: &Document) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for (_, o) in doc.scan_objects() {
        let Object::Stream(s) = &*o else { continue };
        let Ok(d) = s.decoded() else { continue };
        for op in parse(&d).ops {
            if op.is("Tj") {
                out.extend(op.operands.first().and_then(|o| o.as_string()).map(|s| s.bytes.clone()));
            }
        }
    }
    out
}

fn outcomes(p: &Prepared) -> Vec<Outcome> {
    p.reports()[0].annotations.iter().map(|e| e.outcome.clone()).collect()
}

// ── T8: form fields (O8) ─────────────────────────────────────────────────────────────────────

#[test]
fn fields_are_redrawn_only_when_they_must_be() {
    // Without /NeedAppearances: stored appearances stay (even the stale one); the missing ones
    // are drawn from their values.
    let doc = synth::open(&form(false));
    let p = prep(&doc);
    assert_eq!(
        outcomes(&p),
        [
            Outcome::Drawn(AppearanceSource::Stored),
            Outcome::Drawn(AppearanceSource::Regenerated),
            Outcome::Drawn(AppearanceSource::Regenerated),
            Outcome::Drawn(AppearanceSource::Stored),
            Outcome::Drawn(AppearanceSource::Stored),
            Outcome::Drawn(AppearanceSource::Regenerated),
        ]
    );
    let (out, _) = overlay(&doc, &p);
    let text = shown_text(&synth::open(&out));
    assert!(text.contains(&b"Old value".to_vec()) && text.contains(&b"Lisbon".to_vec()) && text.contains(&b"Large".to_vec()), "{text:?}");

    // With /NeedAppearances: text and choice fields are redrawn from their values.
    let doc = synth::open(&form(true));
    let p = prep(&doc);
    assert_eq!(outcomes(&p)[0], Outcome::Drawn(AppearanceSource::Regenerated));
    let (out, _) = overlay(&doc, &p);
    let text = shown_text(&synth::open(&out));
    assert!(text.contains(&b"New value".to_vec()) && !text.contains(&b"Old value".to_vec()), "{text:?}");
}

#[test]
fn fields_render_as_the_control() {
    // The control: the same form filled by PdfCraft's own form code, which stores each field's
    // appearance for its value, rendered and put through the model.
    let bytes = form(true);
    let doc = synth::open(&bytes);
    let mut filled = doc.clone();
    pdfcraft_forms::set_value(&mut filled, "name", &pdfcraft_forms::FieldValue::Text("New value".into())).expect("name");
    pdfcraft_forms::set_value(&mut filled, "city", &pdfcraft_forms::FieldValue::Text("Lisbon".into())).expect("city");
    pdfcraft_forms::set_value(&mut filled, "size", &pdfcraft_forms::FieldValue::Choice(vec!["Large".into()])).expect("size");
    // The check box without appearances gets the ones PdfCraft's form code draws for a check box
    // (its value doesn't change, so `set_value` leaves its appearance alone).
    let later = pdfcraft_forms::fields(&filled).into_iter().find(|f| f.name == "later").expect("later");
    let w = later.widgets[0].clone();
    let ap = pdfcraft_forms::appearance::check_box_states(&mut filled, &w, later.kind, "Yes");
    filled.update_dict(w.obj, |d| d.set(b"AP".to_vec(), Object::Dict(ap))).expect("widget");
    let filled = synth::save(&filled, true);
    let p = prep(&doc);
    let (out, _) = overlay(&doc, &p);
    let d = max_diff(&render(&out, 100.0), &control(&filled, 100.0));
    assert!(d <= 2, "{d} levels from the control");
}

#[test]
fn a_redraw_that_would_show_a_question_mark_falls_back_or_is_unresolved() {
    // /NeedAppearances, a value Helvetica can't draw: the clean stored appearance is drawn; without
    // one, the field is unresolved, naming the character; a stored one PdfCraft drew with ? too.
    let make = |ap: &str, mark: &str| {
        synth::open(&synth::raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> /NeedAppearances true >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R /Annots [6 0 R] >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
            synth::raw_stream("", ""),
            format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (word) /V <FEFF65E5> /F 4 /Rect [20 150 160 170] {ap} {mark} >>"),
            synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 140 20]", "0 g 2 2 10 10 re f"),
        ]))
    };
    let p = prep(&make("/AP << /N 7 0 R >>", ""));
    assert_eq!(outcomes(&p), [Outcome::Drawn(AppearanceSource::Stored)]);
    let p = prep(&make("", ""));
    assert_eq!(outcomes(&p), [Outcome::Unresolved(Rule::AnnotationUndrawableText)]);
    assert!(p.reports()[0].unresolved[0].detail.as_deref().is_some_and(|d| d.contains("U+65E5")));
    let p = prep(&make("/AP << /N 7 0 R >>", "/PCUndrawn <FEFF65E5>"));
    assert_eq!(outcomes(&p), [Outcome::Unresolved(Rule::AnnotationUndrawableText)]);
}

// ── T9: signatures and redaction marks ───────────────────────────────────────────────────────

/// Text that would claim the output is signed, or that redactions were applied.
fn claims(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "is signed",
        "was signed",
        "has been signed",
        "signature is valid",
        "redactions were applied",
        "redactions applied",
        "has been redacted",
        "was redacted",
    ]
    .iter()
    .any(|c| t.contains(c))
}

#[test]
fn signed_sources_warn_and_the_output_carries_no_signature() {
    let bytes = signed_source();
    let before = bytes.clone();
    let src = Document::open(Arc::new(bytes.clone())).expect("opens");
    // Independent check: the sign crate sees a signed field in the source.
    let trust = pdfcraft_sign::pdf::TrustStore::default();
    assert!(pdfcraft_sign::pdf::list(&src, &bytes, &trust).iter().any(|s| s.signed));
    let p = prep(&src);
    assert_eq!(p.warnings(), [SourceWarning::Signed { fields: 1 }]);
    assert_eq!(outcomes(&p), [Outcome::Drawn(AppearanceSource::Stored)], "the appearance is drawn as plain graphics");
    let (out, report) = overlay(&src, &p);
    let warnings = report.warnings();
    assert_eq!(
        warnings,
        [
            "Document A has 1 signed signature field. The overlay shows their appearance only. It is not signed and proves nothing about those signatures."
        ]
    );
    // The output: no form, annotation, signature dictionary or byte range, and nothing the sign
    // crate takes for a signature.
    let doc = synth::open(&out);
    let raw = String::from_utf8_lossy(&out);
    for key in ["/AcroForm", "/Annots", "/Sig", "/ByteRange", "/SigFlags"] {
        assert!(!raw.contains(key), "{key} in the output");
    }
    assert!(pdfcraft_sign::pdf::list(&doc, &out, &trust).is_empty());
    for (_, o) in doc.scan_objects() {
        if let Some(d) = o.as_dict() {
            assert!(!matches!(d.name(b"Type"), Some(b"Sig" | b"DocTimeStamp")) && !d.contains(b"ByteRange"));
        }
    }
    // Neither the report nor the metadata says the output is signed.
    assert!(!claims(&report.text()), "{}", report.text());
    let info = doc.trailer().get(b"Info").and_then(|i| doc.dict(i)).expect("info");
    let subject = info.get(b"Subject").and_then(|s| doc.resolve(s).as_string().map(|s| s.to_text())).unwrap_or_default();
    assert!(!claims(&subject));
    // L11: the source is only read.
    assert!(!src.is_modified());
    assert_eq!(**src.bytes(), before);
}

#[test]
fn redaction_marks_warn_and_are_drawn_as_they_appear() {
    let bytes = synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R] >>".into(),
        synth::raw_stream("", "BT /F1 14 Tf 20 150 Td (Secret account 1234) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Annot /Subtype /Redact /F 4 /Rect [18 145 230 165] /QuadPoints [18 165 230 165 18 145 230 145] /OverlayText (REMOVED) /IC [0 0 0] >>".into(),
        "<< /Type /Annot /Subtype /Redact /F 4 /Rect [18 20 100 40] /IC [0 0 0] >>".into(),
    ]);
    let src = synth::open(&bytes);
    // Independent count: the redact crate's marks.
    assert_eq!(pdfcraft_redact::marks(&src).len(), 2);
    let p = prep(&src);
    assert_eq!(p.warnings(), [SourceWarning::RedactionMarks { page: 0, count: 2 }]);
    // The first is drawn as PdfCraft shows it; the second, without quadrilaterals or an
    // appearance, shows nothing on screen either, but still counts in the warning.
    assert_eq!(outcomes(&p), [Outcome::Drawn(AppearanceSource::Generated), Outcome::Excluded(pdfcraft_overlay::Exclusion::NoAppearance)]);
    let (out, report) = overlay(&src, &p);
    assert_eq!(report.warnings(), ["Document A, page 1: 2 redaction marks not applied: the content under them is still in the overlay."]);
    // Never applied: the content under the mark is still there.
    assert!(shown_text(&synth::open(&out)).contains(&b"Secret account 1234".to_vec()));
    assert!(!claims(&report.text()), "{}", report.text());
    assert!(!src.is_modified());
}

/// The defensive check turns down a signature or byte range that reached an output.
#[test]
fn the_defensive_check_finds_signatures() {
    let doc = synth::open(&form(false));
    let p = prep(&doc);
    let mut out = LayeredOutput::new().expect("output");
    out.add_page(Some(p.source(0).expect("page")), None, &placement(&doc), &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED)
        .expect("composes");
    let good = out.finish().expect("finishes");
    for (what, key, value) in
        [("a signature dictionary", "Type", Object::name("Sig")), ("a byte range", "ByteRange", Object::Array(vec![Object::Int(0)]))]
    {
        let mut bad = good.clone();
        let mut d = pdfcraft_cos::Dict::new();
        d.set(key.as_bytes().to_vec(), value);
        let r = bad.add(Object::Dict(d));
        let page = pdfcraft_model::pages(&bad)[0].obj;
        bad.update_dict(page, |p| p.set(b"PieceInfo".to_vec(), Object::Ref(r))).expect("page");
        assert!(matches!(check_output(&bad), Err(pdfcraft_overlay::OverlayError::Check { .. })), "{what}");
    }
}

//! Regression tests for the vendored renderer's luminosity soft masks (step 3 PRD, decision
//! D2; vendor/README.md, hayro patch 10). A luminosity mask takes its values from PDF's
//! luminosity, 0.30 R + 0.59 G + 0.11 B, as Poppler and PDFium compute it. Gray masks, alpha
//! masks, image soft masks, transparency groups and ordinary painting must not change.
//!
//! Each case paints one 60 × 60 pt page and reads the pixel at its centre at 72 dpi.

mod common;

use std::sync::Arc;

use common::synth::{self, Page, Res};
use pdfcraft_cos::{Dict, Object};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

/// The centre pixel of page 1, composited onto white, as 0-to-255 floats.
fn centre(bytes: &Arc<Vec<u8>>) -> [f64; 3] {
    let mut r = PageRenderer::new(bytes.clone(), RenderConfig::default());
    let page = r.render(RenderRequest { page: 0, scale: 1.0, ..RenderRequest::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    let (w, h) = (page.width as usize, page.height as usize);
    let i = ((h / 2) * w + w / 2) * 4;
    let px = &page.rgba[i..i + 4];
    let back = 255.0 - f64::from(px[3]);
    [f64::from(px[0]) + back, f64::from(px[1]) + back, f64::from(px[2]) + back]
}

#[track_caller]
fn assert_near(got: [f64; 3], want: [f64; 3], what: &str) {
    for c in 0..3 {
        assert!((got[c] - want[c]).abs() <= 1.0, "{what}: got {got:?}, want {want:?} within 1 level");
    }
}

fn form(content: &str, group: Option<Dict>, resources: Option<Dict>) -> Res {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(60), Object::Int(60)]));
    if let Some(g) = group {
        d.set(b"Group".to_vec(), Object::Dict(g));
    }
    if let Some(r) = resources {
        d.set(b"Resources".to_vec(), Object::Dict(r));
    }
    Res::Stream(d, content.as_bytes().to_vec())
}

fn group(cs: &str, isolated: bool) -> Dict {
    let mut g = Dict::new();
    g.set(b"S".to_vec(), Object::name("Transparency"));
    g.set(b"CS".to_vec(), Object::name(cs));
    if isolated {
        g.set(b"I".to_vec(), Object::Bool(true));
    }
    g
}

/// A page painting black (or `paint`) through a soft mask of type `kind` whose group draws
/// `mask_content`. `backdrop` is the mask's `/BC`, if any.
fn masked(kind: &str, mask_content: &str, mask_resources: Option<Dict>, backdrop: Option<[f64; 3]>, paint: &str) -> Arc<Vec<u8>> {
    let content_len = mask_content.len();
    let res = mask_resources
        .map(|r| {
            let mut out = Vec::new();
            pdfcraft_cos::serialize(&Object::Dict(r), &mut out);
            String::from_utf8(out).expect("a dictionary of names and numbers is text")
        })
        .unwrap_or_default();
    let bc = backdrop.map(|c| format!("/BC [{} {} {}]", c[0], c[1], c[2])).unwrap_or_default();
    let page_content = format!("q /M gs {paint} 0 0 60 60 re f Q");
    synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 60 60] /Resources << /ExtGState << /M 4 0 R >> >> /Contents 5 0 R >>".into(),
        format!("<< /Type /ExtGState /SMask << /Type /Mask /S /{kind} /G 6 0 R {bc} >> >>"),
        synth::raw_stream("", &page_content),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 60 60] /Group << /S /Transparency /CS /DeviceRGB >> /Resources {} /Length {content_len} >>\nstream\n{mask_content}\nendstream",
            if res.is_empty() { "<< >>".to_string() } else { res }
        ),
    ])
}

/// Coloured mask content: the mask value is PDF's luminosity, not Rec. 709's. Before the patch
/// these were 17 to 32 levels off (pure green: 72.6 instead of 104.6).
#[test]
fn coloured_luminosity_masks_use_pdf_luminosity() {
    for rgb in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.2, 0.6, 0.9], [1.0, 1.0, 0.0], [0.4, 0.1, 0.7]] {
        let l = 0.30 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2];
        let pdf = masked("Luminosity", &format!("{} {} {} rg 0 0 60 60 re f", rgb[0], rgb[1], rgb[2]), None, None, "0 g");
        let want = 255.0 * (1.0 - l);
        assert_near(centre(&pdf), [want; 3], &format!("black through a mask of {rgb:?}"));
    }
}

/// Gray mask content is the same under any luminosity weights: unchanged by the patch.
#[test]
fn gray_luminosity_masks_are_unchanged() {
    for g in [0.0, 0.25, 0.5, 0.8, 1.0] {
        let pdf = masked("Luminosity", &format!("{g} g 0 0 60 60 re f"), None, None, "0 g");
        assert_near(centre(&pdf), [255.0 * (1.0 - g); 3], &format!("gray {g}"));
    }
    // A white backdrop where the group draws nothing: mask 1, the paint shows in full.
    let pdf = masked("Luminosity", "0 g 0 0 1 1 re f", None, Some([1.0, 1.0, 1.0]), "1 0 0 rg");
    assert_near(centre(&pdf), [255.0, 0.0, 0.0], "white backdrop");
}

/// Alpha masks take the group's opacity, whatever its colour: unchanged by the patch.
#[test]
fn alpha_masks_are_unchanged() {
    let mut h = Dict::new();
    h.set(b"ca".to_vec(), Object::Real(0.25));
    let mut states = Dict::new();
    states.set(b"H".to_vec(), Object::Dict(h));
    let mut res = Dict::new();
    res.set(b"ExtGState".to_vec(), Object::Dict(states));
    for colour in ["0 g", "1 0 0 rg", "0 1 0 rg", "0.2 0.6 0.9 rg"] {
        let pdf = masked("Alpha", &format!("/H gs {colour} 0 0 60 60 re f"), Some(res.clone()), None, "0 g");
        assert_near(centre(&pdf), [255.0 * 0.75; 3], &format!("alpha mask drawn in {colour}"));
    }
}

/// Transparency groups and blending don't involve mask luminosity: unchanged by the patch.
#[test]
fn transparency_groups_are_unchanged() {
    let mut mb = Dict::new();
    mb.set(b"BM".to_vec(), Object::name("Multiply"));
    let mut states = Dict::new();
    states.set(b"MB".to_vec(), Object::Dict(mb));
    let mut res = Dict::new();
    res.set(b"ExtGState".to_vec(), Object::Dict(states));
    let page = Page::new(60.0, 60.0, "/G Do").with_resource(
        "XObject",
        "G",
        form("1 0.5 0 rg 0 0 60 60 re f /MB gs 0.5 1 1 rg 0 0 60 60 re f", Some(group("DeviceRGB", true)), Some(res)),
    );
    assert_near(centre(&synth::pdf(&[page])), [127.5, 127.5, 0.0], "Multiply inside an isolated group");
}

/// Ordinary painting and an image soft mask (an image's own `/SMask`) are unchanged.
#[test]
fn unrelated_rendering_is_unchanged() {
    for (op, want) in [("1 0 0 rg", [255.0, 0.0, 0.0]), ("0.2 0.6 0.9 rg", [51.0, 153.0, 229.5]), ("0.5 g", [127.5; 3])] {
        let pdf = synth::pdf(&[Page::new(60.0, 60.0, format!("{op} 0 0 60 60 re f"))]);
        assert_near(centre(&pdf), want, op);
    }
    // A red 2 × 2 image with a 50% gray soft mask: red over white at half opacity.
    let bytes = synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 60 60] /Resources << /XObject << /I 4 0 R >> >> /Contents 6 0 R >>".into(),
        synth::raw_stream(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 5 0 R /Filter /ASCIIHexDecode",
            &format!("{}>", "FF0000".repeat(4)),
        ),
        synth::raw_stream(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode",
            "80808080>",
        ),
        synth::raw_stream("", "q 60 0 0 60 0 0 cm /I Do Q"),
    ]);
    assert_near(centre(&bytes), [255.0, 127.0, 127.0], "image soft mask");
}

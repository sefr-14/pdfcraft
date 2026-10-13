//! Compositing (overlay PRD, Compositing; step 3 PRD, Technical design): one overlay page from
//! two source pages and an OV2 placement.
//!
//! Each document draws one rectangle in its layer colour over the whole page, under a graphics
//! state with Multiply blending, opacity 1 and a luminosity soft mask. The mask's group is the
//! source page as a Form XObject, drawn where the placement puts it, over a white backdrop, and
//! its transfer function is the light-tone function m_t(L) = clamp(1 − L / t, 0, 1). So a
//! document's colour shows exactly where its page has ink, in proportion to how dark the ink is,
//! and the two colours multiply where both documents have ink.
//!
//! The source page's content is copied, not rewritten: its operators stay as they are inside the
//! source form, so vector content stays vector. All geometry comes from one OV2 matrix per
//! document, written at full precision.

use pdfcraft_content::{Op, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{OverlayError, Placement, frame_of_page};

/// The smallest and largest light-tone threshold t (overlay PRD, Light tones).
pub const MIN_THRESHOLD: f64 = 0.5;
pub const MAX_THRESHOLD: f64 = 1.0;

/// The default colours: red on A and cyan on B (overlay PRD, decision 5).
pub const RED: [u8; 3] = [204, 51, 51];
pub const CYAN: [u8; 3] = [51, 204, 204];

/// How the overlay is drawn: each document's layer colour and the light-tone threshold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub colour_a: [u8; 3],
    pub colour_b: [u8; 3],
    /// The paper threshold t in [0.5, 1]: tones at or lighter than t print as paper. 1 is plain
    /// inversion.
    pub threshold: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style { colour_a: RED, colour_b: CYAN, threshold: 1.0 }
    }
}

/// One page of a source document.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub doc: &'a Document,
    /// 0-based.
    pub page: usize,
}

/// Which document of the pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    A,
    B,
}

impl Side {
    pub fn letter(self) -> char {
        match self {
            Side::A => 'A',
            Side::B => 'B',
        }
    }
}

/// The luminosity of a DeviceRGB colour (components 0 to 1), as the overlay PRD defines it:
/// 0.30 R + 0.59 G + 0.11 B.
pub fn luminosity(rgb: [f64; 3]) -> f64 {
    0.30 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2]
}

/// The light-tone function m_t(L) = min(1, max(0, 1 − L / t)). `None` when t is outside
/// [0.5, 1] or either value isn't finite.
pub fn light_tone(threshold: f64, luminosity: f64) -> Option<f64> {
    if !valid_threshold(threshold) || !luminosity.is_finite() {
        return None;
    }
    Some((1.0 - luminosity / threshold).clamp(0.0, 1.0))
}

fn valid_threshold(t: f64) -> bool {
    t.is_finite() && (MIN_THRESHOLD..=MAX_THRESHOLD).contains(&t)
}

fn check_threshold(t: f64) -> Result<(), OverlayError> {
    if valid_threshold(t) {
        Ok(())
    } else {
        Err(OverlayError::OutOfRange { what: "the light-tone threshold", value: t, min: MIN_THRESHOLD, max: MAX_THRESHOLD })
    }
}

/// The soft mask's transfer function for threshold t: one Type 2 (exponential) function,
/// `/Domain [0 1] /Range [0 1] /C0 [1] /C1 [1 − 1/t] /N 1`. It is 1 − L / t, and `/Range`
/// clips it to [0, 1], so it is exactly m_t(L).
pub fn transfer_function(threshold: f64) -> Result<Dict, OverlayError> {
    check_threshold(threshold)?;
    let mut f = Dict::new();
    f.set(b"FunctionType".to_vec(), Object::Int(2));
    f.set(b"Domain".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(1)]));
    f.set(b"Range".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(1)]));
    f.set(b"C0".to_vec(), Object::Array(vec![Object::Int(1)]));
    f.set(b"C1".to_vec(), Object::Array(vec![Object::Real(1.0 - 1.0 / threshold)]));
    f.set(b"N".to_vec(), Object::Int(1));
    Ok(f)
}

/// A new document with one overlay page. See [`compose_page`].
pub fn compose(a: Option<Source<'_>>, b: Option<Source<'_>>, placement: &Placement, style: &Style) -> Result<Document, OverlayError> {
    let mut out = Document::new_empty();
    compose_page(&mut out, a, b, placement, style)?;
    Ok(out)
}

/// Append one overlay page to `out` and return it. `a` and `b` are the pages to compare; either
/// may be missing (a blank in the pairing), not both. `placement` comes from [`crate::place`]
/// for these two pages. The sources are only read.
pub fn compose_page(
    out: &mut Document,
    a: Option<Source<'_>>,
    b: Option<Source<'_>>,
    placement: &Placement,
    style: &Style,
) -> Result<ObjRef, OverlayError> {
    compose_page_marked(out, a, b, placement, style, None)
}

/// An optional-content marker around one document's fill (step 4): the property-list name the
/// page's `/Resources /Properties` gives the group, and the group itself.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Marker {
    pub name: &'static str,
    pub group: ObjRef,
}

/// [`compose_page`], with each document's fill optionally wrapped in its layer's marker:
/// `/OC /<name> BDC … EMC`, the name mapped to the group in the page's `/Properties`. Without
/// markers the page is exactly what [`compose_page`] has always written.
pub(crate) fn compose_page_marked(
    out: &mut Document,
    a: Option<Source<'_>>,
    b: Option<Source<'_>>,
    placement: &Placement,
    style: &Style,
    markers: Option<[Marker; 2]>,
) -> Result<ObjRef, OverlayError> {
    check_threshold(style.threshold)?;
    check_placement(placement)?;
    if a.is_none() && b.is_none() {
        return Err(OverlayError::NoSources);
    }
    let page_box = placement.page_box();
    let tr = out.add(Object::Dict(transfer_function(style.threshold)?));

    let mut states = Dict::new();
    let mut properties = Dict::new();
    let mut ops = Vec::new();
    for (i, (side, source, matrix, colour)) in
        [(Side::A, a, &placement.a, style.colour_a), (Side::B, b, &placement.b, style.colour_b)].into_iter().enumerate()
    {
        let Some(source) = source else { continue };
        let form = source_form(out, source, side)?;
        let group = mask_group(out, form, matrix.0, page_box);
        let name = match side {
            Side::A => "GA",
            Side::B => "GB",
        };
        let marker = markers.and_then(|m| m.get(i).copied());
        states.set(name.as_bytes().to_vec(), Object::Ref(out.add(Object::Dict(graphics_state(group, tr)))));
        if let Some(m) = marker {
            properties.set(m.name.as_bytes().to_vec(), Object::Ref(m.group));
            ops.push(Op::new("BDC", vec![Object::name("OC"), Object::name(m.name)]));
        }
        ops.push(Op::new("q", Vec::new()));
        ops.push(Op::new("gs", vec![Object::name(name)]));
        ops.push(Op::new("rg", colour.iter().map(|c| Object::Real(f64::from(*c) / 255.0)).collect()));
        ops.push(Op::new("re", vec![Object::Int(0), Object::Int(0), Object::Real(page_box[2]), Object::Real(page_box[3])]));
        ops.push(Op::new("f", Vec::new()));
        ops.push(Op::new("Q", Vec::new()));
        if marker.is_some() {
            ops.push(Op::new("EMC", Vec::new()));
        }
    }
    let contents = out.add(Object::Stream(Stream::flate(Dict::new(), &serialize_ops(&ops))));

    let mut resources = Dict::new();
    resources.set(b"ExtGState".to_vec(), Object::Dict(states));
    if !properties.is_empty() {
        resources.set(b"Properties".to_vec(), Object::Dict(properties));
    }
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"MediaBox".to_vec(), real_array(&page_box));
    if placement.unit != 1.0 {
        page.set(b"UserUnit".to_vec(), Object::Real(placement.unit));
    }
    page.set(b"Group".to_vec(), Object::Dict(transparency_group(false)));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    page.set(b"Contents".to_vec(), Object::Ref(contents));
    append_page(out, page)
}

/// The placement must describe a real page: finite, positive size and unit, finite matrices.
fn check_placement(p: &Placement) -> Result<(), OverlayError> {
    for (what, v) in [("the output page width", p.width), ("the output page height", p.height), ("the output page unit", p.unit)] {
        if !v.is_finite() {
            return Err(OverlayError::NotFinite { what });
        }
        if v <= 0.0 {
            return Err(OverlayError::NotPositive { what, value: v });
        }
    }
    if !p.page_box().iter().all(|v| v.is_finite()) {
        return Err(OverlayError::NotFinite { what: "the output page box" });
    }
    if !(p.a.0.iter().chain(p.b.0.iter()).all(|v| v.is_finite())) {
        return Err(OverlayError::NotFinite { what: "the placement" });
    }
    Ok(())
}

/// The source page as a Form XObject in `out`, drawn in its own user space: `page_as_form` copies
/// its content and resources, then its `/BBox` becomes OV2's visible box (the crop box clipped to
/// the media box) and its `/Matrix` the identity, so the placement does all the geometry.
fn source_form(out: &mut Document, source: Source<'_>, side: Side) -> Result<ObjRef, OverlayError> {
    let fail = |reason: String| OverlayError::Source { side: side.letter(), page: source.page, reason };
    let (frame, _warnings) = frame_of_page(source.doc, source.page).map_err(|e| fail(e.to_string()))?;
    let (form, _size) = pdfcraft_organize::page_as_form(out, source.doc, source.page).map_err(|e| fail(e.to_string()))?;
    out.update_dict(form, |d| {
        d.set(b"BBox".to_vec(), real_array(&frame.visible));
        d.set(b"Matrix".to_vec(), Object::Array([1, 0, 0, 1, 0, 0].into_iter().map(Object::Int).collect()));
    })
    .map_err(|e| fail(e.to_string()))?;
    Ok(form)
}

/// The soft mask's group: isolated, in DeviceRGB, covering the page, drawing the source form under
/// `matrix` (source user space → output user space), written at full precision.
fn mask_group(out: &mut Document, form: ObjRef, matrix: [f64; 6], page_box: [f64; 4]) -> ObjRef {
    let content = serialize_ops(&[
        Op::new("q", Vec::new()),
        Op::new("cm", matrix.iter().map(|v| Object::Real(*v)).collect()),
        Op::new("Do", vec![Object::name("S")]),
        Op::new("Q", Vec::new()),
    ]);
    let mut xobjects = Dict::new();
    xobjects.set(b"S".to_vec(), Object::Ref(form));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), real_array(&page_box));
    d.set(b"Group".to_vec(), Object::Dict(transparency_group(true)));
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    out.add(Object::Stream(Stream::flate(d, &content)))
}

/// `<< /S /Transparency /CS /DeviceRGB >>`, isolated or not (never knockout).
fn transparency_group(isolated: bool) -> Dict {
    let mut g = Dict::new();
    g.set(b"S".to_vec(), Object::name("Transparency"));
    g.set(b"CS".to_vec(), Object::name("DeviceRGB"));
    if isolated {
        g.set(b"I".to_vec(), Object::Bool(true));
    }
    g
}

/// Multiply, opacity 1, and a luminosity soft mask over a white backdrop through `tr`.
fn graphics_state(group: ObjRef, tr: ObjRef) -> Dict {
    let mut mask = Dict::new();
    mask.set(b"Type".to_vec(), Object::name("Mask"));
    mask.set(b"S".to_vec(), Object::name("Luminosity"));
    mask.set(b"G".to_vec(), Object::Ref(group));
    mask.set(b"BC".to_vec(), Object::Array(vec![Object::Int(1), Object::Int(1), Object::Int(1)]));
    mask.set(b"TR".to_vec(), Object::Ref(tr));
    let mut gs = Dict::new();
    gs.set(b"Type".to_vec(), Object::name("ExtGState"));
    gs.set(b"BM".to_vec(), Object::name("Multiply"));
    gs.set(b"CA".to_vec(), Object::Int(1));
    gs.set(b"ca".to_vec(), Object::Int(1));
    gs.set(b"SMask".to_vec(), Object::Dict(mask));
    gs
}

fn real_array(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

/// Append `page` to the end of `out`'s page tree (its root `/Pages` node) and return it.
fn append_page(out: &mut Document, mut page: Dict) -> Result<ObjRef, OverlayError> {
    let no_tree = || OverlayError::Output { reason: "the output document has no page tree".into() };
    let root = out.root().ok_or_else(no_tree)?;
    let pages = out.get(root).as_dict().and_then(|c| c.reference(b"Pages")).ok_or_else(no_tree)?;
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    let r = out.add(Object::Dict(page));
    out.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(i64::try_from(kids.len()).unwrap_or(i64::MAX)));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .map_err(|e| OverlayError::Output { reason: e.to_string() })?;
    Ok(r)
}

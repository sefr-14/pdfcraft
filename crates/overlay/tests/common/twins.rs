//! Layered sources and their twins (step 4 PRD: L5, L6, T4, T5).
//!
//! A case describes one source page as a tree of content nodes, each layer marked with whether it
//! shows. From that one description the generator writes two files: the source, with its optional
//! content, and the twin, which draws only what should be visible and has no optional content at
//! all. The twin comes from the generator's own knowledge of the case (the `shown` flags written
//! into it), never from the resolver, so comparing the resolver's result with it is a real check.
//!
//! Everything hidden carries the canary: a fill in a unique colour and a unique text, so a leak
//! can be found in any decoded stream and in any render.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use super::synth;

/// The canary's text, drawn by hidden content only.
pub const CANARY_TEXT: &str = "CANARY-7Q4";
/// The canary's colour as written in content streams, and as 8-bit RGB.
pub const CANARY_OPERANDS: &str = "0.91 0.13 0.57";
pub const CANARY_RGB: [u8; 3] = [232, 33, 145];

/// One piece of content.
#[derive(Clone, Debug)]
pub enum Node {
    /// Operators drawn as written wherever they are visible.
    Ops(&'static str),
    /// `/OC /<property> BDC … EMC`, shown or not.
    Layer(&'static str, bool, Vec<Node>),
    /// The start and end of a layer, written separately so a block can span content streams.
    Begin(&'static str, bool),
    End,
    /// Hidden content: a fill in the canary colour and the canary text, saved and restored.
    Canary,
    /// Other marked content (a tag, `/MCID`), given by its opening (`/P << /MCID 0 >> BDC`).
    Tag(&'static str, Vec<Node>),
    /// Draws XObject `name`, which shows when it and everything around it do.
    Do(&'static str),
    /// A graphics-state change that later visible content uses: kept even inside a hidden
    /// block, as viewers apply it.
    Live(&'static str),
    /// A clipping path `x y w h` in a hidden block that later content is drawn through: written
    /// `re W f` in the source; the twin keeps the clip and paints nothing (`re W n`).
    LiveClip(&'static str),
}

/// What an XObject is.
#[derive(Clone, Debug)]
pub enum Kind {
    Form(Vec<Node>),
    /// A 2 × 2 image: grey when the case expects it shown, the canary colour when hidden.
    Image,
}

#[derive(Clone, Debug)]
pub struct XObject {
    pub name: &'static str,
    /// `/OC`: the object (by name) and whether it shows.
    pub oc: Option<(&'static str, bool)>,
    pub kind: Kind,
    /// A transparency group (for a soft mask's form).
    pub group: bool,
}

impl XObject {
    pub fn form(name: &'static str, content: Vec<Node>) -> Self {
        XObject { name, oc: None, kind: Kind::Form(content), group: false }
    }

    pub fn with_oc(mut self, object: &'static str, shown: bool) -> Self {
        self.oc = Some((object, shown));
        self
    }

    pub fn image(name: &'static str) -> Self {
        XObject { name, oc: None, kind: Kind::Image, group: false }
    }

    /// Whether this XObject is drawn where its `Do` is visible.
    pub fn shown(&self) -> bool {
        self.oc.is_none_or(|(_, s)| s)
    }
}

/// One layered source page.
#[derive(Clone, Debug, Default)]
pub struct Case {
    pub name: &'static str,
    /// Named objects (groups, membership dictionaries, …), bodies with `{name}` references.
    pub objects: Vec<(&'static str, String)>,
    /// The catalog's `/OCProperties`, with `{name}` references. `None`: no optional content.
    pub oc_properties: Option<String>,
    /// Marker properties: property name → object name. Pages, forms and patterns share them.
    pub properties: Vec<(&'static str, &'static str)>,
    /// The page's content streams.
    pub contents: Vec<Vec<Node>>,
    pub xobjects: Vec<XObject>,
    /// Tiling patterns: name and content (a 20 × 20 cell).
    pub patterns: Vec<(&'static str, Vec<Node>)>,
    /// Graphics states with a luminosity soft mask: name and the form (an XObject by name) that is
    /// its group.
    pub soft_masks: Vec<(&'static str, &'static str)>,
    /// Explicit decisions: object name and state.
    pub decisions: Vec<(&'static str, bool)>,
}

/// A case written out.
pub struct Built {
    pub source: Arc<Vec<u8>>,
    pub twin: Arc<Vec<u8>>,
    /// Object numbers of the named objects in `source`.
    pub numbers: HashMap<&'static str, u32>,
}

fn canary() -> String {
    format!("q {CANARY_OPERANDS} rg 40 40 60 60 re f BT /FC 14 Tf 40 120 Td ({CANARY_TEXT}) Tj ET Q\n")
}

/// Write `nodes` into the source and the twin. `stack` holds the shown flags of the enclosing
/// layers.
fn emit(nodes: &[Node], stack: &mut Vec<bool>, xobjects: &[XObject], source: &mut String, twin: &mut String) {
    for n in nodes {
        let shown = stack.iter().all(|s| *s);
        match n {
            Node::Ops(s) => {
                source.push_str(s);
                source.push('\n');
                if shown {
                    twin.push_str(s);
                    twin.push('\n');
                }
            }
            Node::Layer(p, v, kids) => {
                source.push_str(&format!("/OC /{p} BDC\n"));
                stack.push(*v);
                emit(kids, stack, xobjects, source, twin);
                stack.pop();
                source.push_str("EMC\n");
            }
            Node::Begin(p, v) => {
                source.push_str(&format!("/OC /{p} BDC\n"));
                stack.push(*v);
            }
            Node::End => {
                source.push_str("EMC\n");
                stack.pop();
            }
            Node::Canary => {
                // Meant to be hidden; a case that shows it puts it in the twin too, where the
                // tests catch it.
                source.push_str(&canary());
                if shown {
                    twin.push_str(&canary());
                }
            }
            Node::Tag(open, kids) => {
                source.push_str(open);
                source.push('\n');
                if shown {
                    twin.push_str(open);
                    twin.push('\n');
                }
                emit(kids, stack, xobjects, source, twin);
                source.push_str("EMC\n");
                if shown {
                    twin.push_str("EMC\n");
                }
            }
            Node::Do(x) => {
                source.push_str(&format!("/{x} Do\n"));
                let drawn = xobjects.iter().find(|o| o.name == *x).is_none_or(XObject::shown);
                if shown && drawn {
                    twin.push_str(&format!("/{x} Do\n"));
                }
            }
            Node::Live(s) => {
                source.push_str(s);
                source.push('\n');
                twin.push_str(s);
                twin.push('\n');
            }
            Node::LiveClip(r) => {
                source.push_str(&format!("{r} re W f\n"));
                twin.push_str(&format!("{r} re W {}\n", if shown { "f" } else { "n" }));
            }
        }
    }
}

/// Source and twin text of a content tree, inside layers shown as `stack` says.
pub fn texts(nodes: &[Node], xobjects: &[XObject], stack: &[bool]) -> (String, String) {
    let (mut source, mut twin) = (String::new(), String::new());
    emit(nodes, &mut stack.to_vec(), xobjects, &mut source, &mut twin);
    (source, twin)
}

/// Replace `{name}` with `n 0 R`.
fn refs(body: &str, numbers: &HashMap<&'static str, u32>) -> String {
    let mut out = body.to_string();
    for (k, n) in numbers {
        out = out.replace(&format!("{{{k}}}"), &format!("{n} 0 R"));
    }
    out
}

/// Write the case: the source (with its layers) and the twin (only what shows, no layers).
pub fn build(case: &Case) -> Built {
    // Source numbering: 1 catalog, 2 pages, 3 page, 4 font, then content streams, named
    // objects, XObjects, patterns, graphics states.
    let mut numbers: HashMap<&'static str, u32> = HashMap::new();
    let mut next = 5 + case.contents.len() as u32;
    for (name, _) in &case.objects {
        numbers.insert(name, next);
        next += 1;
    }
    for x in &case.xobjects {
        numbers.insert(x.name, next);
        next += 1;
    }
    for (name, _) in &case.patterns {
        numbers.insert(name, next);
        next += 1;
    }
    let source = write(case, &numbers, true);
    let twin = write(case, &numbers, false);
    Built { source, twin, numbers }
}

fn write(case: &Case, numbers: &HashMap<&'static str, u32>, source: bool) -> Arc<Vec<u8>> {
    let pick = |(s, t): (String, String)| if source { s } else { t };
    let properties: String = if source { case.properties.iter().map(|(p, o)| format!("/{p} {{{o}}} ")).collect() } else { String::new() };
    let resources = |extra: &str| refs(&format!("<< /Properties << {properties}>> /Font << /FC 4 0 R >> {extra}>>"), numbers);
    let xobjects: String = case.xobjects.iter().map(|x| format!("/{} {{{}}} ", x.name, x.name)).collect();
    let patterns: String = case.patterns.iter().map(|(p, _)| format!("/{p} {{{p}}} ")).collect();
    let states: String =
        case.soft_masks.iter().map(|(g, form)| format!("/{g} << /Type /ExtGState /SMask << /S /Luminosity /G {{{form}}} >> >> ")).collect();
    let page_resources = resources(&format!("/XObject << {xobjects}>> /Pattern << {patterns}>> /ExtGState << {states}>> "));

    let catalog = match (&case.oc_properties, source) {
        (Some(p), true) => refs(&format!("<< /Type /Catalog /Pages 2 0 R /OCProperties {p} >>"), numbers),
        _ => "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
    };
    // The twin's page draws everything from one stream; the source keeps the case's streams.
    let mut streams: Vec<String> = Vec::new();
    let mut stack = Vec::new();
    let mut twin_all = String::new();
    for c in &case.contents {
        let (mut s, mut t) = (String::new(), String::new());
        emit(c, &mut stack, &case.xobjects, &mut s, &mut t);
        twin_all.push_str(&t);
        streams.push(s);
    }
    let content_refs: Vec<String> = (0..case.contents.len()).map(|i| format!("{} 0 R", 5 + i)).collect();
    let mut objects = vec![
        catalog,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources {page_resources} /Contents [{}] >>", content_refs.join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    for (i, s) in streams.iter().enumerate() {
        if source {
            objects.push(synth::raw_stream("", s));
        } else if i == 0 {
            objects.push(synth::raw_stream("", &twin_all));
        } else {
            objects.push(synth::raw_stream("", ""));
        }
    }
    for (_, body) in &case.objects {
        objects.push(refs(body, numbers));
    }
    for x in &case.xobjects {
        let oc = match (x.oc, source) {
            (Some((o, _)), true) => refs(&format!("/OC {{{o}}} "), numbers),
            _ => String::new(),
        };
        let body = match &x.kind {
            Kind::Form(content) => {
                let text = pick(texts(content, &case.xobjects, &[x.shown()]));
                let group = if x.group { "/Group << /S /Transparency /CS /DeviceRGB >> " } else { "" };
                synth::raw_stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {group}/Resources {page_resources} {oc}"), &text)
            }
            Kind::Image => {
                // The twin never draws a hidden image, so it holds no canary at all.
                let pixel = if x.shown() || !source { "808080" } else { "E82191" };
                synth::raw_stream(
                    &format!(
                        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode {oc}"
                    ),
                    &format!("{}>", pixel.repeat(4)),
                )
            }
        };
        objects.push(body);
    }
    for (_, content) in &case.patterns {
        let text = pick(texts(content, &case.xobjects, &[]));
        objects.push(synth::raw_stream(
            &format!("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 /Resources {page_resources}"),
            &text,
        ));
    }
    synth::raw_pdf(&objects)
}

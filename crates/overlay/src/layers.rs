//! The overlay's two layers (step 4, OV4.1): `Old` and `New`.
//!
//! Every output document gets exactly two optional content groups, `Old` (the previous revision)
//! and `New` (the updated revision), shared by all its pages and both on by default. On each page
//! each document's fill is wrapped in its layer's marker, so toggling a layer hides or shows that
//! whole revision. With both on, a page is OV3's page plus the two marker pairs, which don't
//! change the graphics state, so the approved compositing is unchanged.
//!
//! [`LayeredOutput`] builds such a document page by page and checks the finished document before
//! handing it over ([`check_output`]): any optional content other than the two layers, any
//! annotation, form, action or script is an error, so a leak can't be saved by mistake.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::parse;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::composite::{Marker, compose_page_marked};
use crate::oc::prepare::PreparedSource;
use crate::report::{DocumentReport, OverlayReport, PageEntry};
use crate::{CYAN, OverlayError, Placement, RED, Side, Source, Style};

/// Which revision a document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Revision {
    /// The previous revision.
    Old,
    /// The updated revision.
    New,
}

impl Revision {
    /// The layer's name, exactly as the output shows it (O2).
    pub fn name(self) -> &'static str {
        match self {
            Revision::Old => "Old",
            Revision::New => "New",
        }
    }

    /// The name the page's `/Resources /Properties` gives the layer's group.
    pub(crate) fn property(self) -> &'static str {
        match self {
            Revision::Old => "PCOld",
            Revision::New => "PCNew",
        }
    }
}

/// Which document is which revision. By default A is `Old` and B is `New`; the caller can swap
/// them (O1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    pub a: Revision,
    pub b: Revision,
}

impl Default for Roles {
    fn default() -> Self {
        Roles { a: Revision::Old, b: Revision::New }
    }
}

impl Roles {
    /// B is `Old` and A is `New`.
    pub const SWAPPED: Roles = Roles { a: Revision::New, b: Revision::Old };

    /// The revision of `side`.
    pub fn of(self, side: Side) -> Revision {
        match side {
            Side::A => self.a,
            Side::B => self.b,
        }
    }

    fn check(self) -> Result<(), OverlayError> {
        if self.a == self.b { Err(OverlayError::SameRole { role: self.a.name() }) } else { Ok(()) }
    }
}

/// How a layered overlay is drawn: each revision's colour and the light-tone threshold. The
/// colours follow the role, not the side (O1): `Old` red (204, 51, 51), `New` cyan (51, 204, 204).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerStyle {
    pub old: [u8; 3],
    pub new: [u8; 3],
    /// The paper threshold t in [0.5, 1], as in [`Style`].
    pub threshold: f64,
}

impl Default for LayerStyle {
    fn default() -> Self {
        LayerStyle { old: RED, new: CYAN, threshold: 1.0 }
    }
}

impl LayerStyle {
    /// The colour of revision `r`.
    pub fn colour(&self, r: Revision) -> [u8; 3] {
        match r {
            Revision::Old => self.old,
            Revision::New => self.new,
        }
    }

    /// OV3's per-side style for these roles.
    pub fn for_roles(&self, roles: Roles) -> Style {
        Style { colour_a: self.colour(roles.a), colour_b: self.colour(roles.b), threshold: self.threshold }
    }
}

/// How a page's two documents were registered, for the output's metadata and report (O2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationKind {
    /// No registration: B placed as the sheet match puts it.
    Placed,
    /// A similarity fit of picked points.
    Similarity,
    /// An affine fit of picked points.
    Affine,
    /// A matrix given by the caller, not fitted here.
    Matrix,
}

/// One page's registration, as the metadata states it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegistrationNote {
    pub kind: RegistrationKind,
    /// The fit's RMS residual and largest residual on A's sheet, in millimetres, for fitted pages.
    pub rms_mm: Option<f64>,
    pub max_mm: Option<f64>,
}

impl RegistrationNote {
    /// A page placed without registration.
    pub const PLACED: RegistrationNote = RegistrationNote { kind: RegistrationKind::Placed, rms_mm: None, max_mm: None };

    /// The registration as the metadata and the report state it.
    pub fn text(&self) -> String {
        self.describe().unwrap_or_else(|e| e.to_string())
    }

    fn describe(&self) -> Result<String, OverlayError> {
        let mut s = match self.kind {
            RegistrationKind::Placed => "no registration (placed by the sheet match)",
            RegistrationKind::Similarity => "similarity registration",
            RegistrationKind::Affine => "affine registration",
            RegistrationKind::Matrix => "registration matrix given",
        }
        .to_string();
        for (what, v) in [("RMS residual", self.rms_mm), ("largest residual", self.max_mm)] {
            if let Some(v) = v {
                if !v.is_finite() || v < 0.0 {
                    return Err(OverlayError::NotFinite { what: "a registration residual" });
                }
                s.push_str(&format!(", {what} {v:.3} mm"));
            }
        }
        Ok(s)
    }
}

/// The two layer groups of an output document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerGroups {
    pub old: ObjRef,
    pub new: ObjRef,
}

impl LayerGroups {
    pub fn of(&self, r: Revision) -> ObjRef {
        match r {
            Revision::Old => self.old,
            Revision::New => self.new,
        }
    }
}

/// A layered overlay document under construction. Each [`add_page`](Self::add_page) appends one
/// page or, on an error, leaves the document as it was. [`finish`](Self::finish) writes the
/// metadata and checks the whole document before returning it.
pub struct LayeredOutput {
    doc: Document,
    groups: LayerGroups,
    notes: Vec<RegistrationNote>,
    report: OverlayReport,
    /// Documents already in the report: (prepared document, side).
    listed: Vec<(u64, Side)>,
}

impl LayeredOutput {
    /// An empty document with the two layers.
    pub fn new() -> Result<Self, OverlayError> {
        let mut doc = Document::new_empty();
        let groups = add_layers(&mut doc)?;
        Ok(LayeredOutput { doc, groups, notes: Vec::new(), report: OverlayReport::default(), listed: Vec::new() })
    }

    pub fn groups(&self) -> LayerGroups {
        self.groups
    }

    pub fn page_count(&self) -> usize {
        self.notes.len()
    }

    /// Append one overlay page: A's and B's fills wrapped in their roles' layers, as OV3 draws
    /// them otherwise. Either side may be blank, not both. The sources are prepared pages
    /// ([`prepare`](crate::oc::prepare::prepare)); a page with anything unresolved is refused,
    /// with every item of both sides.
    ///
    pub fn add_page(
        &mut self,
        a: Option<PreparedSource<'_>>,
        b: Option<PreparedSource<'_>>,
        placement: &Placement,
        style: &LayerStyle,
        roles: Roles,
        registration: RegistrationNote,
    ) -> Result<ObjRef, OverlayError> {
        roles.check()?;
        registration.describe()?;
        let (prepared_a, prepared_b) = (a, b);
        let unresolved = |s: Option<PreparedSource<'_>>| s.map(|s| s.unresolved().to_vec()).unwrap_or_default();
        let (ua, ub) = (unresolved(a), unresolved(b));
        if !ua.is_empty() || !ub.is_empty() {
            return Err(OverlayError::Unresolved { a: ua, b: ub, roles });
        }
        let (a, b) = (a.map(|s| s.source()), b.map(|s| s.source()));
        for (side, source) in [(Side::A, a), (Side::B, b)] {
            if let Some(s) = source {
                annotations_drawn(s, side)?;
            }
        }
        let markers = [Side::A, Side::B].map(|side| {
            let r = roles.of(side);
            Marker { name: r.property(), group: self.groups.of(r) }
        });
        // All or nothing: a failed page leaves the document as it was.
        let before = self.doc.clone();
        match compose_page_marked(&mut self.doc, a, b, placement, &style.for_roles(roles), Some(markers)) {
            Ok(page) => {
                self.notes.push(registration);
                for (side, source) in [(Side::A, prepared_a), (Side::B, prepared_b)] {
                    let Some(p) = source.map(|s| s.prepared()) else { continue };
                    if !self.listed.contains(&(p.id(), side)) {
                        self.listed.push((p.id(), side));
                        self.report.documents.push(DocumentReport {
                            side,
                            role: roles.of(side),
                            warnings: p.warnings().to_vec(),
                            notes: p.note_texts(),
                        });
                    }
                }
                self.report.pages.push(PageEntry {
                    registration,
                    a: prepared_a.map(|s| s.report().clone()),
                    b: prepared_b.map(|s| s.report().clone()),
                });
                Ok(page)
            }
            Err(e) => {
                self.doc = before;
                Err(e)
            }
        }
    }

    /// The report so far.
    pub fn report(&self) -> &OverlayReport {
        &self.report
    }

    /// Write the metadata (O2) and check the document ([`check_output`]). The document is
    /// returned only if the check passes.
    pub fn finish(self) -> Result<Document, OverlayError> {
        self.finish_with_report().map(|(doc, _)| doc)
    }

    /// [`finish`](Self::finish), with the generation report.
    pub fn finish_with_report(mut self) -> Result<(Document, OverlayReport), OverlayError> {
        if self.notes.is_empty() {
            return Err(OverlayError::NoSources);
        }
        write_metadata(&mut self.doc, &self.notes)?;
        check_output(&self.doc)?;
        Ok((self.doc, self.report))
    }
}

/// A new layered document with one overlay page. See [`LayeredOutput::add_page`].
pub fn compose_layered(
    a: Option<PreparedSource<'_>>,
    b: Option<PreparedSource<'_>>,
    placement: &Placement,
    style: &LayerStyle,
    roles: Roles,
    registration: RegistrationNote,
) -> Result<Document, OverlayError> {
    let mut out = LayeredOutput::new()?;
    out.add_page(a, b, placement, style, roles, registration)?;
    out.finish()
}

/// Preparation draws every annotation into its page's content and removes `/Annots`; a page that
/// still has them didn't come from it, and is refused rather than losing them.
fn annotations_drawn(source: Source<'_>, side: Side) -> Result<(), OverlayError> {
    let doc = source.doc;
    let pages = pdfcraft_model::pages(doc);
    let Some(page) = pages.get(source.page) else {
        return Err(OverlayError::NoSuchPage { page: source.page, count: pages.len() });
    };
    let annots = page.dict.get(b"Annots").map(|a| doc.resolve(a));
    if annots.as_deref().and_then(Object::as_array).is_some_and(|a| !a.is_empty()) {
        return Err(OverlayError::Source {
            side: side.letter(),
            page: source.page,
            reason: "the page still has annotations, so it wasn't prepared".into(),
        });
    }
    Ok(())
}

/// Add `Old` and `New` and the catalog's `/OCProperties` to a document that has none.
fn add_layers(doc: &mut Document) -> Result<LayerGroups, OverlayError> {
    let root = doc.root().ok_or_else(|| OverlayError::Output { reason: "the output document has no catalog".into() })?;
    if doc.get(root).as_dict().is_some_and(|c| c.contains(b"OCProperties")) {
        return Err(OverlayError::Output { reason: "the output document already has optional content".into() });
    }
    let group = |name: &str| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("OCG"));
        d.set(b"Name".to_vec(), Object::String(PdfString::text(name)));
        Object::Dict(d)
    };
    let old = doc.add(group(Revision::Old.name()));
    let new = doc.add(group(Revision::New.name()));
    let both = || Object::Array(vec![Object::Ref(old), Object::Ref(new)]);
    let mut config = Dict::new();
    config.set(b"Name".to_vec(), Object::String(PdfString::text("Overlay")));
    config.set(b"BaseState".to_vec(), Object::name("ON"));
    config.set(b"ON".to_vec(), both());
    config.set(b"OFF".to_vec(), Object::Array(Vec::new()));
    config.set(b"Order".to_vec(), both());
    let mut props = Dict::new();
    props.set(b"OCGs".to_vec(), both());
    props.set(b"D".to_vec(), Object::Dict(config));
    doc.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props))).map_err(|e| OverlayError::Output { reason: e.to_string() })?;
    Ok(LayerGroups { old, new })
}

/// The document information `/Subject` and the XMP `dc:description` (O2): the two layers and each
/// page's registration. No file name goes into the metadata.
fn write_metadata(doc: &mut Document, notes: &[RegistrationNote]) -> Result<(), OverlayError> {
    let mut text = String::from("Overlay comparison with two layers, Old (previous revision) and New (updated revision).");
    for (i, n) in notes.iter().enumerate() {
        text.push_str(&format!(" Page {}: {}.", i.saturating_add(1), n.describe()?));
    }
    let mut info = Dict::new();
    info.set(b"Subject".to_vec(), Object::String(PdfString::text(&text)));
    let info = doc.add(Object::Dict(info));
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));

    let xmp = format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
   <dc:format>application/pdf</dc:format>\n\
   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>",
        xml_escape(&text)
    );
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Metadata"));
    d.set(b"Subtype".to_vec(), Object::name("XML"));
    let meta = doc.add(Object::Stream(Stream::from_raw(d, xmp.into_bytes())));
    let root = doc.root().ok_or_else(|| OverlayError::Output { reason: "the output document has no catalog".into() })?;
    doc.update_dict(root, |c| c.set(b"Metadata".to_vec(), Object::Ref(meta))).map_err(|e| OverlayError::Output { reason: e.to_string() })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Nesting limit for direct objects inside one object.
const MAX_DEPTH: usize = 64;

/// The defensive check of a finished overlay (step 4 PRD, Defensive check; L1). It passes only
/// when:
///
/// - the catalog's `/OCProperties` holds exactly `Old` and `New`, both on in the default
///   configuration, in that order, with no locking, radio-button groups, auto-states or extra
///   configurations;
/// - no other optional content group and no membership dictionary exists anywhere, and no
///   object has an `/OC` entry;
/// - the only optional-content markers are the page markers, each naming its page's `Old` or
///   `New` property, balanced, and no content stream holds an inline optional-content dictionary;
/// - there are no annotations, no interactive form, no signatures, no actions and no JavaScript;
/// - every content stream can be read completely, so none of this can hide in unread bytes.
///
/// Returns the two groups.
pub fn check_output(doc: &Document) -> Result<LayerGroups, OverlayError> {
    let fail = |found: String| OverlayError::Check { found };
    let root = doc.root().ok_or_else(|| fail("the document has no catalog".into()))?;
    let catalog = doc.dict(&Object::Ref(root)).ok_or_else(|| fail("the catalog isn't a dictionary".into()))?;
    for key in [&b"AcroForm"[..], b"OpenAction", b"AA"] {
        if catalog.contains(key) {
            return Err(fail(format!("the catalog has /{}", String::from_utf8_lossy(key))));
        }
    }
    if let Some(names) = catalog.get(b"Names").and_then(|n| doc.dict(n))
        && names.contains(b"JavaScript")
    {
        return Err(fail("the document has JavaScript".into()));
    }
    let groups = check_layers(doc, &catalog)?;

    // Content streams: page contents (with the page's properties), forms, tiling patterns and
    // Type 3 glyphs.
    let mut page_streams: HashMap<ObjRef, Dict> = HashMap::new();
    for (i, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        let n = i.saturating_add(1);
        if page.dict.get(b"Annots").map(|a| doc.resolve(a)).as_deref().and_then(Object::as_array).is_some_and(|a| !a.is_empty()) {
            return Err(fail(format!("page {n} has annotations")));
        }
        let properties =
            page.dict.get(b"Resources").and_then(|r| doc.dict(r)).and_then(|r| r.get(b"Properties").and_then(|p| doc.dict(p))).unwrap_or_default();
        let contents = page.dict.get(b"Contents").map(|c| doc.resolve(c));
        let list: Vec<Object> = match contents.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => page.dict.get(b"Contents").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        for c in list {
            let r = c.as_ref().ok_or_else(|| fail(format!("page {n} has a direct content stream")))?;
            page_streams.insert(r, properties.clone());
        }
    }
    let mut other_streams: HashSet<ObjRef> = HashSet::new();
    for (r, obj) in doc.scan_objects() {
        check_object(r, &obj, &groups, 0)?;
        if let Object::Stream(s) = &*obj
            && (s.dict.name(b"Subtype") == Some(&b"Form"[..]) || s.dict.int(b"PatternType") == Some(1))
        {
            other_streams.insert(r);
        }
        if let Some(procs) = obj.as_dict().and_then(|d| d.get(b"CharProcs")).and_then(|p| doc.dict(p)) {
            for (_, v) in procs.iter() {
                if let Some(pr) = v.as_ref() {
                    other_streams.insert(pr);
                }
            }
        }
    }
    let mut sorted: Vec<(ObjRef, Option<Dict>)> =
        page_streams.into_iter().map(|(r, p)| (r, Some(p))).chain(other_streams.into_iter().map(|r| (r, None))).collect();
    sorted.sort_by_key(|(r, p)| (p.is_none(), r.num, r.generation));
    for (r, properties) in sorted {
        check_content(doc, r, properties.as_ref(), &groups)?;
    }
    Ok(groups)
}

/// The catalog's optional content: exactly the two layers, as [`add_layers`] writes them.
fn check_layers(doc: &Document, catalog: &Dict) -> Result<LayerGroups, OverlayError> {
    let fail = |found: &str| OverlayError::Check { found: found.into() };
    let props = catalog.get(b"OCProperties").and_then(|p| doc.dict(p)).ok_or_else(|| fail("the catalog has no optional content properties"))?;
    let refs = |key: &[u8], d: &Dict| -> Option<Vec<ObjRef>> {
        d.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_array().map(|a| a.iter().map(Object::as_ref).collect::<Option<Vec<_>>>()))?
    };
    let ocgs = refs(b"OCGs", &props).ok_or_else(|| fail("/OCGs isn't an array of groups"))?;
    let [old, new] = ocgs.as_slice() else {
        return Err(fail("the document doesn't have exactly two layers"));
    };
    let (old, new) = (*old, *new);
    for (r, name) in [(old, Revision::Old.name()), (new, Revision::New.name())] {
        let d = doc.dict(&Object::Ref(r)).ok_or_else(|| fail("a layer isn't a dictionary"))?;
        let ok = d.name(b"Type") == Some(&b"OCG"[..])
            && d.get(b"Name").and_then(|n| doc.resolve(n).as_string().map(PdfString::to_text)).as_deref() == Some(name)
            && !d.contains(b"Intent")
            && !d.contains(b"Usage");
        if !ok {
            return Err(OverlayError::Check { found: format!("the layer {name} isn't the plain group the overlay writes") });
        }
    }
    if props.contains(b"Configs") {
        return Err(fail("the document has alternative layer configurations"));
    }
    let config = props.get(b"D").and_then(|d| doc.dict(d)).ok_or_else(|| fail("the layers have no default configuration"))?;
    for key in [&b"Locked"[..], b"RBGroups", b"AS", b"Intent"] {
        if config.contains(key) {
            return Err(OverlayError::Check { found: format!("the default configuration has /{}", String::from_utf8_lossy(key)) });
        }
    }
    if config.name(b"BaseState").is_some_and(|b| b != b"ON") {
        return Err(fail("the default configuration doesn't start with every layer on"));
    }
    if config.get(b"OFF").is_some_and(|o| doc.resolve(o).as_array().is_none_or(|a| !a.is_empty())) {
        return Err(fail("a layer is off by default"));
    }
    if refs(b"ON", &config).is_none_or(|on| on.iter().any(|r| *r != old && *r != new)) {
        return Err(fail("the default configuration's /ON isn't the two layers"));
    }
    if refs(b"Order", &config) != Some(vec![old, new]) {
        return Err(fail("the layers aren't listed as Old, New"));
    }
    Ok(LayerGroups { old, new })
}

/// One object and the direct objects inside it: no other group, no membership dictionary, no
/// `/OC` entry, no annotation, action or script.
fn check_object(r: ObjRef, obj: &Object, groups: &LayerGroups, depth: usize) -> Result<(), OverlayError> {
    let fail = |found: String| Err(OverlayError::Check { found: format!("{found} (object {} {})", r.num, r.generation) });
    if depth > MAX_DEPTH {
        return fail("objects nested too deeply to check".into());
    }
    match obj {
        Object::Dict(_) | Object::Stream(_) => {
            let Some(d) = obj.as_dict() else { return Ok(()) };
            match d.name(b"Type") {
                Some(b"OCG") if depth > 0 || (r != groups.old && r != groups.new) => {
                    return fail("an optional content group other than Old and New".into());
                }
                Some(b"OCMD") => return fail("an optional content membership dictionary".into()),
                Some(b"Annot") => return fail("an annotation".into()),
                Some(b"Action") => return fail("an action".into()),
                Some(b"Sig" | b"DocTimeStamp") => return fail("a signature".into()),
                _ => {}
            }
            for key in [&b"OC"[..], b"AA", b"JS", b"OpenAction", b"ByteRange"] {
                if d.contains(key) {
                    return fail(format!("an /{} entry", String::from_utf8_lossy(key)));
                }
            }
            if d.name(b"S") == Some(&b"JavaScript"[..]) {
                return fail("JavaScript".into());
            }
            for (_, v) in d.iter() {
                check_object(r, v, groups, depth.saturating_add(1))?;
            }
        }
        Object::Array(a) => {
            for v in a {
                check_object(r, v, groups, depth.saturating_add(1))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// One content stream: completely readable, and its only optional-content markers are a page's
/// `Old` and `New` markers (`properties` is the page's `/Properties`; `None` for forms, patterns
/// and glyphs, which may have none).
fn check_content(doc: &Document, r: ObjRef, properties: Option<&Dict>, groups: &LayerGroups) -> Result<(), OverlayError> {
    let fail = |found: String| Err(OverlayError::Check { found: format!("{found} (content stream {} {})", r.num, r.generation) });
    let data = match &*doc.get(r) {
        Object::Stream(s) => match s.decoded() {
            Ok(d) => d,
            Err(e) => return fail(format!("a content stream can't be decoded ({e}), so it can't be checked")),
        },
        _ => return fail("a content stream isn't a stream".into()),
    };
    let parsed = parse(&data);
    if parsed.skipped > 0 {
        return fail(format!("{} bytes of a content stream can't be read, so it can't be checked", parsed.skipped));
    }
    let mut open: Vec<bool> = Vec::new();
    for op in &parsed.ops {
        match op.op.as_slice() {
            b"BMC" => open.push(false),
            b"BDC" => {
                let optional = op.name(0) == Some(&b"OC"[..]);
                if optional {
                    let Some(properties) = properties else {
                        return fail("an optional-content marker outside the page markers".into());
                    };
                    let Some(name) = op.name(1) else {
                        return fail("an optional-content marker with an inline dictionary".into());
                    };
                    let expected = match name {
                        b"PCOld" => groups.old,
                        b"PCNew" => groups.new,
                        _ => return fail("an optional-content marker that isn't Old or New".into()),
                    };
                    if properties.get(name).and_then(Object::as_ref) != Some(expected) || !open.is_empty() {
                        return fail("a layer marker that doesn't name its layer, or is nested".into());
                    }
                }
                open.push(optional);
            }
            b"EMC" => {
                let closed = open.pop();
                if closed.is_none() {
                    return fail("an unbalanced marked-content end".into());
                }
            }
            _ => {}
        }
    }
    if open.iter().any(|o| *o) {
        return fail("a layer marker that is never closed".into());
    }
    Ok(())
}

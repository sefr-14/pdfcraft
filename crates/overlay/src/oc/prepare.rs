//! Source preparation (OV4.3): an in-memory copy of a source document whose pages show exactly
//! what the source shows under its default layer configuration and the caller's decisions, with
//! no optional content left in anything the overlay copies.
//!
//! For each page, its content and everything its kept content draws (forms, Type 3 glyphs, the
//! groups of soft masks set by `gs`, tiling patterns) is resolved stream by stream
//! ([`resolve`](super::resolve)): visible markers are removed, hidden blocks and hidden XObjects
//! are removed, and a stream that can't be resolved leaves the page unresolved, with each item
//! reported. Then, in a document that uses optional content (it has `/OCProperties`, or the
//! caller made decisions), each resource dictionary the page draws through keeps only what kept
//! content uses: XObjects, fonts, graphics states and patterns that only hidden content used are
//! left out, and the optional-content entries of `/Properties` go, so hidden content isn't carried
//! into the overlay at all. Finally the copied resources are searched for any optional content
//! left (private application data, say), which makes the page unresolved too.
//!
//! Annotations (OV4.4, [`annotations`](crate::annotations)) are drawn into the copy's page as the
//! source shows them, after its content, and the page keeps none: each is drawn, excluded by a
//! named rule, omitted by the caller's decision, or unresolved.
//!
//! The source document is only read. A document without optional content, annotations and
//! decisions is copied unchanged, so its overlay is the overlay OV3 makes.

use std::collections::{HashMap, HashSet};
use std::fmt;

use pdfcraft_content::{Pieces, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::eval::{Evaluator, Note, Problem};
use super::resolve::{self, Names, Plan, Uses};
use super::{Rule, Unresolved, Visibility};
use crate::annotations::{self as an, AnnotationEntry, AppearanceSource, Exclusion, Outcome, SourceWarning, Stored};
use crate::{OverlayError, Source};

/// Deepest chain of streams drawn inside streams that is followed (forms in forms, say).
const MAX_DEPTH: usize = 32;
/// Most streams visited for one document.
const MAX_VISITS: usize = 100_000;
/// Most objects searched for leftover optional content on one page.
const MAX_SCAN: usize = 1 << 20;
/// Nesting limit for direct objects inside one object.
const MAX_NESTING: usize = 64;

/// What preparation found and did on one page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageReport {
    /// 0-based.
    pub page: usize,
    /// Whether the page's content reaches optional content at all (a marker, or an XObject with
    /// `/OC`).
    pub optional: bool,
    /// The optional-content objects the page's content refers to: reference, the group's name,
    /// and what it shows (`None`: unresolved).
    pub layers: Vec<(ObjRef, Option<String>, Option<bool>)>,
    pub hidden_blocks: usize,
    pub hidden_xobjects: usize,
    pub markers_removed: usize,
    /// Graphics-state operators kept from hidden blocks because later content uses them.
    pub kept_state: usize,
    /// Clipping paths kept from hidden blocks.
    pub kept_clips: usize,
    pub streams_rewritten: usize,
    /// Resource entries left out because only hidden content used them, and optional-content
    /// properties removed.
    pub resources_removed: usize,
    /// Every annotation of the page, in `/Annots` order, with what happened to it.
    pub annotations: Vec<AnnotationEntry>,
    /// Why the page can't be composed into layers yet. Empty: it can.
    pub unresolved: Vec<Unresolved>,
}

/// Identifies each prepared document, so a report lists a document once however many pages it
/// gives.
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A prepared copy of one source document.
pub struct Prepared {
    id: u64,
    doc: Document,
    reports: Vec<PageReport>,
    notes: Vec<Note>,
    warnings: Vec<SourceWarning>,
    names: HashMap<ObjRef, String>,
    touched: bool,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared").field("reports", &self.reports).field("notes", &self.notes).field("touched", &self.touched).finish_non_exhaustive()
    }
}

impl Prepared {
    /// The prepared copy (for tests and the proof; the overlay reads it through
    /// [`PreparedSource`]).
    pub fn doc(&self) -> &Document {
        &self.doc
    }

    /// One report per prepared page, in page order.
    pub fn reports(&self) -> &[PageReport] {
        &self.reports
    }

    /// What the evaluator noted about the document's layers (auto-states not applied, and the
    /// like).
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// Warnings the output is correct with: signed signature fields, redaction marks not applied.
    pub fn warnings(&self) -> &[SourceWarning] {
        &self.warnings
    }

    /// A layer's name, for messages about [`notes`](Self::notes).
    pub fn name(&self, r: ObjRef) -> Option<&str> {
        self.names.get(&r).map(String::as_str)
    }

    /// Whether the document uses optional content, so its copy was prepared (resources pruned)
    /// rather than copied unchanged.
    pub fn touched(&self) -> bool {
        self.touched
    }

    /// Prepared page `page` (0-based), for [`LayeredOutput::add_page`](crate::LayeredOutput::add_page).
    pub fn source(&self, page: usize) -> Result<PreparedSource<'_>, OverlayError> {
        let count = self.reports.len();
        let report = self.reports.iter().find(|r| r.page == page).ok_or(OverlayError::NoSuchPage { page, count })?;
        Ok(PreparedSource { prepared: self, report })
    }
}

/// One prepared page.
#[derive(Clone, Copy, Debug)]
pub struct PreparedSource<'a> {
    prepared: &'a Prepared,
    report: &'a PageReport,
}

impl<'a> PreparedSource<'a> {
    /// 0-based.
    pub fn page(&self) -> usize {
        self.report.page
    }

    pub fn report(&self) -> &'a PageReport {
        self.report
    }

    /// Why the page can't be composed into layers yet. Empty: it can.
    pub fn unresolved(&self) -> &'a [Unresolved] {
        &self.report.unresolved
    }

    /// The prepared page as an OV3 source.
    pub fn source(&self) -> Source<'a> {
        Source { doc: &self.prepared.doc, page: self.report.page }
    }

    pub(crate) fn prepared(&self) -> &'a Prepared {
        self.prepared
    }
}

impl Prepared {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The layer notes as the report states them.
    pub(crate) fn note_texts(&self) -> Vec<String> {
        let name = |g: ObjRef| self.name(g).map_or_else(|| format!("object {} {}", g.num, g.generation), |n| format!("\"{n}\""));
        self.notes.iter().map(|n| crate::report::note_text(n, &name)).collect()
    }
}

/// Prepare pages `pages` (0-based) of `doc` under its default layer configuration and
/// `visibility`'s decisions. Errors only for a page that doesn't exist or a decision that can't
/// apply; anything that can't be resolved is reported per page.
pub fn prepare(doc: &Document, pages: &[usize], visibility: &Visibility) -> Result<Prepared, OverlayError> {
    let all = pdfcraft_model::pages(doc);
    let mut wanted = pages.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    if let Some(&page) = wanted.iter().find(|p| **p >= all.len()) {
        return Err(OverlayError::NoSuchPage { page, count: all.len() });
    }
    check_decisions(doc, visibility)?;
    let has_oc = doc.root().and_then(|r| doc.dict(&Object::Ref(r))).is_some_and(|c| c.contains(b"OCProperties"));
    let touched = has_oc || !visibility.decisions.is_empty();
    let walked = walk(doc, &wanted, visibility, has_oc, touched);
    // OV4.10: a document without `/OCProperties` whose pages use optional content anyway. Its
    // groups are shown, and the copy has to drop them as a layered document's does, so it is
    // walked again with its resources pruned and checked.
    let (w, reports, names, mut warnings, touched) =
        if !touched && walked.0.ev.consulted() { walk(doc, &wanted, visibility, has_oc, true) } else { walked };
    // An untyped dictionary can be decided only where these pages use it as optional content.
    for &(r, _) in &visibility.decisions {
        let untyped = doc.dict(&Object::Ref(r)).is_some_and(|d| d.name(b"Type").is_none());
        if untyped && !reports.iter().any(|p| p.layers.iter().any(|(l, _, _)| *l == r)) {
            return Err(OverlayError::Decision {
                reason: format!("object {} {} isn't a layer, and these pages don't use it as one", r.num, r.generation),
            });
        }
    }
    let signed = an::signed_fields(doc);
    if signed > 0 {
        warnings.insert(0, SourceWarning::Signed { fields: signed });
    }
    let notes = w.ev.notes().to_vec();
    Ok(Prepared { id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed), doc: w.copy, reports, notes, warnings, names, touched })
}

/// What one walk of `wanted` gives: the walker (with the copy and the evaluator), the page
/// reports, the layer names, the warnings, and whether the copy was pruned (`touched`).
type Walked<'a> = (Walker<'a>, Vec<PageReport>, HashMap<ObjRef, String>, Vec<SourceWarning>, bool);

/// Walk `wanted` (sorted, existing) once, pruning resources when `touched`.
fn walk<'a>(doc: &'a Document, wanted: &[usize], visibility: &Visibility, has_oc: bool, touched: bool) -> Walked<'a> {
    let all = pdfcraft_model::pages(doc);
    let mut w = Walker {
        src: doc,
        ev: Evaluator::new(doc, visibility),
        copy: doc.clone(),
        prune: touched,
        layered: has_oc,
        surrounding: Vec::new(),
        strip_oc: HashSet::new(),
        done: HashMap::new(),
        active: Vec::new(),
        visits: 0,
        omit: visibility.omit.iter().copied().collect(),
        fields: None,
        need_appearances: pdfcraft_forms::need_appearances(doc),
        pending_detail: None,
    };
    let mut warnings = Vec::new();
    let mut reports = Vec::with_capacity(wanted.len());
    let mut names = HashMap::new();
    for &p in wanted {
        let Some(page) = all.get(p) else { continue };
        let mut t = Tally::default();
        w.page(page.obj, &page.dict, &mut t);
        w.strip();
        if touched
            && t.problems.is_empty()
            && t.issues.is_empty()
            && let Err(rule) = scan(&w.copy, page.obj)
        {
            t.issues.push(rule);
        }
        let marks = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        let count = marks.iter().filter(|a| doc.dict(a).is_some_and(|d| d.name(b"Subtype") == Some(&b"Redact"[..]))).count();
        if count > 0 {
            warnings.push(SourceWarning::RedactionMarks { page: p, count });
        }
        let report = t.report(p, &w.ev);
        for (r, name, _) in &report.layers {
            if let Some(n) = name {
                names.insert(*r, n.clone());
            }
        }
        reports.push(report);
    }
    for g in w.ev.listed_groups().collect::<Vec<_>>() {
        if let Some(n) = w.ev.name(g) {
            names.entry(g).or_insert(n);
        }
    }
    (w, reports, names, warnings, touched)
}

/// Decisions must name dictionaries that can be optional content (a group, a membership
/// dictionary, or an untyped dictionary a file uses as one), each set one way.
fn check_decisions(doc: &Document, visibility: &Visibility) -> Result<(), OverlayError> {
    let mut seen: HashMap<ObjRef, bool> = HashMap::new();
    for &(r, on) in &visibility.decisions {
        let what = format!("object {} {}", r.num, r.generation);
        let ok = doc.dict(&Object::Ref(r)).is_some_and(|d| matches!(d.name(b"Type"), None | Some(b"OCG" | b"OCMD")));
        if !ok {
            return Err(OverlayError::Decision { reason: format!("{what} isn't a layer or a layer membership of this document") });
        }
        if seen.insert(r, on).is_some_and(|before| before != on) {
            return Err(OverlayError::Decision { reason: format!("{what} is set both on and off") });
        }
    }
    for &r in &visibility.omit {
        let annotation =
            doc.dict(&Object::Ref(r)).is_some_and(|d| d.contains(b"Subtype") && (d.name(b"Type") == Some(&b"Annot"[..]) || d.contains(b"Rect")));
        if !annotation {
            return Err(OverlayError::Decision { reason: format!("object {} {} isn't an annotation of this document", r.num, r.generation) });
        }
    }
    Ok(())
}

/// One page's findings, added up over its streams.
#[derive(Clone, Default)]
struct Tally {
    problems: Vec<Problem>,
    issues: Vec<Rule>,
    optional: bool,
    seen: Vec<(ObjRef, Option<bool>)>,
    hidden_blocks: usize,
    hidden_xobjects: usize,
    markers_removed: usize,
    kept_state: usize,
    kept_clips: usize,
    streams_rewritten: usize,
    resources_removed: usize,
    annotations: Vec<AnnotationEntry>,
    /// Annotations that can't be drawn faithfully: (rule, the annotation, its label, detail).
    annotation_items: Vec<(Rule, Option<ObjRef>, String, Option<String>)>,
    /// Marker names the streams' own resources don't define (OV4.8, OV4.9), from the stream and
    /// everything it draws.
    undefined: Vec<Vec<u8>>,
}

impl Tally {
    fn absorb(&mut self, plan: &Plan) {
        self.problems.extend(plan.problems.iter().cloned());
        self.issues.extend(plan.issues.iter().copied());
        self.optional |= plan.optional;
        self.seen.extend(plan.seen.iter().copied());
        self.hidden_blocks = self.hidden_blocks.saturating_add(plan.hidden_blocks);
        self.hidden_xobjects = self.hidden_xobjects.saturating_add(plan.hidden_xobjects);
        self.markers_removed = self.markers_removed.saturating_add(plan.markers_removed);
        self.kept_state = self.kept_state.saturating_add(plan.kept_state);
        self.kept_clips = self.kept_clips.saturating_add(plan.kept_clips);
        self.add_undefined(&plan.undefined);
    }

    fn add_undefined(&mut self, names: &[Vec<u8>]) {
        for n in names {
            if !self.undefined.contains(n) {
                self.undefined.push(n.clone());
            }
        }
    }

    /// Add the findings of a stream drawn on this page. `again`: the stream was resolved and
    /// written on an earlier page, so its rewrites aren't counted twice.
    fn merge(&mut self, other: &Tally, again: bool) {
        self.problems.extend(other.problems.iter().cloned());
        self.issues.extend(other.issues.iter().copied());
        self.optional |= other.optional;
        self.seen.extend(other.seen.iter().copied());
        self.hidden_blocks = self.hidden_blocks.saturating_add(other.hidden_blocks);
        self.hidden_xobjects = self.hidden_xobjects.saturating_add(other.hidden_xobjects);
        self.markers_removed = self.markers_removed.saturating_add(other.markers_removed);
        self.kept_state = self.kept_state.saturating_add(other.kept_state);
        self.kept_clips = self.kept_clips.saturating_add(other.kept_clips);
        self.add_undefined(&other.undefined);
        if !again {
            self.streams_rewritten = self.streams_rewritten.saturating_add(other.streams_rewritten);
            self.resources_removed = self.resources_removed.saturating_add(other.resources_removed);
        }
    }

    fn report(mut self, page: usize, ev: &Evaluator<'_>) -> PageReport {
        self.seen.sort_by_key(|(r, _)| (r.num, r.generation));
        self.seen.dedup();
        let named = |r: ObjRef| (r, ev.name(r));
        let mut unresolved: Vec<Unresolved> = self
            .problems
            .iter()
            .map(|p| Unresolved {
                page,
                objects: p.objects.iter().copied().map(named).collect(),
                rule: p.rule,
                settleable: p.settleable,
                detail: None,
            })
            .chain(self.issues.iter().map(|rule| Unresolved { page, objects: Vec::new(), rule: *rule, settleable: false, detail: None }))
            .chain(self.annotation_items.iter().map(|(rule, r, label, detail)| Unresolved {
                page,
                objects: r.iter().map(|r| (*r, Some(label.clone()))).collect(),
                rule: *rule,
                settleable: r.is_some(),
                detail: match (r, detail) {
                    (Some(_), d) => d.clone(),
                    (None, Some(d)) => Some(format!("{label}: {d}")),
                    (None, None) => Some(label.clone()),
                },
            }))
            .collect();
        let mut kept: Vec<Unresolved> = Vec::with_capacity(unresolved.len());
        for u in unresolved.drain(..) {
            if !kept.contains(&u) {
                kept.push(u);
            }
        }
        PageReport {
            page,
            optional: self.optional,
            layers: self.seen.into_iter().map(|(r, v)| (r, ev.name(r), v)).collect(),
            hidden_blocks: self.hidden_blocks,
            hidden_xobjects: self.hidden_xobjects,
            markers_removed: self.markers_removed,
            kept_state: self.kept_state,
            kept_clips: self.kept_clips,
            streams_rewritten: self.streams_rewritten,
            resources_removed: self.resources_removed,
            annotations: self.annotations,
            unresolved: kept,
        }
    }
}

/// A stream drawn inside another, already resolved.
struct Done {
    /// The resources it was resolved with, when it has none of its own.
    inherited: Option<Dict>,
    /// The surrounding resource dictionaries it was resolved under (`Names::surrounding`), when it
    /// or a stream it draws has a marker name its own resources don't define, in a document with
    /// `/OCProperties` (OV4.9); otherwise empty.
    context: Vec<Dict>,
    /// Its rewritten data (`None`: unchanged).
    data: Option<Vec<u8>>,
    /// The names its kept content uses from those resources.
    uses: Uses,
    /// What resolving it (and everything it draws) found.
    findings: Tally,
}

/// What kind of stream is drawn, which decides where its resources come from.
enum Kind {
    /// A form, a tiling pattern or a soft mask's group: its own `/Resources`, else the drawing
    /// stream's.
    Content,
    /// A Type 3 glyph: its font's `/Resources` (given here), else the drawing stream's.
    Glyph(Option<Dict>),
}

struct Walker<'a> {
    src: &'a Document,
    ev: Evaluator<'a>,
    copy: Document,
    /// Whether resources are pruned (the document uses optional content).
    prune: bool,
    /// Whether the document has `/OCProperties`: only then are surrounding resource dictionaries
    /// read for undefined marker names (OV4.9).
    layered: bool,
    /// The resource dictionaries around the stream being resolved that the standard doesn't apply
    /// to it, innermost last (`Names::surrounding`): the page's ancestors' in the page tree, then
    /// the page's and those of the streams drawing it. Empty unless `layered`.
    surrounding: Vec<Dict>,
    /// XObjects drawn visibly whose `/OC` is removed from the copy.
    strip_oc: HashSet<ObjRef>,
    done: HashMap<ObjRef, Done>,
    /// Streams being resolved, to stop at a stream that draws itself.
    active: Vec<ObjRef>,
    visits: usize,
    /// Annotations the caller decided to omit.
    omit: HashSet<ObjRef>,
    /// Form fields by widget, read when a page has a widget to redraw.
    fields: Option<HashMap<ObjRef, (pdfcraft_forms::Field, pdfcraft_forms::Widget)>>,
    /// The form's `/NeedAppearances` (O8).
    need_appearances: bool,
    /// The detail of the last annotation found unresolved (the character that can't be drawn).
    pending_detail: Option<String>,
}

/// One annotation to draw: its appearance in the copy, the matrix that places it, and its opacity.
struct Draw {
    appearance: ObjRef,
    matrix: [f64; 6],
    /// The page-space box the placed appearance covers.
    covers: [f64; 4],
    opacity: Option<(f64, f64, Option<Object>)>,
    source: AppearanceSource,
}

impl Walker<'_> {
    fn page(&mut self, obj: ObjRef, dict: &Dict, t: &mut Tally) {
        let src = self.src;
        let resources = dict.get(b"Resources").and_then(|r| src.dict(r)).unwrap_or_default();
        self.surrounding = if self.layered { ancestor_resources(src, dict) } else { Vec::new() };
        let list: Vec<Object> = match dict.get(b"Contents").map(|c| src.resolve(c)).as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => dict.get(b"Contents").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        let mut pieces = Vec::with_capacity(list.len());
        for c in &list {
            if let Object::Stream(s) = &*src.resolve(c) {
                match s.decoded() {
                    Ok(d) => pieces.push(d),
                    Err(_) => {
                        t.issues.push(Rule::Unreadable);
                        return;
                    }
                }
            }
        }
        let joined = Pieces::join(&pieces);
        let (new, mut uses) = self.content(joined.data(), &resources, false, 0, t);

        // Annotations, drawn after the content.
        let annots: Vec<Object> = dict.get(b"Annots").map(|a| src.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        let rotate = dict.get(b"Rotate").and_then(|r| src.resolve(r).as_int()).unwrap_or(0);
        let mut draws: Vec<(usize, Draw)> = Vec::new();
        for (k, entry) in annots.iter().enumerate() {
            let r = entry.as_ref();
            let Some(d) = src.dict(entry) else {
                t.annotations.push(AnnotationEntry {
                    index: k,
                    object: r,
                    subtype: String::new(),
                    outcome: Outcome::Excluded(Exclusion::UnusableAppearance("it isn't a dictionary".into())),
                });
                continue;
            };
            let subtype = String::from_utf8_lossy(d.name(b"Subtype").unwrap_or_default()).into_owned();
            let label = format!("{} annotation {}", if subtype.is_empty() { "an untyped" } else { subtype.as_str() }, k.saturating_add(1));
            let decided = self.annotation(r, &d, &resources, rotate, t, &mut uses);
            let omitted = r.is_some_and(|r| self.omit.contains(&r));
            let outcome = match decided {
                Ok(draw) if omitted => {
                    let _ = draw;
                    Outcome::Omitted(None)
                }
                Ok(draw) => {
                    let source = draw.source;
                    draws.push((t.annotations.len(), draw));
                    Outcome::Drawn(source)
                }
                Err(Outcome::Unresolved(rule)) if omitted && is_annotation_rule(rule) => Outcome::Omitted(Some(rule)),
                Err(Outcome::Unresolved(rule)) => {
                    if is_annotation_rule(rule) {
                        let detail = self.pending_detail.take();
                        t.annotation_items.push((rule, r, label, detail));
                    }
                    Outcome::Unresolved(rule)
                }
                Err(other) => other,
            };
            self.pending_detail = None;
            t.annotations.push(AnnotationEntry { index: k, object: r, subtype, outcome });
        }

        // The content must end in its initial graphics state for the annotations to be placed as
        // viewers place them: close what it leaves open, or refuse if it restores too much.
        let mut drawing = String::new();
        let mut extra_xobjects = Dict::new();
        let mut extra_states = Dict::new();
        if !draws.is_empty() {
            let resolved = parse(new.as_deref().unwrap_or(joined.data()));
            let (mut depth, mut lowest, mut text) = (0i64, 0i64, false);
            for op in &resolved.ops {
                match op.op.as_slice() {
                    b"q" => depth = depth.saturating_add(1),
                    b"Q" => {
                        depth = depth.saturating_sub(1);
                        lowest = lowest.min(depth);
                    }
                    b"BT" => text = true,
                    b"ET" => text = false,
                    _ => {}
                }
            }
            if lowest < 0 || text {
                for (i, _) in draws.drain(..) {
                    if let Some(e) = t.annotations.get_mut(i) {
                        let omitted = e.object.is_some_and(|r| self.omit.contains(&r));
                        e.outcome =
                            if omitted { Outcome::Omitted(Some(Rule::AnnotationPlacement)) } else { Outcome::Unresolved(Rule::AnnotationPlacement) };
                        if !omitted {
                            t.annotation_items.push((
                                Rule::AnnotationPlacement,
                                e.object,
                                format!("{} annotation {}", e.subtype, e.index.saturating_add(1)),
                                None,
                            ));
                        }
                    }
                }
            } else {
                drawing.push_str(&"Q\n".repeat(usize::try_from(depth).unwrap_or(0).saturating_add(1)));
                let taken = |cat: &[u8], name: &str| resources.get(cat).and_then(|c| src.dict(c)).is_some_and(|c| c.contains(name.as_bytes()));
                let mut n = 0usize;
                for (_, draw) in &draws {
                    let (xname, gname) = loop {
                        let (x, g) = (format!("PCAn{n}"), format!("PCAg{n}"));
                        n = n.saturating_add(1);
                        if !taken(b"XObject", &x) && !taken(b"ExtGState", &g) {
                            break (x, g);
                        }
                    };
                    match &draw.opacity {
                        None => {
                            drawing.push_str(&format!("q {} cm /{xname} Do Q\n", an::matrix(draw.matrix)));
                            extra_xobjects.set(xname.into_bytes(), Object::Ref(draw.appearance));
                        }
                        Some((stroke, fill, blend)) => {
                            // The appearance as one transparency group, so its parts don't compound.
                            let mut inner = Dict::new();
                            inner.set(b"A".to_vec(), Object::Ref(draw.appearance));
                            let mut res = Dict::new();
                            res.set(b"XObject".to_vec(), Object::Dict(inner));
                            let mut group = Dict::new();
                            group.set(b"S".to_vec(), Object::name("Transparency"));
                            let mut fd = Dict::new();
                            fd.set(b"Type".to_vec(), Object::name("XObject"));
                            fd.set(b"Subtype".to_vec(), Object::name("Form"));
                            fd.set(b"BBox".to_vec(), Object::Array(draw.covers.iter().map(|v| Object::Real(*v)).collect()));
                            fd.set(b"Group".to_vec(), Object::Dict(group));
                            fd.set(b"Resources".to_vec(), Object::Dict(res));
                            let body = format!("q {} cm /A Do Q\n", an::matrix(draw.matrix));
                            let form = self.copy.add(Object::Stream(Stream::flate(fd, body.as_bytes())));
                            let mut gs = Dict::new();
                            gs.set(b"Type".to_vec(), Object::name("ExtGState"));
                            gs.set(b"CA".to_vec(), Object::Real(*stroke));
                            gs.set(b"ca".to_vec(), Object::Real(*fill));
                            if let Some(bm) = blend {
                                gs.set(b"BM".to_vec(), bm.clone());
                            }
                            drawing.push_str(&format!("q /{gname} gs /{xname} Do Q\n"));
                            extra_xobjects.set(xname.into_bytes(), Object::Ref(form));
                            extra_states.set(gname.into_bytes(), Object::Dict(gs));
                        }
                    }
                }
            }
        }

        let contents = new.map(|data| {
            t.streams_rewritten = t.streams_rewritten.saturating_add(1);
            self.copy.add(Object::Stream(Stream::flate(Dict::new(), &data)))
        });
        let pruned = self.prune.then(|| prune(src, &resources, &uses));
        if let Some((_, removed)) = &pruned {
            t.resources_removed = t.resources_removed.saturating_add(*removed);
        }
        let drawn = !drawing.is_empty();
        if contents.is_none() && pruned.is_none() && !drawn && annots.is_empty() {
            return;
        }
        let mut res = pruned.map(|(p, _)| p).unwrap_or_else(|| resources.clone());
        if drawn {
            for (cat, extra) in [(&b"XObject"[..], &extra_xobjects), (b"ExtGState", &extra_states)] {
                if extra.is_empty() {
                    continue;
                }
                let mut merged = res.get(cat).and_then(|c| src.dict(c)).unwrap_or_default();
                for (k, v) in extra.iter() {
                    merged.set(k.clone(), v.clone());
                }
                res.set(cat.to_vec(), Object::Dict(merged));
            }
        }
        let body: Vec<Object> = match contents {
            Some(c) => vec![Object::Ref(c)],
            None => list.clone(),
        };
        let new_contents = if drawn {
            let before = self.copy.add(Object::Stream(Stream::flate(Dict::new(), b"q\n")));
            let after = self.copy.add(Object::Stream(Stream::flate(Dict::new(), drawing.as_bytes())));
            let mut all = vec![Object::Ref(before)];
            all.extend(body);
            all.push(Object::Ref(after));
            Some(Object::Array(all))
        } else {
            contents.map(Object::Ref)
        };
        let set_resources = pruned_or_drawn(self.prune, drawn);
        let updated = self.copy.update_dict(obj, |d| {
            if let Some(c) = new_contents {
                d.set(b"Contents".to_vec(), c);
            }
            if set_resources {
                d.set(b"Resources".to_vec(), Object::Dict(res));
            }
            // Nothing interactive goes on: every annotation is drawn, excluded or reported.
            d.remove(b"Annots");
        });
        if updated.is_err() {
            t.issues.push(Rule::Unreadable);
        }
    }

    /// Decide one annotation. `Ok`: draw it as given. `Err`: why not (excluded, or unresolved
    /// with the rule; layer problems go into `t`).
    fn annotation(&mut self, r: Option<ObjRef>, d: &Dict, resources: &Dict, rotate: i64, t: &mut Tally, uses: &mut Uses) -> Result<Draw, Outcome> {
        let src = self.src;
        let subtype = d.name(b"Subtype").unwrap_or_default().to_vec();
        let flags = an::flags(src, d);
        let excluded = |e: Exclusion| Err(Outcome::Excluded(e));
        match subtype.as_slice() {
            b"Popup" => return excluded(Exclusion::Popup),
            b"Link" => return excluded(Exclusion::Link),
            _ => {}
        }
        if flags & an::HIDDEN != 0 {
            return excluded(Exclusion::Hidden);
        }
        if flags & an::NO_VIEW != 0 {
            return excluded(Exclusion::NoView);
        }
        if flags & an::INVISIBLE != 0 && !an::is_known(&subtype) {
            return excluded(Exclusion::InvisibleUnknown);
        }
        if let Some(oc) = d.get(b"OC") {
            let v = self.ev.visibility(oc);
            if let Some(g) = oc.as_ref() {
                t.seen.push((g, v.visible));
            }
            t.optional = true;
            match v.visible {
                Some(false) => return excluded(Exclusion::LayerOff),
                Some(true) => {}
                None => {
                    let rule = v.problems.first().map_or(Rule::NotOptionalContent, |p| p.rule);
                    t.problems.extend(v.problems);
                    return Err(Outcome::Unresolved(rule));
                }
            }
        }
        let Some(rect) = an::rect(src, d) else { return excluded(Exclusion::NoRect) };
        let stored = an::stored(src, d);
        let usable = |ap: ObjRef| -> Result<(), String> {
            let Object::Stream(s) = &*src.get(ap) else { return Err("it isn't a stream".into()) };
            s.decoded().map_err(|e| format!("it can't be decoded: {e}"))?;
            let (bbox, matrix) = an::form_box(src, &s.dict)?;
            an::fit(bbox, matrix, rect).map(|_| ()).ok_or_else(|| "its bounding box has no area".to_string())
        };
        let stored_ok: Result<ObjRef, (bool, String)> = match stored {
            Stored::Stream(ap) => usable(ap).map(|_| ap).map_err(|why| (false, why)),
            Stored::Missing => Err((true, "it has none".into())),
            Stored::NoState => return excluded(Exclusion::NoAppearance),
            Stored::Invalid => Err((false, "it isn't a stream".into())),
        };
        if an::NOT_DRAWN.contains(&subtype.as_slice()) {
            return match stored_ok {
                Ok(_) => Err(Outcome::Unresolved(Rule::AnnotationNotDrawn)),
                Err((true, _)) => excluded(Exclusion::NoAppearance),
                Err((false, why)) => excluded(Exclusion::UnusableAppearance(why)),
            };
        }
        let mark = pdfcraft_fonts::undrawn_mark(d);

        // Choose the appearance: stored, generated (comments) or redrawn from the value (fields).
        let (appearance, source) = if subtype == b"Widget" {
            self.widget(r, d, &stored_ok)?
        } else {
            match &stored_ok {
                Ok(ap) => (*ap, AppearanceSource::Stored),
                Err((missing, why)) => {
                    if let Some((what, c)) = pdfcraft_annot::appearance::undrawable_text(d) {
                        self.pending_detail = Some(format!("\"{}\" (U+{:04X}) in {what}", c.escape_debug(), u32::from(c)));
                        return Err(Outcome::Unresolved(Rule::AnnotationUndrawableText));
                    }
                    match pdfcraft_annot::appearance::build(d) {
                        Some(stream) => (self.copy.add(Object::Stream(stream)), AppearanceSource::Generated),
                        None if *missing => return excluded(Exclusion::NoAppearance),
                        None => return excluded(Exclusion::UnusableAppearance(why.clone())),
                    }
                }
            }
        };
        if source == AppearanceSource::Stored
            && let Some(c) = mark
        {
            // O15: PdfCraft drew this appearance with ? in place of characters its fonts can't
            // draw; drawing it would show ? as if it were the text.
            self.pending_detail = Some(format!("? drawn in place of \"{}\" (U+{:04X})", c.escape_debug(), u32::from(c)));
            return Err(Outcome::Unresolved(Rule::AnnotationUndrawnText));
        }

        // The appearance's own optional content, and any inside it.
        if source == AppearanceSource::Stored {
            if let Some(oc) = src.dict(&Object::Ref(appearance)).and_then(|a| a.get(b"OC").cloned()) {
                let v = self.ev.visibility(&oc);
                if let Some(g) = oc.as_ref() {
                    t.seen.push((g, v.visible));
                }
                t.optional = true;
                match v.visible {
                    Some(false) => return excluded(Exclusion::LayerOff),
                    Some(true) => {
                        self.strip_oc.insert(appearance);
                    }
                    None => {
                        let rule = v.problems.first().map_or(Rule::NotOptionalContent, |p| p.rule);
                        t.problems.extend(v.problems);
                        return Err(Outcome::Unresolved(rule));
                    }
                }
            }
            self.child(appearance, Kind::Content, resources, 1, t, uses);
        }

        // Placement: Algorithm 8.1, then NoRotate.
        let form = self.copy.dict(&Object::Ref(appearance)).unwrap_or_default();
        let (bbox, fm) = an::form_box(&self.copy, &form).map_err(|why| Outcome::Excluded(Exclusion::UnusableAppearance(why)))?;
        let a = an::fit(bbox, fm, rect).ok_or_else(|| Outcome::Excluded(Exclusion::UnusableAppearance("its bounding box has no area".into())))?;
        let matrix = if flags & an::NO_ROTATE != 0 { an::upright(a, rect, rotate) } else { a };
        let whole = an::then(fm, matrix);
        let corners = [(bbox[0], bbox[1]), (bbox[2], bbox[1]), (bbox[0], bbox[3]), (bbox[2], bbox[3])]
            .map(|(x, y)| (whole[0] * x + whole[2] * y + whole[4], whole[1] * x + whole[3] * y + whole[5]));
        let mut covers = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for (x, y) in corners {
            covers = [covers[0].min(x), covers[1].min(y), covers[2].max(x), covers[3].max(y)];
        }
        if !matrix.iter().chain(covers.iter()).all(|v| v.is_finite()) {
            return excluded(Exclusion::UnusableAppearance("its placement isn't finite".into()));
        }
        Ok(Draw { appearance, matrix, covers, opacity: an::opacity(src, d), source })
    }

    /// A widget's appearance (O8): redrawn from the field's value when its appearance is missing or
    /// the form sets `/NeedAppearances`; otherwise, or when redrawing would show `?`, the stored one.
    fn widget(&mut self, r: Option<ObjRef>, d: &Dict, stored: &Result<ObjRef, (bool, String)>) -> Result<(ObjRef, AppearanceSource), Outcome> {
        use pdfcraft_forms::FieldKind;
        let missing = matches!(stored, Err((true, _)));
        let wanted = missing || self.need_appearances;
        let field = if wanted {
            let src = self.src;
            let fields = self.fields.get_or_insert_with(|| {
                pdfcraft_forms::fields(src).into_iter().flat_map(|f| f.widgets.clone().into_iter().map(move |w| (w.obj, (f.clone(), w)))).collect()
            });
            r.and_then(|r| fields.get(&r).cloned())
        } else {
            None
        };
        let clean_stored = || match stored {
            Ok(ap) if pdfcraft_fonts::undrawn_mark(d).is_none() => Some(*ap),
            _ => None,
        };
        if let Some((f, w)) = field {
            match f.kind {
                FieldKind::Text | FieldKind::Combo | FieldKind::List => {
                    let (stream, bad) = pdfcraft_forms::appearance::field_appearance_drawn(self.src, &f, &w, &f.value, true);
                    match bad {
                        None => return Ok((self.copy.add(Object::Stream(stream)), AppearanceSource::Regenerated)),
                        Some(c) => {
                            if let Some(ap) = clean_stored() {
                                return Ok((ap, AppearanceSource::Stored));
                            }
                            self.pending_detail = Some(format!("\"{}\" (U+{:04X}) in the field's value", c.escape_debug(), u32::from(c)));
                            return Err(Outcome::Unresolved(Rule::AnnotationUndrawableText));
                        }
                    }
                }
                FieldKind::CheckBox | FieldKind::Radio if missing => {
                    let on = w.on_state.clone().unwrap_or_else(|| "Yes".into());
                    let states = pdfcraft_forms::appearance::check_box_states(&mut self.copy, &w, f.kind, &on);
                    let state = d.name(b"AS").map(<[u8]>::to_vec).unwrap_or_else(|| b"Off".to_vec());
                    let picked = states.get(b"N").and_then(|n| self.copy.dict(n)).and_then(|n| n.get(&state).and_then(Object::as_ref));
                    return match picked {
                        Some(ap) => Ok((ap, AppearanceSource::Regenerated)),
                        None => Err(Outcome::Excluded(Exclusion::NoAppearance)),
                    };
                }
                _ => {}
            }
        }
        match stored {
            Ok(ap) => Ok((*ap, AppearanceSource::Stored)),
            Err((true, _)) => Err(Outcome::Excluded(Exclusion::NoAppearance)),
            Err((false, why)) => Err(Outcome::Excluded(Exclusion::UnusableAppearance(why.clone()))),
        }
    }

    /// Resolve one content stream drawn with `resources` and everything its kept content draws.
    /// Returns its rewritten data (`None`: unchanged) and the names of `resources` it uses,
    /// including those used by streams it draws that have no resources of their own.
    fn content(&mut self, data: &[u8], resources: &Dict, inherits: bool, depth: usize, t: &mut Tally) -> (Option<Vec<u8>>, Uses) {
        let src = self.src;
        let parsed = parse(data);
        let plan = resolve::plan(&parsed.ops, parsed.skipped, &Names { doc: src, resources, surrounding: &self.surrounding }, &mut self.ev, inherits);
        let names = Names { doc: src, resources, surrounding: &[] };
        t.absorb(&plan);
        self.strip_oc.extend(plan.visible_oc_xobjects.iter().copied());
        let mut uses = plan.uses.clone();
        let next = depth.saturating_add(1);

        let mut xobjects = plan.uses.xobjects.clone();
        xobjects.sort();
        xobjects.dedup();
        for n in &xobjects {
            let Some(r) = names.entry(b"XObject", n).and_then(|o| o.as_ref()) else { continue };
            let is_form = matches!(&*src.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Form"[..]));
            if is_form {
                self.child(r, Kind::Content, resources, next, t, &mut uses);
            }
        }
        let mut fonts = plan.uses.fonts.clone();
        fonts.sort();
        fonts.dedup();
        for n in &fonts {
            let Some(font) = names.entry(b"Font", n).and_then(|f| src.dict(&f)) else { continue };
            if font.name(b"Subtype") != Some(&b"Type3"[..]) {
                continue;
            }
            let own = font.get(b"Resources").and_then(|r| src.dict(r));
            let procs: Vec<ObjRef> =
                font.get(b"CharProcs").and_then(|p| src.dict(p)).map(|p| p.iter().filter_map(|(_, v)| v.as_ref()).collect()).unwrap_or_default();
            for r in procs {
                self.child(r, Kind::Glyph(own.clone()), resources, next, t, &mut uses);
            }
        }
        let mut states = plan.uses.states.clone();
        states.sort();
        states.dedup();
        for n in &states {
            let group = names
                .entry(b"ExtGState", n)
                .and_then(|g| src.dict(&g))
                .and_then(|g| g.get(b"SMask").and_then(|m| src.dict(m)))
                .and_then(|m| m.get(b"G").and_then(Object::as_ref));
            if let Some(r) = group {
                self.child(r, Kind::Content, resources, next, t, &mut uses);
            }
        }
        let mut patterns = plan.uses.patterns.clone();
        patterns.sort();
        patterns.dedup();
        for n in &patterns {
            let Some(r) = names.entry(b"Pattern", n).and_then(|o| o.as_ref()) else { continue };
            let tiling = matches!(&*src.get(r), Object::Stream(s) if s.dict.int(b"PatternType") == Some(1));
            if tiling {
                self.child(r, Kind::Content, resources, next, t, &mut uses);
            }
        }

        let new = if plan.edits.is_empty() {
            None
        } else {
            match resolve::apply(data, &parsed.ops, &plan) {
                Some(d) => Some(d),
                None => {
                    t.issues.push(Rule::Unreadable);
                    None
                }
            }
        };
        (new, uses)
    }

    /// Resolve stream `r`, drawn from a stream whose resources are `inherited`, and write its copy.
    fn child(&mut self, r: ObjRef, kind: Kind, inherited: &Dict, depth: usize, t: &mut Tally, parent: &mut Uses) {
        if depth > MAX_DEPTH || self.active.contains(&r) {
            t.issues.push(Rule::TooDeep);
            return;
        }
        let src = self.src;
        let obj = src.get(r);
        let Object::Stream(s) = &*obj else { return };
        let (own, glyph) = match kind {
            Kind::Content => (s.dict.get(b"Resources").and_then(|x| src.dict(x)), false),
            Kind::Glyph(font) => (font, true),
        };
        if let Some(done) = self.done.get(&r) {
            let reuse = match (&done.inherited, own.is_some()) {
                (_, true) => true,
                (Some(before), false) => before == inherited,
                (None, false) => false,
            };
            // OV4.9: whether content under an undefined marker name may be shown depends on the
            // surrounding resources, so a stream with such a name (or drawing one) is reused only
            // under the same surroundings and resolved again anywhere else: a result cached in one
            // place never stands in for the check in another.
            let same_surroundings = !self.layered
                || done.findings.undefined.is_empty()
                || done.context.split_last().is_some_and(|(last, around)| last == inherited && around == self.surrounding.as_slice());
            if reuse && same_surroundings {
                t.merge(&done.findings, true);
                if own.is_none() {
                    parent.extend(&done.uses);
                }
                return;
            }
        }
        self.visits = self.visits.saturating_add(1);
        if self.visits > MAX_VISITS {
            t.issues.push(Rule::TooDeep);
            return;
        }
        let data = match s.decoded() {
            Ok(d) => d,
            Err(_) => {
                t.issues.push(Rule::Unreadable);
                return;
            }
        };
        let resources = own.clone().unwrap_or_else(|| inherited.clone());
        let mut sub = Tally::default();
        self.active.push(r);
        // The drawing stream's resources surround this one (OV4.9).
        if self.layered {
            self.surrounding.push(inherited.clone());
        }
        let (new, uses) = self.content(&data, &resources, true, depth, &mut sub);
        let context = if self.layered && !sub.undefined.is_empty() { self.surrounding.clone() } else { Vec::new() };
        if self.layered {
            self.surrounding.pop();
        }
        self.active.pop();
        if own.is_none() {
            parent.extend(&uses);
        }
        if let Some(before) = self.done.get(&r) {
            // Drawn again with other resources: it must resolve the same way.
            if before.data != new {
                sub.issues.push(Rule::AmbiguousResources);
            }
            t.merge(&sub, true);
            return;
        }
        let prune_own = self.prune && !glyph && own.is_some();
        if new.is_some() || prune_own {
            let current = self.copy.get(r);
            if let Object::Stream(cs) = &*current {
                let mut dict = cs.dict.clone();
                if prune_own && let Some(o) = &own {
                    let (p, removed) = prune(src, o, &uses);
                    sub.resources_removed = sub.resources_removed.saturating_add(removed);
                    dict.set(b"Resources".to_vec(), Object::Dict(p));
                }
                let stream = match &new {
                    Some(d) => {
                        dict.remove(b"Length");
                        dict.remove(b"DL");
                        sub.streams_rewritten = sub.streams_rewritten.saturating_add(1);
                        Stream::flate(dict, d)
                    }
                    None => Stream { dict, raw: cs.raw.clone() },
                };
                self.copy.set(r, Object::Stream(stream));
            }
        }
        t.merge(&sub, false);
        self.done.insert(r, Done { inherited: own.is_none().then(|| inherited.clone()), context, data: new, uses, findings: sub });
    }

    /// Remove `/OC` from the XObjects drawn visibly.
    fn strip(&mut self) {
        for r in self.strip_oc.iter().copied().collect::<Vec<_>>() {
            if self.copy.get(r).as_dict().is_some_and(|d| d.contains(b"OC")) {
                let _ = self.copy.update_dict(r, |d| {
                    d.remove(b"OC");
                });
            }
        }
    }
}

/// The `/Resources` of `page`'s ancestors in the page tree, innermost last, following `/Parent`
/// at most `MAX_NESTING` levels and never through the same node twice.
fn ancestor_resources(src: &Document, page: &Dict) -> Vec<Dict> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut next = page.get(b"Parent").and_then(Object::as_ref);
    for _ in 0..MAX_NESTING {
        let Some(r) = next else { break };
        if !seen.insert(r) {
            break;
        }
        let Some(node) = src.dict(&Object::Ref(r)) else { break };
        if let Some(res) = node.get(b"Resources").and_then(|x| src.dict(x)) {
            out.push(res);
        }
        next = node.get(b"Parent").and_then(Object::as_ref);
    }
    out.reverse();
    out
}

/// Whether the page's resources are rewritten: pruned (a document with layers), or extended with
/// the annotations drawn.
fn pruned_or_drawn(prune: bool, drawn: bool) -> bool {
    prune || drawn
}

/// The rules an omit decision settles.
fn is_annotation_rule(rule: Rule) -> bool {
    matches!(rule, Rule::AnnotationUndrawnText | Rule::AnnotationUndrawableText | Rule::AnnotationNotDrawn | Rule::AnnotationPlacement)
}

/// `resources` keeping only what `uses` names in the categories content draws by name, and
/// without optional-content properties. Returns the number of entries left out.
fn prune(src: &Document, resources: &Dict, uses: &Uses) -> (Dict, usize) {
    let mut out = resources.clone();
    let mut removed = 0usize;
    for (cat, names) in [(&b"XObject"[..], &uses.xobjects), (b"Font", &uses.fonts), (b"ExtGState", &uses.states), (b"Pattern", &uses.patterns)] {
        let Some(entries) = resources.get(cat).and_then(|c| src.dict(c)) else { continue };
        let mut kept = Dict::new();
        for (k, v) in entries.iter() {
            if names.contains(k) {
                kept.set(k.clone(), v.clone());
            } else {
                removed = removed.saturating_add(1);
            }
        }
        out.set(cat.to_vec(), Object::Dict(kept));
    }
    if let Some(props) = resources.get(b"Properties").and_then(|p| src.dict(p)) {
        let mut kept = Dict::new();
        for (k, v) in props.iter() {
            let optional = src.dict(v).is_some_and(|d| matches!(d.name(b"Type"), Some(b"OCG" | b"OCMD")));
            if optional {
                removed = removed.saturating_add(1);
            } else {
                kept.set(k.clone(), v.clone());
            }
        }
        out.set(b"Properties".to_vec(), Object::Dict(kept));
    }
    (out, removed)
}

/// Search everything the overlay will copy from the page (its resources and all they reach, as
/// `page_as_form` copies them) for optional content.
fn scan(doc: &Document, page: ObjRef) -> Result<(), Rule> {
    let Some(resources) = doc.dict(&Object::Ref(page)).and_then(|d| d.get(b"Resources").cloned()) else { return Ok(()) };
    let mut work: Vec<ObjRef> = Vec::new();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    look(&resources, 0, &mut work)?;
    while let Some(r) = work.pop() {
        if !seen.insert(r) {
            continue;
        }
        if seen.len() > MAX_SCAN {
            return Err(Rule::TooDeep);
        }
        look(&doc.get(r), 0, &mut work)?;
    }
    Ok(())
}

/// One object's direct parts: optional content is an error; references are queued.
fn look(o: &Object, depth: usize, work: &mut Vec<ObjRef>) -> Result<(), Rule> {
    if depth > MAX_NESTING {
        return Err(Rule::TooDeep);
    }
    match o {
        Object::Ref(r) => work.push(*r),
        Object::Array(a) => {
            for x in a {
                look(x, depth.saturating_add(1), work)?;
            }
        }
        Object::Dict(_) | Object::Stream(_) => {
            if let Some(d) = o.as_dict() {
                if matches!(d.name(b"Type"), Some(b"OCG" | b"OCMD")) || d.contains(b"OC") {
                    return Err(Rule::InResources);
                }
                for (_, v) in d.iter() {
                    look(v, depth.saturating_add(1), work)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

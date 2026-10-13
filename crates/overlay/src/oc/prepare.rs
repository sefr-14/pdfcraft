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
//! The source document is only read. A document without optional content and without decisions
//! is copied unchanged, so its overlay is the overlay OV3 makes.

use std::collections::{HashMap, HashSet};
use std::fmt;

use pdfcraft_content::{Pieces, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::eval::{Evaluator, Note, Problem};
use super::resolve::{self, Names, Plan, Uses};
use super::{Rule, Unresolved, Visibility};
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
    /// Why the page can't be composed into layers yet. Empty: it can.
    pub unresolved: Vec<Unresolved>,
}

/// A prepared copy of one source document.
pub struct Prepared {
    doc: Document,
    reports: Vec<PageReport>,
    notes: Vec<Note>,
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
    let mut w = Walker {
        src: doc,
        ev: Evaluator::new(doc, visibility),
        copy: doc.clone(),
        prune: touched,
        strip_oc: HashSet::new(),
        done: HashMap::new(),
        active: Vec::new(),
        visits: 0,
    };
    let mut reports = Vec::with_capacity(wanted.len());
    let mut names = HashMap::new();
    for p in wanted {
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
    let notes = w.ev.notes().to_vec();
    Ok(Prepared { doc: w.copy, reports, notes, names, touched })
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
            .map(|p| Unresolved { page, objects: p.objects.iter().copied().map(named).collect(), rule: p.rule, settleable: p.settleable })
            .chain(self.issues.iter().map(|rule| Unresolved { page, objects: Vec::new(), rule: *rule, settleable: false }))
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
            unresolved: kept,
        }
    }
}

/// A stream drawn inside another, already resolved.
struct Done {
    /// The resources it was resolved with, when it has none of its own.
    inherited: Option<Dict>,
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
    /// XObjects drawn visibly whose `/OC` is removed from the copy.
    strip_oc: HashSet<ObjRef>,
    done: HashMap<ObjRef, Done>,
    /// Streams being resolved, to stop at a stream that draws itself.
    active: Vec<ObjRef>,
    visits: usize,
}

impl Walker<'_> {
    fn page(&mut self, obj: ObjRef, dict: &Dict, t: &mut Tally) {
        let src = self.src;
        let resources = dict.get(b"Resources").and_then(|r| src.dict(r)).unwrap_or_default();
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
        let (new, uses) = self.content(joined.data(), &resources, false, 0, t);
        let contents = new.map(|data| {
            t.streams_rewritten = t.streams_rewritten.saturating_add(1);
            self.copy.add(Object::Stream(Stream::flate(Dict::new(), &data)))
        });
        let pruned = self.prune.then(|| prune(src, &resources, &uses));
        if let Some((_, removed)) = &pruned {
            t.resources_removed = t.resources_removed.saturating_add(*removed);
        }
        if contents.is_some() || pruned.is_some() {
            let updated = self.copy.update_dict(obj, |d| {
                if let Some(c) = contents {
                    d.set(b"Contents".to_vec(), Object::Ref(c));
                }
                if let Some((p, _)) = pruned {
                    d.set(b"Resources".to_vec(), Object::Dict(p));
                }
            });
            if updated.is_err() {
                t.issues.push(Rule::Unreadable);
            }
        }
    }

    /// Resolve one content stream drawn with `resources` and everything its kept content draws.
    /// Returns its rewritten data (`None`: unchanged) and the names of `resources` it uses,
    /// including those used by streams it draws that have no resources of their own.
    fn content(&mut self, data: &[u8], resources: &Dict, inherits: bool, depth: usize, t: &mut Tally) -> (Option<Vec<u8>>, Uses) {
        let src = self.src;
        let parsed = parse(data);
        let names = Names { doc: src, resources };
        let plan = resolve::plan(&parsed.ops, parsed.skipped, &names, &mut self.ev, inherits);
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
            match (&done.inherited, own.is_some()) {
                (_, true) => {
                    t.merge(&done.findings, true);
                    return;
                }
                (Some(before), false) if before == inherited => {
                    t.merge(&done.findings, true);
                    parent.extend(&done.uses);
                    return;
                }
                _ => {}
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
        let (new, uses) = self.content(&data, &resources, true, depth, &mut sub);
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
        self.done.insert(r, Done { inherited: own.is_none().then(|| inherited.clone()), data: new, uses, findings: sub });
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

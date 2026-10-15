//! The visibility evaluator (OV4.2): what an optional-content reference of a source shows under
//! the file's default configuration and the caller's explicit decisions.
//!
//! The rules (step 4 PRD, Resolving optional content), checked on 12 Oct against Adobe's PDF
//! Reference 1.7 (sixth edition, the basis of ISO 32000-1), §4.10, and against the Arlington
//! model of ISO 32000-2; the membership rules rechecked on 13 Oct against ISO 32000-1 §8.11
//! (OV4.7):
//!
//! - A group's state starts from the default configuration (`/D`): `/BaseState` (ON when
//!   absent; the standard allows only ON there, and OFF is followed with a note, as viewers do),
//!   then `/ON` and `/OFF`. Any other base state, or a configuration the standard doesn't allow,
//!   makes every group unresolved.
//! - A group takes part only when one of its intents (`/Intent`, View when absent) is in the
//!   configuration's set (`/D /Intent`, View when absent; All is every intent). Otherwise the
//!   standard says it has no effect on visibility, but pdfcraft's display uses its state anyway,
//!   so it is unresolved until an explicit decision settles it (O13).
//! - Membership dictionaries: `/VE` (And, Or, Not with one operand; groups as leaves; at most 32
//!   deep) and `/P` (AnyOn by default) over `/OCGs` are evaluated independently. When both are
//!   present they must both resolve and agree, or the dictionary is unresolved (the standard
//!   prefers `/VE` only in an informative note, and pdfcraft's display reads `/P`). With no group
//!   left and no `/VE`, a membership dictionary has no effect: its content is shown (ISO 32000-1
//!   Table 99). References to deleted objects in `/OCGs`, which pdfcraft's display counts as
//!   groups that are on, leave it unresolved whenever they change the result.
//! - Auto-states: Print and Export events aren't applied (they don't change the display). A View
//!   event with category View is unresolved when it would change a group's state, and recorded
//!   when it wouldn't; any other category is unresolved for groups whose usage dictionary has it.
//! - An explicit decision on a group or a membership dictionary fixes its visibility.
//!
//! Values combine in three-valued logic: a result that doesn't depend on an unresolved operand
//! (an AnyOn with one member visible, say) is resolved, and the operand's problem is dropped.
//! Otherwise every problem that could change the result is kept.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use super::{Rule, Visibility};

/// Deepest visibility expression the standard allows a reader to need (and the PRD sets).
pub const MAX_EXPRESSION_DEPTH: usize = 32;
/// Most expression nodes followed for one membership dictionary, so shared sub-expressions
/// can't multiply into an exponential walk.
const MAX_EXPRESSION_NODES: usize = 10_000;
/// Longest array of groups read (`/OCGs`, `/ON`, `/OFF`, an auto-state's groups).
const MAX_LIST: usize = 1 << 16;

/// Three-valued visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tri {
    On,
    Off,
    Unknown,
}

impl Tri {
    fn of(b: bool) -> Tri {
        if b { Tri::On } else { Tri::Off }
    }

    fn not(self) -> Tri {
        match self {
            Tri::On => Tri::Off,
            Tri::Off => Tri::On,
            Tri::Unknown => Tri::Unknown,
        }
    }
}

/// One reason a reference can't be resolved: the objects involved, the rule, and whether an
/// explicit decision on one of those objects settles it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Problem {
    pub objects: Vec<ObjRef>,
    pub rule: Rule,
    pub settleable: bool,
}

/// What a reference shows: `visible` is `Some` when it is resolved, and then `problems` is
/// empty; otherwise `problems` says why not.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub visible: Option<bool>,
    pub problems: Vec<Problem>,
}

impl Verdict {
    fn known(v: bool) -> Verdict {
        Verdict { visible: Some(v), problems: Vec::new() }
    }

    fn problem(objects: Vec<ObjRef>, rule: Rule, settleable: bool) -> Verdict {
        Verdict { visible: None, problems: vec![Problem { objects, rule, settleable }] }
    }

    fn tri(&self) -> Tri {
        match self.visible {
            Some(v) => Tri::of(v),
            None => Tri::Unknown,
        }
    }

    /// A combined result: known values drop the operands' problems.
    fn from_tri(t: Tri, problems: Vec<Problem>) -> Verdict {
        match t {
            Tri::On => Verdict::known(true),
            Tri::Off => Verdict::known(false),
            Tri::Unknown => {
                let mut problems = problems;
                dedup(&mut problems);
                Verdict { visible: None, problems }
            }
        }
    }
}

fn dedup(problems: &mut Vec<Problem>) {
    let mut seen = HashSet::new();
    problems.retain(|p| seen.insert(p.clone()));
}

/// Something the report states that is demonstrably correct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// Print or Export auto-states aren't applied: they change what the source prints or exports,
    /// not what it shows.
    PrintExportNotApplied { groups: Vec<ObjRef> },
    /// A View auto-state that leaves the group as the default configuration has it.
    AutoStateNoChange { group: ObjRef },
    /// A membership dictionary with a visibility expression, which pdfcraft's own display ignores
    /// (it reads `/OCGs` and `/P`). `with_policy`: the dictionary also has groups in `/OCGs`, and
    /// the two readings agree (they must, OV4.7); otherwise the expression alone decides.
    /// `display_differs` says whether pdfcraft shows the content differently from the overlay
    /// (`None`: can't be told).
    ExpressionIgnoredByDisplay { membership: Option<ObjRef>, with_policy: bool, display_differs: Option<bool> },
    /// A membership dictionary with no group in `/OCGs` and no `/VE`: it has no effect on
    /// visibility, so its content is shown.
    MembershipNoEffect { membership: Option<ObjRef> },
    /// The default configuration's `/BaseState` is OFF, which the standard doesn't allow there
    /// (its value must be ON); the overlay follows it, as pdfcraft's display and pdf.js do: groups
    /// not listed in `/ON` are hidden.
    BaseStateOff,
    /// The default configuration's `/Intent` isn't View, which ISO 32000-1 Table 101 requires
    /// there ("shall be View for the document's default configuration"). Groups are matched with
    /// it as written: it can only leave more groups unresolved, never show more than the source's
    /// tab (OV4.7, F7).
    ConfigurationIntent { intents: Vec<String> },
    /// Content marked `/OC /Name BDC` with a name neither the stream's resources nor any resources
    /// around it define (OV4.9; seen in Bluebeam output, often in files without any layer
    /// configuration). It names no group or membership dictionary, so it isn't optional content
    /// (ISO 32000-1 §8.11.3.2; without `/OCProperties`, §8.11.4.2 has readers ignore optional
    /// content altogether), and viewers, PdfCraft's display included, show it. It is shown.
    UndefinedMarker,
}

/// A group listed in `/OCGs`: its default state and anything that makes it unresolved.
struct Group {
    state: bool,
    problems: Vec<Problem>,
}

/// The evaluator for one source document.
pub struct Evaluator<'a> {
    doc: &'a Document,
    groups: HashMap<ObjRef, Group>,
    /// Problems with the configuration itself; they make every group unresolved.
    global: Vec<Problem>,
    decisions: HashMap<ObjRef, bool>,
    notes: Vec<Note>,
    cache: HashMap<ObjRef, Verdict>,
}

impl<'a> Evaluator<'a> {
    /// Read `doc`'s default configuration and the explicit decisions.
    pub fn new(doc: &'a Document, visibility: &Visibility) -> Self {
        let mut ev = Evaluator {
            doc,
            groups: HashMap::new(),
            global: Vec::new(),
            decisions: visibility.decisions.iter().copied().collect(),
            notes: Vec::new(),
            cache: HashMap::new(),
        };
        ev.read_configuration();
        ev
    }

    /// Notes for the report, in the order found.
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    /// The groups listed in the catalog's `/OCGs`.
    pub fn listed_groups(&self) -> impl Iterator<Item = ObjRef> + '_ {
        self.groups.keys().copied()
    }

    /// A group's `/Name`, for messages.
    pub fn name(&self, r: ObjRef) -> Option<String> {
        let d = self.doc.dict(&Object::Ref(r))?;
        d.get(b"Name").and_then(|n| self.doc.resolve(n).as_string().map(|s| s.to_text()))
    }

    /// Whether `r` is an optional content group or membership dictionary of this file, or a
    /// group its configuration lists.
    pub fn is_optional_content(&self, r: ObjRef) -> bool {
        self.groups.contains_key(&r) || self.doc.dict(&Object::Ref(r)).is_some_and(|d| matches!(d.name(b"Type"), Some(b"OCG" | b"OCMD")))
    }

    fn global_problem(&mut self, rule: Rule) {
        let p = Problem { objects: Vec::new(), rule, settleable: false };
        if !self.global.contains(&p) {
            self.global.push(p);
        }
    }

    fn read_configuration(&mut self) {
        let doc = self.doc;
        let Some(catalog) = doc.root().and_then(|r| doc.dict(&Object::Ref(r))) else { return };
        let Some(props) = catalog.get(b"OCProperties").map(|p| doc.resolve(p)) else { return };
        let Some(props) = props.as_dict().cloned() else {
            self.global_problem(Rule::MalformedConfiguration);
            return;
        };
        let Some(listed) = props.get(b"OCGs").and_then(|o| self.refs(o)) else {
            self.global_problem(Rule::MalformedConfiguration);
            return;
        };
        let Some(config) = props.get(b"D").and_then(|d| doc.dict(d)) else {
            self.global_problem(Rule::MalformedConfiguration);
            return;
        };
        let base = match config.get(b"BaseState").map(|b| doc.resolve(b)) {
            None => Some(true),
            Some(b) => match b.as_name() {
                Some(b"ON") => Some(true),
                Some(b"OFF") => Some(false),
                _ => None,
            },
        };
        let Some(base) = base else {
            self.global_problem(Rule::BaseState);
            return;
        };
        if !base {
            self.note(Note::BaseStateOff);
        }
        let on = match config.get(b"ON") {
            None => Some(Vec::new()),
            Some(o) => self.refs(o),
        };
        let off = match config.get(b"OFF") {
            None => Some(Vec::new()),
            Some(o) => self.refs(o),
        };
        let intents = match config.get(b"Intent") {
            None => Some(vec![b"View".to_vec()]),
            Some(i) => names(doc, i),
        };
        let (Some(on), Some(off), Some(config_intents)) = (on, off, intents) else {
            self.global_problem(Rule::MalformedConfiguration);
            return;
        };
        if config_intents.iter().any(|i| i != b"View") {
            self.note(Note::ConfigurationIntent { intents: config_intents.iter().map(|i| String::from_utf8_lossy(i).into_owned()).collect() });
        }
        let (on, off): (HashSet<ObjRef>, HashSet<ObjRef>) = (on.into_iter().collect(), off.into_iter().collect());
        let all = config_intents.iter().any(|i| i == b"All");
        for g in listed {
            let mut problems = Vec::new();
            let state = match (on.contains(&g), off.contains(&g)) {
                (true, true) => {
                    problems.push(Problem { objects: vec![g], rule: Rule::OnAndOff, settleable: true });
                    base
                }
                (true, false) => true,
                (false, true) => false,
                (false, false) => base,
            };
            match doc.dict(&Object::Ref(g)) {
                Some(d) if d.name(b"Type") == Some(&b"OCG"[..]) => {
                    let intents = match d.get(b"Intent") {
                        None => Some(vec![b"View".to_vec()]),
                        Some(i) => names(doc, i),
                    };
                    match intents {
                        None => problems.push(Problem { objects: vec![g], rule: Rule::MalformedIntent, settleable: true }),
                        Some(i) if !all && !i.iter().any(|x| config_intents.contains(x)) => {
                            problems.push(Problem { objects: vec![g], rule: Rule::IntentMismatch, settleable: true });
                        }
                        Some(_) => {}
                    }
                }
                _ => problems.push(Problem { objects: vec![g], rule: Rule::NotOptionalContent, settleable: true }),
            }
            self.groups.insert(g, Group { state, problems });
        }
        if let Some(auto) = config.get(b"AS") {
            self.read_auto_states(auto);
        }
    }

    /// The default configuration's auto-states (`/AS`).
    fn read_auto_states(&mut self, auto: &Object) {
        let doc = self.doc;
        let Some(entries) = doc.resolve(auto).as_array().cloned() else {
            self.global_problem(Rule::MalformedAutoState);
            return;
        };
        if entries.len() > MAX_LIST {
            self.global_problem(Rule::MalformedAutoState);
            return;
        }
        for e in &entries {
            let Some(d) = doc.dict(e) else {
                self.global_problem(Rule::MalformedAutoState);
                continue;
            };
            let groups = match d.get(b"OCGs") {
                None => Some(Vec::new()),
                Some(o) => self.refs(o),
            };
            let Some(groups) = groups else {
                self.global_problem(Rule::MalformedAutoState);
                continue;
            };
            let event = d.get(b"Event").and_then(|v| doc.resolve(v).as_name().map(<[u8]>::to_vec));
            let categories = d.get(b"Category").and_then(|c| match &*doc.resolve(c) {
                Object::Array(_) => names(doc, c),
                _ => None,
            });
            match (event.as_deref(), categories) {
                (Some(b"Print" | b"Export"), Some(_)) => {
                    let listed: Vec<ObjRef> = groups.into_iter().filter(|g| self.groups.contains_key(g)).collect();
                    if !listed.is_empty() {
                        self.note(Note::PrintExportNotApplied { groups: listed });
                    }
                }
                (Some(b"View"), Some(categories)) => {
                    for g in groups {
                        self.apply_view_auto_state(g, &categories);
                    }
                }
                _ => {
                    for g in groups {
                        self.group_problem(g, Rule::MalformedAutoState);
                    }
                }
            }
        }
    }

    /// A View auto-state on group `g`: View-category states that would change the group, and any
    /// other category the group's usage dictionary has, are unresolved.
    fn apply_view_auto_state(&mut self, g: ObjRef, categories: &[Vec<u8>]) {
        let doc = self.doc;
        let Some(state) = self.groups.get(&g).map(|x| x.state) else { return };
        let usage = doc.dict(&Object::Ref(g)).and_then(|d| d.get(b"Usage").and_then(|u| doc.dict(u)));
        for c in categories {
            let entry = usage.as_ref().and_then(|u| u.get(c));
            if c == b"View" {
                let Some(view) = entry else { continue };
                let view_state = doc.dict(view).and_then(|v| v.get(b"ViewState").map(|s| doc.resolve(s)));
                match view_state.as_deref().map(Object::as_name) {
                    None => {}
                    Some(Some(b"ON")) | Some(Some(b"OFF")) => {
                        let wants = view_state.as_deref().and_then(Object::as_name) == Some(&b"ON"[..]);
                        if wants == state {
                            self.note(Note::AutoStateNoChange { group: g });
                        } else {
                            self.group_problem(g, Rule::AutoStateView);
                        }
                    }
                    Some(_) => self.group_problem(g, Rule::MalformedAutoState),
                }
            } else if entry.is_some() {
                self.group_problem(g, Rule::AutoStateEnvironment);
            }
        }
    }

    pub(crate) fn note(&mut self, n: Note) {
        if !self.notes.contains(&n) {
            self.notes.push(n);
        }
    }

    fn group_problem(&mut self, g: ObjRef, rule: Rule) {
        if let Some(group) = self.groups.get_mut(&g) {
            let p = Problem { objects: vec![g], rule, settleable: true };
            if !group.problems.contains(&p) {
                group.problems.push(p);
            }
        }
    }

    /// An array of references (a single reference counts as one), at most [`MAX_LIST`] long;
    /// `None` for anything else.
    fn refs(&self, o: &Object) -> Option<Vec<ObjRef>> {
        match &*self.doc.resolve(o) {
            Object::Array(a) if a.len() <= MAX_LIST => a.iter().map(Object::as_ref).collect(),
            _ => None,
        }
    }

    /// What `o`, used as optional content (`/OC` entries and the property of `/OC … BDC`), shows.
    pub fn visibility(&mut self, o: &Object) -> Verdict {
        match o {
            Object::Ref(r) => {
                if let Some(v) = self.cache.get(r) {
                    return v.clone();
                }
                let v = self.reference(*r);
                self.cache.insert(*r, v.clone());
                v
            }
            Object::Dict(d) if d.name(b"Type") == Some(&b"OCMD"[..]) => self.membership(None, d),
            Object::Dict(_) => Verdict::problem(Vec::new(), Rule::Inline, false),
            _ => Verdict::problem(Vec::new(), Rule::NotOptionalContent, false),
        }
    }

    fn reference(&mut self, r: ObjRef) -> Verdict {
        let Some(d) = self.doc.dict(&Object::Ref(r)) else {
            return self.decided(r).unwrap_or_else(|| Verdict::problem(vec![r], Rule::NotOptionalContent, true));
        };
        match d.name(b"Type") {
            Some(b"OCG") => self.group(r),
            Some(b"OCMD") => self.membership(Some(r), &d),
            _ => self.decided(r).unwrap_or_else(|| Verdict::problem(vec![r], Rule::NotOptionalContent, true)),
        }
    }

    fn decided(&self, r: ObjRef) -> Option<Verdict> {
        self.decisions.get(&r).map(|v| Verdict::known(*v))
    }

    /// A group: the configuration's problems first (no decision can repair the configuration),
    /// then an explicit decision, then its listed state.
    fn group(&self, r: ObjRef) -> Verdict {
        if !self.global.is_empty() {
            return Verdict { visible: None, problems: self.global.clone() };
        }
        if let Some(v) = self.decided(r) {
            return v;
        }
        match self.groups.get(&r) {
            None => Verdict::problem(vec![r], Rule::NotInOcgs, true),
            Some(g) if g.problems.is_empty() => Verdict::known(g.state),
            Some(g) => Verdict { visible: None, problems: g.problems.clone() },
        }
    }

    /// A membership dictionary (`r` when it is an indirect object).
    ///
    /// ISO 32000-1 §8.11.2.2 and Table 99: `/OCGs` is a group or an array of groups; null entries
    /// and references to deleted objects are ignored, and with none left the dictionary "shall
    /// have no effect on the visibility of any content" (recorded deviation from the step 4 PRD,
    /// which held this row unresolved until the clause was confirmed). `/P` is AnyOn by default.
    ///
    /// `/VE` and `/OCGs` with `/P` are two readings of one dictionary. The standard prefers `/VE`
    /// only in an informative note (§8.11.2.2, NOTE 2), and pdfcraft's display reads only `/OCGs`
    /// and `/P`. So when both are present they must agree (OV4.7, F1): both resolved and equal,
    /// or the dictionary is unresolved (`Rule::ExpressionDisagrees` when they differ). With only
    /// one present, that one decides.
    ///
    /// pdfcraft's display counts references to deleted objects in `/OCGs` as groups that are on,
    /// where the standard ignores them. The dictionary is evaluated both ways, and when the two
    /// differ it is unresolved (`Rule::DeletedMember`, OV4.7, F2).
    fn membership(&mut self, r: Option<ObjRef>, d: &Dict) -> Verdict {
        if let Some(v) = r.and_then(|r| self.decided(r)) {
            return v;
        }
        let own = |rule: Rule| Problem { objects: r.into_iter().collect(), rule, settleable: r.is_some() };
        let doc = self.doc;

        // `/OCGs`: one group by reference, or an array of groups, with the references to deleted
        // objects counted apart. Direct nulls are ignored by every reader.
        let members: Result<(Vec<ObjRef>, usize), ()> = match d.get(b"OCGs") {
            None => Ok((Vec::new(), 0)),
            Some(o) => {
                let resolved = doc.resolve(o);
                match (o, &*resolved) {
                    (Object::Ref(_), Object::Null) => Ok((Vec::new(), 1)),
                    (_, Object::Null) => Ok((Vec::new(), 0)),
                    (Object::Ref(g), Object::Dict(_)) => Ok((vec![*g], 0)),
                    (_, Object::Array(a)) if a.len() <= MAX_LIST => {
                        let mut live = Vec::new();
                        let mut deleted = 0usize;
                        let mut ok = true;
                        for x in a.iter() {
                            match x {
                                Object::Null => {}
                                Object::Ref(g) if matches!(&*doc.get(*g), Object::Null) => deleted = deleted.saturating_add(1),
                                Object::Ref(g) => live.push(*g),
                                _ => ok = false,
                            }
                        }
                        if ok { Ok((live, deleted)) } else { Err(()) }
                    }
                    _ => Err(()),
                }
            }
        };
        let policy = match d.get(b"P").map(|p| doc.resolve(p)) {
            None => Some(Policy::AnyOn),
            Some(p) => match p.as_name() {
                Some(b"AnyOn") => Some(Policy::AnyOn),
                Some(b"AllOn") => Some(Policy::AllOn),
                Some(b"AnyOff") => Some(Policy::AnyOff),
                Some(b"AllOff") => Some(Policy::AllOff),
                _ => None,
            },
        };
        let is_group = |g: &ObjRef| doc.dict(&Object::Ref(*g)).is_some_and(|d| d.name(b"Type") == Some(&b"OCG"[..]));
        // The `/OCGs` and `/P` reading, with deleted references ignored (the standard) or counted
        // as groups that are on (pdfcraft's display). `None`: no group, so no effect.
        let by_policy = |ev: &Evaluator, deleted_on: bool| -> Option<(Tri, Vec<Problem>)> {
            match &members {
                Err(()) => Some((Tri::Unknown, vec![own(Rule::MalformedMembership)])),
                Ok((live, deleted)) => {
                    let deleted = if deleted_on { *deleted } else { 0 };
                    if live.is_empty() && deleted == 0 {
                        None
                    } else if !live.iter().all(is_group) {
                        Some((Tri::Unknown, vec![own(Rule::MalformedMembership)]))
                    } else {
                        Some(match policy {
                            None => (Tri::Unknown, vec![own(Rule::UnknownPolicy)]),
                            Some(policy) => {
                                let mut verdicts: Vec<Verdict> = live.iter().map(|g| ev.group(*g)).collect();
                                verdicts.extend(std::iter::repeat_n(Verdict::known(true), deleted));
                                policy.combine(&verdicts)
                            }
                        })
                    }
                }
            }
        };
        let standard = by_policy(self, false);
        let deleted = members.as_ref().map(|(_, n)| *n).unwrap_or(0);
        let display = if deleted > 0 { by_policy(self, true) } else { standard.clone() };

        let expression = match d.get(b"VE") {
            Some(ve) => {
                let mut budget = MAX_EXPRESSION_NODES;
                Some(match self.expression(ve, 0, &mut Vec::new(), &mut budget) {
                    Ok(x) => x,
                    Err(rule) => (Tri::Unknown, vec![own(rule)]),
                })
            }
            None => None,
        };
        let disagree = own(Rule::ExpressionDisagrees);
        let (v, note) = combine_readings(r, expression.clone(), standard.clone(), &display, &disagree);
        if deleted > 0 {
            let (shown, _) = combine_readings(r, expression, display.clone(), &display, &disagree);
            if shown != v {
                let mut problems = v.problems;
                problems.extend(shown.problems);
                problems.push(own(Rule::DeletedMember));
                dedup(&mut problems);
                return Verdict { visible: None, problems };
            }
        }
        if let Some(n) = note {
            self.note(n);
        }
        v
    }

    /// A visibility expression. `Err` for a malformed one.
    fn expression(&mut self, o: &Object, depth: usize, path: &mut Vec<ObjRef>, budget: &mut usize) -> Result<(Tri, Vec<Problem>), Rule> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(Rule::MalformedExpression);
        }
        *budget = budget.checked_sub(1).ok_or(Rule::TooDeep)?;
        let pushed = match o {
            Object::Ref(r) => {
                if path.contains(r) {
                    return Err(Rule::MalformedExpression);
                }
                path.push(*r);
                true
            }
            _ => false,
        };
        let result = self.expression_inner(o, depth, path, budget);
        if pushed {
            path.pop();
        }
        result
    }

    fn expression_inner(&mut self, o: &Object, depth: usize, path: &mut Vec<ObjRef>, budget: &mut usize) -> Result<(Tri, Vec<Problem>), Rule> {
        let resolved = self.doc.resolve(o);
        let Object::Array(a) = &*resolved else { return Err(Rule::MalformedExpression) };
        let Some((op, operands)) = a.split_first() else { return Err(Rule::MalformedExpression) };
        let op = self.doc.resolve(op).as_name().map(<[u8]>::to_vec);
        let arity_ok = match op.as_deref() {
            Some(b"Not") => operands.len() == 1,
            Some(b"And" | b"Or") => !operands.is_empty(),
            _ => false,
        };
        if !arity_ok {
            return Err(Rule::MalformedExpression);
        }
        let mut values = Vec::with_capacity(operands.len());
        let mut problems = Vec::new();
        for x in operands {
            let leaf = match x {
                Object::Ref(r) => self.doc.dict(&Object::Ref(*r)).filter(|d| d.name(b"Type") == Some(&b"OCG"[..])).map(|_| *r),
                _ => None,
            };
            let (t, p) = match leaf {
                Some(g) => {
                    let v = self.group(g);
                    (v.tri(), v.problems)
                }
                None => match &*self.doc.resolve(x) {
                    Object::Array(_) => self.expression(x, depth.saturating_add(1), path, budget)?,
                    _ => return Err(Rule::MalformedExpression),
                },
            };
            values.push(t);
            problems.extend(p);
        }
        let t = match op.as_deref() {
            Some(b"Not") => values.first().copied().map_or(Tri::Unknown, Tri::not),
            Some(b"And") => and(&values),
            _ => or(&values),
        };
        Ok((t, if t == Tri::Unknown { problems } else { Vec::new() }))
    }
}

/// One membership dictionary's verdict from its two readings: the visibility expression (when
/// present) and the `/OCGs` and `/P` reading (`None`: no group, so no effect). `display` is how
/// pdfcraft's own display reads `/OCGs` and `/P`, for the note. Both readings present: they must
/// both resolve and agree (OV4.7, F1); otherwise the dictionary is unresolved.
fn combine_readings(
    r: Option<ObjRef>,
    expression: Option<(Tri, Vec<Problem>)>,
    policy: Option<(Tri, Vec<Problem>)>,
    display: &Option<(Tri, Vec<Problem>)>,
    disagree: &Problem,
) -> (Verdict, Option<Note>) {
    match (expression, policy) {
        (None, None) => (Verdict::known(true), Some(Note::MembershipNoEffect { membership: r })),
        (None, Some((t, problems))) => (Verdict::from_tri(t, problems), None),
        (Some((t, problems)), None) => {
            let v = Verdict::from_tri(t, problems);
            // pdfcraft's display ignores /VE: with no group it shows the content.
            let shown = match display {
                None => Some(true),
                Some((Tri::On, _)) => Some(true),
                Some((Tri::Off, _)) => Some(false),
                Some((Tri::Unknown, _)) => None,
            };
            let display_differs = match (v.visible, shown) {
                (Some(a), Some(b)) => Some(a != b),
                _ => None,
            };
            let note = v.visible.map(|_| Note::ExpressionIgnoredByDisplay { membership: r, with_policy: false, display_differs });
            (v, note)
        }
        (Some((te, pe)), Some((tp, pp))) => match (te, tp) {
            (Tri::Unknown, _) | (_, Tri::Unknown) => {
                let mut problems = Vec::new();
                if te == Tri::Unknown {
                    problems.extend(pe);
                }
                if tp == Tri::Unknown {
                    problems.extend(pp);
                }
                (Verdict::from_tri(Tri::Unknown, problems), None)
            }
            (a, b) if a == b => (
                Verdict::from_tri(a, Vec::new()),
                Some(Note::ExpressionIgnoredByDisplay { membership: r, with_policy: true, display_differs: Some(false) }),
            ),
            _ => (Verdict { visible: None, problems: vec![disagree.clone()] }, None),
        },
    }
}

#[derive(Clone, Copy, Debug)]
enum Policy {
    AnyOn,
    AllOn,
    AnyOff,
    AllOff,
}

impl Policy {
    fn combine(self, members: &[Verdict]) -> (Tri, Vec<Problem>) {
        let values: Vec<Tri> = members.iter().map(Verdict::tri).collect();
        let negated: Vec<Tri> = values.iter().map(|t| t.not()).collect();
        let t = match self {
            Policy::AnyOn => or(&values),
            Policy::AllOn => and(&values),
            Policy::AnyOff => or(&negated),
            Policy::AllOff => and(&negated),
        };
        let problems = if t == Tri::Unknown { members.iter().flat_map(|v| v.problems.iter().cloned()).collect() } else { Vec::new() };
        (t, problems)
    }
}

fn and(values: &[Tri]) -> Tri {
    if values.contains(&Tri::Off) {
        Tri::Off
    } else if values.iter().all(|v| *v == Tri::On) {
        Tri::On
    } else {
        Tri::Unknown
    }
}

fn or(values: &[Tri]) -> Tri {
    if values.contains(&Tri::On) {
        Tri::On
    } else if values.iter().all(|v| *v == Tri::Off) {
        Tri::Off
    } else {
        Tri::Unknown
    }
}

/// A name or an array of names; `None` for anything else.
fn names(doc: &Document, o: &Object) -> Option<Vec<Vec<u8>>> {
    match &*doc.resolve(o) {
        Object::Name(n) => Some(vec![n.clone()]),
        Object::Array(a) if a.len() <= MAX_LIST => a.iter().map(|x| doc.resolve(x).as_name().map(<[u8]>::to_vec)).collect(),
        _ => None,
    }
}

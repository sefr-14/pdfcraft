//! Resolving the optional content of one content stream (OV4.3): which optional-content markers
//! and drawn XObjects are visible, hidden or unresolved, and the edits that remove hidden drawing.
//!
//! - A visible block keeps its content; its `BDC` and matching `EMC` are removed.
//! - A hidden block loses its drawing: paths, text, images, inline images, shadings, XObjects,
//!   marked content, and the comments between them. Viewers hide only the painting of optional
//!   content and still apply its graphics-state changes (pdfcraft's display and pdf.js both do),
//!   so a state change inside a hidden block that later visible content still uses is kept, as an
//!   operator on its own; a clipping path that later content is drawn through is kept and ended
//!   with `n`; every other operator goes with the block. Nested blocks are visible only if every
//!   enclosing block is (PDF Reference 1.7, §4.10.2).
//! - An XObject with `/OC` that is hidden loses every `Do` that draws it.
//! - Anything that can't be decided is unresolved: a marker or XObject whose visibility the
//!   evaluator can't resolve, a marker whose property is missing or written inline, unbalanced
//!   marked content, a hidden block that a graphics-state save or restore, a text object or a path
//!   crosses, hidden text that changes where visible text goes, hidden text that may clip later
//!   content, or a stream the parser couldn't read completely.
//!
//! Edits are byte-range replacements on the decoded stream (a deleted operator becomes a single
//! space; a clipping path's painting operator becomes `n`), so every byte outside the edited
//! operators and hidden blocks stays as it was. The rewritten stream is parsed again and must give
//! exactly the expected operators.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Op, parse, write_op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use super::Rule;
use super::eval::{Evaluator, Problem};

/// A stream's named resources.
pub(crate) struct Names<'a> {
    pub doc: &'a Document,
    pub resources: &'a Dict,
}

impl Names<'_> {
    /// The entry `name` of resource category `cat`, unresolved (a reference stays a reference).
    pub fn entry(&self, cat: &[u8], name: &[u8]) -> Option<Object> {
        self.resources.get(cat).and_then(|c| self.doc.dict(c)).and_then(|d| d.get(name).cloned())
    }
}

/// One edit to a content stream, by operator index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Delete,
    /// The painting operator of a kept clipping path in a hidden block: written as `n`, so the
    /// clip still applies and nothing is painted.
    EndPath,
}

/// Names the kept content of a stream uses, so the walk can follow them and keep them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Uses {
    pub xobjects: Vec<Vec<u8>>,
    pub fonts: Vec<Vec<u8>>,
    pub states: Vec<Vec<u8>>,
    pub patterns: Vec<Vec<u8>>,
}

impl Uses {
    pub fn extend(&mut self, other: &Uses) {
        self.xobjects.extend(other.xobjects.iter().cloned());
        self.fonts.extend(other.fonts.iter().cloned());
        self.states.extend(other.states.iter().cloned());
        self.patterns.extend(other.patterns.iter().cloned());
    }
}

/// The decision for one content stream.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub edits: Vec<(usize, Edit)>,
    /// Hidden blocks, as operator indices of their `BDC` and `EMC`: the bytes between their
    /// operators (whitespace and comments) go too.
    pub hidden_ranges: Vec<(usize, usize)>,
    /// Visibility that couldn't be resolved, from the evaluator.
    pub problems: Vec<Problem>,
    /// Conditions in the stream itself that can't be resolved.
    pub issues: Vec<Rule>,
    /// Whether the stream reaches optional content at all (a marker, or an XObject with `/OC`).
    pub optional: bool,
    /// Optional-content objects the stream refers to, with their resolved visibility.
    pub seen: Vec<(ObjRef, Option<bool>)>,
    pub hidden_blocks: usize,
    pub kept_state: usize,
    /// Clipping paths kept from hidden blocks.
    pub kept_clips: usize,
    pub markers_removed: usize,
    pub hidden_xobjects: usize,
    /// XObjects with `/OC` drawn by visible content (their `/OC` is removed from the copy).
    pub visible_oc_xobjects: Vec<ObjRef>,
    pub uses: Uses,
}

/// Graphics-state categories, for deciding which state changes in hidden content are still used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Cat {
    Fill,
    Stroke,
    LineWidth,
    LineCap,
    LineJoin,
    Miter,
    Dash,
    Intent,
    Flatness,
    State,
    Ctm,
    Clip,
    CharSpace,
    WordSpace,
    Scale,
    Leading,
    Font,
    Render,
    Rise,
}

const ALL: [Cat; 19] = [
    Cat::Fill,
    Cat::Stroke,
    Cat::LineWidth,
    Cat::LineCap,
    Cat::LineJoin,
    Cat::Miter,
    Cat::Dash,
    Cat::Intent,
    Cat::Flatness,
    Cat::State,
    Cat::Ctm,
    Cat::Clip,
    Cat::CharSpace,
    Cat::WordSpace,
    Cat::Scale,
    Cat::Leading,
    Cat::Font,
    Cat::Render,
    Cat::Rise,
];

/// The category an operator sets, if it is a graphics-state operator that can stand on its own.
fn sets(op: &[u8]) -> Option<Cat> {
    Some(match op {
        b"rg" | b"g" | b"k" | b"cs" | b"sc" | b"scn" => Cat::Fill,
        b"RG" | b"G" | b"K" | b"CS" | b"SC" | b"SCN" => Cat::Stroke,
        b"w" => Cat::LineWidth,
        b"J" => Cat::LineCap,
        b"j" => Cat::LineJoin,
        b"M" => Cat::Miter,
        b"d" => Cat::Dash,
        b"ri" => Cat::Intent,
        b"i" => Cat::Flatness,
        b"gs" => Cat::State,
        b"cm" => Cat::Ctm,
        b"Tc" => Cat::CharSpace,
        b"Tw" => Cat::WordSpace,
        b"Tz" => Cat::Scale,
        b"TL" => Cat::Leading,
        b"Tf" => Cat::Font,
        b"Tr" => Cat::Render,
        b"Ts" => Cat::Rise,
        _ => return None,
    })
}

/// Whether a later setter fully replaces the category (a matrix or clip composes instead, and an
/// ExtGState may set only some parameters).
fn replaceable(c: Cat) -> bool {
    !matches!(c, Cat::Ctm | Cat::Clip | Cat::State)
}

/// Categories a text-showing or text-positioning operator sets as a side effect, which a hidden
/// block can't keep without the operator itself.
fn side_effects(op: &[u8]) -> &'static [Cat] {
    match op {
        b"TD" => &[Cat::Leading],
        b"\"" => &[Cat::WordSpace, Cat::CharSpace],
        _ => &[],
    }
}

fn is_path_construction(op: &[u8]) -> bool {
    matches!(op, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

fn is_path_painting(op: &[u8]) -> bool {
    matches!(op, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n")
}

fn is_text_showing(op: &[u8]) -> bool {
    matches!(op, b"Tj" | b"TJ" | b"'" | b"\"")
}

fn is_text_positioning(op: &[u8]) -> bool {
    matches!(op, b"Td" | b"TD" | b"Tm" | b"T*")
}

/// Whether operator `op` uses category `c` (conservatively: text, XObjects, inline images and
/// shadings use everything).
fn uses(op: &[u8], c: Cat) -> bool {
    let common = matches!(c, Cat::Ctm | Cat::Clip | Cat::State | Cat::Intent | Cat::Flatness);
    let stroke = matches!(c, Cat::Stroke | Cat::LineWidth | Cat::LineCap | Cat::LineJoin | Cat::Miter | Cat::Dash);
    match op {
        b"f" | b"F" | b"f*" => common || c == Cat::Fill,
        b"S" | b"s" => common || stroke,
        b"B" | b"B*" | b"b" | b"b*" => common || stroke || c == Cat::Fill,
        b"Tj" | b"TJ" | b"'" | b"\"" | b"Do" | b"BI" | b"sh" => true,
        b"T*" | b"TD" => c == Cat::Leading,
        _ => false,
    }
}

/// Is the state change of category `c` at the end of a hidden block still used by what follows
/// (`after`, the rest of the stream)? A later setter at the same level replaces it, a restore
/// below the block's level discards it, and the end of the stream ends it.
fn live(after: &[Op], c: Cat) -> bool {
    let mut overwritten: Vec<bool> = vec![false];
    for op in after {
        match op.op.as_slice() {
            b"q" => {
                let top = overwritten.last().copied().unwrap_or(false);
                overwritten.push(top);
            }
            b"Q" => {
                overwritten.pop();
                if overwritten.is_empty() {
                    return false;
                }
            }
            o => {
                let shadowed = overwritten.last().copied().unwrap_or(false);
                if !shadowed && uses(o, c) {
                    return true;
                }
                if replaceable(c)
                    && (sets(o) == Some(c) || side_effects(o).contains(&c))
                    && let Some(top) = overwritten.last_mut()
                {
                    *top = true;
                    if overwritten.len() == 1 {
                        return false;
                    }
                }
            }
        }
    }
    false
}

/// Matching ends of marked content: for each `BMC`/`BDC`, the index of its `EMC`. `None` when the
/// stream's marked content doesn't balance.
fn matching_ends(ops: &[Op]) -> Option<HashMap<usize, usize>> {
    let mut open = Vec::new();
    let mut ends = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"BMC" | b"BDC" => open.push(i),
            b"EMC" => {
                let start = open.pop()?;
                ends.insert(start, i);
            }
            _ => {}
        }
    }
    open.is_empty().then_some(ends)
}

/// Three-valued visibility of the content at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Vis {
    On,
    Off,
    Unknown,
}

fn combine(outer: Vis, inner: Vis) -> Vis {
    match (outer, inner) {
        (Vis::Off, _) | (_, Vis::Off) => Vis::Off,
        (Vis::Unknown, _) | (_, Vis::Unknown) => Vis::Unknown,
        _ => Vis::On,
    }
}

fn vis_of(v: Option<bool>) -> Vis {
    match v {
        Some(true) => Vis::On,
        Some(false) => Vis::Off,
        None => Vis::Unknown,
    }
}

fn is_oc_marker(op: &Op) -> bool {
    op.is("BDC") && op.name(0) == Some(&b"OC"[..])
}

/// Plan one content stream. `skipped` is the parser's count of unreadable bytes; `inherits_state`
/// is true for streams drawn inside another (forms, glyphs, patterns), whose text rendering mode
/// comes from where they are drawn.
pub(crate) fn plan(ops: &[Op], skipped: usize, names: &Names<'_>, ev: &mut Evaluator<'_>, inherits_state: bool) -> Plan {
    let mut plan = Plan::default();
    let has_markers = ops.iter().any(is_oc_marker);
    plan.optional = has_markers;
    let ends = matching_ends(ops);
    if has_markers && ends.is_none() {
        plan.issues.push(Rule::Unbalanced);
    }
    let ends = ends.unwrap_or_default();
    let mut seen: HashMap<ObjRef, Option<bool>> = HashMap::new();

    // Pass 1: visibility of every marker and drawn XObject outside hidden blocks.
    let mut stack: Vec<Vis> = Vec::new();
    let mut hidden: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while let Some(op) = ops.get(i) {
        let effective = stack.iter().copied().fold(Vis::On, combine);
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                let oc = is_oc_marker(op);
                let mine = if oc {
                    let target = match op.operands.get(1) {
                        Some(Object::Name(n)) => names.entry(b"Properties", n),
                        _ => None,
                    };
                    match (target, op.operands.get(1)) {
                        (Some(o), _) => {
                            let v = ev.visibility(&o);
                            if let Some(r) = o.as_ref() {
                                seen.insert(r, v.visible);
                            }
                            if v.visible.is_none() {
                                plan.problems.extend(v.problems);
                            }
                            vis_of(v.visible)
                        }
                        (None, Some(Object::Name(_))) => {
                            plan.problems.push(Problem { objects: Vec::new(), rule: Rule::UnknownProperty, settleable: false });
                            Vis::Unknown
                        }
                        (None, _) => {
                            plan.problems.push(Problem { objects: Vec::new(), rule: Rule::Inline, settleable: false });
                            Vis::Unknown
                        }
                    }
                } else {
                    Vis::On
                };
                if oc {
                    match combine(effective, mine) {
                        Vis::Off => {
                            if let Some(&end) = ends.get(&i) {
                                hidden.push((i, end));
                                plan.hidden_blocks += 1;
                                // Everything to the end is hidden; skip over it here.
                                i = end.saturating_add(1);
                                continue;
                            }
                        }
                        Vis::On => {
                            if let Some(&end) = ends.get(&i) {
                                plan.edits.push((i, Edit::Delete));
                                plan.edits.push((end, Edit::Delete));
                                plan.markers_removed += 1;
                            }
                        }
                        Vis::Unknown => {}
                    }
                }
                stack.push(mine);
            }
            b"EMC" => {
                stack.pop();
            }
            b"Do" => {
                if let Some(name) = op.name(0) {
                    let target = names.entry(b"XObject", name);
                    let oc = target.as_ref().and_then(|t| names.doc.dict(t)).and_then(|d| d.get(b"OC").cloned());
                    match oc {
                        Some(oc) => {
                            plan.optional = true;
                            let v = ev.visibility(&oc);
                            if let Some(r) = oc.as_ref() {
                                seen.insert(r, v.visible);
                            }
                            match vis_of(v.visible) {
                                Vis::Off => {
                                    plan.edits.push((i, Edit::Delete));
                                    plan.hidden_xobjects += 1;
                                }
                                Vis::On => {
                                    if let Some(r) = target.as_ref().and_then(Object::as_ref) {
                                        plan.visible_oc_xobjects.push(r);
                                    }
                                    plan.uses.xobjects.push(name.to_vec());
                                }
                                Vis::Unknown => {
                                    plan.problems.extend(v.problems);
                                    plan.uses.xobjects.push(name.to_vec());
                                }
                            }
                        }
                        None => plan.uses.xobjects.push(name.to_vec()),
                    }
                }
            }
            _ => note_use(op, &mut plan.uses),
        }
        i += 1;
    }

    if plan.optional && skipped > 0 {
        plan.issues.push(Rule::Unreadable);
    }

    // Pass 2: the hidden blocks.
    if !hidden.is_empty() {
        let text_open = text_objects(ops);
        let path_open = open_paths(ops);
        let render_unknown = ops.iter().any(|o| o.is("Tr") && o.num(0).is_none_or(|v| !(0.0..=3.0).contains(&v)));
        for (s, e) in hidden {
            let tr_set_before = ops.get(..s).unwrap_or_default().iter().any(|o| o.is("Tr"));
            let text_may_clip = render_unknown || (inherits_state && !tr_set_before);
            hide(ops, s, e, &text_open, &path_open, text_may_clip, &mut plan);
        }
    }
    plan.edits.sort_by_key(|(i, _)| *i);
    plan.edits.dedup_by_key(|(i, _)| *i);
    plan.issues.sort();
    plan.issues.dedup();
    let mut seen: Vec<(ObjRef, Option<bool>)> = seen.into_iter().collect();
    seen.sort_by_key(|(r, _)| (r.num, r.generation));
    plan.seen = seen;
    plan
}

/// Names an operator of kept content uses.
fn note_use(op: &Op, uses: &mut Uses) {
    match op.op.as_slice() {
        b"Tf" => {
            if let Some(n) = op.name(0) {
                uses.fonts.push(n.to_vec());
            }
        }
        b"gs" => {
            if let Some(n) = op.name(0) {
                uses.states.push(n.to_vec());
            }
        }
        b"scn" | b"SCN" => {
            if let Some(Object::Name(n)) = op.operands.last() {
                uses.patterns.push(n.clone());
            }
        }
        _ => {}
    }
}

/// For each operator index, whether a text object is open just before it.
fn text_objects(ops: &[Op]) -> Vec<bool> {
    let mut open = false;
    ops.iter()
        .map(|op| {
            let before = open;
            match op.op.as_slice() {
                b"BT" => open = true,
                b"ET" => open = false,
                _ => {}
            }
            before
        })
        .collect()
}

/// For each operator index, whether a path (or a clip waiting for its painting operator) is under
/// construction just before it.
fn open_paths(ops: &[Op]) -> Vec<bool> {
    let mut open = false;
    ops.iter()
        .map(|op| {
            let before = open;
            let o = op.op.as_slice();
            if is_path_construction(o) || matches!(o, b"W" | b"W*") {
                open = true;
            } else if is_path_painting(o) {
                open = false;
            }
            before
        })
        .collect()
}

/// Plan the hidden block from `s` (its `BDC`) to `e` (its `EMC`).
fn hide(ops: &[Op], s: usize, e: usize, text_open: &[bool], path_open: &[bool], text_may_clip: bool, plan: &mut Plan) {
    let inside = |i: usize| ops.get(i).map(|o| o.op.as_slice()).unwrap_or_default();
    let in_text = text_open.get(s).copied().unwrap_or(false);

    // Structure: saves and restores, text objects and paths must stay inside the block.
    let mut depth: i64 = 0;
    let mut text = in_text;
    let mut crosses = path_open.get(s).copied().unwrap_or(false) || path_open.get(e).copied().unwrap_or(false);
    for i in s.saturating_add(1)..e {
        match inside(i) {
            b"q" => depth += 1,
            b"Q" => {
                depth -= 1;
                if depth < 0 {
                    crosses = true;
                }
            }
            b"BT" => {
                if in_text || text {
                    crosses = true;
                }
                text = true;
            }
            b"ET" => {
                if in_text || !text {
                    crosses = true;
                }
                text = false;
            }
            _ => {}
        }
    }
    if depth != 0 || text != in_text {
        crosses = true;
    }
    if crosses {
        plan.issues.push(Rule::HiddenBlockCrossesStructure);
        return;
    }
    let after = ops.get(e.saturating_add(1)..).unwrap_or_default();

    // Hidden text inside a text object that continues with visible text.
    if in_text {
        let block_shows = (s..e).any(|i| is_text_showing(inside(i)));
        let block_moves = (s..e).any(|i| is_text_positioning(inside(i)) || matches!(inside(i), b"'" | b"\""));
        let mut continues_line = false;
        let mut continues_object = false;
        for op in after {
            let o = op.op.as_slice();
            if o == b"ET" {
                break;
            }
            if matches!(o, b"Tj" | b"TJ") && !continues_object {
                continues_line = true;
            }
            if is_text_showing(o) || is_text_positioning(o) {
                continues_object = true;
            }
        }
        if (block_shows && continues_line) || (block_moves && continues_object) {
            plan.issues.push(Rule::HiddenTextMovesVisibleText);
            return;
        }
    }

    // The block's top-level operators: state changes are candidates; everything else goes.
    let mut depth: i64 = 0;
    let mut last: HashMap<Cat, Vec<usize>> = HashMap::new();
    // Categories whose last change in the block was a side effect of a text operator.
    let mut by_text: HashSet<Cat> = HashSet::new();
    // The current top-level path: its construction operators and clipping operators.
    let mut path: Vec<usize> = Vec::new();
    let mut clip_ops: Vec<usize> = Vec::new();
    let mut shows_text = false;
    let clip_live = live(after, Cat::Clip);
    let mut kept: HashSet<usize> = HashSet::new();
    let mut end_path: HashSet<usize> = HashSet::new();
    for i in s.saturating_add(1)..e {
        let o = inside(i);
        if o == b"q" {
            depth += 1;
            continue;
        }
        if o == b"Q" {
            depth -= 1;
            continue;
        }
        if depth > 0 {
            continue;
        }
        if is_text_showing(o) {
            shows_text = true;
        }
        for c in side_effects(o) {
            last.entry(*c).or_default().clear();
            by_text.insert(*c);
        }
        if is_path_construction(o) {
            path.push(i);
        } else if o == b"W" || o == b"W*" {
            clip_ops.push(i);
        } else if is_path_painting(o) {
            if !clip_ops.is_empty() && clip_live {
                kept.extend(path.iter().copied());
                kept.extend(clip_ops.iter().copied());
                end_path.insert(i);
            }
            path.clear();
            clip_ops.clear();
        } else if let Some(c) = sets(o) {
            by_text.remove(&c);
            let entry = last.entry(c).or_default();
            // A colour space resets the colour: only the run from the last space-setting
            // operator on is the colour that follows. Other categories keep their last setter,
            // and composing ones (matrix, ExtGState) keep every setter.
            match c {
                Cat::Fill | Cat::Stroke if matches!(o, b"cs" | b"CS" | b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K") => {
                    entry.clear();
                    entry.push(i);
                }
                Cat::Fill | Cat::Stroke | Cat::Ctm | Cat::State => entry.push(i),
                _ => {
                    entry.clear();
                    entry.push(i);
                }
            }
        }
    }
    // A text operator's side effect that later content uses can't be kept without the text.
    if by_text.iter().any(|c| live(after, *c)) {
        plan.issues.push(Rule::HiddenTextMovesVisibleText);
        return;
    }
    // Hidden text in a clipping render mode adds to the clip at the end of its text object, and
    // viewers differ on whether hidden text does.
    if shows_text && text_may_clip && clip_live {
        plan.issues.push(Rule::HiddenClippingText);
        return;
    }
    for c in ALL {
        if let Some(list) = last.get(&c)
            && !list.is_empty()
            && live(after, c)
        {
            kept.extend(list.iter().copied());
        }
    }
    let mut edits: Vec<(usize, Edit)> = vec![(s, Edit::Delete), (e, Edit::Delete)];
    for i in s.saturating_add(1)..e {
        if end_path.contains(&i) {
            if inside(i) != b"n" {
                edits.push((i, Edit::EndPath));
            }
        } else if !kept.contains(&i) {
            edits.push((i, Edit::Delete));
        }
    }
    plan.kept_state += kept.iter().filter(|i| sets(inside(**i)).is_some()).count();
    plan.kept_clips += end_path.len();
    for &i in &kept {
        if let Some(op) = ops.get(i) {
            note_use(op, &mut plan.uses);
        }
    }
    plan.edits.extend(edits);
    plan.hidden_ranges.push((s, e));
}

/// Apply `plan`'s edits to `data` (whose operators are `ops`), then parse the result again and
/// check it gives exactly the expected operators. `None` if it doesn't.
pub(crate) fn apply(data: &[u8], ops: &[Op], plan: &Plan) -> Option<Vec<u8>> {
    let by_index: HashMap<usize, Edit> = plan.edits.iter().copied().collect();
    let mut hidden = vec![false; ops.len()];
    for &(s, e) in &plan.hidden_ranges {
        for h in hidden.get_mut(s..=e)? {
            *h = true;
        }
    }
    let mut out = Vec::with_capacity(data.len());
    let mut cursor = 0usize;
    let mut expected: Vec<&Op> = Vec::with_capacity(ops.len());
    let end_path = pdfcraft_content::Op::new("n", Vec::new());
    for (i, op) in ops.iter().enumerate() {
        let (start, end) = (op.span.start, op.span.end);
        if start < cursor || end < start {
            return None;
        }
        let within = hidden.get(i).copied().unwrap_or(false);
        // Between two operators of a hidden block: whitespace and comments go with it.
        if within && i > 0 && hidden.get(i - 1).copied().unwrap_or(false) {
            out.push(b' ');
        } else {
            out.extend_from_slice(data.get(cursor..start)?);
        }
        match by_index.get(&i) {
            None => {
                let bytes = data.get(start..end)?;
                if within && bytes.contains(&b'%') {
                    // A comment inside a kept operator of a hidden block: write the operator anew.
                    write_op(op, &mut out);
                } else {
                    out.extend_from_slice(bytes);
                }
                expected.push(op);
            }
            Some(Edit::Delete) => out.push(b' '),
            Some(Edit::EndPath) => {
                out.extend_from_slice(b" n ");
                expected.push(&end_path);
            }
        }
        cursor = end;
    }
    out.extend_from_slice(data.get(cursor..)?);
    let again = parse(&out);
    if again.skipped > 0 || again.ops.len() != expected.len() {
        return None;
    }
    let same = again.ops.iter().zip(&expected).all(|(a, b)| a.op == b.op && a.operands == b.operands && a.inline == b.inline);
    same.then_some(out)
}

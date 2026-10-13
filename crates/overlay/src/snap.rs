//! Snapping reference points to a page's drawn lines (the wizard's Alignment step).
//!
//! A click near a line end, a crossing of two lines, the middle of a straight line or a line
//! itself lands exactly on it, in the page's displayed frame, so the same feature is picked on A
//! and on B whatever the zoom or the hand. Paths are read as `pdfcraft-measure` reads them for
//! the measuring tools (painted subpaths, rectangles, curves flattened to 0.2 units, nested forms
//! and their matrices), with limits sized for dense engineering sheets, and content in a layer
//! the tab hides is left out, as the wizard's pages show it. Text, images, shadings and
//! annotations aren't snapped to.

use std::collections::HashSet;

use pdfcraft_content::{Matrix, Op, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{OverlayError, Point, frame_of_page};

/// Straight segments kept per page (a curve counts once per flattened piece). The densest real
/// sheet tried, a 36 × 24 in structural elevation, has about 42,000.
pub const MAX_SEGMENTS: usize = 400_000;
/// Decoded content read per page, across the page and its forms.
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_STREAMS: usize = 8192;
/// Forms nested in forms.
const MAX_DEPTH: usize = 32;
/// `q` without `Q`.
const MAX_SAVES: usize = 256;
/// Marked-content nesting followed for layers; deeper blocks keep their parent's state.
const MAX_MARKED: usize = 1024;
/// Segments near the pointer tested pairwise for crossings.
const MAX_LOCAL: usize = 256;
/// Curves are flattened until no control point is further than this from the chord.
const FLATNESS: f64 = 0.2;

/// What a snapped point lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// The end of a line or curve, or a rectangle's corner.
    End,
    /// Where two lines cross or meet.
    Intersection,
    /// The middle of a straight line.
    Midpoint,
    /// The nearest point on a line.
    OnLine,
}

/// Which targets a click snaps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapOptions {
    pub ends: bool,
    pub intersections: bool,
    pub midpoints: bool,
    pub on_line: bool,
}

impl Default for SnapOptions {
    /// Ends and crossings: the targets that are the same feature on two revisions. A midpoint
    /// moves when a line is split or extended, and a point along a line isn't fixed at all.
    fn default() -> Self {
        SnapOptions { ends: true, intersections: true, midpoints: false, on_line: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub point: Point,
    pub kind: SnapKind,
    /// From the requested point, in points.
    pub distance: f64,
}

/// A page's drawn lines in its displayed frame (points, y up, origin at the sheet's lower-left).
#[derive(Clone, Debug, Default)]
pub struct Lines {
    segments: Vec<[Point; 2]>,
    /// Ends of lines and curves and rectangle corners (not the vertices of flattened curves).
    ends: Vec<Point>,
    /// Middles of straight segments.
    mids: Vec<Point>,
    /// A limit stopped reading early: only part of the page snaps.
    pub truncated: bool,
    /// Content streams that couldn't be decoded and were skipped.
    pub unreadable: usize,
}

impl Lines {
    pub fn segments(&self) -> &[[Point; 2]] {
        &self.segments
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The target nearest `at` within `tolerance` points: ends, crossings and midpoints first,
    /// the nearest one winning; a point on a line only when none is in reach. `None` when nothing
    /// is, or `at` or `tolerance` isn't a finite, non-negative distance.
    pub fn snap(&self, at: Point, tolerance: f64, options: SnapOptions) -> Option<Snap> {
        if !at.is_finite() || !tolerance.is_finite() || tolerance < 0.0 {
            return None;
        }
        let mut best: Option<Snap> = None;
        let consider = |best: &mut Option<Snap>, p: Point, kind: SnapKind| {
            let distance = dist(at, p);
            if distance <= tolerance && best.is_none_or(|b| distance < b.distance - 1e-9) {
                *best = Some(Snap { point: p, kind, distance });
            }
        };
        if options.ends {
            for &p in &self.ends {
                consider(&mut best, p, SnapKind::End);
            }
        }
        if options.intersections {
            let near: Vec<&[Point; 2]> = self.segments.iter().filter(|[a, b]| dist(at, nearest(at, *a, *b)) <= tolerance).take(MAX_LOCAL).collect();
            for (i, s) in near.iter().enumerate() {
                for t in near.iter().skip(i + 1) {
                    if let Some(p) = crossing(s[0], s[1], t[0], t[1]) {
                        consider(&mut best, p, SnapKind::Intersection);
                    }
                }
            }
        }
        if options.midpoints {
            for &p in &self.mids {
                consider(&mut best, p, SnapKind::Midpoint);
            }
        }
        if options.on_line && best.is_none() {
            for [a, b] in &self.segments {
                consider(&mut best, nearest(at, *a, *b), SnapKind::OnLine);
            }
        }
        best
    }
}

/// The drawn lines of page `page` (0-based) of `doc`, in its displayed frame, leaving out content
/// in the optional content groups `hidden` lists. Reading is lenient: an undecodable stream is
/// skipped and counted, and limits stop it early with `truncated` set.
pub fn lines(doc: &Document, page: usize, hidden: &HashSet<ObjRef>) -> Result<Lines, OverlayError> {
    let (frame, _) = frame_of_page(doc, page)?;
    let all = pdfcraft_model::pages(doc);
    let count = all.len();
    let p = all.get(page).ok_or(OverlayError::NoSuchPage { page, count })?;
    let resources = p.dict.get(b"Resources").map(|o| doc.resolve(o));
    let empty = Dict::new();
    let resources = resources.as_deref().and_then(Object::as_dict).unwrap_or(&empty);
    let mut w = Walker { doc, hidden, lines: Lines::default(), forms: HashSet::new(), bytes: 0, streams: 0 };
    if let Some(contents) = p.dict.get(b"Contents") {
        w.contents(contents, resources, frame.to_display, 0);
    }
    Ok(w.lines)
}

fn dist(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn nearest(p: Point, a: Point, b: Point) -> Point {
    let (vx, vy) = (b.x - a.x, b.y - a.y);
    let len = vx * vx + vy * vy;
    if !len.is_finite() || len <= 1e-18 {
        return a;
    }
    let t = (((p.x - a.x) * vx + (p.y - a.y) * vy) / len).clamp(0.0, 1.0);
    Point::new(a.x + vx * t, a.y + vy * t)
}

/// Where segments ab and cd cross, ends included; parallel segments don't.
fn crossing(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let (rx, ry, sx, sy) = (b.x - a.x, b.y - a.y, d.x - c.x, d.y - c.y);
    let den = rx * sy - ry * sx;
    if !den.is_finite() || den.abs() < 1e-12 {
        return None;
    }
    let (qx, qy) = (c.x - a.x, c.y - a.y);
    let t = (qx * sy - qy * sx) / den;
    let u = (qx * ry - qy * rx) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then(|| Point::new(a.x + t * rx, a.y + t * ry))
}

fn mid(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// The path being built: kept until it is painted, dropped by `n` or when hidden.
#[derive(Default)]
struct Path {
    segments: Vec<[Point; 2]>,
    ends: Vec<Point>,
    mids: Vec<Point>,
    /// Each open subpath's start and current point, for the closing edge a fill adds.
    open: Vec<(Point, Point)>,
    start: Option<Point>,
    current: Option<Point>,
    /// The current subpath's start hasn't been recorded as an end yet.
    start_pending: bool,
}

impl Path {
    fn len(&self) -> usize {
        self.segments.len()
    }

    fn line(&mut self, a: Point, b: Point, straight: bool) {
        if !(a.is_finite() && b.is_finite()) || dist(a, b) <= 1e-9 {
            return;
        }
        self.segments.push([a, b]);
        if straight {
            self.mids.push(mid(a, b));
        }
    }

    fn end(&mut self, p: Point) {
        if self.start_pending {
            self.start_pending = false;
            if let Some(s) = self.start {
                self.ends.push(s);
            }
        }
        if p.is_finite() {
            self.ends.push(p);
        }
    }

    fn move_to(&mut self, p: Point) {
        self.finish_subpath();
        self.start = Some(p);
        self.current = Some(p);
        self.start_pending = true;
    }

    fn finish_subpath(&mut self) {
        if let (Some(s), Some(c)) = (self.start, self.current)
            && dist(s, c) > 1e-9
        {
            self.open.push((s, c));
        }
    }

    fn close(&mut self) {
        if let (Some(s), Some(c)) = (self.start, self.current) {
            self.line(c, s, true);
            self.current = Some(s);
        }
    }

    fn curve(&mut self, [a, b, c, d]: [Point; 4], depth: usize) {
        let flat = dist(b, nearest(b, a, d)).max(dist(c, nearest(c, a, d))) <= FLATNESS;
        if depth >= 12 || flat {
            self.line(a, d, false);
            return;
        }
        let (ab, bc, cd) = (mid(a, b), mid(b, c), mid(c, d));
        let (abc, bcd) = (mid(ab, bc), mid(bc, cd));
        let m = mid(abc, bcd);
        self.curve([a, ab, abc, m], depth + 1);
        self.curve([m, bcd, cd, d], depth + 1);
    }
}

struct Walker<'a> {
    doc: &'a Document,
    hidden: &'a HashSet<ObjRef>,
    lines: Lines,
    /// Forms being drawn, against cycles.
    forms: HashSet<ObjRef>,
    bytes: usize,
    streams: usize,
}

impl Walker<'_> {
    fn full(&self, pending: usize) -> bool {
        self.lines.segments.len().saturating_add(pending) >= MAX_SEGMENTS
    }

    /// A page's `/Contents` (a stream or an array read as one) or a form's stream.
    fn contents(&mut self, object: &Object, resources: &Dict, matrix: Matrix, depth: usize) {
        if depth >= MAX_DEPTH || self.streams >= MAX_STREAMS {
            self.lines.truncated = true;
            return;
        }
        let o = self.doc.resolve(object);
        let parts: Vec<Object> = match &*o {
            Object::Array(a) => a.clone(),
            other => vec![other.clone()],
        };
        let mut data = Vec::new();
        for part in &parts {
            let s = self.doc.resolve(part);
            let Object::Stream(s) = &*s else { continue };
            let room = MAX_BYTES.saturating_sub(self.bytes).saturating_sub(data.len());
            if room == 0 {
                self.lines.truncated = true;
                break;
            }
            match s.decoded_within(room) {
                Ok(d) => {
                    data.extend(d);
                    data.push(b'\n');
                }
                Err(_) => self.lines.unreadable = self.lines.unreadable.saturating_add(1),
            }
        }
        self.bytes = self.bytes.saturating_add(data.len());
        self.streams = self.streams.saturating_add(1);
        self.ops(&parse(&data).ops, resources, matrix, depth);
    }

    /// Whether the optional content `oc` (a group or membership dictionary) is hidden in the tab.
    /// A membership dictionary follows its `/P` policy over its groups; one with a visibility
    /// expression is taken as shown (snapping is a help, never a filter that loses lines).
    fn oc_hidden(&self, oc: &Object) -> bool {
        let r = oc.as_ref();
        let d = self.doc.resolve(oc);
        let Some(d) = d.as_dict() else { return false };
        if d.name(b"Type") != Some(b"OCMD") {
            return r.is_some_and(|r| self.hidden.contains(&r));
        }
        if d.get(b"VE").is_some() {
            return false;
        }
        let groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| self.doc.resolve(g)).as_deref() {
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
            _ => d.get(b"OCGs").and_then(Object::as_ref).into_iter().collect(),
        };
        if groups.is_empty() {
            return false;
        }
        let on = groups.iter().filter(|g| !self.hidden.contains(g)).count();
        match d.name(b"P") {
            Some(b"AllOn") => on < groups.len(),
            Some(b"AnyOff") => on == groups.len(),
            Some(b"AllOff") => on > 0,
            _ => on == 0,
        }
    }

    /// Add a painted path's lines and targets, within the limit.
    fn keep(&mut self, path: Path, hidden: bool) {
        if hidden {
            return;
        }
        let room = MAX_SEGMENTS.saturating_sub(self.lines.segments.len());
        if path.segments.len() > room {
            self.lines.truncated = true;
        }
        self.lines.segments.extend(path.segments.into_iter().take(room));
        self.lines.ends.extend(path.ends.into_iter().take(room.saturating_mul(2)));
        self.lines.mids.extend(path.mids.into_iter().take(room));
    }

    fn ops(&mut self, ops: &[Op], resources: &Dict, initial: Matrix, depth: usize) {
        let mut matrix = initial;
        let mut saves: Vec<Matrix> = Vec::new();
        // Each open marked-content block: whether its content is hidden.
        let mut marked: Vec<bool> = Vec::new();
        // Blocks past the nesting limit, so their ends don't close the ones followed.
        let mut beyond = 0usize;
        let mut path = Path::default();
        let at = |m: &Matrix, x: f64, y: f64| {
            let (x, y) = m.apply(x, y);
            Point::new(x, y)
        };
        for op in ops {
            let hidden = marked.last().copied().unwrap_or(false);
            if self.full(path.len()) {
                // The path that reached the limit is kept as far as it goes: it is most likely
                // painted, and a sheet drawn as one long path would otherwise snap to nothing.
                self.keep(std::mem::take(&mut path), hidden);
                self.lines.truncated = true;
                break;
            }
            match op.op.as_slice() {
                b"q" => {
                    if saves.len() >= MAX_SAVES {
                        self.lines.truncated = true;
                        break;
                    }
                    saves.push(matrix);
                }
                b"Q" => matrix = saves.pop().unwrap_or(initial),
                b"cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        matrix = m.then(&matrix);
                    }
                }
                b"BMC" | b"BDC" => {
                    let mut hide = hidden;
                    if !hide && op.op.as_slice() == b"BDC" && op.name(0) == Some(b"OC") {
                        let props = resources.get(b"Properties").map(|p| self.doc.resolve(p));
                        let oc = match op.operands.get(1) {
                            Some(Object::Name(n)) => props.as_deref().and_then(Object::as_dict).and_then(|p| p.get(n)).cloned(),
                            _ => None,
                        };
                        hide = oc.is_some_and(|oc| self.oc_hidden(&oc));
                    }
                    if marked.len() < MAX_MARKED {
                        marked.push(hide);
                    } else {
                        beyond = beyond.saturating_add(1);
                    }
                }
                b"EMC" => {
                    if beyond > 0 {
                        beyond -= 1;
                    } else {
                        marked.pop();
                    }
                }
                b"m" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        path.move_to(at(&matrix, x, y));
                    }
                }
                b"l" => {
                    if let (Some(a), Some([x, y])) = (path.current, op.nums::<2>()) {
                        let b = at(&matrix, x, y);
                        path.line(a, b, true);
                        path.end(b);
                        path.current = Some(b);
                    }
                }
                b"c" | b"v" | b"y" => {
                    let Some(a) = path.current else { continue };
                    let pts = match op.op.as_slice() {
                        b"c" => op.nums::<6>().map(|[x1, y1, x2, y2, x3, y3]| [a, at(&matrix, x1, y1), at(&matrix, x2, y2), at(&matrix, x3, y3)]),
                        b"v" => op.nums::<4>().map(|[x2, y2, x3, y3]| [a, a, at(&matrix, x2, y2), at(&matrix, x3, y3)]),
                        _ => op.nums::<4>().map(|[x1, y1, x3, y3]| {
                            let d = at(&matrix, x3, y3);
                            [a, at(&matrix, x1, y1), d, d]
                        }),
                    };
                    if let Some(pts) = pts
                        && pts.iter().all(|p| p.is_finite())
                    {
                        path.curve(pts, 0);
                        path.end(pts[3]);
                        path.current = Some(pts[3]);
                    }
                }
                b"h" => path.close(),
                b"re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        path.finish_subpath();
                        let c = [at(&matrix, x, y), at(&matrix, x + w, y), at(&matrix, x + w, y + h), at(&matrix, x, y + h)];
                        for (i, p) in c.iter().enumerate() {
                            if let Some(q) = c.get((i + 1) % 4) {
                                path.line(*p, *q, true);
                            }
                            if p.is_finite() {
                                path.ends.push(*p);
                            }
                        }
                        path.start = Some(c[0]);
                        path.current = Some(c[0]);
                        path.start_pending = false;
                    }
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    let paint = op.op.as_slice();
                    if matches!(paint, b"s" | b"b" | b"b*") {
                        path.close();
                    }
                    if matches!(paint, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*") {
                        // A fill closes every open subpath.
                        path.finish_subpath();
                        for (s, c) in std::mem::take(&mut path.open) {
                            path.line(c, s, true);
                        }
                    }
                    let done = std::mem::take(&mut path);
                    if paint != b"n" {
                        self.keep(done, hidden);
                    }
                }
                b"Do" => {
                    if hidden {
                        continue;
                    }
                    let Some(name) = op.name(0) else { continue };
                    let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o));
                    let Some(entry) = xobjects.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).cloned() else { continue };
                    let r = entry.as_ref();
                    if r.is_some_and(|r| self.forms.contains(&r)) {
                        continue;
                    }
                    let form = self.doc.resolve(&entry);
                    let Object::Stream(s) = &*form else { continue };
                    if s.dict.name(b"Subtype") != Some(b"Form") || s.dict.get(b"OC").is_some_and(|oc| self.oc_hidden(oc)) {
                        continue;
                    }
                    let m = s
                        .dict
                        .get(b"Matrix")
                        .map(|m| self.doc.resolve(m))
                        .and_then(|o| o.as_array().and_then(|a| Matrix::from_operands(a)))
                        .unwrap_or(Matrix::IDENTITY)
                        .then(&matrix);
                    let own = s.dict.get(b"Resources").map(|o| self.doc.resolve(o));
                    let res = own.as_deref().and_then(Object::as_dict).unwrap_or(resources);
                    if let Some(r) = r {
                        self.forms.insert(r);
                    }
                    self.contents(&entry, res, m, depth + 1);
                    if let Some(r) = r {
                        self.forms.remove(&r);
                    }
                }
                _ => {}
            }
        }
    }
}

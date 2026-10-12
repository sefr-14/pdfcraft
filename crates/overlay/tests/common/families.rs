//! The step 3 synthetic suite (OV3.2): the 18 families of the step 3 PRD as source PDFs, their
//! overlays, and for every overlay page the probes the measurements read, with their expected
//! values. Deterministic: the same code always writes the same bytes.
//!
//! Positions are given in each document's displayed frame (points, origin at the bottom left of
//! the page as shown, y up). Source content is drawn in that frame under the inverse of OV2's
//! displayed-frame matrix, so a target drawn at a displayed point is at that point whatever the
//! page's rotation, box origin or unit. Probe positions on the overlay come from the OV2
//! placement, never from the drawing code.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_overlay::{
    Affine, DisplayFrame, LayerStyle, LayeredOutput, Limits, Model, Placement, Point, PointPair, RegistrationNote, Roles, Settings, SheetCase,
    SheetMatch, Side, Similarity, Source, Style, fit, frame, light_tone, luminosity, place, propose,
};

/// Build every overlay through step 4's layered output (`Old` = A, `New` = B, both on) instead of
/// OV3's `compose_page`. The step 4 proof sets it to rerun the OV3 suite with layers (L4).
pub static LAYERED: AtomicBool = AtomicBool::new(false);

use super::measure::Pair;
use super::synth::{self, MM, Page, Res};

/// What a patch should show.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    /// The patch's per-channel median, within `tol` levels.
    Colour { rgb: [f64; 3], tol: f64 },
    /// The patch's per-channel mean (for hatching), within `tol` levels.
    Mean { rgb: [f64; 3], tol: f64 },
    /// Under a black layer colour, the mask value recovered as 1 − P / 255, within `tol` levels.
    Mask { m: f64, tol: f64 },
    /// No single expected value; measured and compared across renderers.
    Report,
}

#[derive(Clone, Debug)]
pub struct Patch {
    pub name: String,
    /// On the output page, in its user space units.
    pub rect: [f64; 4],
    pub expect: Expect,
    pub criterion: &'static str,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    pub side: Side,
    /// Where OV2 puts the target's centre on the output page (user space units).
    pub centre: (f64, f64),
    /// Half the measuring window, in output units.
    pub half: f64,
    /// The other document's target this one must coincide with, if any.
    pub partner: Option<String>,
    /// Whether the target lies on the output page (false: clipped away, must be absent).
    pub on_page: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    /// Identical lines (displacement 0).
    Shared,
    /// B's line displaced by this many mm perpendicular to A's.
    Displaced(f64),
    AOnly,
    BOnly,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub orientation_deg: f64,
    pub width_mm: f64,
    pub kind: LineKind,
    /// Sub-pixel offset index k (offset k × 0.06 pt).
    pub offset: u8,
    /// The cell centre on the output page (units), and the unit normal to the lines (y up).
    pub centre: (f64, f64),
    pub normal: (f64, f64),
    /// How far either side of the centre the profile runs, in output units.
    pub half_len: f64,
}

/// One overlay page and what to measure on it.
#[derive(Clone, Debug)]
pub struct Case {
    pub page: usize,
    /// The source pages this overlay page compares (0-based in the set's A and B files).
    pub a_page: Option<usize>,
    pub b_page: Option<usize>,
    pub id: String,
    pub family: &'static str,
    pub title: String,
    pub pair: Pair,
    pub style: Style,
    pub placement: Placement,
    /// Resolutions to render at, in dpi. Usually 150 and 300.
    pub dpis: Vec<f64>,
    pub patches: Vec<Patch>,
    pub targets: Vec<Target>,
    pub lines: Vec<Line>,
    /// Facts checked without rendering, as (what, passed, detail).
    pub checks: Vec<(String, bool, String)>,
}

/// One file of the suite: the sources, the overlay and its cases.
pub struct Set {
    pub name: &'static str,
    pub a: Arc<Vec<u8>>,
    pub b: Arc<Vec<u8>>,
    pub overlay: Arc<Vec<u8>>,
    pub cases: Vec<Case>,
}

pub const DPIS: [f64; 2] = [150.0, 300.0];

/// Something to draw on a source page, in its displayed frame.
#[derive(Clone, Debug)]
pub enum Draw {
    /// A filled black disc of diameter `size` centred at `at` (four Bézier arcs). Targets are
    /// discs, not squares: Poppler and PDFium snap the edges of axis-aligned rectangles to the
    /// pixel grid, which would move a square's centroid by up to a pixel in any document.
    Disc { at: (f64, f64), size: f64 },
    /// Operators drawn as they are, in displayed coordinates.
    Ops(String),
}

/// A source page: its boxes, rotation and unit, and what to draw.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub media: [f64; 4],
    pub crop: Option<[f64; 4]>,
    pub rotate: i64,
    pub user_unit: Option<f64>,
    pub group: Option<Dict>,
    pub draw: Vec<Draw>,
    pub resources: Vec<(&'static str, String, Res)>,
}

impl Sheet {
    pub fn plain(w: f64, h: f64) -> Sheet {
        Sheet { media: [0.0, 0.0, w, h], crop: None, rotate: 0, user_unit: None, group: None, draw: Vec::new(), resources: Vec::new() }
    }

    /// The displayed frame as pdfcraft-model reads these boxes (crop clipped to media, rotation
    /// to a multiple of 90, units below 1 read as 1).
    pub fn frame(&self) -> DisplayFrame {
        let m = self.media;
        let visible = match self.crop {
            Some(c) => {
                let r = [c[0].max(m[0]), c[1].max(m[1]), c[2].min(m[2]), c[3].min(m[3])];
                if r[2] > r[0] && r[3] > r[1] { r } else { m }
            }
            None => m,
        };
        let rot = self.rotate.rem_euclid(360) / 90 * 90;
        let unit = self.user_unit.filter(|u| *u >= 1.0).map_or(1.0, |u| u.min(75_000.0));
        frame(visible, rot, unit).expect("a valid synthetic sheet")
    }

    /// Displayed point → user space.
    pub fn user(&self, p: (f64, f64)) -> (f64, f64) {
        self.frame().to_display.invert().expect("invertible").apply(p.0, p.1)
    }

    fn page(&self) -> Page {
        let inv = self.frame().to_display.invert().expect("invertible").0;
        let mut body = format!("q {} {} {} {} {} {} cm\n", inv[0], inv[1], inv[2], inv[3], inv[4], inv[5]);
        for d in &self.draw {
            match d {
                Draw::Disc { at, size } => {
                    let (x, y, r) = (at.0, at.1, size / 2.0);
                    let k = r * 0.552_284_749_830_793_6;
                    body.push_str(&format!(
                        "0 g {} {y} m {} {} {} {} {x} {} c {} {} {} {} {} {y} c {} {} {} {} {x} {} c {} {} {} {} {} {y} c f\n",
                        x + r,
                        x + r,
                        y + k,
                        x + k,
                        y + r,
                        y + r,
                        x - k,
                        y + r,
                        x - r,
                        y + k,
                        x - r,
                        x - r,
                        y - k,
                        x - k,
                        y - r,
                        y - r,
                        x + k,
                        y - r,
                        x + r,
                        y - k,
                        x + r
                    ));
                }
                Draw::Ops(s) => {
                    body.push_str(s);
                    body.push('\n');
                }
            }
        }
        body.push('Q');
        Page {
            media: self.media,
            crop: self.crop,
            rotate: self.rotate,
            user_unit: self.user_unit,
            group: self.group.clone(),
            contents: vec![body],
            resources: self.resources.clone(),
        }
    }
}

/// How B is matched to A: the sheet factor and the registration.
#[derive(Clone, Debug)]
pub enum Registration {
    Identity,
    Matrix(Matrix),
    Similarity(Vec<PointPair>),
    Affine(Vec<PointPair>),
}

#[derive(Clone, Debug)]
pub enum SheetChoice {
    Off,
    /// OV2's suggestion (same sheet → off, same shape → its factor).
    Suggested,
    /// The uniform fit of a different-shape pair.
    UniformFit,
}

/// A target to draw and find: (name, side, displayed point on that side's sheet, half window in
/// pt, the partner it must coincide with).
pub type TargetSpec = (String, Side, (f64, f64), f64, Option<String>);

/// One overlay page before it is built.
pub struct Spec {
    pub id: String,
    pub title: String,
    pub a: Option<Sheet>,
    pub b: Option<Sheet>,
    pub sheet: SheetChoice,
    pub registration: Registration,
    pub expand: bool,
    pub pair: Pair,
    pub threshold: f64,
    /// Patches on A's displayed frame: (name, rect, expectation, criterion).
    pub patches: Vec<(String, [f64; 4], Expect, &'static str)>,
    pub targets: Vec<TargetSpec>,
    /// Lines on A's displayed frame (centre and normal in that frame; half length in pt).
    pub lines: Vec<Line>,
    pub dpis: Vec<f64>,
}

impl Spec {
    fn new(id: &str, title: &str, a: Option<Sheet>, b: Option<Sheet>, pair: Pair) -> Spec {
        Spec {
            id: id.into(),
            title: title.into(),
            a,
            b,
            sheet: SheetChoice::Off,
            registration: Registration::Identity,
            expand: false,
            pair,
            threshold: 1.0,
            patches: Vec::new(),
            targets: Vec::new(),
            lines: Vec::new(),
            dpis: DPIS.to_vec(),
        }
    }
}

/// Build a set: one A file and one B file (a page per spec side; a missing side gets no page),
/// and the overlay with one page per spec.
pub fn build(name: &'static str, family: &'static str, specs: Vec<Spec>) -> Set {
    let a_pages: Vec<Page> = specs.iter().filter_map(|s| s.a.as_ref().map(Sheet::page)).collect();
    let b_pages: Vec<Page> = specs.iter().filter_map(|s| s.b.as_ref().map(Sheet::page)).collect();
    let a_bytes = synth::pdf(&a_pages);
    let b_bytes = synth::pdf(&b_pages);
    let (a_doc, b_doc) = (synth::open(&a_bytes), synth::open(&b_bytes));
    let mut out = Document::new_empty();
    let mut layered = LAYERED.load(Ordering::Relaxed).then(|| LayeredOutput::new().expect("a layered output"));
    let mut cases = Vec::new();
    let (mut ai, mut bi) = (0, 0);
    for (page, s) in specs.into_iter().enumerate() {
        let sa = s.a.as_ref().map(|_| {
            ai += 1;
            Source { doc: &a_doc, page: ai - 1 }
        });
        let sb = s.b.as_ref().map(|_| {
            bi += 1;
            Source { doc: &b_doc, page: bi - 1 }
        });
        let ref_sheet = s.a.as_ref().or(s.b.as_ref()).expect("a page has at least one side");
        let fa = s.a.as_ref().unwrap_or(ref_sheet).frame();
        let fb = s.b.as_ref().unwrap_or(ref_sheet).frame();
        // OV2: sheet factor, registration, placement.
        let sheet = match s.sheet {
            SheetChoice::Off => SheetMatch::Off,
            SheetChoice::Suggested => propose(fa.size(), fb.size(), &Settings::default()).expect("proposal").suggested().expect("a suggestion"),
            SheetChoice::UniformFit => {
                let p = propose(fa.size(), fb.size(), &Settings::default()).expect("proposal");
                assert_eq!(p.case, SheetCase::DifferentShape);
                SheetMatch::Scaled { factor: p.fit }
            }
        };
        let k = sheet.transform(fa.size(), fb.size()).expect("sheet transform");
        let (wb, hb) = fb.size();
        let corners = [Point::new(0.0, 0.0), Point::new(wb, hb)].map(|c| c.transformed(&k));
        let limits = Limits { diag_a: fa.diagonal(), diag_b: corners[0].dist(corners[1]), settings: Settings::default() };
        let registration = match &s.registration {
            Registration::Identity => Matrix::IDENTITY,
            Registration::Matrix(m) => *m,
            Registration::Similarity(pairs) => fit::<Similarity>(pairs, &limits).expect("similarity fit").transform.matrix(),
            Registration::Affine(pairs) => fit::<Affine>(pairs, &limits).expect("affine fit").transform.matrix(),
        };
        let placement = place(&fa, &fb, sheet, &registration, s.expand, &Settings::default()).expect("placement");
        let style = Style { colour_a: s.pair.a, colour_b: s.pair.b, threshold: s.threshold };
        match layered.as_mut() {
            Some(l) => {
                let layer_style = LayerStyle { old: s.pair.a, new: s.pair.b, threshold: s.threshold };
                l.add_page(sa, sb, &placement, &layer_style, Roles::default(), RegistrationNote::PLACED).expect("composes with layers");
            }
            None => {
                pdfcraft_overlay::compose_page(&mut out, sa, sb, &placement, &style).expect("composes");
            }
        }

        // A's displayed point → output; B's displayed point → output.
        let to_out = |side: Side, p: (f64, f64)| -> (f64, f64) {
            let (sheet, m) = match side {
                Side::A => (s.a.as_ref().unwrap_or(ref_sheet), placement.a),
                Side::B => (s.b.as_ref().unwrap_or(ref_sheet), placement.b),
            };
            let u = sheet.user(p);
            m.apply(u.0, u.1)
        };
        let page_box = placement.page_box();
        let on_page = |c: (f64, f64), half: f64| c.0 - half >= 0.0 && c.1 - half >= 0.0 && c.0 + half <= page_box[2] && c.1 + half <= page_box[3];
        let patches = s
            .patches
            .iter()
            .map(|(name, r, expect, criterion)| {
                let (x0, y0) = to_out(Side::A, (r[0], r[1]));
                let (x1, y1) = to_out(Side::A, (r[2], r[3]));
                Patch { name: name.clone(), rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)], expect: expect.clone(), criterion }
            })
            .collect();
        let unit_scale = 1.0 / placement.unit;
        let targets = s
            .targets
            .iter()
            .map(|(name, side, p, half, partner)| {
                let c = to_out(*side, *p);
                let half = half * unit_scale;
                Target { name: name.clone(), side: *side, centre: c, half, partner: partner.clone(), on_page: on_page(c, half) }
            })
            .collect();
        let lines = s
            .lines
            .iter()
            .map(|l| {
                let c = to_out(Side::A, l.centre);
                let tip = to_out(Side::A, (l.centre.0 + l.normal.0, l.centre.1 + l.normal.1));
                let (nx, ny) = (tip.0 - c.0, tip.1 - c.1);
                let n = (nx * nx + ny * ny).sqrt();
                Line { centre: c, normal: (nx / n, ny / n), half_len: l.half_len * unit_scale, ..l.clone() }
            })
            .collect();
        cases.push(Case {
            page,
            a_page: sa.map(|x| x.page),
            b_page: sb.map(|x| x.page),
            id: s.id,
            family,
            title: s.title,
            pair: s.pair,
            style,
            placement,
            dpis: s.dpis,
            patches,
            targets,
            lines,
            checks: Vec::new(),
        });
    }
    if let Some(l) = layered {
        out = l.finish().expect("the layered overlay passes its check");
    }
    let overlay = synth::save(&out, true);
    // Facts checked on the written file: each page box is the placement's.
    let written = synth::open(&overlay);
    let pages = pdfcraft_model::pages(&written);
    for (case, p) in cases.iter_mut().zip(&pages) {
        let media: Vec<f64> = written
            .resolve(p.dict.get(b"MediaBox").expect("media box"))
            .as_array()
            .expect("array")
            .iter()
            .map(|o| written.resolve(o).as_f64().expect("number"))
            .collect();
        let want = case.placement.page_box();
        let worst = media.iter().zip(want).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
        case.checks.push(("page box equals the placement's (criterion 9a)".into(), worst <= 1e-9, format!("largest difference {worst:e} pt")));
        let unit = p.dict.get(b"UserUnit").and_then(|u| written.resolve(u).as_f64()).unwrap_or(1.0);
        case.checks.push(("page unit equals the placement's".into(), unit == case.placement.unit, format!("{unit}")));
    }
    Set { name, a: a_bytes, b: b_bytes, overlay, cases }
}

// ── Families ─────────────────────────────────────────────────────────────────────────────────

/// Black ink in A only, in B only, in both, and paper, for the four palette pairs (criterion 1).
pub fn overlap() -> Set {
    let pairs = [
        ("red-cyan", Pair::RED_CYAN),
        ("green-magenta", Pair::GREEN_MAGENTA),
        ("azure-orange", Pair::AZURE_ORANGE),
        ("spring-rose", Pair::SPRING_ROSE),
    ];
    let specs = pairs
        .iter()
        .map(|(n, pair)| {
            let (p1, p2, p3) = ((105.0, 195.0), (215.0, 195.0), (325.0, 195.0));
            let mut a = Sheet::plain(400.0, 300.0);
            a.draw = vec![Draw::Disc { at: p1, size: 90.0 }, Draw::Disc { at: p2, size: 90.0 }];
            let mut b = Sheet::plain(400.0, 300.0);
            b.draw = vec![Draw::Disc { at: p1, size: 90.0 }, Draw::Disc { at: p3, size: 90.0 }];
            let mut s = Spec::new(
                &format!("F01-overlap-{n}"),
                &format!("Overlap, {n}: left both (dark), middle A only, right B only, bottom paper"),
                Some(a),
                Some(b),
                *pair,
            );
            let sq = |c: (f64, f64)| [c.0 - 45.0, c.1 - 45.0, c.0 + 45.0, c.1 + 45.0];
            s.patches = vec![
                ("both".into(), sq(p1), Expect::Colour { rgb: pair.overlap(), tol: 2.0 }, "1"),
                ("A only".into(), sq(p2), Expect::Colour { rgb: pair.predict(1.0, 0.0), tol: 2.0 }, "1"),
                ("B only".into(), sq(p3), Expect::Colour { rgb: pair.predict(0.0, 1.0), tol: 2.0 }, "1"),
                ("paper".into(), sq((105.0, 75.0)), Expect::Colour { rgb: [255.0; 3], tol: 0.0 }, "1"),
            ];
            s
        })
        .collect();
    build("f01-overlap", "1, 18", specs)
}

fn patch_grid(i: usize, cols: usize, top: f64) -> [f64; 4] {
    let (col, row) = (i % cols, i / cols);
    let x = 36.0 + col as f64 * 72.0;
    let y = top - 54.0 - row as f64 * 72.0;
    [x, y, x + 54.0, y + 54.0]
}

fn ops_patch(r: [f64; 4], paint: &str) -> Draw {
    Draw::Ops(format!("q {paint} {} {} {} {} re f Q", r[0], r[1], r[2] - r[0], r[3] - r[1]))
}

fn separation() -> Res {
    let mut f = Dict::new();
    f.set(b"FunctionType".to_vec(), Object::Int(2));
    f.set(b"Domain".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(1)]));
    f.set(b"C0".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(0), Object::Int(0)]));
    f.set(b"C1".to_vec(), Object::Array(vec![Object::Int(0), Object::Real(0.8), Object::Real(0.6), Object::Int(0)]));
    f.set(b"N".to_vec(), Object::Int(1));
    Res::Obj(Object::Array(vec![Object::name("Separation"), Object::name("PdfCraftSpot"), Object::name("DeviceCMYK"), Object::Dict(f)]))
}

/// Filled geometry and coloured objects (family 9).
pub fn colours() -> Set {
    let items: Vec<(&str, &str, Option<[f64; 3]>, &'static str)> = vec![
        ("black", "0 g", Some([0.0, 0.0, 0.0]), "2"),
        ("gray 25%", "0.25 g", Some([0.25; 3]), "2"),
        ("gray 50%", "0.5 g", Some([0.5; 3]), "2"),
        ("gray 75%", "0.75 g", Some([0.75; 3]), "2"),
        ("red", "1 0 0 rg", Some([1.0, 0.0, 0.0]), "9b"),
        ("green", "0 1 0 rg", Some([0.0, 1.0, 0.0]), "9b"),
        ("blue", "0 0 1 rg", Some([0.0, 0.0, 1.0]), "9b"),
        ("cyan", "0 1 1 rg", Some([0.0, 1.0, 1.0]), "9b"),
        ("magenta", "1 0 1 rg", Some([1.0, 0.0, 1.0]), "9b"),
        ("yellow", "1 1 0 rg", Some([1.0, 1.0, 0.0]), "9b"),
        ("rgb 0.2 0.6 0.9", "0.2 0.6 0.9 rg", Some([0.2, 0.6, 0.9]), "9b"),
        ("rgb 0.4 0.1 0.7", "0.4 0.1 0.7 rg", Some([0.4, 0.1, 0.7]), "9b"),
        ("rgb 0.9 0.6 0.1", "0.9 0.6 0.1 rg", Some([0.9, 0.6, 0.1]), "9b"),
        ("cmyk K", "0 0 0 1 k", None, "9 (report)"),
        ("cmyk 50% K", "0 0 0 0.5 k", None, "9 (report)"),
        ("cmyk C", "1 0 0 0 k", None, "9 (report)"),
        ("cmyk M", "0 1 0 0 k", None, "9 (report)"),
        ("cmyk Y", "0 0 1 0 k", None, "9 (report)"),
        ("cmyk rich black", "0.6 0.4 0.4 1 k", None, "9 (report)"),
        ("spot 100%", "/Spot cs 1 scn", None, "9 (report)"),
        ("spot 50%", "/Spot cs 0.5 scn", None, "9 (report)"),
    ];
    let specs = [("red-cyan", Pair::RED_CYAN), ("green-magenta", Pair::GREEN_MAGENTA)]
        .iter()
        .map(|(n, pair)| {
            let mut a = Sheet::plain(612.0, 792.0);
            a.resources.push(("ColorSpace", "Spot".into(), separation()));
            let mut s = Spec::new(
                &format!("F09-colours-{n}"),
                &format!("Coloured fills in A, {n}: each prints in A's colour at 1 − luminosity"),
                None,
                None,
                *pair,
            );
            for (i, (name, paint, rgb, criterion)) in items.iter().enumerate() {
                let r = patch_grid(i, 7, 720.0);
                a.draw.push(ops_patch(r, paint));
                let expect = match rgb {
                    Some(c) => Expect::Colour { rgb: pair.predict(light_tone(1.0, luminosity(*c)).expect("t"), 0.0), tol: 2.0 },
                    None => Expect::Report,
                };
                s.patches.push(((*name).into(), r, expect, criterion));
            }
            s.a = Some(a);
            s
        })
        .collect();
    build("f09-colours", "9, 18", specs)
}

/// Gray hatching, screened backgrounds and light fills, at t = 1 and t = 0.8 (family 10).
pub fn tones() -> Set {
    let mut specs = Vec::new();
    for (n, pair) in [("red-cyan", Pair::RED_CYAN), ("green-magenta", Pair::GREEN_MAGENTA)] {
        for t in [1.0, 0.8] {
            let mut a = Sheet::plain(612.0, 792.0);
            let mut s = Spec::new(
                &format!("F10-tones-{n}-t{t}"),
                &format!("Hatching, screens and light fills, {n}, t = {t}: lighter than t prints as paper"),
                None,
                None,
                pair,
            );
            s.threshold = t;
            let fills: [(&str, &str, [f64; 3]); 6] = [
                ("screen 10%", "0.9 g", [0.9; 3]),
                ("screen 20%", "0.8 g", [0.8; 3]),
                ("screen 30%", "0.7 g", [0.7; 3]),
                ("light yellow", "1 1 0.8 rg", [1.0, 1.0, 0.8]),
                ("light blue", "0.85 0.9 1 rg", [0.85, 0.9, 1.0]),
                ("tinted background", "0.96 g", [0.96; 3]),
            ];
            for (i, (name, paint, rgb)) in fills.iter().enumerate() {
                let r = patch_grid(i, 7, 720.0);
                a.draw.push(ops_patch(r, paint));
                let l = luminosity(*rgb);
                let m = light_tone(t, l).expect("t");
                let expect = if l > t {
                    Expect::Colour { rgb: [255.0; 3], tol: 0.0 }
                } else if (l - t).abs() < 1e-12 {
                    Expect::Colour { rgb: pair.predict(m, 0.0), tol: 1.0 }
                } else {
                    Expect::Colour { rgb: pair.predict(m, 0.0), tol: 2.0 }
                };
                s.patches.push(((*name).into(), r, expect, if l > t { "2 (paper)" } else { "2" }));
            }
            // Hatching: gray 0.4 lines 0.1 mm wide every 1 mm at 45°, clipped to the patch.
            let r = patch_grid(7, 7, 720.0);
            let (w, sp) = (0.1 * MM, 1.0 * MM);
            let step = sp * 2f64.sqrt();
            let mut ops = format!("q {} {} 54 54 re W n 0.4 G {w} w 0 J\n", r[0], r[1]);
            let mut k = -60.0;
            while k < 60.0 {
                let x = r[0] + k * step;
                ops.push_str(&format!("{x} {} m {} {} l S\n", r[1], x + 54.0, r[1] + 54.0));
                k += 1.0;
            }
            ops.push('Q');
            a.draw.push(Draw::Ops(ops));
            let expect = if t == 1.0 { Expect::Mean { rgb: pair.predict((w / sp) * (1.0 - 0.4), 0.0), tol: 2.0 } } else { Expect::Report };
            s.patches.push(("hatching 0.1 mm at 1 mm, gray 0.4".into(), r, expect, if t == 1.0 { "2" } else { "2 (report)" }));
            s.a = Some(a);
            specs.push(s);
        }
    }
    build("f10-tones", "10, 18", specs)
}

fn form_res(content: &str, group: Option<Dict>, resources: Option<Dict>) -> Res {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(612), Object::Int(792)]));
    if let Some(g) = group {
        d.set(b"Group".to_vec(), Object::Dict(g));
    }
    if let Some(r) = resources {
        d.set(b"Resources".to_vec(), Object::Dict(r));
    }
    Res::Stream(d, content.as_bytes().to_vec())
}

fn transparency_group(cs: &str, isolated: bool) -> Dict {
    let mut g = Dict::new();
    g.set(b"S".to_vec(), Object::name("Transparency"));
    g.set(b"CS".to_vec(), Object::name(cs));
    if isolated {
        g.set(b"I".to_vec(), Object::Bool(true));
    }
    g
}

fn gs(entries: &[(&str, Object)]) -> Res {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("ExtGState"));
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v.clone());
    }
    Res::Dict(d)
}

/// Transparency and soft masks in the source (family 11).
pub fn transparency() -> Set {
    let pair = Pair::RED_CYAN;
    let mut a = Sheet::plain(612.0, 792.0);
    let mut s = Spec::new("F11-transparency", "Transparency in A: each patch prints at the coverage of the result on white", None, None, pair);
    let mut half = Dict::new();
    half.set(b"ca".to_vec(), Object::Real(0.5));
    let mut half_states = Dict::new();
    half_states.set(b"H".to_vec(), Object::Dict(half));
    let mut half_res = Dict::new();
    half_res.set(b"ExtGState".to_vec(), Object::Dict(half_states));
    a.resources = vec![
        ("ExtGState", "C25".into(), gs(&[("ca", Object::Real(0.25))])),
        ("ExtGState", "C50".into(), gs(&[("ca", Object::Real(0.5))])),
        ("ExtGState", "C75".into(), gs(&[("ca", Object::Real(0.75))])),
        ("ExtGState", "MB".into(), gs(&[("BM", Object::name("Multiply"))])),
        ("XObject", "LumGroup".into(), form_res("0.5 g 0 0 612 792 re f", Some(transparency_group("DeviceRGB", false)), None)),
        ("XObject", "AlphaGroup".into(), form_res("/H gs 1 0 0 rg 0 0 612 792 re f", Some(transparency_group("DeviceRGB", false)), Some(half_res))),
        ("ExtGState", "SL".into(), gs(&[("SMask", synth::mask_ref("Luminosity", "LumGroup"))])),
        ("ExtGState", "SA".into(), gs(&[("SMask", synth::mask_ref("Alpha", "AlphaGroup"))])),
    ];
    let items: [(&str, String, f64); 6] = [
        ("black at opacity 0.25", "/C25 gs 0 g".into(), 0.25),
        ("black at opacity 0.5", "/C50 gs 0 g".into(), 0.5),
        ("black at opacity 0.75", "/C75 gs 0 g".into(), 0.75),
        ("gray 0.5 multiplied by gray 0.5", String::new(), 0.75),
        ("black through a luminosity soft mask of gray 0.5", "/SL gs 0 g".into(), 0.5),
        ("black through an alpha soft mask of opacity 0.5", "/SA gs 0 g".into(), 0.5),
    ];
    for (i, (name, paint, alpha)) in items.iter().enumerate() {
        let r = patch_grid(i, 7, 720.0);
        if paint.is_empty() {
            a.draw.push(ops_patch(r, "0.5 g"));
            a.draw.push(ops_patch(r, "/MB gs 0.5 g"));
        } else {
            a.draw.push(ops_patch(r, paint));
        }
        s.patches.push(((*name).into(), r, Expect::Colour { rgb: pair.predict(*alpha, 0.0), tol: 2.0 }, "2"));
    }
    s.a = Some(a);

    // A page with its own transparency group (which page_as_form doesn't carry over, R6).
    let mut g = Sheet::plain(612.0, 792.0);
    g.group = Some(transparency_group("DeviceGray", true));
    g.resources = vec![("ExtGState", "C50".into(), gs(&[("ca", Object::Real(0.5))]))];
    let mut sg =
        Spec::new("F11-page-group", "A page with its own DeviceGray isolated group: black at 0.5 and gray 0.5 both print at 0.5", None, None, pair);
    for (i, (name, paint)) in [("black at opacity 0.5", "/C50 gs 0 g"), ("gray 0.5", "0.5 g")].iter().enumerate() {
        let r = patch_grid(i, 7, 720.0);
        g.draw.push(ops_patch(r, paint));
        sg.patches.push(((*name).into(), r, Expect::Colour { rgb: pair.predict(0.5, 0.0), tol: 2.0 }, "2"));
    }
    sg.a = Some(g);
    build("f11-transparency", "11", vec![s, sg])
}

/// A gray image of `blocks` (each 8 × 8 pixels), one row.
fn gray_blocks(values: &[u8]) -> (Dict, Vec<u8>) {
    let (w, h) = (values.len() * 8, 8);
    let mut data = Vec::with_capacity(w * h);
    for _ in 0..h {
        for v in values {
            data.extend(std::iter::repeat_n(*v, 8));
        }
    }
    (synth::image_dict(w, h, "DeviceGray"), data)
}

fn rgb_blocks(values: &[[u8; 3]]) -> (Dict, Vec<u8>) {
    let (w, h) = (values.len() * 8, 8);
    let mut data = Vec::with_capacity(w * h * 3);
    for _ in 0..h {
        for v in values {
            for _ in 0..8 {
                data.extend_from_slice(v);
            }
        }
    }
    (synth::image_dict(w, h, "DeviceRGB"), data)
}

/// A seeded scan-like gray image: off-white paper with noise and a dark line every 16 rows.
/// Returns the image and the median paper value of rows 2 to 13 (between the first two lines).
fn scan_image() -> (Dict, Vec<u8>, u8) {
    let (w, h) = (64, 64);
    let mut rng = synth::Rng(0x5CA0_0003);
    let mut data = Vec::with_capacity(w * h);
    let mut paper = Vec::new();
    for y in 0..h {
        for _ in 0..w {
            let v = if y % 16 == 0 { 26.0 } else { 240.0 + (rng.unit() - 0.5) * 10.0 };
            let v = v.round() as u8;
            if (2..14).contains(&y) {
                paper.push(v);
            }
            data.push(v);
        }
    }
    paper.sort_unstable();
    (synth::image_dict(w, h, "DeviceGray"), data, paper[paper.len() / 2])
}

/// Embedded raster images and a grayscale scan (family 12).
pub fn images() -> Set {
    let pair = Pair::RED_CYAN;
    let gray = [0u8, 64, 128, 191, 255];
    let rgb = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255], [51, 153, 230]];
    let (scan_dict, scan_data, scan_paper) = scan_image();
    let mut specs = Vec::new();
    for t in [1.0, 0.8] {
        let mut a = Sheet::plain(612.0, 792.0);
        let (gd, gdata) = gray_blocks(&gray);
        let (rd, rdata) = rgb_blocks(&rgb);
        let mut stencil = Dict::new();
        stencil.set(b"Type".to_vec(), Object::name("XObject"));
        stencil.set(b"Subtype".to_vec(), Object::name("Image"));
        stencil.set(b"Width".to_vec(), Object::Int(8));
        stencil.set(b"Height".to_vec(), Object::Int(8));
        stencil.set(b"ImageMask".to_vec(), Object::Bool(true));
        stencil.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        a.resources = vec![
            ("XObject", "Gray".into(), Res::Stream(gd, gdata)),
            ("XObject", "Rgb".into(), Res::Stream(rd, rdata)),
            ("XObject", "Stencil".into(), Res::Stream(stencil, vec![0; 8])),
            ("XObject", "Scan".into(), Res::Stream(scan_dict.clone(), scan_data.clone())),
        ];
        let mut s =
            Spec::new(&format!("F12-images-t{t}"), &format!("Images in A at t = {t}: gray and colour blocks, a stencil, a scan"), None, None, pair);
        s.threshold = t;
        // Gray blocks: 5 × 54 pt wide, 54 pt high, at (36, 666).
        a.draw.push(Draw::Ops("q 270 0 0 54 36 666 cm /Gray Do Q".into()));
        for (i, v) in gray.iter().enumerate() {
            let x = 36.0 + i as f64 * 54.0;
            let l = f64::from(*v) / 255.0;
            s.patches.push((
                format!("gray image block {v}"),
                [x, 666.0, x + 54.0, 720.0],
                Expect::Colour { rgb: pair.predict(light_tone(t, l).expect("t"), 0.0), tol: 2.0 },
                "2",
            ));
        }
        a.draw.push(Draw::Ops("q 216 0 0 54 36 576 cm /Rgb Do Q".into()));
        for (i, c) in rgb.iter().enumerate() {
            let x = 36.0 + i as f64 * 54.0;
            let l = luminosity(c.map(|v| f64::from(v) / 255.0));
            s.patches.push((
                format!("rgb image block {c:?}"),
                [x, 576.0, x + 54.0, 630.0],
                Expect::Colour { rgb: pair.predict(light_tone(t, l).expect("t"), 0.0), tol: 2.0 },
                "2",
            ));
        }
        a.draw.push(Draw::Ops("q 0 g 54 0 0 54 36 486 cm /Stencil Do Q".into()));
        s.patches.push((
            "stencil mask painted black".into(),
            [36.0, 486.0, 90.0, 540.0],
            Expect::Colour { rgb: pair.predict(light_tone(t, 0.0).expect("t"), 0.0), tol: 2.0 },
            "2",
        ));
        // The scan, 216 pt square at (36, 216): rows 2 to 13 of 64 are paper.
        a.draw.push(Draw::Ops("q 216 0 0 216 36 216 cm /Scan Do Q".into()));
        let row = 216.0 / 64.0;
        let top = 216.0 + 216.0;
        let paper_rect = [36.0 + 20.0, top - 13.0 * row, 36.0 + 196.0, top - 3.0 * row];
        let l = f64::from(scan_paper) / 255.0;
        let m = light_tone(t, l).expect("t");
        let expect = if l > t { Expect::Colour { rgb: [255.0; 3], tol: 0.0 } } else { Expect::Colour { rgb: pair.predict(m, 0.0), tol: 2.0 } };
        s.patches.push((format!("scan paper (median {scan_paper})"), paper_rect, expect, "2"));
        s.a = Some(a);
        specs.push(s);
    }
    build("f12-images", "12", specs)
}

/// The five target points of a `w × h` displayed sheet: four corners inset 40 pt and the centre.
pub fn target_points(w: f64, h: f64) -> [(f64, f64); 5] {
    [(40.0, 40.0), (w - 40.0, 40.0), (w - 40.0, h - 40.0), (40.0, h - 40.0), (w / 2.0, h / 2.0)]
}

/// A and B with targets at the same displayed points on sheets `a` and `b`; `b_points` maps A's
/// points to where B draws them (identity when B matches A directly).
fn coincide(id: &str, title: &str, mut a: Sheet, mut b: Sheet, b_points: impl Fn((f64, f64)) -> (f64, f64), size_a: f64, size_b: f64) -> Spec {
    let (w, h) = a.frame().size();
    let pts = target_points(w, h);
    for p in pts {
        a.draw.push(Draw::Disc { at: p, size: size_a });
        b.draw.push(Draw::Disc { at: b_points(p), size: size_b });
    }
    let mut s = Spec::new(id, title, Some(a), Some(b), Pair::RED_CYAN);
    for (i, p) in pts.iter().enumerate() {
        s.targets.push((format!("A{i}"), Side::A, *p, size_a.max(12.0), Some(format!("B{i}"))));
        s.targets.push((format!("B{i}"), Side::B, b_points(*p), size_a.max(12.0), Some(format!("A{i}"))));
    }
    s
}

/// Rotated pages and nonzero page-box origins (family 13).
pub fn rotation() -> Set {
    let mut specs = Vec::new();
    for rot in [0, 90, 180, 270] {
        for (ox, oy) in [(0.0, 0.0), (-100.0, 50.0)] {
            let (vw, vh) = if rot % 180 == 0 { (380.0, 280.0) } else { (280.0, 380.0) };
            let mut a = Sheet::plain(0.0, 0.0);
            a.media = [ox, oy, ox + vw + 20.0, oy + vh + 20.0];
            a.crop = Some([ox + 10.0, oy + 10.0, ox + 10.0 + vw, oy + 10.0 + vh]);
            a.rotate = rot;
            let b = Sheet::plain(380.0, 280.0);
            specs.push(coincide(
                &format!("F13-rotate{rot}-origin{ox}_{oy}"),
                &format!("/Rotate {rot}, media origin ({ox}, {oy}), crop inside media: A and B squares coincide"),
                a,
                b,
                |p| p,
                12.0,
                12.0,
            ));
        }
    }
    for rot in [0, 90] {
        // A crop box partly outside the media box: OV2 clips it to the media box.
        let mut a = Sheet::plain(0.0, 0.0);
        let (vw, vh) = if rot == 0 { (380.0, 280.0) } else { (280.0, 380.0) };
        a.media = [0.0, 0.0, vw, vh];
        a.crop = Some([-50.0, 0.0, vw, vh + 30.0]);
        a.rotate = rot;
        specs.push(coincide(
            &format!("F13-rotate{rot}-crop-outside"),
            &format!("/Rotate {rot}, crop box partly outside the media box (clipped to it): squares coincide"),
            a,
            Sheet::plain(380.0, 280.0),
            |p| p,
            12.0,
            12.0,
        ));
    }
    // A /Rotate that isn't a multiple of 90: pdfcraft reads 100 as 90.
    let mut a = Sheet::plain(280.0, 380.0);
    a.rotate = 100;
    specs.push(coincide(
        "F13-rotate100",
        "/Rotate 100 (not a multiple of 90), read as 90 by PdfCraft",
        a,
        Sheet::plain(380.0, 280.0),
        |p| p,
        12.0,
        12.0,
    ));
    build("f13-rotation", "13", specs)
}

/// `/UserUnit` (family 14).
pub fn user_unit() -> Set {
    let mut specs = Vec::new();
    let mut a = Sheet::plain(190.0, 140.0);
    a.user_unit = Some(2.0);
    specs.push(coincide(
        "F14-unit2",
        "A has /UserUnit 2 (190 × 140 units = 380 × 280 pt): squares coincide",
        a,
        Sheet::plain(380.0, 280.0),
        |p| p,
        12.0,
        12.0,
    ));
    let mut a = Sheet::plain(380.0, 280.0);
    a.user_unit = Some(0.5);
    specs.push(coincide(
        "F14-unit0.5",
        "A has /UserUnit 0.5, read as 1 (as Acrobat and PdfCraft do): squares coincide",
        a,
        Sheet::plain(380.0, 280.0),
        |p| p,
        12.0,
        12.0,
    ));
    // An expanded output wider than 14,400 pt gets its own unit. Rendered at 36 dpi: at 150 dpi
    // the page would be 30,000 px wide, past every renderer's limit (step 3 PRD, D6).
    let mut s = coincide(
        "F14-output-unit",
        "B shifted 14,100 pt with expansion: the output page needs /UserUnit",
        Sheet::plain(380.0, 280.0),
        Sheet::plain(380.0, 280.0),
        |p| p,
        40.0,
        40.0,
    );
    s.registration = Registration::Matrix(Matrix::translate(14_100.0, 0.0));
    s.expand = true;
    for t in &mut s.targets {
        t.4 = None;
    }
    s.dpis = vec![36.0];
    specs.push(s);
    build("f14-user-unit", "14", specs)
}

/// Sheet sizes and OV2 placements (family 15).
pub fn sheets() -> Set {
    let mut specs = Vec::new();
    let mut s = coincide(
        "F15-same-sheet",
        "Same sheet within 0.5 pt: 1:1, squares coincide",
        Sheet::plain(400.0, 300.0),
        Sheet::plain(400.3, 299.8),
        |p| p,
        12.0,
        12.0,
    );
    s.sheet = SheetChoice::Suggested;
    specs.push(s);
    let mut s = coincide(
        "F15-same-shape",
        "Same shape, factor 2 (6 × 9 in against 3 × 4.5 in): squares coincide",
        Sheet::plain(432.0, 648.0),
        Sheet::plain(216.0, 324.0),
        |p| (p.0 / 2.0, p.1 / 2.0),
        12.0,
        6.0,
    );
    s.sheet = SheetChoice::Suggested;
    specs.push(s);
    // Different shape: B's points mapped through the inverse of the centred uniform fit.
    let (wa, ha, wb, hb) = (432.0, 648.0, 396.0, 612.0);
    let fit_k = propose((wa, ha), (wb, hb), &Settings::default()).expect("proposal").fit;
    let g = SheetMatch::Scaled { factor: fit_k }.transform((wa, ha), (wb, hb)).expect("transform");
    let gi = g.invert().expect("invertible");
    let mut s = coincide(
        "F15-different-shape",
        "Different shape (6 × 9 in against 5.5 × 8.5 in), uniform fit, centred: squares coincide",
        Sheet::plain(wa, ha),
        Sheet::plain(wb, hb),
        |p| gi.apply(p.0, p.1),
        12.0,
        12.0 / fit_k,
    );
    s.sheet = SheetChoice::UniformFit;
    specs.push(s);
    // Similarity and affine registrations fitted from the targets themselves.
    let th = 0.5f64.to_radians();
    let sim = Matrix([1.002 * th.cos(), 1.002 * th.sin(), -1.002 * th.sin(), 1.002 * th.cos(), 5.0, -3.0]);
    let aff = Matrix([1.0, 0.0, 0.003, 1.0005, 4.0, 2.0]);
    for (name, m, affine) in [("similarity", sim, false), ("affine", aff, true)] {
        let mi = m.invert().expect("invertible");
        let mut s = coincide(
            &format!("F15-{name}"),
            &format!("{name} registration fitted from the five targets: squares coincide"),
            Sheet::plain(432.0, 648.0),
            Sheet::plain(432.0, 648.0),
            |p| mi.apply(p.0, p.1),
            12.0,
            12.0,
        );
        let pairs: Vec<PointPair> = target_points(432.0, 648.0)
            .iter()
            .map(|p| {
                let b = mi.apply(p.0, p.1);
                PointPair::new(Point::new(b.0, b.1), Point::new(p.0, p.1))
            })
            .collect();
        s.registration = if affine { Registration::Affine(pairs) } else { Registration::Similarity(pairs) };
        specs.push(s);
    }
    // Full-size ARCH D against ARCH B. PdfCraft can't render ARCH D whole at 300 dpi (8,192 px a
    // side); the attempt is made and recorded, not skipped.
    let mut s = coincide(
        "F15-arch-d-b",
        "ARCH D against ARCH B, factor 2: squares coincide",
        Sheet::plain(1728.0, 2592.0),
        Sheet::plain(864.0, 1296.0),
        |p| (p.0 / 2.0, p.1 / 2.0),
        24.0,
        12.0,
    );
    s.sheet = SheetChoice::Suggested;
    specs.push(s);
    build("f15-sheets", "15", specs)
}

/// Output-page expansion and clipping (family 16).
pub fn expansion() -> Set {
    let mut specs = Vec::new();
    for expand in [false, true] {
        let mut s = coincide(
            &format!("F16-{}", if expand { "expanded" } else { "clipped" }),
            if expand {
                "B shifted (150, 80) pt with expansion: both sheets whole"
            } else {
                "B shifted (150, 80) pt, no expansion: B clipped at A's sheet"
            },
            Sheet::plain(400.0, 300.0),
            Sheet::plain(400.0, 300.0),
            |p| p,
            12.0,
            12.0,
        );
        s.registration = Registration::Matrix(Matrix::translate(150.0, 80.0));
        s.expand = expand;
        for t in &mut s.targets {
            t.4 = None;
        }
        specs.push(s);
    }
    build("f16-expansion", "16", specs)
}

/// Light-tone ramps under a black layer colour at t = 1, 0.8 and 0.5 (family 17).
pub fn light_tones() -> Set {
    let mut specs = Vec::new();
    for t in [1.0, 0.8, 0.5] {
        let mut ls: Vec<f64> = (0..=10).map(|i| f64::from(i) / 10.0).collect();
        for extra in [t / 2.0, t] {
            if !ls.iter().any(|l| (l - extra).abs() < 1e-9) {
                ls.push(extra);
            }
        }
        ls.sort_by(f64::total_cmp);
        let mut a = Sheet::plain(612.0, 792.0);
        let pair = Pair { a: [0, 0, 0], b: [51, 204, 204] };
        let mut s = Spec::new(&format!("F17-ramp-t{t}"), &format!("Gray ramp under a black layer colour, t = {t}"), None, None, pair);
        s.threshold = t;
        for (i, l) in ls.iter().enumerate() {
            let r = patch_grid(i, 7, 720.0);
            a.draw.push(ops_patch(r, &format!("{l} g")));
            s.patches.push((format!("L = {l}"), r, Expect::Mask { m: light_tone(t, *l).expect("t"), tol: 1.0 }, "6"));
        }
        s.a = Some(a);
        specs.push(s);
    }
    build("f17-light-tones", "17", specs)
}

pub const ORIENTATIONS: [f64; 4] = [0.0, 90.0, 45.0, 15.0];
pub const WIDTHS_MM: [f64; 4] = [0.13, 0.18, 0.25, 0.35];
pub const DISPLACEMENTS_MM: [f64; 7] = [0.0, 0.025, 0.05, 0.1, 0.15, 0.25, 0.5];
/// Sub-pixel offset step: a quarter pixel at 300 dpi, an eighth at 150 dpi.
pub const OFFSET_STEP: f64 = 0.06;

/// The line study: identical, A-only, B-only and displaced lines at every orientation, width,
/// displacement and sub-pixel offset, for both colour pairs (families 1 to 8 and 18).
pub fn lines() -> Set {
    let mut specs = Vec::new();
    // Every cell origin is a multiple of 0.48 pt: a whole pixel at 150 and at 300 dpi.
    let (cell_w, cell_h) = (90.24, 30.24);
    let (x0, y0) = (36.0, 48.0);
    for (pname, pair) in [("red-cyan", Pair::RED_CYAN), ("green-magenta", Pair::GREEN_MAGENTA)] {
        for theta in ORIENTATIONS {
            let mut a = Sheet::plain(792.0, 1224.0);
            let mut b = Sheet::plain(792.0, 1224.0);
            let mut s = Spec::new(
                &format!("F01-08-lines-{pname}-{theta}deg"),
                &format!("Line study, {pname}, {theta}°: rows are width × (0 to 0.5 mm, A only, B only); columns are 8 sub-pixel offsets"),
                None,
                None,
                pair,
            );
            let (dir, n) = ((theta.to_radians().cos(), theta.to_radians().sin()), (-theta.to_radians().sin(), theta.to_radians().cos()));
            let kinds: Vec<LineKind> = DISPLACEMENTS_MM
                .iter()
                .map(|d| if *d == 0.0 { LineKind::Shared } else { LineKind::Displaced(*d) })
                .chain([LineKind::AOnly, LineKind::BOnly])
                .collect();
            let mut row = 0;
            for w_mm in WIDTHS_MM {
                for kind in &kinds {
                    for k in 0..8u8 {
                        let c = (x0 + f64::from(k) * cell_w + 45.12, y0 + f64::from(row) * cell_h + 14.88);
                        let off = f64::from(k) * OFFSET_STEP;
                        let w = w_mm * MM;
                        let seg = |shift: f64| {
                            let (cx, cy) = (c.0 + n.0 * shift, c.1 + n.1 * shift);
                            format!("q 0 G {w} w 0 J {} {} m {} {} l S Q", cx - dir.0 * 10.0, cy - dir.1 * 10.0, cx + dir.0 * 10.0, cy + dir.1 * 10.0)
                        };
                        let d = match kind {
                            LineKind::Displaced(d) => d * MM,
                            _ => 0.0,
                        };
                        if *kind != LineKind::BOnly {
                            a.draw.push(Draw::Ops(seg(off)));
                        }
                        if *kind != LineKind::AOnly {
                            b.draw.push(Draw::Ops(seg(off + d)));
                        }
                        s.lines.push(Line {
                            orientation_deg: theta,
                            width_mm: w_mm,
                            kind: *kind,
                            offset: k,
                            centre: c,
                            normal: n,
                            half_len: d + w + 2.0,
                        });
                    }
                    row += 1;
                }
            }
            s.a = Some(a);
            s.b = Some(b);
            specs.push(s);
        }
    }
    build("f01-08-lines", "1 to 8, 18", specs)
}

/// The whole suite, in a fixed order.
pub fn all() -> Vec<Set> {
    vec![overlap(), colours(), tones(), transparency(), images(), rotation(), user_unit(), sheets(), expansion(), light_tones(), lines()]
}

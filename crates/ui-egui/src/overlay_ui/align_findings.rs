//! OVERLAY.ALIGN.PAGE.1, stage A1 findings (this fork): the full numbers behind the
//! characterisation in `align_characterisation`, over the whole placement transform: sheet sizes
//! and factors, `/Rotate`, CropBox origins and `/UserUnit`.
//!
//! Every case opens the wizard on Old and New documents built from page specifications, sets the
//! sheet choices as a user would, and picks three exact reference points on the first pair (the
//! default's reference pair) from a known correspondence `T₁` (New's displayed sheet → Old's).
//! Generate's plan ([`Wizard::plan`]) then gives each pair its sheet transform `k` and
//! registration `R`, and New's displayed point `p` lands at `C(p)`, `C = k then R`.
//!
//! What `T₁` means on another pair depends on where the misregistration lives. Four readings are
//! measured; each writes `T = pre then X then post` with a pair-independent `X`, so
//! `T_P = pre_P then (pre₁⁻¹ then T₁ then post₁⁻¹) then post_P`:
//!
//! | Reading | the error lives in | pre | post |
//! |---|---|---|---|
//! | Old display | Old's displayed frame, after New's sheet scaling | `k` | identity |
//! | New display | New's displayed frame, before its sheet scaling | identity | `k` |
//! | Old user space | Old's PDF user space (unrotated, uncropped) | `k then D_Old⁻¹` | `D_Old` |
//! | New user space | New's PDF user space | `D_New⁻¹` | `D_New then k` |
//!
//! (`D` is a page's user space → displayed frame transform: `/Rotate`, the visible box's origin
//! and `/UserUnit`.) The wizard's `C` is exactly the Old-display reading. A pair is compatible
//! with the default when all four readings agree on it: then carrying the default is right
//! whatever produced the error. The disagreement is measured at New's sheet corners.
//!
//! With `A1_FINDINGS=<file>` the tables are written there as Markdown.

use std::fmt::Write as _;

use super::*;

/// One page: its MediaBox, optional CropBox, `/Rotate` and `/UserUnit`.
#[derive(Clone, Copy, Debug)]
struct Sheet {
    media: [f64; 4],
    crop: Option<[f64; 4]>,
    rotate: i64,
    unit: Option<f64>,
}

const fn plain(w: f64, h: f64) -> Sheet {
    Sheet { media: [0.0, 0.0, w, h], crop: None, rotate: 0, unit: None }
}

impl Sheet {
    fn describe(&self) -> String {
        let b = |r: [f64; 4]| format!("[{} {} {} {}]", r[0], r[1], r[2], r[3]);
        let mut s = format!("MediaBox {}", b(self.media));
        if let Some(c) = self.crop {
            let _ = write!(s, ", CropBox {}", b(c));
        }
        if self.rotate != 0 {
            let _ = write!(s, ", /Rotate {}", self.rotate);
        }
        if let Some(u) = self.unit {
            let _ = write!(s, ", /UserUnit {u}");
        }
        s
    }
}

fn pdf(pages: &[Sheet]) -> Vec<u8> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
    let b = |r: [f64; 4]| format!("[{} {} {} {}]", r[0], r[1], r[2], r[3]);
    for (i, p) in pages.iter().enumerate() {
        let mut d = format!("<< /Type /Page /Parent 2 0 R /MediaBox {}", b(p.media));
        if let Some(c) = p.crop {
            let _ = write!(d, " /CropBox {}", b(c));
        }
        if p.rotate != 0 {
            let _ = write!(d, " /Rotate {}", p.rotate);
        }
        if let Some(u) = p.unit {
            let _ = write!(d, " /UserUnit {u}");
        }
        let _ = write!(d, " /Contents {} 0 R >>", 4 + 2 * i);
        objs.push(d);
        let body = "0 g 20 20 40 10 re f";
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn wizard(old: &[Sheet], new: &[Sheet]) -> (PdfCraftApp, Wizard) {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").expect("language");
    app.open_bytes("new.pdf", None, pdf(new)).expect("New opens");
    app.open_bytes("old.pdf", None, pdf(old)).expect("Old opens");
    let old_id = app.views.iter().map(|v| v.id).nth(1).expect("Old's tab");
    let w = Wizard::new(&app, old_id);
    assert_eq!(w.matched_rows().len(), old.len().min(new.len()));
    (app, w)
}

fn display(w: &Wizard, side: Side, page: usize) -> pdfcraft_overlay::DisplayFrame {
    let doc = w.snapshot(side).and_then(|s| s.doc.as_ref()).expect("the snapshot reads");
    pdfcraft_overlay::frame_of_page(doc, page).expect("a frame").0
}

fn k_of(w: &mut Wizard, pair: (usize, usize)) -> Matrix {
    let (sa, sb) = w.sheet_sizes(pair).expect("sheet sizes");
    w.sheet_match(pair).transform(sa, sb).expect("sheet transform")
}

fn inv(m: &Matrix) -> Matrix {
    m.invert().expect("invertible")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    OldDisplay,
    NewDisplay,
    OldUser,
    NewUser,
}

const READINGS: [Reading; 4] = [Reading::OldDisplay, Reading::NewDisplay, Reading::OldUser, Reading::NewUser];

impl Reading {
    fn name(self) -> &'static str {
        match self {
            Reading::OldDisplay => "Old display",
            Reading::NewDisplay => "New display",
            Reading::OldUser => "Old user space",
            Reading::NewUser => "New user space",
        }
    }

    /// `(pre, post)` for `pair` under this reading.
    fn parts(self, w: &mut Wizard, (pa, pb): (usize, usize)) -> (Matrix, Matrix) {
        let k = k_of(w, (pa, pb));
        match self {
            Reading::OldDisplay => (k, Matrix::IDENTITY),
            Reading::NewDisplay => (Matrix::IDENTITY, k),
            Reading::OldUser => {
                let d = display(w, Side::A, pa).to_display;
                (k.then(&inv(&d)), d)
            }
            Reading::NewUser => {
                let d = display(w, Side::B, pb).to_display;
                (inv(&d), d.then(&k))
            }
        }
    }

    /// `T_P` from the first pair's `t1`.
    fn carry(self, w: &mut Wizard, first: (usize, usize), t1: &Matrix, pair: (usize, usize)) -> Matrix {
        let (pre1, post1) = self.parts(w, first);
        let x = inv(&pre1).then(t1).then(&inv(&post1));
        let (pre, post) = self.parts(w, pair);
        pre.then(&x).then(&post)
    }
}

fn similarity(degrees: f64, scale: f64, tx: f64, ty: f64) -> Matrix {
    let (s, c) = degrees.to_radians().sin_cos();
    Matrix([scale * c, scale * s, -scale * s, scale * c, tx, ty])
}

/// The misregistration every case uses: 0.3°, 0.2 % larger, 20 pt right, 7 pt up.
fn error() -> Matrix {
    similarity(0.3, 1.002, 20.0, 7.0)
}

fn fmt_m(m: &Matrix) -> String {
    let v: Vec<String> = m.0.iter().map(|x| format!("{x:.6}")).collect();
    format!("[{}]", v.join(" "))
}

fn mm(pt: f64) -> f64 {
    pt / PT_PER_MM
}

/// A control point: its name, where it is on New's displayed sheet, where today's placement
/// puts it on Old, and where each reading says it belongs.
type ControlPoint = (String, (f64, f64), (f64, f64), [(f64, f64); 4]);

/// One pair's measurements.
struct PairFinding {
    pair: (usize, usize),
    old: Sheet,
    new: Sheet,
    old_size: (f64, f64),
    new_size: (f64, f64),
    k: Matrix,
    r: Matrix,
    c: Matrix,
    /// Per control point: (name, New display point, observed on Old, expected per reading).
    points: Vec<ControlPoint>,
}

impl PairFinding {
    /// The worst error of the wizard's placement under `reading`, in points.
    fn worst(&self, reading: Reading) -> f64 {
        let i = READINGS.iter().position(|r| *r == reading).unwrap_or(0);
        self.points.iter().map(|(_, _, o, e)| (o.0 - e[i].0).hypot(o.1 - e[i].1)).fold(0.0, f64::max)
    }

    /// The largest disagreement between any reading and the wizard's placement: zero when the
    /// default is right whatever produced the error.
    fn disagreement(&self) -> f64 {
        READINGS.iter().map(|r| self.worst(*r)).fold(0.0, f64::max)
    }
}

struct Case {
    name: &'static str,
    note: &'static str,
    old: Vec<Sheet>,
    new: Vec<Sheet>,
    setup: fn(&mut Wizard),
}

fn run(case: &Case) -> Vec<PairFinding> {
    let (_app, mut w) = wizard(&case.old, &case.new);
    (case.setup)(&mut w);
    let pairs: Vec<(usize, usize)> = w.matched_rows().into_iter().map(|(_, p)| p).collect();
    let first = pairs[0];
    let t1 = k_of(&mut w, first).then(&error());
    // Three exact points on the first pair, New's at fixed places of its displayed sheet.
    let (_, (wb, hb)) = w.sheet_sizes(first).expect("sizes");
    let picks_at = [(0.1 * wb, 0.1 * hb), (0.9 * wb, 0.15 * hb), (0.5 * wb, 0.9 * hb)];
    let mut picks = Picks::default();
    for &(x, y) in &picks_at {
        let (ax, ay) = t1.apply(x, y);
        assert!(picks.click(Side::A, Point::new(ax, ay), Side::A));
    }
    for &(x, y) in &picks_at {
        assert!(picks.click(Side::B, Point::new(x, y), Side::A));
    }
    w.aligns.insert(first, picks);
    assert_eq!(w.alignment_problem(), None, "{}", case.name);
    assert_eq!(w.sheet_size_problem(), None, "{}", case.name);
    assert_eq!(w.sheets_problem(), None, "{}", case.name);
    let rows = w.pairs.clone();
    let plan = w.plan(&rows).expect("the plan");
    let mut out = Vec::new();
    for (row, pp) in rows.iter().zip(&plan.pairs) {
        let Some(pair) = row.pages() else { continue };
        let (sa, sb) = w.sheet_sizes(pair).expect("sizes");
        let k = pp.sheet.transform(sa, sb).expect("k");
        let c = k.then(&pp.registration);
        let expected: Vec<Matrix> = READINGS.iter().map(|r| r.carry(&mut w, first, &t1, pair)).collect();
        let (wb, hb) = sb;
        let mut points: Vec<(String, (f64, f64))> = vec![
            ("lower left".into(), (0.0, 0.0)),
            ("lower right".into(), (wb, 0.0)),
            ("upper right".into(), (wb, hb)),
            ("upper left".into(), (0.0, hb)),
        ];
        if pair == first {
            for (i, p) in picks_at.iter().enumerate() {
                points.push((format!("pick {}", i + 1), *p));
            }
        }
        let i = pair.0;
        out.push(PairFinding {
            pair,
            old: case.old[i],
            new: case.new[pair.1],
            old_size: sa,
            new_size: sb,
            k,
            r: pp.registration,
            c,
            points: points
                .into_iter()
                .map(|(name, p)| {
                    let e: Vec<(f64, f64)> = expected.iter().map(|m| m.apply(p.0, p.1)).collect();
                    (name, p, c.apply(p.0, p.1), [e[0], e[1], e[2], e[3]])
                })
                .collect(),
        });
    }
    out
}

fn report(case: &Case, findings: &[PairFinding], md: &mut String) {
    let _ = writeln!(md, "\n### {}\n\n{}\n", case.name, case.note);
    for (n, f) in findings.iter().enumerate() {
        let role = if n == 0 { "default's reference pair" } else { "affected pair" };
        let _ = writeln!(md, "**Pair Old p. {} · New p. {} ({role})**\n", f.pair.0 + 1, f.pair.1 + 1);
        let _ = writeln!(md, "- Old: {} → displayed {} × {} pt", f.old.describe(), f.old_size.0, f.old_size.1);
        let _ = writeln!(md, "- New: {} → displayed {} × {} pt", f.new.describe(), f.new_size.0, f.new_size.1);
        let _ = writeln!(md, "- `k` = {}", fmt_m(&f.k));
        let _ = writeln!(md, "- `R` = {}", fmt_m(&f.r));
        let _ = writeln!(md, "- `C = k then R` = {}\n", fmt_m(&f.c));
        let names: Vec<&str> = READINGS.iter().map(|r| r.name()).collect();
        let _ = writeln!(
            md,
            "| Control point (New displayed) | Observed on Old | Expected: {} |\n|---|---|---|---|---|---|",
            names.join(" | Expected: ")
        );
        for (name, p, o, e) in &f.points {
            let cell = |x: (f64, f64)| {
                let d = (o.0 - x.0).hypot(o.1 - x.1);
                format!("({:.3}, {:.3}) Δ {:.3} pt / {:.3} mm", x.0, x.1, d, mm(d))
            };
            let _ = writeln!(
                md,
                "| {name} ({:.1}, {:.1}) | ({:.3}, {:.3}) | {} | {} | {} | {} |",
                p.0,
                p.1,
                o.0,
                o.1,
                cell(e[0]),
                cell(e[1]),
                cell(e[2]),
                cell(e[3])
            );
        }
        let _ = writeln!(
            md,
            "\nWorst error of today's placement — Old display {:.4} pt, New display {:.4} pt ({:.3} mm), Old user space {:.4} pt, New user space {:.4} pt ({:.3} mm). Disagreement between readings: **{:.4} pt ({:.3} mm)**.\n",
            f.worst(Reading::OldDisplay),
            f.worst(Reading::NewDisplay),
            mm(f.worst(Reading::NewDisplay)),
            f.worst(Reading::OldUser),
            f.worst(Reading::NewUser),
            mm(f.worst(Reading::NewUser)),
            f.disagreement(),
            mm(f.disagreement()),
        );
    }
}

fn off(w: &mut Wizard) {
    let _ = w;
}

fn factor_half_shared(w: &mut Wizard) {
    let first = w.first_pair().expect("pair");
    w.sheets.insert(first, SheetState { choice: SheetChoice::Factor(0.5), confirmed: true });
    let pairs: Vec<(usize, usize)> = w.matched_rows().into_iter().map(|(_, p)| p).collect();
    for p in pairs {
        w.allow_different.insert(p);
    }
}

fn per_pair_half_then_one(w: &mut Wizard) {
    w.same_sheet_for_all = false;
    let pairs: Vec<(usize, usize)> = w.matched_rows().into_iter().map(|(_, p)| p).collect();
    w.sheets.insert(pairs[0], SheetState { choice: SheetChoice::Factor(0.5), confirmed: true });
    w.allow_different.insert(pairs[0]);
}

fn matching_off(w: &mut Wizard) {
    w.match_sheets = false;
}

fn cases() -> Vec<Case> {
    let a = plain(300.0, 200.0);
    vec![
        Case {
            name: "C1 Equal sheets",
            note: "Both pairs Old 300 × 200, New 300 × 200; Match sheet sizes on (same sheet, 1:1).",
            old: vec![a, a],
            new: vec![a, a],
            setup: off,
        },
        Case {
            name: "C2 One shared factor 0.5, same sizes",
            note: "Both pairs Old 300 × 200, New 600 × 400; the first pair's 0.5 shared; OV6.9 override ticked.",
            old: vec![a, a],
            new: vec![plain(600.0, 400.0), plain(600.0, 400.0)],
            setup: factor_half_shared,
        },
        Case {
            name: "C3 One shared factor 0.5, other sizes, same proportion",
            note: "Pair 1 Old 300 × 200 / New 600 × 400; pair 2 Old 420 × 280 / New 840 × 560.",
            old: vec![a, plain(420.0, 280.0)],
            new: vec![plain(600.0, 400.0), plain(840.0, 560.0)],
            setup: factor_half_shared,
        },
        Case {
            name: "C4 One shared factor 0.5, pair 2 off its proportion",
            note: "Pair 2 Old 420 × 297 / New 840 × 600: the factor's centring moves New by (0, −1.5) pt.",
            old: vec![a, plain(420.0, 297.0)],
            new: vec![plain(600.0, 400.0), plain(840.0, 600.0)],
            setup: factor_half_shared,
        },
        Case {
            name: "C5 Sheet scale per pair: 0.5 then 1:1",
            note: "Pair 1 Old 300 × 200 / New 600 × 400 at 0.5; pair 2 Old and New 300 × 200 at 1:1; alignment shared.",
            old: vec![a, a],
            new: vec![plain(600.0, 400.0), a],
            setup: per_pair_half_then_one,
        },
        Case {
            name: "C6 Match sheet sizes off, other sizes",
            note: "Pair 1 300 × 200 both; pair 2 420 × 297 both; every pair 1:1 at its lower-left corner.",
            old: vec![a, plain(420.0, 297.0)],
            new: vec![a, plain(420.0, 297.0)],
            setup: matching_off,
        },
        Case {
            name: "C7 New page 2 rotated",
            note: "Same displayed size everywhere; pair 2's New is MediaBox 200 × 300 with /Rotate 90.",
            old: vec![a, a],
            new: vec![a, Sheet { media: [0.0, 0.0, 200.0, 300.0], crop: None, rotate: 90, unit: None }],
            setup: off,
        },
        Case {
            name: "C8 Old page 2 rotated",
            note: "Same displayed size everywhere; pair 2's Old is MediaBox 200 × 300 with /Rotate 270.",
            old: vec![a, Sheet { media: [0.0, 0.0, 200.0, 300.0], crop: None, rotate: 270, unit: None }],
            new: vec![a, a],
            setup: off,
        },
        Case {
            name: "C9 New page 2 cropped elsewhere",
            note: "Same displayed size everywhere; pair 2's New is MediaBox 400 × 300 with CropBox [50 50 350 250].",
            old: vec![a, a],
            new: vec![a, Sheet { media: [0.0, 0.0, 400.0, 300.0], crop: Some([50.0, 50.0, 350.0, 250.0]), rotate: 0, unit: None }],
            setup: off,
        },
        Case {
            name: "C10 New page 2 in /UserUnit 2",
            note: "Same displayed size everywhere; pair 2's New is MediaBox 150 × 100 with /UserUnit 2.",
            old: vec![a, a],
            new: vec![a, Sheet { media: [0.0, 0.0, 150.0, 100.0], crop: None, rotate: 0, unit: Some(2.0) }],
            setup: off,
        },
    ]
}

const EXACT: f64 = 1e-6;

/// Every case measured; the numbers pinned below are what today's wizard does.
#[test]
fn a1_findings() {
    let mut md = String::from("## A1 findings: today's shared alignment, measured\n");
    let mut summary = String::from(
        "\n## Summary\n\n| Case | Affected pair | Old display | New display | Old user space | New user space | Disagreement |\n|---|---|---|---|---|---|---|\n",
    );
    let mut results = Vec::new();
    for case in cases() {
        let f = run(&case);
        // On the reference pair every reading is the truth and the fit reproduces it.
        assert!(f[0].disagreement() < EXACT, "{}: {}", case.name, f[0].disagreement());
        let p = &f[1];
        let _ = writeln!(
            summary,
            "| {} | Old p. {} · New p. {} | {:.4} pt | {:.4} pt ({:.3} mm) | {:.4} pt | {:.4} pt ({:.3} mm) | {:.4} pt ({:.3} mm) |",
            case.name,
            p.pair.0 + 1,
            p.pair.1 + 1,
            p.worst(Reading::OldDisplay),
            p.worst(Reading::NewDisplay),
            mm(p.worst(Reading::NewDisplay)),
            p.worst(Reading::OldUser),
            p.worst(Reading::NewUser),
            mm(p.worst(Reading::NewUser)),
            p.disagreement(),
            mm(p.disagreement()),
        );
        report(&case, &f, &mut md);
        results.push((case.name, READINGS.map(|r| p.worst(r)), p.disagreement()));
    }
    md.push_str(&summary);
    if let Ok(path) = std::env::var("A1_FINDINGS") {
        std::fs::write(&path, &md).expect("write the findings");
    }
    eprintln!("{summary}");
    // Today's placement is the Old-display reading on every pair.
    for (name, worst, _) in &results {
        assert!(worst[0] < EXACT, "{name}: Old display {}", worst[0]);
    }
    // The disagreement on the affected pair, case by case (points; measured, then pinned).
    let pinned = [
        ("C1", 0.0),
        ("C2", 0.0),
        ("C3", 0.0),
        ("C4", 0.008_415),
        ("C5", 21.189_620),
        ("C6", 0.0),
        ("C7", 29.890_283),
        ("C8", 29.867_836),
        ("C9", 0.396_676),
        ("C10", 21.189_620),
    ];
    assert_eq!(results.len(), pinned.len());
    for ((name, _, got), (id, want)) in results.iter().zip(pinned) {
        assert!(name.starts_with(&format!("{id} ")), "{name} is {id}");
        assert!((got - want).abs() < 1e-5, "{name}: {got} pt, pinned {want} pt");
    }
}

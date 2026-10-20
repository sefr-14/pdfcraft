//! Annotations of the sources (OV4.4). Step 4 PRD, T6 and T7:
//!
//! - L7: every drawn annotation is placed with the matrix this test computes on its own from
//!   `/Rect`, `/BBox` and `/Matrix` (Algorithm 8.1), and the overlay renders as the compositing
//!   control does (the source rendered with its annotations, put through the model).
//! - L8: `/CA` 0.5 gives coverage 0.5 without overlapping parts compounding; on pages rotated 0°,
//!   90°, 180° and 270° targets land within 0.5 px at 300 dpi; NoRotate stays upright, pivoting on
//!   the upper-left corner of `/Rect`.
//! - L9: every builder-supported kind without an appearance is drawn, inside its `/Rect`, as the
//!   control drawn from `with_missing_appearances`; unusable appearances are regenerated or
//!   excluded with the reason; text the fonts can't draw is never drawn as `?` (O15); every
//!   annotation ends up drawn, excluded, omitted by decision or unresolved.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::annots::{H, Src, W, ap, with_appearances, without_appearances};
use common::measure::{Image, Pair, centroid};
use common::synth;
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_overlay::{
    AppearanceSource, Exclusion, LayerStyle, Outcome, OverlayError, Placement, Prepared, RegistrationNote, Roles, Rule, Settings, SheetMatch,
    Visibility, check_output, compose_layered, frame_of_page, light_tone, luminosity, place, prepare,
};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

// ── Building sources ─────────────────────────────────────────────────────────────────────────

fn r(n: u32) -> ObjRef {
    ObjRef::new(n, 0)
}

fn prep(doc: &Document, vis: &Visibility) -> Prepared {
    prepare(doc, &[0], vis).expect("prepares")
}

fn placement(doc: &Document) -> Placement {
    let (f, _) = frame_of_page(doc, 0).expect("frame");
    place(&f, &f, SheetMatch::Off, &Matrix::IDENTITY, false, &Settings::default()).expect("placement")
}

fn overlay(doc: &Document, prepared: &Prepared) -> Result<Arc<Vec<u8>>, OverlayError> {
    let out = compose_layered(Some(prepared.source(0)?), None, &placement(doc), &LayerStyle::default(), Roles::default(), RegistrationNote::PLACED)?;
    let bytes = synth::save(&out, true);
    assert!(check_output(&synth::open(&bytes)).is_ok());
    Ok(bytes)
}

fn render(pdf: &Arc<Vec<u8>>, dpi: f64) -> Image {
    let mut r = PageRenderer::new(pdf.clone(), RenderConfig::default());
    let out = r.render(RenderRequest { page: 0, scale: (dpi / 72.0) as f32, ..RenderRequest::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba)
}

/// The compositing control for side A alone: `source` rendered by pdfcraft, each pixel put through
/// the model (m_t of its luminosity, t = 1, in A's layer colour, Old red).
fn control(source: &Arc<Vec<u8>>, dpi: f64) -> Image {
    let img = render(source, dpi);
    let pair = Pair { a: [204, 51, 51], b: [51, 204, 204] };
    let mut rgb = Vec::with_capacity(img.rgb.len());
    for p in img.rgb.chunks(3) {
        let alpha = light_tone(1.0, luminosity([p[0], p[1], p[2]].map(|v| f64::from(v) / 255.0))).unwrap_or(0.0);
        rgb.extend(pair.predict(alpha, 0.0).map(|v| v.round().clamp(0.0, 255.0) as u8));
    }
    Image { width: img.width, height: img.height, rgb }
}

fn max_diff(x: &Image, y: &Image) -> u8 {
    assert_eq!((x.width, x.height), (y.width, y.height));
    x.rgb.iter().zip(&y.rgb).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)
}

fn outcomes(p: &Prepared) -> Vec<(String, Outcome)> {
    p.reports()[0].annotations.iter().map(|e| (e.subtype.clone(), e.outcome.clone())).collect()
}

// ── Independent placement (the test's own Algorithm 8.1) ─────────────────────────────────────

fn apply(m: [f64; 6], (x, y): (f64, f64)) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// PDF Reference 1.7 §8.4.4, Algorithm 8.1, steps 1 and 2: the matrix A that scales and translates
/// the transformed appearance box onto `rect`.
fn algorithm_8_1(bbox: [f64; 4], matrix: [f64; 6], rect: [f64; 4]) -> [f64; 6] {
    let corners = [(bbox[0], bbox[1]), (bbox[0], bbox[3]), (bbox[2], bbox[1]), (bbox[2], bbox[3])].map(|p| apply(matrix, p));
    let xs: Vec<f64> = corners.iter().map(|c| c.0).collect();
    let ys: Vec<f64> = corners.iter().map(|c| c.1).collect();
    let (x0, x1) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
    let (y0, y1) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
    let sx = (rect[2] - rect[0]) / (x1 - x0);
    let sy = (rect[3] - rect[1]) / (y1 - y0);
    [sx, 0.0, 0.0, sy, rect[0] - sx * x0, rect[1] - sy * y0]
}

/// The displayed position (points, y down) of user-space point `p` on a W × H page rotated
/// `rotate` degrees clockwise.
fn displayed(p: (f64, f64), rotate: i64) -> (f64, f64) {
    let (x, y) = p;
    match rotate {
        90 => (y, x),
        180 => (W - x, y),
        270 => (H - y, W - x),
        _ => (x, H - y),
    }
}

/// The page's annotation drawings in the prepared copy: (XObject drawn, cm matrix, through an
/// opacity group).
fn drawings(prepared: &Prepared) -> Vec<(ObjRef, [f64; 6], bool)> {
    let doc = prepared.doc();
    let page = pdfcraft_model::pages(doc)[0].dict.clone();
    let res = doc.dict(page.get(b"Resources").expect("resources")).expect("dict");
    let xo = res.get(b"XObject").and_then(|x| doc.dict(x)).unwrap_or_default();
    let content = synth::page_content(doc, 0);
    let ops = parse(&content).ops;
    let mut out = Vec::new();
    let mut last_cm: Option<[f64; 6]> = None;
    let mut gs = false;
    for op in &ops {
        match op.op.as_slice() {
            b"cm" => last_cm = op.nums::<6>(),
            b"gs" => gs = true,
            b"Q" => {
                last_cm = None;
                gs = false;
            }
            b"Do" => {
                let name = op.name(0).expect("name");
                if !name.starts_with(b"PCAn") {
                    continue;
                }
                let target = xo.get(name).and_then(Object::as_ref).expect("drawn XObject");
                if gs {
                    // The opacity group draws the appearance with the matrix inside.
                    let group = match &*doc.get(target) {
                        Object::Stream(s) => s.clone(),
                        _ => panic!("group form"),
                    };
                    assert!(group.dict.get(b"Group").is_some(), "a transparency group");
                    let inner = parse(&group.decoded().expect("decodes")).ops;
                    let m = inner.iter().find(|o| o.is("cm")).and_then(|o| o.nums::<6>()).expect("matrix");
                    let a = doc
                        .dict(group.dict.get(b"Resources").expect("res"))
                        .and_then(|r| doc.dict(r.get(b"XObject")?))
                        .and_then(|x| x.get(b"A").and_then(Object::as_ref))
                        .expect("appearance");
                    out.push((a, m, true));
                } else {
                    out.push((target, last_cm.expect("a matrix before Do"), false));
                }
            }
            _ => {}
        }
    }
    out
}

/// `/Rect` as written in an annotation body.
fn rect_of(body: &str) -> [f64; 4] {
    let at = body.find("/Rect [").expect("a rectangle") + "/Rect [".len();
    let end = body[at..].find(']').expect("closed") + at;
    let v: Vec<f64> = body[at..end].split_whitespace().map(|x| x.parse().expect("a number")).collect();
    [v[0], v[1], v[2], v[3]]
}

fn close6(a: [f64; 6], b: [f64; 6]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

// ── L7: placement and the control ────────────────────────────────────────────────────────────

/// Annotations with stored appearances of every kind T6 lists, with appearance boxes not at the
/// origin and matrices that scale and turn.
#[test]
fn appearances_are_placed_by_algorithm_8_1() {
    for rotate in [0, 90, 180, 270] {
        let (s, expected) = with_appearances(rotate);
        let numbers = s.numbers();
        let doc = synth::open(&s.bytes());
        let p = prep(&doc, &Visibility::default());
        assert!(p.reports()[0].unresolved.is_empty(), "{:?}", p.reports()[0].unresolved);
        assert!(outcomes(&p).iter().all(|(_, o)| *o == Outcome::Drawn(AppearanceSource::Stored)), "{:?}", outcomes(&p));
        let drawn = drawings(&p);
        assert_eq!(drawn.len(), expected.len());
        for (name, bbox, matrix, rect) in &expected {
            let want_ap = match *name {
                "cb" => numbers["cb-on"],
                "rb" => numbers["rb-off"],
                n => numbers[&format!("{n}-ap")],
            };
            let (_, m, _) = drawn.iter().find(|(a, _, _)| a.num == want_ap).unwrap_or_else(|| panic!("{name} drawn"));
            let want = algorithm_8_1(*bbox, *matrix, *rect);
            assert!(close6(*m, want), "{name} at {rotate}°: {m:?}, expected {want:?}");
        }
        // Nothing interactive is left on the page.
        assert!(!pdfcraft_model::pages(p.doc())[0].dict.contains(b"Annots"));
    }
}

#[test]
fn drawn_annotations_render_as_the_control() {
    for rotate in [0, 90, 180, 270] {
        let (s, _) = with_appearances(rotate);
        let bytes = s.bytes();
        let doc = synth::open(&bytes);
        let p = prep(&doc, &Visibility::default());
        let out = overlay(&doc, &p).expect("composes");
        let d = max_diff(&render(&out, 100.0), &control(&bytes, 100.0));
        assert!(d <= 2, "{rotate}°: {d} levels from the control");
    }
}

// ── L8: opacity, rotation, NoRotate ──────────────────────────────────────────────────────────

#[test]
fn opacity_applies_to_the_appearance_as_a_whole() {
    let mut s = Src::new(0);
    s.content = String::new();
    // Two overlapping black squares, the annotation at 50 % opacity.
    s.obj("sq-ap", ap([0.0, 0.0, 100.0, 60.0], None, "0 g 0 0 60 60 re f 40 0 60 60 re f"));
    s.annot("sq", "<< /Type /Annot /Subtype /Square /Rect [50 50 150 110] /F 4 /CA 0.5 /AP << /N {sq-ap} >> >>");
    let doc = synth::open(&s.bytes());
    let p = prep(&doc, &Visibility::default());
    let drawn = drawings(&p);
    assert_eq!(drawn.len(), 1);
    assert!(drawn[0].2, "drawn through an opacity group");
    let out = overlay(&doc, &p).expect("composes");
    let img = render(&out, 72.0);
    let want = Pair { a: [204, 51, 51], b: [51, 204, 204] }.predict(0.5, 0.0);
    // Displayed y is down: the square spans y 90..150 on screen.
    for (x, y, what) in [(70, 120, "one part"), (100, 120, "the overlap"), (130, 120, "the other part")] {
        let px = img.get(x, y).expect("pixel");
        let err = px.iter().zip(want).map(|(a, b)| (f64::from(*a) - b).abs()).fold(0.0, f64::max);
        assert!(err <= 2.0, "{what}: {px:?}, expected {want:?}");
    }
}

/// A 6 × 6 pt target at (`x`, `y`) in appearance space, in black.
fn target(x: f64, y: f64) -> String {
    format!("0 g {} {} 6 6 re f\n", x - 3.0, y - 3.0)
}

/// The centroid of the A-colour coverage in a box around `at` (pixels).
fn found(img: &Image, at: (f64, f64)) -> (f64, f64) {
    let (x, y) = (at.0.round() as usize, at.1.round() as usize);
    let (cx, cy, _) = centroid(img, x.saturating_sub(40), y.saturating_sub(40), x + 40, y + 40, |p| f64::from(255 - p[1]) / 204.0).expect("ink");
    (cx, cy)
}

#[test]
fn targets_follow_the_page_rotation() {
    let dpi = 300.0;
    let k = dpi / 72.0;
    for rotate in [0, 90, 180, 270] {
        let mut s = Src::new(rotate);
        s.content = String::new();
        let (bbox, matrix, rect) = ([10.0, 10.0, 70.0, 50.0], [1.5, 0.0, 0.0, 1.5, 0.0, 0.0], [100.0, 60.0, 190.0, 120.0]);
        s.obj("a", ap(bbox, Some(matrix), &target(25.0, 40.0)));
        s.annot("t", format!("<< /Type /Annot /Subtype /Square /Rect [{}] /F 4 /AP << /N {{a}} >> >>", rect.map(|v| v.to_string()).join(" ")));
        let doc = synth::open(&s.bytes());
        let p = prep(&doc, &Visibility::default());
        let img = render(&overlay(&doc, &p).expect("composes"), dpi);
        let user = apply(algorithm_8_1(bbox, matrix, rect), apply(matrix, (25.0, 40.0)));
        let (ex, ey) = displayed(user, rotate);
        let (fx, fy) = found(&img, (ex * k, ey * k));
        assert!(
            (fx - ex * k).abs() <= 0.5 && (fy - ey * k).abs() <= 0.5,
            "{rotate}°: found ({fx:.2}, {fy:.2}), expected ({:.2}, {:.2})",
            ex * k,
            ey * k
        );
    }
}

#[test]
fn no_rotate_stays_upright_and_pivots_on_the_upper_left_corner() {
    let dpi = 300.0;
    let k = dpi / 72.0;
    for rotate in [0, 90, 180, 270] {
        let mut s = Src::new(rotate);
        s.content = String::new();
        // Two targets: near the appearance's upper-left and lower-right corners.
        let (bbox, rect) = ([0.0, 0.0, 40.0, 30.0], [120.0, 70.0, 160.0, 100.0]);
        s.obj("a", ap(bbox, None, &format!("{}{}", target(6.0, 24.0), target(34.0, 6.0))));
        s.annot("n", format!("<< /Type /Annot /Subtype /Text /Rect [{}] /F 28 /AP << /N {{a}} >> >>", rect.map(|v| v.to_string()).join(" ")));
        let doc = synth::open(&s.bytes());
        let p = prep(&doc, &Visibility::default());
        let img = render(&overlay(&doc, &p).expect("composes"), dpi);
        // Upright on screen: a target's offset from the upper-left corner of /Rect is the same on
        // screen (x right, y down) as in the unrotated appearance.
        let corner = displayed((rect[0], rect[3]), rotate);
        for (tx, ty) in [(6.0, 24.0), (34.0, 6.0)] {
            let (ux, uy) = apply(algorithm_8_1(bbox, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], rect), (tx, ty));
            let (ex, ey) = (corner.0 + (ux - rect[0]), corner.1 + (rect[3] - uy));
            let (fx, fy) = found(&img, (ex * k, ey * k));
            assert!(
                (fx - ex * k).abs() <= 0.5 && (fy - ey * k).abs() <= 0.5,
                "{rotate}°, target ({tx}, {ty}): found ({fx:.2}, {fy:.2}), expected ({:.2}, {:.2})",
                ex * k,
                ey * k
            );
        }
    }
}

// ── T6: what the screen shows, and layers ────────────────────────────────────────────────────

#[test]
fn screen_rules_and_annotation_layers() {
    let mut s = Src::new(0);
    // "loose" isn't declared and /OFF names it, so viewers disagree (OV4.10, U4); "undeclared"
    // isn't declared either but nothing turns it off, so it shows (U2).
    s.catalog = "/OCProperties << /OCGs [{on} {off}] /D << /OFF [{off} {loose}] >> >>".into();
    s.obj("on", "<< /Type /OCG /Name (Comments) >>");
    s.obj("off", "<< /Type /OCG /Name (Drafts) >>");
    s.obj("loose", "<< /Type /OCG /Name (Loose) >>");
    s.obj("undeclared", "<< /Type /OCG /Name (Undeclared) >>");
    let sq = |name: &str| ap([0.0, 0.0, 20.0, 20.0], None, &format!("0 g 0 0 20 20 re f % {name}"));
    for (name, extra) in [
        ("nozoom", "/Subtype /Text /F 12"),
        ("noprint", "/Subtype /Text /F 0"),
        ("hidden", "/Subtype /Square /F 6"),
        ("noview", "/Subtype /Square /F 36"),
        ("invisible-unknown", "/Subtype /Doodle /F 5"),
        ("invisible-known", "/Subtype /Square /F 5"),
        ("unknown", "/Subtype /Doodle /F 4"),
        ("popup", "/Subtype /Popup /F 4"),
        ("link", "/Subtype /Link /Border [0 0 2] /F 4"),
        ("oc-on", "/Subtype /Square /F 4 /OC {on}"),
        ("oc-off", "/Subtype /Square /F 4 /OC {off}"),
        ("ap-oc-off", "/Subtype /Square /F 4"),
        ("oc-loose", "/Subtype /Square /F 4 /OC {loose}"),
        ("oc-undeclared", "/Subtype /Square /F 4 /OC {undeclared}"),
    ] {
        let apn = format!("{name}-ap");
        s.obj(&apn, sq(name));
        s.annot(name, format!("<< /Type /Annot {extra} /Rect [10 10 30 30] /AP << /N {{{apn}}} >> >>"));
    }
    // An appearance with /OC off, and one with a hidden block inside it.
    s.objects.retain(|(n, _)| n != "ap-oc-off-ap");
    s.obj("ap-oc-off-ap", synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 20 20] /OC {off}", "0 g 0 0 20 20 re f"));
    s.obj(
        "inner-ap",
        synth::raw_stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 20 20] /Resources << /Properties << /D {off} >> >>",
            "0 g 0 0 5 5 re f /OC /D BDC 0.91 0.13 0.57 rg 0 0 20 20 re f EMC",
        ),
    );
    s.annot("inner", "<< /Type /Annot /Subtype /Square /F 4 /Rect [40 10 60 30] /AP << /N {inner-ap} >> >>");
    let numbers = s.numbers();
    let doc = synth::open(&s.bytes());
    let p = prep(&doc, &Visibility::default());
    let got: HashMap<usize, Outcome> = p.reports()[0].annotations.iter().map(|e| (e.index, e.outcome.clone())).collect();
    let drawn = Outcome::Drawn(AppearanceSource::Stored);
    let expect = [
        drawn.clone(),
        drawn.clone(),
        Outcome::Excluded(Exclusion::Hidden),
        Outcome::Excluded(Exclusion::NoView),
        Outcome::Excluded(Exclusion::InvisibleUnknown),
        drawn.clone(),
        drawn.clone(),
        Outcome::Excluded(Exclusion::Popup),
        Outcome::Excluded(Exclusion::Link),
        drawn.clone(),
        Outcome::Excluded(Exclusion::LayerOff),
        Outcome::Excluded(Exclusion::LayerOff),
        Outcome::Unresolved(Rule::NotInOcgs),
        drawn.clone(),
        drawn.clone(),
    ];
    for (i, want) in expect.iter().enumerate() {
        assert_eq!(got.get(&i), Some(want), "annotation {i}");
    }
    // The loose group leaves the page unresolved, naming it; a decision settles it.
    let u = &p.reports()[0].unresolved;
    assert_eq!(u.len(), 1);
    assert_eq!((u[0].rule, u[0].objects[0].1.as_deref()), (Rule::NotInOcgs, Some("Loose")));
    assert!(matches!(overlay(&doc, &p), Err(OverlayError::Unresolved { .. })));
    let decided = prep(&doc, &Visibility { decisions: vec![(r(numbers["loose"]), false)], ..Visibility::default() });
    assert_eq!(decided.reports()[0].annotations[12].outcome, Outcome::Excluded(Exclusion::LayerOff));
    let out = overlay(&doc, &decided).expect("composes");
    let back = synth::open(&out);
    for (_, o) in back.scan_objects() {
        if let Object::Stream(st) = &*o
            && let Ok(d) = st.decoded()
        {
            assert!(!d.windows(14).any(|w| w == b"0.91 0.13 0.57"), "the hidden block inside an appearance is gone");
        }
    }
    // Drawn: two notes, the known Invisible, the unknown kind, the layer that's on, the
    // undeclared one, the inner one.
    assert_eq!(drawings(&decided).len(), 7);
}

// ── L9: missing and unusable appearances ─────────────────────────────────────────────────────

#[test]
fn missing_appearances_are_generated_inside_their_rect() {
    for rotate in [0, 90, 180, 270] {
        let s = without_appearances(rotate);
        let bytes = s.bytes();
        let doc = synth::open(&bytes);
        let p = prep(&doc, &Visibility::default());
        let o = outcomes(&p);
        assert_eq!(o.len(), 16);
        assert!(o.iter().all(|(_, x)| *x == Outcome::Drawn(AppearanceSource::Generated)), "{o:?}");
        let out = overlay(&doc, &p).expect("composes");
        // The control: the source as pdfcraft shows it, with the appearances it draws for display.
        let shown = pdfcraft_annot::with_missing_appearances(&doc).expect("appearances to add");
        let shown = synth::save(&shown, true);
        let d = max_diff(&render(&out, 100.0), &control(&shown, 100.0));
        assert!(d <= 2, "{rotate}°: {d} levels from the control");
    }
    // Ink inside /Rect: one annotation at a time, at 0°, every inked pixel within its rectangle.
    let all = without_appearances(0);
    for name in all.annots.clone() {
        let mut one = Src::new(0);
        one.content = String::new();
        let body = all.objects.iter().find(|(n, _)| *n == name).map(|(_, b)| b.clone()).expect("annotation");
        one.annot(&name, body.clone());
        let doc = synth::open(&one.bytes());
        let p = prep(&doc, &Visibility::default());
        let img = render(&overlay(&doc, &p).expect("composes"), 72.0);
        let rect = rect_of(&body);
        let (x0, x1) = (rect[0].floor() as usize, rect[2].ceil() as usize);
        let (y0, y1) = ((H - rect[3]).floor() as usize, (H - rect[1]).ceil() as usize);
        let mut ink = 0;
        for y in 0..img.height {
            for x in 0..img.width {
                let px = img.get(x, y).expect("pixel");
                if px != [255, 255, 255] {
                    ink += 1;
                    assert!(x + 1 >= x0 && x <= x1 && y + 1 >= y0 && y <= y1, "{name}: ink at ({x}, {y}) outside its rectangle {rect:?}");
                }
            }
        }
        assert!(ink > 0, "{name} drew nothing");
    }
}

#[test]
fn unusable_appearances_are_regenerated_or_excluded() {
    let mut s = Src::new(0);
    s.obj("not-stream", "<< /Foo 1 >>");
    s.obj("zero-box", ap([5.0, 5.0, 5.0, 30.0], None, "0 g 0 0 9 9 re f"));
    s.obj("bad-matrix", synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 20 20] /Matrix [1 0 0 1 (x) 0]", "0 g 0 0 9 9 re f"));
    s.obj("bogus", synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 20 20] /Filter /Bogus", "x"));
    for (i, apn) in ["not-stream", "zero-box", "bad-matrix", "bogus"].iter().enumerate() {
        let x = 10.0 + 40.0 * i as f64;
        let n = if *apn == "not-stream" { "5".to_string() } else { format!("{{{apn}}}") };
        s.annot(&format!("sq{i}"), format!("<< /Type /Annot /Subtype /Square /C [0 0 1] /F 4 /Rect [{x} 100 {} 130] /AP << /N {n} >> >>", x + 30.0));
        s.annot(&format!("odd{i}"), format!("<< /Type /Annot /Subtype /Doodle /F 4 /Rect [{x} 140 {} 170] /AP << /N {n} >> >>", x + 30.0));
    }
    s.annot("sound", "<< /Type /Annot /Subtype /Sound /F 4 /Rect [200 20 220 40] >>");
    s.annot("foreign-stamp", "<< /Type /Annot /Subtype /Stamp /Name /Approved /F 4 /Rect [230 20 290 40] >>");
    s.annot("norect", "<< /Type /Annot /Subtype /Square /F 4 /C [1 0 0] >>");
    let doc = synth::open(&s.bytes());
    let p = prep(&doc, &Visibility::default());
    let o = outcomes(&p);
    for (i, (kind, outcome)) in o.iter().enumerate().take(8) {
        if kind == "Square" {
            assert_eq!(*outcome, Outcome::Drawn(AppearanceSource::Generated), "annotation {i}");
        } else {
            assert!(matches!(outcome, Outcome::Excluded(Exclusion::UnusableAppearance(_))), "annotation {i}: {outcome:?}");
        }
    }
    assert_eq!(o[8].1, Outcome::Excluded(Exclusion::NoAppearance));
    assert_eq!(o[9].1, Outcome::Excluded(Exclusion::NoAppearance), "the screen doesn't draw another program's stamp without artwork");
    assert_eq!(o[10].1, Outcome::Excluded(Exclusion::NoRect));
    assert!(overlay(&doc, &p).is_ok());
}

// ── L9 and O15: text the fonts can't draw ────────────────────────────────────────────────────

#[test]
fn text_the_fonts_cant_draw_is_never_drawn_as_a_question_mark() {
    let mut s = Src::new(0);
    s.catalog = "/AcroForm << /Fields [{field} {marked-field}] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >>".into();
    // A comment without an appearance whose text Helvetica can't draw.
    s.annot("arrow", "<< /Type /Annot /Subtype /FreeText /F 4 /Rect [10 150 110 170] /Contents <FEFF00410020219200420029> /DA (0 g /Helv 10 Tf) >>");
    // A text field without an appearance whose value its font can't draw.
    s.annot("field", "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /F 4 /Rect [10 120 110 140] /V <FEFF65E5672C> >>");
    // O15: appearances PdfCraft drew with ?, marked.
    s.obj("marked-ap", ap([0.0, 0.0, 100.0, 20.0], None, "0 g BT /Helv 10 Tf 2 5 Td (A ? B) Tj ET"));
    s.annot("marked", "<< /Type /Annot /Subtype /FreeText /F 4 /Rect [10 90 110 110] /Contents <FEFF00410020219200420029> /PCUndrawn (\u{2192}) /AP << /N {marked-ap} >> >>");
    s.obj("marked-field-ap", ap([0.0, 0.0, 100.0, 20.0], None, "/Tx BMC BT /Helv 10 Tf 2 5 Td (??) Tj ET EMC"));
    s.annot("marked-field", "<< /Type /Annot /Subtype /Widget /FT /Tx /T (city) /F 4 /Rect [10 60 110 80] /V <FEFF65E5672C> /PCUndrawn <FEFF65E5> /AP << /N {marked-field-ap} >> >>");
    // A real question mark, in an appearance another program drew: drawn.
    s.obj("real-ap", ap([0.0, 0.0, 100.0, 20.0], None, "0 g BT /Helv 10 Tf 2 5 Td (Why?) Tj ET"));
    s.annot("real", "<< /Type /Annot /Subtype /FreeText /F 4 /Rect [10 30 110 50] /Contents (Why?) /AP << /N {real-ap} >> >>");
    let numbers = s.numbers();
    let doc = synth::open(&s.bytes());
    let p = prep(&doc, &Visibility::default());
    let o: Vec<Outcome> = outcomes(&p).into_iter().map(|(_, o)| o).collect();
    assert_eq!(
        o,
        [
            Outcome::Unresolved(Rule::AnnotationUndrawableText),
            Outcome::Unresolved(Rule::AnnotationUndrawableText),
            Outcome::Unresolved(Rule::AnnotationUndrawnText),
            Outcome::Unresolved(Rule::AnnotationUndrawnText),
            Outcome::Drawn(AppearanceSource::Stored),
        ]
    );
    let u = &p.reports()[0].unresolved;
    assert_eq!(u.len(), 4);
    for item in u {
        assert!(item.settleable);
        let detail = item.detail.as_deref().unwrap_or_default();
        assert!(detail.contains("U+2192") || detail.contains("U+65E5"), "{item}");
        assert!(item.objects[0].1.as_deref().is_some_and(|l| l.contains("annotation")), "{item}");
    }
    // Refused, with every item; nothing is written.
    match overlay(&doc, &p) {
        Err(OverlayError::Unresolved { a, .. }) => assert_eq!(&a, u),
        other => panic!("expected Unresolved: {other:?}"),
    }
    // Omitting them is an explicit decision; the report keeps them, and nothing shows `?`.
    let omit = ["arrow", "field", "marked", "marked-field"].map(|n| r(numbers[n])).to_vec();
    let decided = prep(&doc, &Visibility { omit, ..Visibility::default() });
    let o: Vec<Outcome> = outcomes(&decided).into_iter().map(|(_, o)| o).collect();
    assert_eq!(
        o,
        [
            Outcome::Omitted(Some(Rule::AnnotationUndrawableText)),
            Outcome::Omitted(Some(Rule::AnnotationUndrawableText)),
            Outcome::Omitted(Some(Rule::AnnotationUndrawnText)),
            Outcome::Omitted(Some(Rule::AnnotationUndrawnText)),
            Outcome::Drawn(AppearanceSource::Stored),
        ]
    );
    let out = synth::open(&overlay(&doc, &decided).expect("composes"));
    let mut shown = Vec::new();
    for (_, obj) in out.scan_objects() {
        if let Object::Stream(st) = &*obj
            && let Ok(d) = st.decoded()
        {
            shown.extend(
                parse(&d)
                    .ops
                    .into_iter()
                    .filter(|op| op.is("Tj"))
                    .filter_map(|op| op.operands.first().and_then(|o| o.as_string().map(|s| s.bytes.clone()))),
            );
        }
    }
    assert_eq!(shown, vec![b"Why?".to_vec()], "only the real question mark is drawn");
    // An omit decision must name an annotation of the document.
    assert!(matches!(prepare(&doc, &[0], &Visibility { omit: vec![r(4)], ..Visibility::default() }), Err(OverlayError::Decision { .. })));
}

// ── Placement needs the content to end in its initial state ──────────────────────────────────

#[test]
fn annotations_are_drawn_in_the_pages_initial_state() {
    let base = |content: &str| {
        let mut s = Src::new(0);
        s.content = content.into();
        s.obj("a", ap([0.0, 0.0, 20.0, 20.0], None, "0 g 0 0 20 20 re f"));
        s.annot("sq", "<< /Type /Annot /Subtype /Square /F 4 /Rect [100 100 120 120] /AP << /N {a} >> >>");
        s
    };
    // Saves left open and a moved origin: closed before the annotation is drawn.
    let s = base("q 1 0 0 1 50 50 cm q 0 g 0 0 10 10 re f");
    let bytes = s.bytes();
    let doc = synth::open(&bytes);
    let p = prep(&doc, &Visibility::default());
    let ops: Vec<Vec<u8>> = parse(&synth::page_content(p.doc(), 0)).ops.into_iter().map(|o| o.op).collect();
    let tail: Vec<&[u8]> = ops.iter().rev().take(8).rev().map(Vec::as_slice).collect();
    assert_eq!(tail, [&b"f"[..], b"Q", b"Q", b"Q", b"q", b"cm", b"Do", b"Q"], "{ops:?}");
    let d = max_diff(&render(&overlay(&doc, &p).expect("composes"), 72.0), &control(&bytes, 72.0));
    assert!(d <= 2, "{d}");
    // More restores than saves, or a text object left open: the annotation can't be placed
    // reliably.
    for content in ["Q Q 0 g 0 0 10 10 re f", "BT /Helv 10 Tf 10 10 Td (x) Tj"] {
        let doc = synth::open(&base(content).bytes());
        let p = prep(&doc, &Visibility::default());
        assert_eq!(outcomes(&p)[0].1, Outcome::Unresolved(Rule::AnnotationPlacement), "{content}");
        assert_eq!(p.reports()[0].unresolved.iter().map(|u| u.rule).collect::<Vec<_>>(), [Rule::AnnotationPlacement]);
    }
}

// ── No panics ────────────────────────────────────────────────────────────────────────────────

/// Seeded hostile annotations: random kinds, flags, rectangles, appearances (missing, not a
/// stream, degenerate, unreadable, state dictionaries), matrices, opacity, layers, marks and page
/// rotations. Never a panic; a page that resolves composes into an output that passes the
/// defensive check. (OV4.5 runs the full 10,000-case sweep.)
#[test]
fn hostile_annotations_never_panic() {
    let mut rng = common::Rng(0x0154_0004);
    let kinds = [
        "Square",
        "Circle",
        "FreeText",
        "Text",
        "Ink",
        "Line",
        "Polygon",
        "Widget",
        "Link",
        "Popup",
        "Doodle",
        "Sound",
        "3D",
        "Stamp",
        "Redact",
        "Highlight",
    ];
    let rects = ["[10 10 60 40]", "[60 40 10 10]", "[0 0 0 0]", "[1 2 3]", "(x)", "[10 10 1e9 20]", "[-50 -50 400 400]"];
    let aps = [
        "",
        "/AP << /N {a} >>",
        "/AP << /N 5 >>",
        "/AP << /N << /On {a} >> /AS /On >>",
        "/AP << /N << /On {a} >> >>",
        "/AP << /N {z} >>",
        "/AP << /N {b} >>",
        "/AP {a}",
    ];
    let extras = [
        "",
        "/CA 0.5",
        "/CA -3",
        "/ca 0.2 /BM /Multiply",
        "/OC {g}",
        "/OC {m}",
        "/OC 99 0 R",
        "/PCUndrawn (x)",
        "/Contents <FEFF2192>",
        "/Contents (ok) /DA (0 g /Helv 9 Tf)",
        "/QuadPoints [10 40 60 40 10 10 60 10]",
        "/InkList [[10 10 20 20]]",
        "/L [10 10 50 30]",
        "/Vertices [10 10 40 10 25 30]",
        "/FT /Tx /V (v)",
        "/FT /Btn /AS /On",
    ];
    let (mut composed, mut unresolved, mut refused) = (0, 0, 0);
    for i in 0..1_000 {
        let started = std::time::Instant::now();
        let mut s = Src::new([0, 90, 180, 270, 45][(rng.next_u64() % 5) as usize]);
        if rng.next_u64().is_multiple_of(3) {
            s.content = ["q", "Q Q", "BT", "q 1 0 0 1 9 9 cm"][(rng.next_u64() % 4) as usize].into();
        }
        if rng.next_u64().is_multiple_of(2) {
            s.catalog = "/OCProperties << /OCGs [{g}] /D << /OFF [{g}] >> >> /AcroForm << /NeedAppearances true >>".into();
        }
        s.obj("g", "<< /Type /OCG /Name (G) >>");
        s.obj("m", "<< /Type /OCMD /OCGs [] >>");
        let matrix = ["", "/Matrix [0 1 -1 0 0 0]", "/Matrix [1 0 0 1 (x) 0]", "/Matrix [0 0 0 0 0 0]", "/Matrix [1e30 0 0 1e30 0 0]"]
            [(rng.next_u64() % 5) as usize];
        let bbox = ["[0 0 20 20]", "[0 0 0 0]", "[5 5]", "[-1e9 0 1e9 1]"][(rng.next_u64() % 4) as usize];
        let content = ["0 g 0 0 9 9 re f", "/OC /P BDC 0 g 0 0 9 9 re f EMC", "q", ")", "/X Do", "BT (x) Tj ET"][(rng.next_u64() % 6) as usize];
        s.obj(
            "a",
            synth::raw_stream(
                &format!("/Type /XObject /Subtype /Form /BBox {bbox} {matrix} /Resources << /Properties << /P {{g}} >> /XObject << /X {{a}} >> >>"),
                content,
            ),
        );
        s.obj("z", ap([0.0, 0.0, 0.0, 0.0], None, "0 g 0 0 1 1 re f"));
        s.obj("b", synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Filter /Bogus", "x"));
        let n = 1 + (rng.next_u64() % 5) as usize;
        for k in 0..n {
            let kind = kinds[(rng.next_u64() % kinds.len() as u64) as usize];
            let flags = [0, 1, 2, 4, 16, 32, 5, 20][(rng.next_u64() % 8) as usize];
            let rect = rects[(rng.next_u64() % rects.len() as u64) as usize];
            let apx = aps[(rng.next_u64() % aps.len() as u64) as usize];
            let extra = extras[(rng.next_u64() % extras.len() as u64) as usize];
            s.annot(&format!("x{k}"), format!("<< /Type /Annot /Subtype /{kind} /F {flags} /Rect {rect} {apx} {extra} >>"));
        }
        let Ok(doc) = Document::open(s.bytes()) else { continue };
        let numbers = s.numbers();
        let omit = if rng.next_u64().is_multiple_of(4) { vec![r(numbers["x0"])] } else { Vec::new() };
        match prepare(&doc, &[0], &Visibility { omit, ..Visibility::default() }) {
            Err(_) => refused += 1,
            Ok(p) => {
                let entries = p.reports()[0].annotations.len();
                assert_eq!(entries, n, "case {i}: every annotation accounted for");
                if p.reports()[0].unresolved.is_empty() {
                    match compose_layered(
                        Some(p.source(0).expect("page")),
                        None,
                        &placement(&doc),
                        &LayerStyle::default(),
                        Roles::default(),
                        RegistrationNote::PLACED,
                    ) {
                        Ok(out) => {
                            composed += 1;
                            assert!(check_output(&out).is_ok(), "case {i}");
                        }
                        Err(_) => refused += 1,
                    }
                } else {
                    unresolved += 1;
                }
            }
        }
        assert!(!doc.is_modified());
        assert!(started.elapsed().as_secs_f64() < 2.0, "case {i} took {:?}", started.elapsed());
    }
    assert!(composed > 100 && unresolved > 100, "{composed} composed, {unresolved} unresolved, {refused} refused");
}

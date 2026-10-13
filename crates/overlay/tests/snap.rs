//! Snapping reference points to drawn lines (the Alignment step): line ends, crossings, midpoints
//! and points on a line, in the displayed frame; forms, matrices and `/Rotate`; layers hidden in
//! the tab; limits and malformed content.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef};
use pdfcraft_overlay::snap::{Lines, MAX_SEGMENTS, SnapKind, SnapOptions, lines};
use pdfcraft_overlay::{Point, frame_of_page};

/// A one-page PDF from numbered objects (1 the catalog, 2 the pages, 3 the page); a `stream`
/// entry is (dictionary, data).
fn pdf(objects: &[Obj]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        match o {
            Obj::Dict(d) => out.extend_from_slice(d.as_bytes()),
            Obj::Stream(d, data) => {
                out.extend_from_slice(format!("<< {d} /Length {} >>\nstream\n", data.len()).as_bytes());
                out.extend_from_slice(data);
                out.extend_from_slice(b"\nendstream");
            }
        }
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("synthetic PDF opens")
}

enum Obj {
    Dict(String),
    Stream(String, Vec<u8>),
}

/// Catalog, pages and page 3 with `page` entries, its content as object 4, then `more`.
fn page(page: &str, content: &str, more: Vec<Obj>) -> Document {
    let mut objs = vec![
        Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Obj::Dict(format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R {page} >>")),
        Obj::Stream(String::new(), content.as_bytes().to_vec()),
    ];
    objs.extend(more);
    pdf(&objs)
}

fn read(doc: &Document) -> Lines {
    lines(doc, 0, &HashSet::new()).expect("reads")
}

fn snap(l: &Lines, x: f64, y: f64, tolerance: f64, options: SnapOptions) -> Option<(SnapKind, f64, f64)> {
    l.snap(Point::new(x, y), tolerance, options).map(|s| (s.kind, s.point.x, s.point.y))
}

fn close(a: Option<(SnapKind, f64, f64)>, kind: SnapKind, x: f64, y: f64) {
    let Some((k, px, py)) = a else { panic!("no snap; expected {kind:?} at ({x}, {y})") };
    assert_eq!(k, kind, "at ({px}, {py})");
    assert!((px - x).abs() < 1e-9 && (py - y).abs() < 1e-9, "{kind:?} at ({px}, {py}), expected ({x}, {y})");
}

const ALL: SnapOptions = SnapOptions { ends: true, intersections: true, midpoints: true, on_line: true };

/// Two stroked lines under a `cm`, on a page whose crop box starts at (50, 50): every target is
/// in the displayed frame, where the crop box's lower-left corner is (0, 0).
#[test]
fn ends_crossings_midpoints_and_lines_in_the_displayed_frame() {
    // In user space after the cm: (110, 100)–(250, 100) and (160, 50)–(160, 150).
    let doc = page("/MediaBox [0 0 400 300] /CropBox [50 50 350 250]", "1 0 0 1 10 0 cm 100 100 m 240 100 l S 150 50 m 150 150 l S", vec![]);
    let l = read(&doc);
    assert_eq!(l.segments().len(), 2);
    assert!(!l.truncated && l.unreadable == 0);
    let d = SnapOptions::default();
    // The crossing, (160, 100) in user space.
    close(snap(&l, 112.0, 52.0, 5.0, d), SnapKind::Intersection, 110.0, 50.0);
    // A line's end, (110, 100).
    close(snap(&l, 61.0, 49.0, 5.0, d), SnapKind::End, 60.0, 50.0);
    // Out of reach: nothing, and the click stays where it was.
    assert_eq!(snap(&l, 80.0, 80.0, 5.0, ALL), None);
    // The horizontal line's middle, (180, 100), only when midpoints are on.
    assert_eq!(snap(&l, 131.0, 51.0, 3.0, d), None);
    close(snap(&l, 131.0, 51.0, 3.0, ALL), SnapKind::Midpoint, 130.0, 50.0);
    // A point on a line, when nothing else is in reach and the option is on.
    assert_eq!(snap(&l, 150.0, 52.0, 5.0, d), None);
    close(snap(&l, 150.0, 52.0, 5.0, ALL), SnapKind::OnLine, 150.0, 50.0);
    // The nearest discrete target wins: the end (60, 50) over the crossing 50 pt away.
    close(snap(&l, 62.0, 50.0, 60.0, d), SnapKind::End, 60.0, 50.0);
    // Bad requests snap to nothing.
    assert_eq!(snap(&l, f64::NAN, 0.0, 5.0, ALL), None);
    assert_eq!(snap(&l, 60.0, 50.0, f64::INFINITY, ALL), None);
    assert_eq!(snap(&l, 60.0, 50.0, -1.0, ALL), None);
}

/// A rectangle drawn by a form with its own matrix, on a page turned 90°: its corners snap where
/// the displayed frame shows them, and a filled path gains its closing edge.
#[test]
fn forms_matrices_and_rotation() {
    let doc = page(
        "/MediaBox [0 0 400 300] /Rotate 90 /Resources << /XObject << /F 5 0 R >> >>",
        "q 1 0 0 1 30 40 cm /F Do Q 0 0 m 100 0 l 100 50 l f",
        vec![Obj::Stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [2 0 0 2 0 0]".into(), b"10 10 20 20 re S".to_vec())],
    );
    let l = read(&doc);
    // Four edges of the rectangle, two lines and the fill's closing edge.
    assert_eq!(l.segments().len(), 7);
    let (frame, _) = frame_of_page(&doc, 0).unwrap();
    // The rectangle's corner (10, 10) in the form: (20, 20) under its matrix, (50, 60) on the page.
    let (x, y) = frame.to_display.apply(50.0, 60.0);
    close(snap(&l, x + 1.0, y - 1.0, 4.0, SnapOptions::default()), SnapKind::End, x, y);
    // The fill's closing edge from (100, 50) back to (0, 0): its middle, on with midpoints.
    let (x, y) = frame.to_display.apply(50.0, 25.0);
    close(snap(&l, x + 0.5, y + 0.5, 2.0, ALL), SnapKind::Midpoint, x, y);
}

/// Content in a layer the tab hides isn't snapped to, whether it is marked in the content, drawn
/// by a form with its own `/OC`, or governed by a membership dictionary; shown layers are.
#[test]
fn layers_hidden_in_the_tab_are_left_out() {
    let content = "/OC /L1 BDC 0 0 m 100 0 l S EMC /OC /L2 BDC 0 50 m 100 50 l S EMC /OC /M BDC 0 80 m 100 80 l S EMC /F Do /OC /Nope BDC 0 90 m 100 90 l S EMC";
    let doc = page(
        "/MediaBox [0 0 200 200] /Resources << /Properties << /L1 5 0 R /L2 6 0 R /M 7 0 R >> /XObject << /F 8 0 R >> >>",
        content,
        vec![
            Obj::Dict("<< /Type /OCG /Name (L1) >>".into()),
            Obj::Dict("<< /Type /OCG /Name (L2) >>".into()),
            Obj::Dict("<< /Type /OCMD /OCGs [5 0 R 6 0 R] /P /AllOn >>".into()),
            Obj::Stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /OC 5 0 R".into(), b"0 120 m 100 120 l S".to_vec()),
        ],
    );
    let ys = |l: &Lines| -> Vec<f64> { l.segments().iter().map(|s| s[0].y).collect() };
    // Everything shown: five lines, the undefined marker's included (it isn't a layer).
    assert_eq!(ys(&read(&doc)), [0.0, 50.0, 80.0, 120.0, 90.0]);
    // L1 hidden: its line, the form in L1 and the membership needing both are left out.
    let hidden: HashSet<ObjRef> = [ObjRef::new(5, 0)].into();
    assert_eq!(ys(&lines(&doc, 0, &hidden).unwrap()), [50.0, 90.0]);
}

/// Limits stop reading without failing: a page with more segments than the cap keeps the first
/// `MAX_SEGMENTS`, a form that draws itself is followed once, unbalanced markers and an
/// undecodable stream are tolerated, and the rest of the page still snaps.
#[test]
fn limits_cycles_and_malformed_content() {
    let mut big = String::from("0 0 m");
    for i in 0..MAX_SEGMENTS + 10 {
        big.push_str(if i % 2 == 0 { " 1 0 l" } else { " 0 0 l" });
    }
    big.push_str(" S");
    let l = read(&page("/MediaBox [0 0 10 10]", &big, vec![]));
    assert!(l.truncated);
    assert_eq!(l.segments().len(), MAX_SEGMENTS);

    let doc = page(
        "/MediaBox [0 0 200 200] /Resources << /XObject << /F 5 0 R >> >>",
        "EMC EMC /F Do /OC /X BDC 10 10 m 20 10 l S",
        vec![Obj::Stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources << /XObject << /F 5 0 R >> >>".into(),
            b"/F Do 0 30 m 50 30 l S".to_vec(),
        )],
    );
    let l = read(&doc);
    assert_eq!(l.segments().len(), 2, "{:?}", l.segments());
    assert!(!l.truncated);

    let objs = vec![
        Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Obj::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents [4 0 R 5 0 R] >>".into()),
        Obj::Stream("/Filter /NoSuchFilter".into(), b"0 0 m 10 0 l S".to_vec()),
        Obj::Stream(String::new(), b"0 0 m 50 50 l S".to_vec()),
    ];
    let l = read(&pdf(&objs));
    assert_eq!(l.unreadable, 1);
    close(snap(&l, 49.0, 49.0, 3.0, SnapOptions::default()), SnapKind::End, 50.0, 50.0);
    // A page that doesn't exist is an error, not a panic.
    assert!(lines(&pdf(&objs), 3, &HashSet::new()).is_err());
}

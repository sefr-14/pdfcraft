//! Annotated sources for step 4 (T6 to T9): built here so the annotation tests and the proof
//! use the same files.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use super::synth;

/// Page size used throughout: not square, so a rotation shows.
pub const W: f64 = 300.0;
pub const H: f64 = 200.0;

/// A one-page source: 1 catalog, 2 pages, 3 page, 4 Helvetica, 5 content, then named objects with
/// `{name}` references, the page's annotations listed by name.
#[derive(Default)]
pub struct Src {
    pub objects: Vec<(String, String)>,
    pub annots: Vec<String>,
    pub rotate: i64,
    pub catalog: String,
    pub content: String,
    pub properties: String,
}

impl Src {
    pub fn new(rotate: i64) -> Self {
        Src { rotate, content: "0 g 10 10 30 30 re f".into(), ..Src::default() }
    }

    pub fn obj(&mut self, name: &str, body: impl Into<String>) -> &mut Self {
        self.objects.push((name.to_string(), body.into()));
        self
    }

    /// An annotation, listed on the page.
    pub fn annot(&mut self, name: &str, body: impl Into<String>) -> &mut Self {
        self.annots.push(name.to_string());
        self.obj(name, body)
    }

    pub fn numbers(&self) -> HashMap<String, u32> {
        self.objects.iter().enumerate().map(|(i, (n, _))| (n.clone(), 6 + i as u32)).collect()
    }

    pub fn bytes(&self) -> Arc<Vec<u8>> {
        let numbers = self.numbers();
        let refs = |s: &str| {
            let mut out = s.to_string();
            for (k, n) in &numbers {
                out = out.replace(&format!("{{{k}}}"), &format!("{n} 0 R"));
            }
            out
        };
        let annots: String = self.annots.iter().map(|a| format!("{{{a}}} ")).collect();
        let mut objects = vec![
            refs(&format!("<< /Type /Catalog /Pages 2 0 R {} >>", self.catalog)),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            refs(&format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Rotate {} /Resources << /Font << /Helv 4 0 R >> /Properties << {} >> >> /Contents 5 0 R /Annots [{annots}] >>",
                self.rotate, self.properties
            )),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
            synth::raw_stream("", &self.content),
        ];
        objects.extend(self.objects.iter().map(|(_, b)| refs(b)));
        synth::raw_pdf(&objects)
    }
}

/// An appearance stream: `/BBox`, optional `/Matrix`, content.
pub fn ap(bbox: [f64; 4], matrix: Option<[f64; 6]>, content: &str) -> String {
    let m = matrix.map(|m| format!("/Matrix [{}] ", m.map(|v| v.to_string()).join(" "))).unwrap_or_default();
    synth::raw_stream(
        &format!("/Type /XObject /Subtype /Form /BBox [{}] {m}/Resources << /Font << /Helv 4 0 R >> >>", bbox.map(|v| v.to_string()).join(" ")),
        content,
    )
}

/// An annotation's name, `/BBox`, `/Matrix` and `/Rect`.
pub type Placed = (&'static str, [f64; 4], [f64; 6], [f64; 4]);
/// Name, kind, `/BBox`, `/Matrix`, `/Rect`, appearance content.
pub type Spec = (&'static str, &'static str, [f64; 4], Option<[f64; 6]>, [f64; 4], &'static str);

pub fn with_appearances(rotate: i64) -> (Src, Vec<Placed>) {
    let mut s = Src::new(rotate);
    // (annotation, /BBox, /Matrix, /Rect)
    let cases: Vec<Spec> = vec![
        ("hl", "Highlight", [0.0, 0.0, 60.0, 12.0], None, [50.0, 150.0, 110.0, 162.0], "1 1 0 rg 0 0 60 12 re f"),
        (
            "ft",
            "FreeText",
            [10.0, 20.0, 90.0, 40.0],
            None,
            [150.0, 150.0, 230.0, 170.0],
            "0 g BT /Helv 10 Tf 12 25 Td (Note) Tj ET 0 G 10 20 80 20 re S",
        ),
        (
            "ink",
            "Ink",
            [0.0, 0.0, 40.0, 40.0],
            Some([0.5, 0.0, 0.0, 0.5, 0.0, 0.0]),
            [20.0, 60.0, 60.0, 100.0],
            "0 0 1 RG 4 w 2 2 m 38 38 l S 2 38 m 38 2 l S",
        ),
        (
            "poly",
            "Polygon",
            [0.0, 0.0, 50.0, 30.0],
            Some([0.0, 1.0, -1.0, 0.0, 30.0, 0.0]),
            [80.0, 60.0, 110.0, 110.0],
            "0 0.5 0 RG 3 w 5 5 m 45 5 l 25 25 l h S",
        ),
        ("line", "Line", [-5.0, -5.0, 95.0, 15.0], None, [130.0, 80.0, 230.0, 100.0], "0 G 2 w 0 5 m 90 5 l S 90 5 m 80 0 l 80 10 l h f"),
        (
            "stamp",
            "Stamp",
            [0.0, 0.0, 100.0, 40.0],
            Some([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]),
            [200.0, 20.0, 280.0, 60.0],
            "0.8 0 0 RG 3 w 2 2 96 36 re S",
        ),
        ("note", "Text", [0.0, 0.0, 20.0, 20.0], None, [250.0, 120.0, 270.0, 140.0], "1 0.8 0 rg 0 0 20 20 re f 0 G 0.5 w 4 14 m 16 14 l S"),
        ("sq", "Square", [0.0, 0.0, 30.0, 30.0], None, [60.0, 20.0, 90.0, 50.0], "0 0.4 0.8 RG 2 w 1 1 28 28 re S"),
    ];
    let mut expected = Vec::new();
    for (name, subtype, bbox, matrix, rect, content) in cases {
        let apn = format!("{name}-ap");
        s.obj(&apn, ap(bbox, matrix, content));
        // The entries the standard requires of each kind (viewers drop an annotation without
        // them), though the stored appearance is what is drawn.
        let [x0, y0, x1, y1] = rect;
        let required = match subtype {
            "Highlight" => format!("/QuadPoints [{x0} {y1} {x1} {y1} {x0} {y0} {x1} {y0}]"),
            "Polygon" => format!("/Vertices [{x0} {y0} {x1} {y0} {x0} {y1}]"),
            "Ink" => format!("/InkList [[{x0} {y0} {x1} {y1}]]"),
            "Line" => format!("/L [{x0} {y0} {x1} {y1}]"),
            _ => String::new(),
        };
        s.annot(
            name,
            format!(
                "<< /Type /Annot /Subtype /{subtype} /Rect [{}] /F 4 {required} /AP << /N {{{apn}}} >> >>",
                rect.map(|v| v.to_string()).join(" ")
            ),
        );
        expected.push((name, bbox, matrix.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), rect));
    }
    // A check box and a radio button: the appearance /AS selects.
    s.obj("cb-on", ap([0.0, 0.0, 14.0, 14.0], None, "0 G 1 w 0.5 0.5 13 13 re S 2 2 m 12 12 l S 2 12 m 12 2 l S"));
    s.obj("cb-off", ap([0.0, 0.0, 14.0, 14.0], None, "0 G 1 w 0.5 0.5 13 13 re S"));
    s.annot("cb", "<< /Type /Annot /Subtype /Widget /FT /Btn /T (cb) /V /Yes /AS /Yes /Rect [20 120 34 134] /F 4 /AP << /N << /Yes {cb-on} /Off {cb-off} >> >> >>");
    s.obj("rb-on", ap([0.0, 0.0, 14.0, 14.0], None, "0 g 3 3 8 8 re f"));
    s.obj("rb-off", ap([0.0, 0.0, 14.0, 14.0], None, "0 G 1 w 0.5 0.5 13 13 re S"));
    s.annot("rb", "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 49152 /T (rb) /V /B /AS /Off /Rect [40 120 54 134] /F 4 /AP << /N << /B {rb-on} /Off {rb-off} >> >> >>");
    expected.push(("cb", [0.0, 0.0, 14.0, 14.0], [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [20.0, 120.0, 34.0, 134.0]));
    expected.push(("rb", [0.0, 0.0, 14.0, 14.0], [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [40.0, 120.0, 54.0, 134.0]));
    (s, expected)
}

/// Every kind the builder supports, without an appearance.
pub fn without_appearances(rotate: i64) -> Src {
    let mut s = Src::new(rotate);
    let kinds: [(&str, &str); 16] = [
        ("Text", "/Name /Comment /C [1 0.8 0]"),
        ("FileAttachment", "/Name /PushPin /C [0 0 1] /FS << /Type /Filespec /F (note.txt) >>"),
        ("Highlight", "/C [1 1 0] /QuadPoints [10 190 60 190 10 178 60 178]"),
        ("Underline", "/C [0 0 1] /QuadPoints [70 190 120 190 70 178 120 178]"),
        ("StrikeOut", "/C [1 0 0] /QuadPoints [130 190 180 190 130 178 180 178]"),
        ("Squiggly", "/C [0 0.6 0] /QuadPoints [190 190 240 190 190 178 240 178]"),
        ("Redact", "/QuadPoints [10 170 60 170 10 158 60 158]"),
        ("Square", "/C [0 0 1] /Border [0 0 2]"),
        ("Circle", "/C [1 0 0] /IC [1 1 0]"),
        ("Line", "/C [0 0 0] /L [150 100 220 130] /LE [/None /OpenArrow]"),
        ("Polygon", "/C [0 0.5 0] /Vertices [20 60 60 60 40 95]"),
        ("PolyLine", "/C [0.5 0 0.5] /Vertices [70 60 90 90 110 60 130 90]"),
        ("Caret", "/C [0 0 1]"),
        ("Ink", "/C [1 0 0] /InkList [[150 30 170 50 190 30 210 50]]"),
        // PdfCraft draws its own stamps; another program's stamp keeps its own artwork.
        ("Stamp", "/Name /Approved /PCStamp true"),
        ("FreeText", "/Contents (Plain text) /DA (0 g /Helv 10 Tf)"),
    ];
    let rects = [
        [240.0, 150.0, 260.0, 170.0],
        [270.0, 150.0, 290.0, 170.0],
        [10.0, 178.0, 60.0, 190.0],
        [70.0, 178.0, 120.0, 190.0],
        [130.0, 178.0, 180.0, 190.0],
        [190.0, 178.0, 240.0, 190.0],
        [10.0, 158.0, 60.0, 170.0],
        [70.0, 120.0, 110.0, 150.0],
        [120.0, 120.0, 150.0, 150.0],
        [148.0, 98.0, 222.0, 132.0],
        [18.0, 58.0, 62.0, 97.0],
        [68.0, 58.0, 132.0, 92.0],
        [230.0, 100.0, 250.0, 115.0],
        [148.0, 28.0, 212.0, 52.0],
        [220.0, 60.0, 290.0, 90.0],
        [60.0, 5.0, 140.0, 25.0],
    ];
    for ((kind, extra), rect) in kinds.iter().zip(rects) {
        s.annot(kind, format!("<< /Type /Annot /Subtype /{kind} /F 4 /Rect [{}] {extra} >>", rect.map(|v| v.to_string()).join(" ")));
    }
    s
}

/// A form: a text field whose stored appearance is stale, a text field without one, a combo box,
/// a check box and a radio button with stored states, and a check box without appearances.
pub fn form(need_appearances: bool) -> Arc<Vec<u8>> {
    let need = if need_appearances { "/NeedAppearances true" } else { "" };
    synth::raw_pdf(&[
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> {need} >> >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R /Annots [6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        synth::raw_stream("", "0 g 10 10 20 20 re f"),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (New value) /F 4 /Rect [20 150 160 170] /AP << /N 12 0 R >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (city) /V (Lisbon) /F 4 /Rect [20 120 160 140] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (size) /Opt [(Small) (Large)] /V (Large) /F 4 /Rect [20 90 160 110] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Yes /AS /Yes /F 4 /Rect [200 150 214 164] /AP << /N << /Yes 13 0 R /Off 14 0 R >> >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 49152 /T (pick) /V /B /AS /B /F 4 /Rect [200 120 214 134] /AP << /N << /B 13 0 R /Off 14 0 R >> >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (later) /V /Yes /AS /Yes /F 4 /Rect [200 90 214 104] /MK << >> >>".into(),
        synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 140 20] /Resources << /Font << /Helv 4 0 R >> >>", "/Tx BMC BT /Helv 10 Tf 0 g 2 6 Td (Old value) Tj ET EMC"),
        synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 14 14]", "0 G 1 w 0.5 0.5 13 13 re S 0 g 3 3 8 8 re f"),
        synth::raw_stream("/Type /XObject /Subtype /Form /BBox [0 0 14 14]", "0 G 1 w 0.5 0.5 13 13 re S"),
    ])
}

pub fn signed_source() -> Vec<u8> {
    let unsigned = synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        synth::raw_stream("", "BT /F1 14 Tf 20 250 Td (Contract text) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ]);
    // The sign crate's own test identity.
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).expect("the sign crate's test identity");
    let id = pdfcraft_sign::pkcs12::open(&p12, "test").expect("opens");
    let opts =
        pdfcraft_sign::pdf::SignOptions { page: 0, rect: Some([20.0, 100.0, 220.0, 150.0]), date: "D:20261013120000Z".into(), ..Default::default() };
    pdfcraft_sign::pdf::sign(&synth::open(&unsigned), &id, &opts).expect("signs")
}

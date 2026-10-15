//! T10 (step 4 PRD): a drawing-like pair. Old and New ARCH D sheets (36 × 24 in) with the layers
//! A-ARCH, S-STRUCT, E-ELEC (nested inside S-STRUCT's marked content) and M-HVAC (off by default),
//! a title block, revision clouds, text notes, a stamp and dimension lines. New differs from Old in
//! a few places, some of them on the hidden HVAC layer. Everything is synthetic and seeded; no real
//! drawing is used.

#![allow(dead_code)]

use std::fmt::Write as _;
use std::sync::Arc;

use super::synth;

/// ARCH D in points.
pub const W: f64 = 2592.0;
pub const H: f64 = 1728.0;

/// The HVAC layer's unique colour, as written: present in an overlay only when HVAC is shown.
pub const HVAC_OPERANDS: &str = "0.13 0.57 0.91";

/// Object numbers of the four layers (the same in both files).
pub const ARCH: u32 = 6;
pub const STRUCT: u32 = 7;
pub const ELEC: u32 = 8;
pub const HVAC: u32 = 9;

/// Which revision to draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rev {
    Old,
    New,
}

fn walls(rev: Rev) -> String {
    let mut s = String::from("0 G 6 w 0 J 0 j\n");
    // Outer walls and a grid of rooms.
    s.push_str("200 300 1800 1100 re S\n");
    for x in [650.0, 1100.0, 1550.0] {
        let x = if rev == Rev::New && x == 1100.0 { 1160.0 } else { x }; // a moved wall
        let _ = writeln!(s, "{x} 300 m {x} 1400 l S");
    }
    let _ = writeln!(s, "200 850 m 2000 850 l S");
    // Doors: a leaf and its swing.
    let mut doors = vec![(420.0, 850.0), (870.0, 850.0), (1320.0, 850.0)];
    if rev == Rev::New {
        doors.push((1770.0, 850.0)); // an added door
    }
    s.push_str("2 w\n");
    for (x, y) in doors {
        let _ = writeln!(
            s,
            "{x} {y} m {} {} l S {} {y} m {} {} {} {} {} {} c S",
            x + 60.0,
            y + 60.0,
            x + 60.0,
            x + 60.0,
            y + 33.0,
            x + 33.0,
            y + 60.0,
            x,
            y + 60.0
        );
    }
    s
}

fn columns(rev: Rev) -> String {
    let mut s = String::from("0.25 g\n");
    for (i, x) in [200.0, 650.0, 1100.0, 1550.0, 2000.0].iter().enumerate() {
        for y in [300.0, 850.0, 1400.0] {
            if rev == Rev::New && i == 3 && y == 850.0 {
                continue; // a removed column
            }
            let _ = writeln!(s, "{} {} 30 30 re f", x - 15.0, y - 15.0);
        }
    }
    s
}

/// A circle as four Bézier arcs (not stroked).
fn circle(x: f64, y: f64, r: f64) -> String {
    let k = r * 0.5523;
    format!(
        "{} {y} m {} {} {} {} {x} {} c {} {} {} {} {} {y} c {} {} {} {} {x} {} c {} {} {} {} {} {y} c",
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
    )
}

fn electrical(rev: Rev) -> String {
    let mut s = String::from("0.6 0 0.6 RG 2 w\n");
    let mut points = vec![(400.0, 600.0), (900.0, 600.0), (1300.0, 1100.0), (1800.0, 1100.0)];
    if rev == Rev::New {
        points[1] = (950.0, 650.0); // a moved outlet
    }
    for (x, y) in points {
        s.push_str(&circle(x, y, 18.0));
        let _ = writeln!(s, " S {} {y} m {} {y} l S", x - 18.0, x + 18.0);
    }
    s
}

fn hvac(rev: Rev) -> String {
    let mut s = format!("{HVAC_OPERANDS} RG 3 w\n");
    let ducts: &[(f64, f64, f64, f64)] = if rev == Rev::Old { &[(300.0, 1200.0, 1200.0, 80.0)] } else { &[(300.0, 1200.0, 1500.0, 80.0)] };
    for (x, y, w, h) in ducts {
        let _ = writeln!(s, "{x} {y} {w} {h} re S");
        let mut hx = x + 20.0;
        while hx < x + w {
            let _ = writeln!(s, "{hx} {y} m {} {} l S", hx + 40.0, y + h);
            hx += 60.0;
        }
    }
    s
}

fn title_block(rev: Rev) -> String {
    let revision = if rev == Rev::Old { "Rev 2" } else { "Rev 3" };
    format!(
        "0 G 2 w 2050 40 500 220 re S 2050 150 m 2550 150 l S\nBT /TB 28 Tf 2070 200 Td (SYNTHETIC PROJECT) Tj ET\nBT /TB 20 Tf 2070 100 Td (Sheet A-101  {revision}) Tj ET\n"
    )
}

/// One revision of the sheet, as PDF bytes.
pub fn sheet(rev: Rev) -> Arc<Vec<u8>> {
    let content = format!(
        "/OC /A BDC\n{}EMC\n/OC /S BDC\n{}/OC /E BDC\n{}EMC\nEMC\n/OC /M BDC\n{}EMC\n{}",
        walls(rev),
        columns(rev),
        electrical(rev),
        hvac(rev),
        title_block(rev)
    );
    let note = if rev == Rev::Old { "Check beam size" } else { "Beam size checked: W10x22" };
    let mut annots = vec![
        // Revision cloud around the moved wall.
        "<< /Type /Annot /Subtype /Polygon /F 4 /Rect [1040 700 1240 1000] /C [1 0 0] /BE << /S /C /I 1 >> /Vertices [1060 720 1220 720 1220 980 1060 980] >>".to_string(),
        format!("<< /Type /Annot /Subtype /FreeText /F 4 /Rect [1700 1450 2050 1500] /Contents ({note}) /DA (0 0 1 rg /Helv 16 Tf) >>"),
        "<< /Type /Annot /Subtype /Stamp /F 4 /Rect [2100 300 2400 380] /Name /Approved /PCStamp true >>".to_string(),
        "<< /Type /Annot /Subtype /Line /F 4 /Rect [190 1420 2010 1480] /L [200 1450 2000 1450] /LE [/OpenArrow /OpenArrow] /C [0 0 0] >>".to_string(),
        "<< /Type /Annot /Subtype /Line /F 4 /Rect [150 290 190 1410] /L [170 300 170 1400] /LE [/OpenArrow /OpenArrow] /C [0 0 0] >>".to_string(),
    ];
    if rev == Rev::New {
        annots.push("<< /Type /Annot /Subtype /Polygon /F 4 /Rect [1700 750 1900 950] /C [1 0 0] /BE << /S /C /I 1 >> /Vertices [1720 770 1880 770 1880 930 1720 930] >>".to_string());
    }
    let first_annot = 10 + 1;
    let refs: String = (0..annots.len()).map(|i| format!("{} 0 R ", first_annot + i)).collect();
    let mut objects = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [{ARCH} 0 R {STRUCT} 0 R {ELEC} 0 R {HVAC} 0 R] /D << /Order [{ARCH} 0 R [{STRUCT} 0 R {ELEC} 0 R] {HVAC} 0 R] /OFF [{HVAC} 0 R] >> >> >>"
        ),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /Font << /TB 4 0 R /Helv 4 0 R >> /Properties << /A {ARCH} 0 R /S {STRUCT} 0 R /E {ELEC} 0 R /M {HVAC} 0 R >> >> /Contents 5 0 R /Annots [{refs}] >>"
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        synth::raw_stream("", &content),
        "<< /Type /OCG /Name (A-ARCH) >>".into(),
        "<< /Type /OCG /Name (S-STRUCT) >>".into(),
        "<< /Type /OCG /Name (E-ELEC) >>".into(),
        "<< /Type /OCG /Name (M-HVAC) >>".into(),
        "<< /Producer (synthetic) >>".into(),
    ];
    objects.extend(annots);
    synth::raw_pdf(&objects)
}

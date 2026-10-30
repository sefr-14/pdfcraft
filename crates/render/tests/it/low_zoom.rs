//! RENDER.1 (this fork, sefr-14/pdfcraft): how thin lines and small text come out at low zoom.
//! Diagnosis only: nothing here changes how pages render.
//!
//! The pages are synthetic (built below) and rendered through [`PageRenderer::render`] at the
//! scales the page view asks for: `zoom × 96/72 × pixels per point` device pixels per point
//! (`canvas/cache.rs` `render_scale`). Each image is read as **darkness**: 1 minus the luminance
//! (Rec. 709 weights on the 8-bit sRGB values) of the pixel composited over white, with the
//! premultiplied alpha undone. For a line it reports, per device pixel of length:
//!
//! - **ink**: the sum of darkness across the line (1.0 = one fully black pixel of width);
//! - **peak**: the darkest pixel across it;
//! - **effective width**: ink ÷ peak, the width the ink would have at full strength;
//! - **affected pixels**: how many pixels across it are darker than [`AFFECTED`].
//!
//! References, for "how much ink should be there":
//!
//! - a **4× region of interest**: the same PDF rectangle rendered as a tile at four times the
//!   scale and averaged over exact 4 × 4 blocks, so its pixels cover exactly the target's;
//! - **pdftoppm** (Poppler, run as an external program only), when installed, at the same
//!   resolution and cropped to the same device rectangle.
//!
//! The report is `cargo test -p pdfcraft-render --test it low_zoom -- --ignored --nocapture`:
//! Markdown tables on stdout and grey-level images in `target/render1/` (never committed). The
//! other tests pin the baseline it measured (docs/render1-findings.md), with tolerances.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, Tile};

/// Darker than this counts as an affected pixel.
const AFFECTED: f64 = 0.02;
/// The page view's points-to-pixels factor at 100% (96 dpi).
const PT: f64 = 96.0 / 72.0;
/// The reference's scale factor.
const REF: u32 = 4;

/// A page `w` × `h` points drawing `content`, with `resources` and extra `objects` (numbered from
/// 5) and `/Rotate` `rotate`.
struct Pdf<'a> {
    w: f64,
    h: f64,
    rotate: u32,
    content: &'a str,
    resources: &'a str,
    objects: &'a [String],
}

impl Pdf<'_> {
    fn bytes(&self) -> Vec<u8> {
        let mut out = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Rotate {} /Contents 4 0 R /Resources << {} >> >> endobj\n\
             4 0 obj << /Length {} >> stream\n{}\nendstream endobj\n",
            self.w,
            self.h,
            self.rotate,
            self.resources,
            self.content.len(),
            self.content
        );
        for (i, object) in self.objects.iter().enumerate() {
            out.push_str(&format!("{} 0 obj {object} endobj\n", i + 5));
        }
        out.push_str(&format!("trailer << /Size {} /Root 1 0 R >>\n%%EOF", self.objects.len() + 5));
        out.into_bytes()
    }
}

/// An image as darkness, row-major.
#[derive(Clone, Debug)]
struct Gray {
    w: usize,
    h: usize,
    d: Vec<f64>,
    millis: u32,
}

impl Gray {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.d.get(y * self.w + x).copied().unwrap_or(0.0)
    }

    /// The mean of each `k` × `k` block: the image at `1/k` of its scale.
    fn box_down(&self, k: usize) -> Gray {
        let (w, h) = (self.w / k, self.h / k);
        let mut d = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for j in 0..k {
                    for i in 0..k {
                        sum += self.at(x * k + i, y * k + j);
                    }
                }
                d[y * w + x] = sum / (k * k) as f64;
            }
        }
        Gray { w, h, d, millis: self.millis }
    }

    /// What the GPU shows for this texture drawn `k` times smaller with bilinear filtering and no
    /// mipmaps (egui `TextureOptions::LINEAR`): each screen pixel samples the texture once, at its
    /// centre, between the four nearest texels.
    fn bilinear_down(&self, k: f64) -> Gray {
        let (w, h) = ((self.w as f64 / k).floor() as usize, (self.h as f64 / k).floor() as usize);
        let mut d = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (u, v) = ((x as f64 + 0.5) * k - 0.5, (y as f64 + 0.5) * k - 0.5);
                let (x0, y0) = (u.floor().max(0.0), v.floor().max(0.0));
                let (fx, fy) = (u - x0, v - y0);
                let (x0, y0) = (x0 as usize, y0 as usize);
                let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
                let top = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
                let bottom = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
                d[y * w + x] = top * (1.0 - fy) + bottom * fy;
            }
        }
        Gray { w, h, d, millis: 0 }
    }

    /// A grey-level image (white paper, black ink) for the report folder.
    fn save(&self, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/render1");
        let _ = std::fs::create_dir_all(&dir);
        let mut out = format!("P5\n{} {}\n255\n", self.w, self.h).into_bytes();
        out.extend(self.d.iter().map(|d| (255.0 * (1.0 - d.clamp(0.0, 1.0))).round() as u8));
        let _ = std::fs::write(dir.join(format!("{name}.pgm")), out);
    }
}

/// Darkness of a premultiplied RGBA8 pixel composited over white.
fn darkness(px: &[u8]) -> f64 {
    let [r, g, b, a] = [px[0], px[1], px[2], px[3]].map(f64::from);
    // Premultiplied over white: c + (255 - a).
    let over = |c: f64| (c + (255.0 - a)).min(255.0) / 255.0;
    1.0 - (0.2126 * over(r) + 0.7152 * over(g) + 0.0722 * over(b))
}

/// What to render: the whole page or a tile, at `scale`, with Thin Lines on or off.
#[derive(Clone, Copy, Debug)]
struct Shot {
    scale: f64,
    thin: bool,
    tile: Option<Tile>,
}

fn render(pdf: &[u8], shot: Shot) -> Gray {
    let config = RenderConfig { thin_lines: shot.thin, ..RenderConfig::default() };
    let mut r = PageRenderer::new(Arc::new(pdf.to_vec()), config);
    let page = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: shot.tile, scale: shot.scale as f32, tag: 0, preview: false });
    assert!(page.error.is_none(), "{:?}", page.error);
    let d = page.rgba.chunks_exact(4).map(darkness).collect();
    Gray { w: page.width as usize, h: page.height as usize, d, millis: page.millis }
}

/// A device-pixel rectangle at the target scale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Crop {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Gray {
    fn crop(&self, c: Crop) -> Gray {
        let mut d = Vec::with_capacity(c.w * c.h);
        for y in c.y..c.y + c.h {
            for x in c.x..c.x + c.w {
                d.push(self.at(x, y));
            }
        }
        Gray { w: c.w, h: c.h, d, millis: self.millis }
    }
}

/// The 4× reference of `crop`: the same PDF rectangle rendered as a tile at four times `scale`,
/// averaged over exact 4 × 4 blocks. Only the region is rendered, whatever the page size.
fn reference(pdf: &[u8], scale: f64, thin: bool, crop: Crop) -> Gray {
    let k = REF as usize;
    let tile = Tile { x: crop.x as u32 * REF, y: crop.y as u32 * REF, w: crop.w as u32 * REF, h: crop.h as u32 * REF };
    render(pdf, Shot { scale: scale * f64::from(REF), thin, tile: Some(tile) }).box_down(k)
}

/// pdftoppm's rendering of `crop` at the same resolution (grey, its anti-aliasing on), or `None`
/// when pdftoppm isn't installed or fails.
fn pdftoppm(pdf: &[u8], scale: f64, crop: Crop) -> Option<Gray> {
    let dir = std::env::temp_dir().join(format!("pdfcraft-render1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let input = dir.join("in.pdf");
    std::fs::write(&input, pdf).ok()?;
    let out = dir.join("out");
    let dpi = format!("{}", scale * 72.0);
    let status = std::process::Command::new("pdftoppm")
        .args(["-gray", "-r", &dpi, "-x", &crop.x.to_string(), "-y", &crop.y.to_string(), "-W", &crop.w.to_string(), "-H", &crop.h.to_string()])
        .arg("-singlefile")
        .arg(&input)
        .arg(&out)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let bytes = std::fs::read(dir.join("out.pgm")).ok()?;
    let _ = std::fs::remove_dir_all(&dir);
    read_pgm(&bytes)
}

/// A binary PGM (P5, maxval 255) as darkness.
fn read_pgm(bytes: &[u8]) -> Option<Gray> {
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while bytes.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !bytes.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        fields.push(std::str::from_utf8(bytes.get(start..at)?).ok()?.to_string());
    }
    let (w, h): (usize, usize) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let pixels = bytes.get(at + 1..at + 1 + w * h)?;
    Some(Gray { w, h, d: pixels.iter().map(|p| 1.0 - f64::from(*p) / 255.0).collect(), millis: 0 })
}

/// A line's measurements across it, per device pixel of its length.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Line {
    ink: f64,
    peak: f64,
    width: f64,
    affected: f64,
}

/// The measurements of the line through the middle of `img` running along the x axis (`along_x`)
/// or the y axis, from every column (or row) of the crop: ink, peak and affected pixels across it,
/// averaged along it.
fn across(img: &Gray, along_x: bool) -> Line {
    let (n, m) = if along_x { (img.w, img.h) } else { (img.h, img.w) };
    let mut sum = Line::default();
    for i in 0..n {
        let column: Vec<f64> = (0..m).map(|j| if along_x { img.at(i, j) } else { img.at(j, i) }).collect();
        sum.ink += column.iter().sum::<f64>();
        sum.peak += column.iter().copied().fold(0.0, f64::max);
        sum.affected += column.iter().filter(|d| **d > AFFECTED).count() as f64;
    }
    let n = n.max(1) as f64;
    let (ink, peak, affected) = (sum.ink / n, sum.peak / n, sum.affected / n);
    Line { ink, peak, width: if peak > 0.0 { ink / peak } else { 0.0 }, affected }
}

/// The measurements of a slanted line through `img`, `len` device pixels of it inside: ink and
/// affected pixels per pixel of length, and the darkest pixel.
fn slanted(img: &Gray, len: f64) -> Line {
    let ink = img.d.iter().sum::<f64>() / len;
    let peak = img.d.iter().copied().fold(0.0, f64::max);
    let affected = img.d.iter().filter(|d| **d > AFFECTED).count() as f64 / len;
    Line { ink, peak, width: if peak > 0.0 { ink / peak } else { 0.0 }, affected }
}

/// A glyph run's measurements: total ink, mean darkness over the run's box, the 95th percentile of
/// its darkness, and how many pixels are at least half dark.
#[derive(Clone, Copy, Debug, Default)]
struct Run {
    ink: f64,
    p95: f64,
    dark: usize,
}

fn run_metrics(img: &Gray) -> Run {
    let mut d = img.d.clone();
    d.sort_by(f64::total_cmp);
    let p95 = d.get(d.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.0);
    Run { ink: d.iter().sum(), p95, dark: d.iter().filter(|v| **v >= 0.5).count() }
}

// ---- Fixtures ---------------------------------------------------------------------------------

/// The page side of the line fixtures, in points.
const SIDE: f64 = 240.0;

/// The PDF y of a horizontal line whose centre falls `frac` of a pixel below a pixel edge, near
/// device row `row` at `scale` (y down from the top of a `SIDE`-point page).
fn y_at(scale: f64, row: f64, frac: f64) -> f64 {
    SIDE - (row + frac) / scale
}

/// A horizontal stroke `width` points wide whose centre is `frac` of a pixel into device row 40,
/// from x = 20 to 220 points.
fn stroke_h(scale: f64, width: f64, frac: f64) -> Vec<u8> {
    let y = y_at(scale, 40.0, frac);
    let content = format!("0 G {width} w 0 J 20 {y} m 220 {y} l S");
    Pdf { w: SIDE, h: SIDE, rotate: 0, content: &content, resources: "", objects: &[] }.bytes()
}

/// A vertical stroke: the same, along device column 40.
fn stroke_v(scale: f64, width: f64, frac: f64) -> Vec<u8> {
    let x = (40.0 + frac) / scale;
    let content = format!("0 G {width} w 0 J {x} 20 m {x} 220 l S");
    Pdf { w: SIDE, h: SIDE, rotate: 0, content: &content, resources: "", objects: &[] }.bytes()
}

/// A stroke at `degrees` from horizontal through the page centre, 200 points long.
fn stroke_slanted(width: f64, degrees: f64) -> Vec<u8> {
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = (SIDE / 2.0, SIDE / 2.0);
    let content = format!("0 G {width} w 0 J {} {} m {} {} l S", cx - 100.0 * c, cy - 100.0 * s, cx + 100.0 * c, cy + 100.0 * s);
    Pdf { w: SIDE, h: SIDE, rotate: 0, content: &content, resources: "", objects: &[] }.bytes()
}

/// A filled rule `height` points tall (a `re f`, as CAD and word processors draw rules), centred
/// `frac` of a pixel into device row 40.
fn rule(scale: f64, height: f64, frac: f64) -> Vec<u8> {
    let y = y_at(scale, 40.0, frac) - height / 2.0;
    let content = format!("0 g 20 {y} 200 {height} re f");
    Pdf { w: SIDE, h: SIDE, rotate: 0, content: &content, resources: "", objects: &[] }.bytes()
}

/// A horizontal stroke drawn inside a 240-point tiling pattern cell that fills the page: the cell
/// is the page, so the line sits where [`stroke_h`] puts it.
fn stroke_in_pattern(scale: f64, width: f64, frac: f64) -> Vec<u8> {
    let y = y_at(scale, 40.0, frac);
    let cell = format!("0 G {width} w 0 J 20 {y} m 220 {y} l S");
    let pattern = format!(
        "<< /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 {SIDE} {SIDE}] /XStep {SIDE} /YStep {SIDE} /Resources << >> /Length {} >> stream\n{cell}\nendstream",
        cell.len()
    );
    let content = format!("/Pattern cs /P1 scn 0 0 {SIDE} {SIDE} re f");
    Pdf { w: SIDE, h: SIDE, rotate: 0, content: &content, resources: "/Pattern << /P1 5 0 R >>", objects: &[pattern] }.bytes()
}

/// A horizontal stroke drawn as a Type 3 glyph: glyph space is 1/1000 of the 10-point text size,
/// so `width` points is `width × 100` glyph units.
fn stroke_in_type3(scale: f64, width: f64, frac: f64) -> Vec<u8> {
    let y = y_at(scale, 40.0, frac);
    let glyph = format!("1000 0 0 0 20000 1000 d1 {} w 0 J 0 0 m 20000 0 l S", width * 100.0);
    let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 20000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [20000] >>".to_string();
    let proc = format!("<< /Length {} >> stream\n{glyph}\nendstream", glyph.len());
    let content = format!("0 g BT /T3 10 Tf 20 {y} Td (a) Tj ET");
    Pdf { w: SIDE, h: SIDE, rotate: 0, content: &content, resources: "/Font << /T3 5 0 R >>", objects: &[font, proc] }.bytes()
}

/// The horizontal stroke of [`stroke_h`] on a page turned 90° (`/Rotate 90`), where it shows as
/// a vertical line.
fn stroke_rotated(scale: f64, width: f64, frac: f64) -> Vec<u8> {
    // Shown turned clockwise: PDF y runs left to right, so device column = y × scale.
    let y = (40.0 + frac) / scale;
    let content = format!("0 G {width} w 0 J 20 {y} m 220 {y} l S");
    Pdf { w: SIDE, h: SIDE, rotate: 90, content: &content, resources: "", objects: &[] }.bytes()
}

/// The dimension-style text run, in standard-14 `font` at `size` points, at (20, 120).
const TEXT: &str = "1/4\" = 1'-0\"  12'-5 3/8\"  0123456789";

fn text(font: &str, size: f64) -> Vec<u8> {
    let content = format!("0 g BT /F1 {size} Tf 20 120 Td ({}) Tj ET", TEXT.replace('"', "\\\""));
    let font = format!("<< /Type /Font /Subtype /Type1 /BaseFont /{font} >>");
    Pdf { w: SIDE * 2.0, h: SIDE, rotate: 0, content: &content, resources: "/Font << /F1 5 0 R >>", objects: &[font] }.bytes()
}

/// An A1 sheet (594 × 841 mm) of 2,000 thin lines and 400 dimension texts, for timing.
fn dense_sheet() -> Vec<u8> {
    let (w, h) = (1684.0, 2384.0);
    let mut content = String::from("0 G 0.1 w ");
    for i in 0..1000 {
        let t = f64::from(i) / 1000.0;
        content.push_str(&format!("20 {} m {} {} l ", 20.0 + t * (h - 40.0), w - 20.0, 20.0 + t * (h - 40.0)));
        content.push_str(&format!("{} 20 m {} {} l ", 20.0 + t * (w - 40.0), 20.0 + t * (w - 40.0), h - 20.0));
    }
    content.push_str("S 0 g BT /F1 5 Tf ");
    for i in 0..400 {
        let (x, y) = (40.0 + f64::from(i % 20) * 80.0, 40.0 + f64::from(i / 20) * 110.0);
        content.push_str(&format!("1 0 0 1 {x} {y} Tm (12'-5 3/8\\\") Tj "));
    }
    content.push_str("ET");
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string();
    Pdf { w, h, rotate: 0, content: &content, resources: "/Font << /F1 5 0 R >>", objects: &[font] }.bytes()
}

// ---- Measurements -----------------------------------------------------------------------------

/// The zooms and pixels per point the page view uses.
const ZOOMS: [f64; 4] = [0.25, 0.5, 0.75, 1.0];
const PPPS: [f64; 3] = [1.0, 1.5, 2.0];
const WIDTHS: [f64; 5] = [0.0, 0.1, 0.25, 0.5, 1.0];
/// Where a horizontal line's centre falls inside its pixel row: on a pixel edge, or a centre.
const FRACS: [f64; 2] = [0.0, 0.5];

fn scale(zoom: f64, ppp: f64) -> f64 {
    zoom * PT * ppp
}

/// The crop around a horizontal line in device row 40 (rows 32..48), columns from 30% to 70% of
/// the line's length, at `scale`.
fn h_crop(scale: f64) -> Crop {
    let (x0, x1) = ((60.0 * scale).ceil() as usize, (180.0 * scale).floor() as usize);
    Crop { x: x0, y: 32, w: x1.saturating_sub(x0).max(1), h: 16 }
}

/// The crop around a vertical line in device column 40.
fn v_crop(scale: f64) -> Crop {
    let c = h_crop(scale);
    Crop { x: c.y, y: c.x, w: c.h, h: c.w }
}

/// One horizontal line fixture measured at `scale`: as rendered, and its 4× reference.
fn measure_h(pdf: &[u8], scale: f64, thin: bool) -> (Line, Line, Gray) {
    let crop = h_crop(scale);
    let shown = render(pdf, Shot { scale, thin, tile: None }).crop(crop);
    let truth = reference(pdf, scale, thin, crop);
    (across(&shown, true), across(&truth, true), shown)
}

fn fmt(l: Line) -> String {
    format!("{:.2} | {:.2} | {:.2} | {:.1}", l.ink, l.peak, l.width, l.affected)
}

fn report_strokes() {
    println!("\n### Horizontal strokes (ink, peak, effective width, affected px per px of length)\n");
    println!(
        "| width pt | zoom | ppp | offset | geometric px | ink | peak | eff. width | affected | 4× ref ink | 4× ref peak | Thin Lines ink | Thin Lines peak |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for width in WIDTHS {
        for zoom in ZOOMS {
            for ppp in PPPS {
                for frac in FRACS {
                    let s = scale(zoom, ppp);
                    let pdf = stroke_h(s, width, frac);
                    let (line, truth, img) = measure_h(&pdf, s, false);
                    let (thin, _, _) = measure_h(&pdf, s, true);
                    if ppp == 1.0 && frac == 0.0 {
                        img.save(&format!("stroke-{width}pt-{}pct", zoom * 100.0));
                    }
                    println!(
                        "| {width} | {:.0}% | {ppp} | {frac} | {:.2} | {} | {:.2} | {:.2} | {:.2} | {:.2} |",
                        zoom * 100.0,
                        width * s,
                        fmt(line),
                        truth.ink,
                        truth.peak,
                        thin.ink,
                        thin.peak
                    );
                }
            }
        }
    }
}

fn report_orientation() {
    println!("\n### Orientation and rotation (ppp 1, offset 0)\n");
    println!("| width pt | zoom | vertical | 30° | 45° | page /Rotate 90 |");
    println!("|---|---|---|---|---|---|");
    for width in [0.0, 0.25, 1.0] {
        for zoom in ZOOMS {
            let s = scale(zoom, 1.0);
            let v = across(&render(&stroke_v(s, width, 0.0), Shot { scale: s, thin: false, tile: None }).crop(v_crop(s)), false);
            let r = across(&render(&stroke_rotated(s, width, 0.0), Shot { scale: s, thin: false, tile: None }).crop(v_crop(s)), false);
            let mut cells = vec![fmt(v)];
            for degrees in [30.0, 45.0] {
                let img = render(&stroke_slanted(width, degrees), Shot { scale: s, thin: false, tile: None });
                // The middle 40 points of the line, in a square that holds only it.
                let half = (20.0 * s).max(2.0) as usize;
                let (cx, cy) = (img.w / 2, img.h / 2);
                let crop = img.crop(Crop { x: cx - half, y: cy - half, w: 2 * half, h: 2 * half });
                cells.push(fmt(slanted(&crop, 2.0 * half as f64 / degrees.to_radians().cos())));
            }
            cells.push(fmt(r));
            println!("| {width} | {:.0}% | {} |", zoom * 100.0, cells.join(" / "));
        }
    }
}

fn report_rules_and_exempt() {
    println!("\n### Filled rules, and strokes in patterns and Type 3 glyphs (ppp 1)\n");
    println!(
        "| fixture | zoom | offset | geometric px | ink | peak | eff. width | affected | 4× ref ink | 4× ref peak | pdftoppm ink | pdftoppm peak |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for zoom in ZOOMS {
        let s = scale(zoom, 1.0);
        for frac in FRACS {
            let mut rows: Vec<(String, Vec<u8>, f64)> = [0.1, 0.25, 0.5].iter().map(|h| (format!("rule {h} pt"), rule(s, *h, frac), h * s)).collect();
            for w in [0.0, 0.1] {
                rows.push((format!("pattern stroke {w} pt"), stroke_in_pattern(s, w, frac), w * s));
                rows.push((format!("Type 3 stroke {w} pt"), stroke_in_type3(s, w, frac), w * s));
            }
            for (name, pdf, geometric) in rows {
                let (line, truth, img) = measure_h(&pdf, s, false);
                if frac == 0.0 {
                    img.save(&format!("{}-{}pct", name.replace(' ', "-"), zoom * 100.0));
                }
                let oracle = pdftoppm(&pdf, s, h_crop(s)).map(|g| across(&g, true));
                let o = oracle.map_or("n/a | n/a".to_string(), |o| format!("{:.2} | {:.2}", o.ink, o.peak));
                println!("| {name} | {:.0}% | {frac} | {geometric:.2} | {} | {:.2} | {:.2} | {o} |", zoom * 100.0, fmt(line), truth.ink, truth.peak);
            }
        }
    }
}

fn report_text() {
    println!("\n### Dimension text: ink and contrast against the 4× reference and pdftoppm (ppp 1)\n");
    println!(
        "| font | size pt | zoom | ink | ink / 4× ref | p95 | 4× ref p95 | ≥50% px | 4× ref ≥50% px | pdftoppm ink | pdftoppm p95 | pdftoppm ≥50% px |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for font in ["Helvetica", "Courier"] {
        for size in [4.0, 5.0, 6.0, 8.0] {
            let pdf = text(font, size);
            for zoom in ZOOMS {
                let s = scale(zoom, 1.0);
                // The run's box: from x = 18 to 240 points, its baseline at y = 120.
                let crop = Crop {
                    x: (18.0 * s).floor() as usize,
                    y: ((SIDE - 120.0 - size) * s).floor() as usize,
                    w: (222.0 * s).ceil() as usize,
                    h: ((size * 1.4) * s).ceil().max(2.0) as usize,
                };
                let shown = run_metrics(&render(&pdf, Shot { scale: s, thin: false, tile: None }).crop(crop));
                let truth = run_metrics(&reference(&pdf, s, false, crop));
                let oracle = pdftoppm(&pdf, s, crop).map(|g| run_metrics(&g));
                let o = oracle.map_or("n/a | n/a | n/a".to_string(), |o| format!("{:.1} | {:.2} | {}", o.ink, o.p95, o.dark));
                println!(
                    "| {font} | {size} | {:.0}% | {:.1} | {:.2} | {:.2} | {:.2} | {} | {} | {o} |",
                    zoom * 100.0,
                    shown.ink,
                    shown.ink / truth.ink.max(1e-9),
                    shown.p95,
                    truth.p95,
                    shown.dark,
                    truth.dark
                );
            }
        }
    }
}

fn report_oracle_lines() {
    println!("\n### Horizontal strokes against pdftoppm (ppp 1, offset 0)\n");
    println!("| width pt | zoom | ink | peak | pdftoppm ink | pdftoppm peak |");
    println!("|---|---|---|---|---|---|");
    for width in WIDTHS {
        for zoom in ZOOMS {
            let s = scale(zoom, 1.0);
            let pdf = stroke_h(s, width, 0.0);
            let crop = h_crop(s);
            let line = across(&render(&pdf, Shot { scale: s, thin: false, tile: None }).crop(crop), true);
            let oracle = pdftoppm(&pdf, s, crop).map(|g| across(&g, true));
            let o = oracle.map_or("n/a | n/a".to_string(), |o| format!("{:.2} | {:.2}", o.ink, o.peak));
            println!("| {width} | {:.0}% | {:.2} | {:.2} | {o} |", zoom * 100.0, line.ink, line.peak);
        }
    }
}

fn report_minification() {
    println!("\n### Zooming out: a 100% raster shown smaller by the GPU (bilinear, no mipmaps) against a fresh render\n");
    println!("| width pt | shown at | GPU-minified ink | peak | fresh render ink | peak |");
    println!("|---|---|---|---|---|---|");
    let s100 = scale(1.0, 1.0);
    for width in [0.0, 0.25, 0.5] {
        let pdf = stroke_h(s100, width, 0.5);
        let full = render(&pdf, Shot { scale: s100, thin: false, tile: None });
        for k in [2.0, 3.0, 4.0] {
            let small = full.bilinear_down(k);
            // The line was in row 40.5 at 100%: (40.5 + 0.5) / k - 0.5 when minified.
            let row = (41.0 / k - 0.5).round() as usize;
            let crop = Crop { x: small.w * 3 / 10, y: row.saturating_sub(6), w: small.w * 4 / 10, h: 12 };
            let gpu = across(&small.crop(crop), true);
            let fresh_pdf = stroke_h(s100 / k, width, 0.5);
            let fresh = across(&render(&fresh_pdf, Shot { scale: s100 / k, thin: false, tile: None }).crop(h_crop(s100 / k)), true);
            println!("| {width} | {:.0}% | {:.2} | {:.2} | {:.2} | {:.2} |", 100.0 / k, gpu.ink, gpu.peak, fresh.ink, fresh.peak);
        }
    }
}

fn report_cost() {
    println!("\n### Dense A1 sheet (2,000 lines of 0.1 pt, 400 texts): render time and raster size\n");
    println!("| zoom | ppp | pixels | MiB | ms (renderer) | ms (wall) |");
    println!("|---|---|---|---|---|---|");
    let pdf = dense_sheet();
    for zoom in ZOOMS {
        for ppp in PPPS {
            let s = scale(zoom, ppp);
            let start = std::time::Instant::now();
            let img = render(&pdf, Shot { scale: s, thin: false, tile: None });
            let wall = start.elapsed().as_millis();
            let px = img.w * img.h;
            println!("| {:.0}% | {ppp} | {}×{} | {:.1} | {} | {wall} |", zoom * 100.0, img.w, img.h, (px * 4) as f64 / (1024.0 * 1024.0), img.millis);
        }
    }
}

/// The full report (see the module docs). Slow; run on demand.
#[test]
#[ignore = "RENDER.1 report: run with --ignored --nocapture"]
fn low_zoom_report() {
    println!("Darkness: 1 − Rec. 709 luminance of the 8-bit sRGB pixel over white. Scale = zoom × 96/72 × ppp.");
    println!("4× reference: the same device rectangle rendered as a tile at 4× scale, averaged over 4 × 4 blocks.");
    println!("pdftoppm: -gray -r (scale × 72) -x -y -W -H the same device rectangle.");
    report_strokes();
    report_orientation();
    report_rules_and_exempt();
    report_oracle_lines();
    report_text();
    report_minification();
    report_cost();
}

// ---- The metrics themselves -------------------------------------------------------------------

/// A synthetic image: a horizontal band of darkness `profile` (top to bottom) across `w` columns.
fn band(profile: &[f64], w: usize) -> Gray {
    let mut d = Vec::new();
    for v in profile {
        d.extend(std::iter::repeat_n(*v, w));
    }
    Gray { w, h: profile.len(), d, millis: 0 }
}

#[test]
fn a_line_split_over_two_pixels_is_one_pixel_of_ink_two_wide() {
    let line = across(&band(&[0.0, 0.5, 0.5, 0.0], 10), true);
    assert!((line.ink - 1.0).abs() < 1e-12 && (line.peak - 0.5).abs() < 1e-12);
    assert!((line.width - 2.0).abs() < 1e-12 && (line.affected - 2.0).abs() < 1e-12);
    // The same ink in one pixel is one pixel wide; faint fringes count as affected only above
    // the threshold.
    let line = across(&band(&[0.01, 1.0, 0.01], 10), true);
    assert!((line.width - 1.02).abs() < 1e-9 && (line.affected - 1.0).abs() < 1e-12);
    // Vertical: the transposed band.
    let v = band(&[0.0, 0.5, 0.5, 0.0], 3);
    let t = Gray { w: v.h, h: v.w, d: (0..v.w * v.h).map(|i| v.at(i / v.h, i % v.h)).collect(), millis: 0 };
    assert_eq!(across(&t, false), across(&v, true));
}

#[test]
fn the_reference_downsample_and_the_gpu_minification_keep_ink() {
    // 4 × 4 blocks: one fully dark row of four becomes a quarter-dark row.
    let big = band(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 8);
    let small = big.box_down(4);
    assert_eq!((small.w, small.h), (2, 2));
    assert!((across(&small, true).ink - 0.25).abs() < 1e-12);
    // Bilinear at half size samples between texel centres: a 2-pixel band of ink keeps its ink
    // over one pixel; a 1-pixel band can fall between samples and lose half of it.
    let two = band(&[0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0], 8).bilinear_down(2.0);
    assert!((across(&two, true).ink - 1.0).abs() < 1e-12);
    let one = band(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 8).bilinear_down(2.0);
    assert!((across(&one, true).ink - 0.5).abs() < 1e-12);
}

#[test]
fn darkness_undoes_premultiplied_alpha_over_white() {
    assert_eq!(darkness(&[255, 255, 255, 255]), 0.0);
    assert!((darkness(&[0, 0, 0, 255]) - 1.0).abs() < 1e-12);
    // Half-transparent black, premultiplied: (0, 0, 0, 128) over white is grey 127.
    assert!((darkness(&[0, 0, 0, 128]) - 128.0 / 255.0).abs() < 1e-12);
    assert_eq!(darkness(&[0, 0, 0, 0]), 0.0);
}

#[test]
fn a_pgm_reads_back() {
    let g = read_pgm(b"P5\n2 1\n255\n\xff\x00").expect("pgm");
    assert_eq!((g.w, g.h), (2, 1));
    assert_eq!(g.d, vec![0.0, 1.0]);
    assert!(read_pgm(b"P5\n2 1\n255\n\xff").is_none(), "short data");
}

// ---- The baseline (docs/render1-findings.md), pinned so a renderer change shows on purpose ----
//
// Each states its zoom, pixels per point, where the line's centre falls in its pixel (0 = on a
// pixel edge, 0.5 = on a pixel centre), its orientation and Thin Lines. Values are the ones the
// report measured, with a tolerance for 8-bit coverage.

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

/// A horizontal hairline (width 0), 25%, ppp 1, Thin Lines off: one pixel's worth of ink always.
/// Centred on a pixel edge it is two half-dark pixels; on a pixel centre, one black pixel.
#[test]
fn baseline_a_hairline_is_one_pixel_of_ink_split_by_its_position() {
    let s = scale(0.25, 1.0);
    let (edge, _, _) = measure_h(&stroke_h(s, 0.0, 0.0), s, false);
    assert!(close(edge.ink, 1.0, 0.02) && close(edge.peak, 0.5, 0.02) && close(edge.affected, 2.0, 0.01), "{edge:?}");
    let (centre, _, _) = measure_h(&stroke_h(s, 0.0, 0.5), s, false);
    assert!(close(centre.ink, 1.0, 0.02) && close(centre.peak, 1.0, 0.02) && close(centre.affected, 1.0, 0.01), "{centre:?}");
}

/// A horizontal 0.25-point filled rule (`re f`), 25%, ppp 1, centred on a pixel centre, Thin
/// Lines on or off: its geometric coverage (0.083 px) and no more, so it barely shows.
#[test]
fn baseline_a_thin_filled_rule_keeps_only_its_geometric_coverage() {
    let s = scale(0.25, 1.0);
    for thin in [false, true] {
        let (line, truth, _) = measure_h(&rule(s, 0.25, 0.5), s, thin);
        assert!(close(line.ink, 0.25 * s, 0.01) && close(line.peak, 0.25 * s, 0.01), "Thin Lines {thin}: {line:?}");
        assert!(close(line.ink, truth.ink, 0.01), "the 4× reference agrees: {truth:?}");
    }
}

/// A horizontal hairline drawn inside a tiling pattern cell, or as a Type 3 glyph, 25%, ppp 1,
/// centred on a pixel centre, Thin Lines off: (almost) no ink, where the same line drawn directly
/// is one black pixel.
#[test]
fn baseline_a_hairline_in_a_pattern_or_type3_glyph_gets_no_minimum() {
    let s = scale(0.25, 1.0);
    let (pattern, _, _) = measure_h(&stroke_in_pattern(s, 0.0, 0.5), s, false);
    assert!(pattern.ink < 0.05, "pattern: {pattern:?}");
    let (glyph, _, _) = measure_h(&stroke_in_type3(s, 0.0, 0.5), s, false);
    assert!(glyph.ink < 0.05, "Type 3: {glyph:?}");
}

/// A horizontal 1-point stroke, 100%, ppp 2, centred on a pixel centre: 2.67 px of ink with Thin
/// Lines off, one pixel's worth with it on.
#[test]
fn baseline_thin_lines_narrows_a_wide_stroke_to_one_pixel() {
    let s = scale(1.0, 2.0);
    let pdf = stroke_h(s, 1.0, 0.5);
    let (off, _, _) = measure_h(&pdf, s, false);
    let (on, _, _) = measure_h(&pdf, s, true);
    assert!(close(off.ink, s, 0.05) && close(off.peak, 1.0, 0.02), "off: {off:?}");
    assert!(close(on.ink, 1.0, 0.02) && close(on.peak, 1.0, 0.02), "on: {on:?}");
}

/// A horizontal 1-pixel line rendered at 100% (ppp 1, centred on a pixel centre) and shown at
/// 25% by bilinear minification without mipmaps, as a stale raster is while zooming out: every
/// screen pixel's sample misses it.
#[test]
fn baseline_a_line_minified_four_times_without_mipmaps_disappears() {
    let s = scale(1.0, 1.0);
    let full = render(&stroke_h(s, 0.0, 0.5), Shot { scale: s, thin: false, tile: None });
    let small = full.bilinear_down(4.0);
    let crop = Crop { x: small.w * 3 / 10, y: 4, w: small.w * 4 / 10, h: 12 };
    let gpu = across(&small.crop(crop), true);
    assert!(gpu.ink < 0.02, "{gpu:?}");
    // At half size the sample lands between texels: half the ink.
    let half = across(&full.bilinear_down(2.0).crop(Crop { x: 30, y: 14, w: 60, h: 12 }), true);
    assert!(close(half.ink, 0.5, 0.02), "{half:?}");
}

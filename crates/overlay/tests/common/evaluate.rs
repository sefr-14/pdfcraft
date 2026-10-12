//! Rendering and measuring the step 3 suite (OV3.3, OV3.4): renders a case's overlay page in
//! PdfCraft (in process), Poppler or PDFium (separate processes), and measures its patches,
//! targets and line profiles. Shared by the in-process pixel tests and the proof example.

#![allow(dead_code)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use pdfcraft_overlay::Side;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, effective_scale};

use super::families::{Case, Expect, LineKind};
use super::measure::{self, Class, Image, Pair, classify, nearest_colour, recover};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Renderer {
    PdfCraft,
    Poppler,
    Pdfium,
}

impl Renderer {
    pub fn name(self) -> &'static str {
        match self {
            Renderer::PdfCraft => "pdfcraft",
            Renderer::Poppler => "poppler",
            Renderer::Pdfium => "pdfium",
        }
    }
}

/// One render of one page: the image, and how its pixels map to the page.
pub struct Render {
    pub img: Image,
    /// Device pixels per output-page unit, across and down. PdfCraft and Poppler keep the exact
    /// scale and leave a partial pixel at the edge; PDFium stretches the page to its whole-pixel
    /// bitmap, so its two scales differ slightly from dpi / 72.
    pub scale_x: f64,
    pub scale_y: f64,
    /// The page height in units (to flip y).
    pub height_units: f64,
    /// Whether the renderer applied the page's /UserUnit.
    pub honours_unit: bool,
    /// Requested dpi, and the dpi actually rendered (PdfCraft caps whole pages at 8,192 px).
    pub dpi: f64,
    pub effective_dpi: f64,
    pub millis: u128,
}

impl Render {
    pub fn to_px(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.scale_x, (self.height_units - y) * self.scale_y)
    }

    /// Pixel position → output-page units.
    pub fn to_page(&self, x: f64, y: f64) -> (f64, f64) {
        (x / self.scale_x, self.height_units - y / self.scale_y)
    }

    pub fn rect_px(&self, r: [f64; 4]) -> (usize, usize, usize, usize) {
        let (x0, y1) = self.to_px(r[0], r[1]);
        let (x1, y0) = self.to_px(r[2], r[3]);
        (x0.max(0.0).round() as usize, y0.max(0.0).round() as usize, x1.max(0.0).round() as usize, y1.max(0.0).round() as usize)
    }
}

pub fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

/// Render page `page` (0-based) of `pdf` (saved at `path`) at `dpi`.
pub fn render(renderer: Renderer, pdf: &Arc<Vec<u8>>, path: &Path, page: usize, dpi: f64, case: &Case, tmp: &Path) -> Result<Render, String> {
    let page_box = case.placement.page_box();
    let (w_units, h_units, unit) = (page_box[2], page_box[3], case.placement.unit);
    let start = Instant::now();
    let (img, effective_dpi) = match renderer {
        Renderer::PdfCraft => {
            let mut r = PageRenderer::new(pdf.clone(), RenderConfig::default());
            let scale = (dpi / 72.0) as f32;
            let out = r.render(RenderRequest { page, scale, ..RenderRequest::default() });
            if let Some(e) = out.error {
                return Err(format!("PdfCraft: {e}"));
            }
            // The renderer's own cap, for the record: the size it was asked for, in points.
            let eff = effective_scale((w_units * unit) as f32, (h_units * unit) as f32, scale);
            (Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba), f64::from(eff) * 72.0)
        }
        Renderer::Poppler => {
            let prefix = tmp.join("poppler");
            run(Command::new("pdftoppm")
                .arg("-r")
                .arg(format!("{dpi}"))
                .arg("-f")
                .arg((page + 1).to_string())
                .arg("-l")
                .arg((page + 1).to_string())
                .arg("-singlefile")
                .arg(path)
                .arg(&prefix))?;
            let file = prefix.with_extension("ppm");
            let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&file);
            (Image::from_pnm(&bytes)?, dpi)
        }
        Renderer::Pdfium => {
            let dir = tmp.join("pdfium");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            run(Command::new("python3")
                .args(["-m", "pypdfium2_cli", "render"])
                .arg(path)
                .arg("-o")
                .arg(&dir)
                .args(["--prefix", "p-", "--pages"])
                .arg((page + 1).to_string())
                .arg("--scale")
                .arg(format!("{}", dpi / 72.0))
                .args(["--format", "ppm", "--no-x-channel", "--linear"])
                .env("PYPDFIUM_LOGLEVEL", "warning"))?;
            let file =
                std::fs::read_dir(&dir).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).map(|e| e.path()).next().ok_or("PDFium wrote nothing")?;
            let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&dir);
            (Image::from_pnm(&bytes)?, dpi)
        }
    };
    let millis = start.elapsed().as_millis();
    // Did the renderer apply /UserUnit? Compare the longer side with both possibilities.
    let s = effective_dpi / 72.0;
    let (long_units, long_px) = if w_units >= h_units { (w_units, img.width as f64) } else { (h_units, img.height as f64) };
    let honours_unit = unit == 1.0 || (long_px - long_units * unit * s).abs() <= (long_px - long_units * s).abs();
    let k = if honours_unit { unit } else { 1.0 };
    let (scale_x, scale_y) = match renderer {
        Renderer::Pdfium => (img.width as f64 / (w_units * k), img.height as f64 / (h_units * k)),
        _ => (s * k, s * k),
    };
    Ok(Render { img, scale_x, scale_y, height_units: h_units, honours_unit, dpi, effective_dpi, millis })
}

// ── Measurements ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PatchRow {
    pub set: &'static str,
    pub case: String,
    pub patch: String,
    pub criterion: &'static str,
    pub renderer: Renderer,
    pub dpi: f64,
    pub value: [f64; 3],
    pub expected: Option<[f64; 3]>,
    /// For masks: recovered m and expected m.
    pub mask: Option<(f64, f64)>,
    pub tol: f64,
    pub error: f64,
    pub pass: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct TargetRow {
    pub set: &'static str,
    pub case: String,
    pub target: String,
    pub side: Side,
    pub renderer: Renderer,
    pub dpi: f64,
    pub expected: (f64, f64),
    pub measured: Option<(f64, f64)>,
    /// Error in points (output units × unit), and in pixels at 300 dpi.
    pub error_pt: Option<f64>,
    pub on_page: bool,
    pub partner: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub a: usize,
    pub b: usize,
    pub shared: usize,
    pub paper: usize,
    pub near: usize,
    pub under: usize,
    pub unexplained: usize,
}

impl Counts {
    pub fn add(&mut self, c: Class) {
        match c {
            Class::A => self.a += 1,
            Class::B => self.b += 1,
            Class::Shared => self.shared += 1,
            Class::Paper => self.paper += 1,
            Class::NearThreshold => self.near += 1,
            Class::Underdetermined => self.under += 1,
            Class::Unexplained => self.unexplained += 1,
        }
    }

    pub fn ambiguous(&self) -> usize {
        self.near + self.under + self.unexplained
    }
}

#[derive(Clone, Debug)]
pub struct LineRow {
    pub renderer: Renderer,
    pub dpi: f64,
    pub pair: &'static str,
    pub orientation: f64,
    pub width_mm: f64,
    pub kind: LineKind,
    pub offset: u8,
    pub d1: Counts,
    pub nearest: Counts,
}

pub fn pair_name(p: Pair) -> &'static str {
    if p == Pair::RED_CYAN {
        "red-cyan"
    } else if p == Pair::GREEN_MAGENTA {
        "green-magenta"
    } else if p == Pair::AZURE_ORANGE {
        "azure-orange"
    } else if p == Pair::SPRING_ROSE {
        "spring-rose"
    } else {
        "other"
    }
}

pub fn measure_patches(set: &'static str, case: &Case, r: &Render, renderer: Renderer, rows: &mut Vec<PatchRow>) {
    for p in &case.patches {
        let (x0, y0, x1, y1) = r.rect_px(p.rect);
        // The inner half of the patch, so antialiased edges don't count.
        let (qx, qy) = ((x1 - x0) / 4, (y1 - y0) / 4);
        let (ix0, iy0, ix1, iy1) = (x0 + qx, y0 + qy, x1 - qx, y1 - qy);
        let mean = |img: &Image| {
            let mut s = [0.0; 3];
            let mut n = 0.0;
            for y in iy0..iy1 {
                for x in ix0..ix1 {
                    if let Some(px) = img.get(x, y) {
                        for c in 0..3 {
                            s[c] += f64::from(px[c]);
                        }
                        n += 1.0;
                    }
                }
            }
            s.map(|v| if n > 0.0 { v / n } else { f64::NAN })
        };
        let median = measure::median(&r.img, ix0, iy0, ix1, iy1).map(|m| m.map(f64::from)).unwrap_or([f64::NAN; 3]);
        let (value, expected, tol, mask) = match &p.expect {
            Expect::Colour { rgb, tol } => (median, Some(*rgb), *tol, None),
            Expect::Mean { rgb, tol } => (mean(&r.img), Some(*rgb), *tol, None),
            Expect::Mask { m, tol } => {
                let got = 1.0 - (median[0] + median[1] + median[2]) / 3.0 / 255.0;
                (median, None, *tol, Some((got, *m)))
            }
            Expect::Report => (median, None, 0.0, None),
        };
        let error = match (expected, mask) {
            (Some(e), _) => (0..3).map(|c| (value[c] - e[c]).abs()).fold(0.0, f64::max),
            (_, Some((got, want))) => (got - want).abs() * 255.0,
            _ => f64::NAN,
        };
        let pass = if expected.is_some() || mask.is_some() { Some(error <= tol + 1e-9) } else { None };
        rows.push(PatchRow {
            set,
            case: case.id.clone(),
            patch: p.name.clone(),
            criterion: p.criterion,
            renderer,
            dpi: r.dpi,
            value,
            expected,
            mask,
            tol,
            error,
            pass,
        });
    }
}

pub fn measure_targets(set: &'static str, case: &Case, r: &Render, renderer: Renderer, rows: &mut Vec<TargetRow>) {
    for t in &case.targets {
        let (cx, cy) = r.to_px(t.centre.0, t.centre.1);
        let h = t.half * r.scale_x.max(r.scale_y);
        let measured = if t.on_page {
            let (x0, y0) = ((cx - h).max(0.0) as usize, (cy - h).max(0.0) as usize);
            let (x1, y1) = ((cx + h).ceil() as usize, (cy + h).ceil() as usize);
            let pair = case.pair;
            let side = t.side;
            measure::centroid(&r.img, x0, y0, x1, y1, |px| {
                let rec = recover(px, pair);
                if side == Side::A { rec.alpha_a } else { rec.alpha_b }
            })
            .map(|(x, y, _)| r.to_page(x, y))
        } else {
            None
        };
        let error_pt = measured.map(|m| ((m.0 - t.centre.0).powi(2) + (m.1 - t.centre.1).powi(2)).sqrt() * case.placement.unit);
        rows.push(TargetRow {
            set,
            case: case.id.clone(),
            target: t.name.clone(),
            side: t.side,
            renderer,
            dpi: r.dpi,
            expected: t.centre,
            measured,
            error_pt,
            on_page: t.on_page,
            partner: t.partner.clone(),
        });
    }
}

pub fn measure_lines(case: &Case, r: &Render, renderer: Renderer, rows: &mut Vec<LineRow>) {
    for l in &case.lines {
        let (cx, cy) = r.to_px(l.centre.0, l.centre.1);
        // The normal in pixels: y flips.
        let (nx, ny) = (l.normal.0, -l.normal.1);
        let h = l.half_len * r.scale_x.max(r.scale_y);
        let pixels = measure::profile(&r.img, (cx - nx * h, cy - ny * h), (cx + nx * h, cy + ny * h));
        let (mut d1, mut nearest) = (Counts::default(), Counts::default());
        for (x, y) in pixels {
            if let Some(px) = r.img.get(x, y) {
                d1.add(classify(px, case.pair).0);
                nearest.add(nearest_colour(px, case.pair));
            }
        }
        rows.push(LineRow {
            renderer,
            dpi: r.dpi,
            pair: pair_name(case.pair),
            orientation: l.orientation_deg,
            width_mm: l.width_mm,
            kind: l.kind,
            offset: l.offset,
            d1,
            nearest,
        });
    }
}

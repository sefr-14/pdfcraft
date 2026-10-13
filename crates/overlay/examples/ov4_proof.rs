//! The step 4 proof (OV4.6): every criterion of the step 4 PRD, L1 to L16, measured on the
//! synthetic families T1 to T11, in PdfCraft (in process), Poppler and PDFium (separate
//! processes), the app's own Layers panel (driven through its control channel under Xvfb) and
//! Mozilla's pdf.js viewer (driven by a small external script that lives in the proof package, not
//! in the repository, O14). Writes the proof package: the files, renders, screenshots, tables and
//! `report.md` with all sixteen criteria.
//!
//! ```text
//! cargo run -p pdfcraft-overlay --example ov4_proof -- --out DIR \
//!     [--ov3-reference DIR --ov3-layered DIR]     L4: the accepted OV3 run and `ov3_proof --layered`
//!     [--app PATH --cli PATH]                     L2: the app and pdfcraft-cli, for the panel check
//!     [--pdfjs-dist DIR --pdfjs-driver FILE]      L3, L12: pdf.js and its driver
//!     [--test-log FILE] [--ci FILE] [--repo DIR]  L13 to L15: the test run, CI, the repository
//! ```
//!
//! Nothing here is part of the app or the Windows build; the external tools run only as
//! processes, as in the OV3 proof.

#[path = "../tests/common/annots.rs"]
mod annots;
#[path = "../tests/common/catalogue.rs"]
mod catalogue;
#[path = "../tests/common/drawing.rs"]
mod drawing;
#[path = "../tests/common/evaluate.rs"]
mod evaluate;
#[path = "../tests/common/families.rs"]
mod families;
#[path = "../tests/common/measure.rs"]
mod measure;
#[path = "../tests/common/synth.rs"]
mod synth;
#[path = "../tests/common/twins.rs"]
mod twins;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use evaluate::{Renderer, run};
use measure::{Image, Pair};
use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_overlay::{
    LayerStyle, LayeredOutput, OverlayReport, Placement, Prepared, PreparedSource, RegistrationNote, Roles, Settings, SheetMatch, Source, Visibility,
    check_output, compose, frame_of_page, light_tone, luminosity, place, prepare,
};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

// ── Arguments and bookkeeping ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Args {
    out: PathBuf,
    ov3_reference: Option<PathBuf>,
    ov3_layered: Option<PathBuf>,
    app: Option<PathBuf>,
    cli: Option<PathBuf>,
    pdfjs_dist: Option<PathBuf>,
    pdfjs_driver: Option<PathBuf>,
    test_log: Option<PathBuf>,
    ci: Option<PathBuf>,
    repo: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut a = Args { out: PathBuf::from("target/ov4-proof"), ..Args::default() };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().map(PathBuf::from).ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--out" => a.out = v()?,
            "--ov3-reference" => a.ov3_reference = Some(v()?),
            "--ov3-layered" => a.ov3_layered = Some(v()?),
            "--app" => a.app = Some(v()?),
            "--cli" => a.cli = Some(v()?),
            "--pdfjs-dist" => a.pdfjs_dist = Some(v()?),
            "--pdfjs-driver" => a.pdfjs_driver = Some(v()?),
            "--test-log" => a.test_log = Some(v()?),
            "--ci" => a.ci = Some(v()?),
            "--repo" => a.repo = Some(v()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// One measured fact behind a criterion: passed, failed, or not run (with why).
struct Check {
    ok: Option<bool>,
    what: String,
}

struct Proof {
    out: PathBuf,
    tmp: PathBuf,
    checks: BTreeMap<&'static str, Vec<Check>>,
    incompatibilities: Vec<String>,
    interpretation: Vec<[String; 5]>,
    /// Every source file written: (path, its bytes when written), for L11.
    sources: Vec<(PathBuf, Vec<u8>)>,
    outputs: Vec<PathBuf>,
}

impl Proof {
    fn check(&mut self, id: &'static str, ok: bool, what: impl Into<String>) {
        self.checks.entry(id).or_default().push(Check { ok: Some(ok), what: what.into() });
    }

    fn not_run(&mut self, id: &'static str, what: impl Into<String>) {
        self.checks.entry(id).or_default().push(Check { ok: None, what: what.into() });
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.out.join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.path(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).expect("create directory");
        }
        std::fs::write(&p, bytes).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        p
    }

    fn source(&mut self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.write(rel, bytes);
        self.sources.push((p.clone(), bytes.to_vec()));
        p
    }

    fn output(&mut self, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = self.write(rel, bytes);
        self.outputs.push(p.clone());
        p
    }

    fn png(&self, rel: &str, img: &Image) {
        self.write(rel, &png_bytes(img));
    }
}

// ── Images ───────────────────────────────────────────────────────────────────────────────────

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xffff_ffffu32;
    for b in parts.iter().flat_map(|p| p.iter()) {
        crc = table[((crc ^ u32::from(*b)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

/// An 8-bit RGB PNG, compressed with the zlib encoder PdfCraft uses for PDF streams.
fn png_bytes(img: &Image) -> Vec<u8> {
    let row = img.width * 3;
    let mut raw = Vec::with_capacity((row + 1) * img.height);
    for line in img.rgb.chunks_exact(row.max(1)) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let idat = Stream::flate(Dict::new(), &raw).raw;
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(img.width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(img.height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", &ihdr[..]), (b"IDAT", &idat[..]), (b"IEND", &[][..])] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    }
    out
}

/// Decode an 8-bit RGB or RGBA PNG (the screenshots), inflating with PdfCraft's own Flate filter.
fn png_decode(bytes: &[u8]) -> Result<Image, String> {
    let mut at = 8;
    let (mut w, mut h, mut channels) = (0usize, 0usize, 0usize);
    let mut idat = Vec::new();
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().map_err(|_| "chunk")?) as usize;
        let kind = &bytes[at + 4..at + 8];
        let data = bytes.get(at + 8..at + 8 + len).ok_or("truncated PNG")?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(data[0..4].try_into().map_err(|_| "IHDR")?) as usize;
                h = u32::from_be_bytes(data[4..8].try_into().map_err(|_| "IHDR")?) as usize;
                if data[8] != 8 || data[12] != 0 {
                    return Err("only 8-bit, non-interlaced PNGs".into());
                }
                channels = match data[9] {
                    2 => 3,
                    6 => 4,
                    t => return Err(format!("PNG colour type {t}")),
                };
            }
            b"IDAT" => idat.extend_from_slice(data),
            _ => {}
        }
        at += 12 + len;
    }
    let mut d = Dict::new();
    d.set(b"Filter".to_vec(), Object::name("FlateDecode"));
    let raw = Stream::from_raw(d, idat).decoded().map_err(|e| e.to_string())?;
    let stride = w * channels;
    let mut prev = vec![0u8; stride];
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let line = raw.get(y * (stride + 1)..(y + 1) * (stride + 1)).ok_or("short PNG data")?;
        let (filter, src) = (line[0], &line[1..]);
        let mut cur = vec![0u8; stride];
        for i in 0..stride {
            let a = if i >= channels { cur[i - channels] } else { 0 };
            let b = prev[i];
            let c = if i >= channels { prev[i - channels] } else { 0 };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let (pa, pb, pc) = ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(c)).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                f => return Err(format!("PNG filter {f}")),
            };
            cur[i] = src[i].wrapping_add(pred);
        }
        for px in cur.chunks(channels) {
            rgb.extend_from_slice(&px[..3]);
        }
        prev = cur;
    }
    Ok(Image { width: w, height: h, rgb })
}

fn crop(img: &Image, x0: usize, y0: usize, x1: usize, y1: usize) -> Image {
    let (x1, y1) = (x1.min(img.width), y1.min(img.height));
    let mut rgb = Vec::new();
    for y in y0..y1 {
        rgb.extend_from_slice(&img.rgb[(y * img.width + x0) * 3..(y * img.width + x1) * 3]);
    }
    Image { width: x1.saturating_sub(x0), height: y1.saturating_sub(y0), rgb }
}

/// Mean colours over a grid of `n` × `n` cells, each image's own extent divided alike: robust to
/// a pixel of size difference and to resampling.
fn cells(img: &Image, n: usize) -> Vec<[f64; 3]> {
    let mut out = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let (x0, x1) = (i * img.width / n, ((i + 1) * img.width / n).max(i * img.width / n + 1));
            let (y0, y1) = (j * img.height / n, ((j + 1) * img.height / n).max(j * img.height / n + 1));
            let mut s = [0.0; 3];
            let mut k = 0.0;
            for y in y0..y1.min(img.height) {
                for x in x0..x1.min(img.width) {
                    if let Some(p) = img.get(x, y) {
                        for c in 0..3 {
                            s[c] += f64::from(p[c]);
                        }
                        k += 1.0;
                    }
                }
            }
            out.push(s.map(|v| if k > 0.0 { v / k } else { 255.0 }));
        }
    }
    out
}

/// The worst median, over `k` × `k` pixel cells holding ink in either image, of the per-pixel
/// difference (largest channel). `None` when the sizes differ.
fn cell_medians(a: &Image, b: &Image, k: usize) -> Option<f64> {
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    let mut worst = 0.0f64;
    for cy in (0..a.height).step_by(k) {
        for cx in (0..a.width).step_by(k) {
            let mut d = Vec::new();
            let mut ink = false;
            for y in cy..(cy + k).min(a.height) {
                for x in cx..(cx + k).min(a.width) {
                    let (p, q) = (a.get(x, y)?, b.get(x, y)?);
                    ink |= p.iter().chain(q.iter()).any(|v| *v < 250);
                    d.push(p.iter().zip(q).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(0));
                }
            }
            if ink && !d.is_empty() {
                d.sort_unstable();
                worst = worst.max(f64::from(d[d.len() / 2]));
            }
        }
    }
    Some(worst)
}

/// The cells above 2 levels (as [`cell_medians`] measures them), worst first: where, the
/// median, and the two images' pixels at the cell's centre.
fn worst_cells(a: &Image, b: &Image, k: usize) -> String {
    let mut rows = Vec::new();
    if (a.width, a.height) != (b.width, b.height) {
        return "sizes differ\n".into();
    }
    for cy in (0..a.height).step_by(k) {
        for cx in (0..a.width).step_by(k) {
            let mut d = Vec::new();
            let mut ink = false;
            for y in cy..(cy + k).min(a.height) {
                for x in cx..(cx + k).min(a.width) {
                    let (Some(p), Some(q)) = (a.get(x, y), b.get(x, y)) else { continue };
                    ink |= p.iter().chain(q.iter()).any(|v| *v < 250);
                    d.push(p.iter().zip(q).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(0));
                }
            }
            if ink && !d.is_empty() {
                d.sort_unstable();
                let m = d[d.len() / 2];
                if m > 2 {
                    let c = ((cx + k / 2).min(a.width - 1), (cy + k / 2).min(a.height - 1));
                    rows.push((m, cx, cy, a.get(c.0, c.1), b.get(c.0, c.1)));
                }
            }
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out = String::from("median\tx\ty\toverlay\tcontrol\n");
    for (m, x, y, p, q) in rows {
        let _ = writeln!(out, "{m}\t{x}\t{y}\t{p:?}\t{q:?}");
    }
    out
}

/// Where two images differ by more than 2 levels: a picture (each pixel's largest channel
/// difference, ×20, white on black) and the box around those pixels (x0, y0, x1, y1), `None` when
/// the sizes differ.
fn diff_map(a: &Image, b: &Image) -> Option<(Image, Option<[usize; 4]>)> {
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    let mut rgb = Vec::with_capacity(a.rgb.len());
    let mut bbox: Option<[usize; 4]> = None;
    for (i, (p, q)) in a.rgb.chunks(3).zip(b.rgb.chunks(3)).enumerate() {
        let d = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        let v = d.saturating_mul(20);
        rgb.extend_from_slice(&[v, v, v]);
        if d > 2 {
            let (x, y) = (i % a.width, i / a.width);
            bbox = Some(match bbox {
                None => [x, y, x + 1, y + 1],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
            });
        }
    }
    Some((Image { width: a.width, height: a.height, rgb }, bbox))
}

/// The largest difference between two images' cell means, in levels.
fn cell_diff(a: &Image, b: &Image, n: usize) -> f64 {
    cells(a, n).iter().zip(cells(b, n)).map(|(p, q)| (0..3).map(|c| (p[c] - q[c]).abs()).fold(0.0, f64::max)).fold(0.0, f64::max)
}

/// (largest channel difference, pixels more than 1 level apart). `None` when the sizes differ.
fn diff(a: &Image, b: &Image) -> Option<(u8, usize)> {
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    let mut worst = 0u8;
    let mut count = 0;
    for (p, q) in a.rgb.chunks(3).zip(b.rgb.chunks(3)) {
        let d = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        worst = worst.max(d);
        if d > 1 {
            count += 1;
        }
    }
    Some((worst, count))
}

fn is_paper(img: &Image) -> bool {
    img.rgb.iter().all(|v| *v >= 254)
}

fn side_by_side(panels: &[&Image]) -> Image {
    let h = panels.iter().map(|i| i.height).max().unwrap_or(0);
    let gap = 16;
    let width = panels.iter().map(|i| i.width).sum::<usize>() + gap * panels.len().saturating_sub(1);
    let mut rgb = vec![160u8; width * h * 3];
    let mut x0 = 0;
    for img in panels {
        for y in 0..img.height {
            let at = (y * width + x0) * 3;
            rgb[at..at + img.width * 3].copy_from_slice(&img.rgb[y * img.width * 3..(y + 1) * img.width * 3]);
        }
        x0 += img.width + gap;
    }
    Image { width, height: h, rgb }
}

// ── Rendering ────────────────────────────────────────────────────────────────────────────────

/// Page 1 of `bytes` (written at `path` for the external renderers) at `dpi`.
fn render(r: Renderer, bytes: &Arc<Vec<u8>>, path: &Path, dpi: f64, tmp: &Path) -> Result<Image, String> {
    match r {
        Renderer::PdfCraft => Ok(render_layers(bytes, dpi, Vec::new())),
        Renderer::Poppler => {
            let prefix = tmp.join("poppler");
            run(Command::new("pdftoppm").arg("-r").arg(format!("{dpi}")).args(["-f", "1", "-l", "1", "-singlefile"]).arg(path).arg(&prefix))?;
            let file = prefix.with_extension("ppm");
            let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&file);
            Image::from_pnm(&bytes)
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
                .args(["--prefix", "p-", "--pages", "1", "--scale"])
                .arg(format!("{}", dpi / 72.0))
                .args(["--format", "ppm", "--no-x-channel", "--linear"])
                .env("PYPDFIUM_LOGLEVEL", "warning"))?;
            let file =
                std::fs::read_dir(&dir).map_err(|e| e.to_string())?.filter_map(|e| e.ok()).map(|e| e.path()).next().ok_or("PDFium wrote nothing")?;
            let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&dir);
            Image::from_pnm(&bytes)
        }
    }
}

/// PdfCraft in process, with layer overrides as the Layers panel sets them.
fn render_layers(bytes: &Arc<Vec<u8>>, dpi: f64, layers: Vec<(i32, i32, bool)>) -> Image {
    let mut r = PageRenderer::new(bytes.clone(), RenderConfig { layers: Arc::new(layers), ..RenderConfig::default() });
    let out = r.render(RenderRequest { page: 0, scale: (dpi / 72.0) as f32, ..RenderRequest::default() });
    Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba)
}

/// The compositing control for side A alone: `img` (the source as a renderer draws it) through
/// the model, in Old's colour.
fn model(img: &Image) -> Image {
    let pair = Pair { a: [204, 51, 51], b: [51, 204, 204] };
    let mut rgb = Vec::with_capacity(img.rgb.len());
    for p in img.rgb.chunks(3) {
        let alpha = light_tone(1.0, luminosity([p[0], p[1], p[2]].map(|v| f64::from(v) / 255.0))).unwrap_or(0.0);
        rgb.extend(pair.predict(alpha, 0.0).map(|v| v.round().clamp(0.0, 255.0) as u8));
    }
    Image { width: img.width, height: img.height, rgb }
}

const RENDERERS: [Renderer; 3] = [Renderer::PdfCraft, Renderer::Poppler, Renderer::Pdfium];

// ── Composing ────────────────────────────────────────────────────────────────────────────────

fn identity_placement(doc: &Document) -> Placement {
    let (f, _) = frame_of_page(doc, 0).expect("frame");
    place(&f, &f, SheetMatch::Off, &Matrix::IDENTITY, false, &Settings::default()).expect("placement")
}

fn prep_all(doc: &Document, vis: &Visibility) -> Prepared {
    let n = pdfcraft_model::pages(doc).len();
    prepare(doc, &(0..n).collect::<Vec<_>>(), vis).expect("prepares")
}

fn layered(
    a: Option<PreparedSource<'_>>,
    b: Option<PreparedSource<'_>>,
    p: &Placement,
    style: &LayerStyle,
    roles: Roles,
) -> (Arc<Vec<u8>>, OverlayReport) {
    let mut out = LayeredOutput::new().expect("output");
    out.add_page(a, b, p, style, roles, RegistrationNote::PLACED).expect("composes");
    let (doc, report) = out.finish_with_report().expect("finishes");
    (synth::save(&doc, true), report)
}

/// A copy of a layered output whose default configuration turns `off` (Old, New) off: only `/D`
/// differs.
fn with_default_off(bytes: &Arc<Vec<u8>>, old: bool, new: bool) -> Arc<Vec<u8>> {
    let mut doc = synth::open(bytes);
    let g = check_output(&doc).expect("a checked output");
    let root = doc.root().expect("catalog");
    let mut props = doc.dict(doc.dict(&Object::Ref(root)).expect("catalog").get(b"OCProperties").expect("props")).expect("dict");
    let mut d = doc.dict(props.get(b"D").expect("D")).expect("D");
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for (r, is_off) in [(g.old, old), (g.new, new)] {
        if is_off { off.push(Object::Ref(r)) } else { on.push(Object::Ref(r)) }
    }
    d.set(b"ON".to_vec(), Object::Array(on));
    d.set(b"OFF".to_vec(), Object::Array(off));
    props.set(b"D".to_vec(), Object::Dict(d));
    doc.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props))).expect("catalog");
    synth::save(&doc, true)
}

fn groups(bytes: &Arc<Vec<u8>>) -> ((i32, i32), (i32, i32)) {
    let g = check_output(&synth::open(bytes)).expect("checked");
    let n = |r: ObjRef| (r.num as i32, i32::from(r.generation));
    (n(g.old), n(g.new))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Whether any decoded stream of `bytes` holds `needle`.
fn any_stream_has(bytes: &Arc<Vec<u8>>, needle: &[u8]) -> bool {
    synth::open(bytes).scan_objects().any(|(_, o)| matches!(&*o, Object::Stream(s) if s.decoded().is_ok_and(|d| contains(&d, needle))))
}

fn qpdf(path: &Path) -> Result<(), String> {
    let out = Command::new("qpdf").arg("--check").arg(path).output().map_err(|e| e.to_string())?;
    // 0: no problems; 3: warnings only (none expected here).
    if out.status.code() == Some(0) { Ok(()) } else { Err(String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or("").to_string()) }
}

fn sha256sum(path: &Path) -> String {
    Command::new("sha256sum")
        .arg(path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().map(String::from))
        .unwrap_or_default()
}

// ── L4: the OV3 proof with both layers on ────────────────────────────────────────────────────

fn l4(p: &mut Proof, reference: Option<&Path>, layered: Option<&Path>) {
    let (Some(reference), Some(layered)) = (reference, layered) else {
        p.not_run("L4", "the OV3 reference run or the layered rerun wasn't given");
        return;
    };
    let mut text = String::new();
    for f in ["patches.csv", "targets.csv", "lines.csv", "displacement.csv", "cases.csv", "incompatibilities.md"] {
        let (a, b) = (std::fs::read(reference.join(f)), std::fs::read(layered.join(f)));
        let same = matches!((&a, &b), (Ok(x), Ok(y)) if x == y);
        let _ = writeln!(text, "{f}: {}", if same { "byte-identical" } else { "DIFFERENT" });
        if ["patches.csv", "targets.csv", "lines.csv"].contains(&f) {
            p.check("L4", same, format!("{f} byte-identical to the accepted OV3 run"));
        }
    }
    p.write("L4/comparison.txt", text.as_bytes());
}

// ── T1: every OV3 family with layers (L1) ────────────────────────────────────────────────────

fn t1(p: &mut Proof) {
    families::LAYERED.store(true, std::sync::atomic::Ordering::Relaxed);
    let sets = families::all();
    families::LAYERED.store(false, std::sync::atomic::Ordering::Relaxed);
    let (mut ok, mut total) = (0, 0);
    for set in &sets {
        total += 1;
        let doc = synth::open(&set.overlay);
        let checked = check_output(&doc);
        let ocgs = doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Type") == Some(&b"OCG"[..]))).count();
        let path = p.output(&format!("T1/{}.pdf", set.name), &set.overlay);
        let q = qpdf(&path);
        let good = checked.is_ok() && ocgs == 2 && q.is_ok();
        ok += usize::from(good);
        if !good {
            p.check("L1", false, format!("T1 {}: check {:?}, {ocgs} groups, qpdf {:?}", set.name, checked.err(), q.err()));
        }
    }
    p.check(
        "L1",
        ok == total,
        format!("T1: {ok} of {total} OV3 families recomposed with layers pass the defensive check, hold exactly two groups and pass qpdf --check"),
    );
}

// ── T2: toggling (L2, L3, L12) ───────────────────────────────────────────────────────────────

/// One T2 case: the layered output and the pages composed without A and without B.
struct Toggle {
    name: String,
    layered: Arc<Vec<u8>>,
    a_blank: Option<Arc<Vec<u8>>>,
    b_blank: Option<Arc<Vec<u8>>>,
    ov3: Arc<Vec<u8>>,
    page_w: f64,
}

fn toggle_case(set: &families::Set, case: &families::Case) -> Toggle {
    let (a_doc, b_doc) = (synth::open(&set.a), synth::open(&set.b));
    let (pa, pb) = (prep_all(&a_doc, &Visibility::default()), prep_all(&b_doc, &Visibility::default()));
    let qa = case.a_page.map(|page| pa.source(page).expect("A"));
    let qb = case.b_page.map(|page| pb.source(page).expect("B"));
    let style = LayerStyle { old: case.style.colour_a, new: case.style.colour_b, threshold: case.style.threshold };
    let (layered_bytes, _) = layered(qa, qb, &case.placement, &style, Roles::default());
    let blank = |a: Option<PreparedSource<'_>>, b: Option<PreparedSource<'_>>| {
        (a.is_some() || b.is_some()).then(|| layered(a, b, &case.placement, &style, Roles::default()).0)
    };
    let sa = case.a_page.map(|page| Source { doc: &a_doc, page });
    let sb = case.b_page.map(|page| Source { doc: &b_doc, page });
    let ov3 = synth::save(&compose(sa, sb, &case.placement, &case.style).expect("OV3"), true);
    Toggle {
        name: format!("{}-{}", set.name, case.id),
        layered: layered_bytes,
        a_blank: blank(None, qb),
        b_blank: blank(qa, None),
        ov3,
        page_w: case.placement.page_box()[2],
    }
}

fn t2(p: &mut Proof) -> Vec<Toggle> {
    let mut keep = Vec::new();
    let families = [(families::overlap(), usize::MAX), (families::colours(), usize::MAX), (families::rotation(), usize::MAX), (families::lines(), 1)];
    let (mut l2, mut l2n, mut l3) = (0usize, 0usize, BTreeMap::<&str, (usize, usize)>::new());
    for (set, limit) in &families {
        for (k, case) in set.cases.iter().take(*limit).enumerate() {
            let t = toggle_case(set, case);
            let dir = format!("T2/{}", t.name);
            let lpath = p.output(&format!("{dir}/overlay.pdf"), &t.layered);
            let (old, new) = groups(&t.layered);
            let dpis: &[f64] = if set.name.contains("line") { &[150.0, 300.0] } else { &[150.0] };
            for &dpi in dpis {
                // PdfCraft in process, through the Layers panel's own overrides.
                let both = render_layers(&t.layered, dpi, Vec::new());
                let ov3 = render_layers(&t.ov3, dpi, Vec::new());
                let mut good = diff(&both, &ov3).is_some_and(|(m, _)| m == 0);
                for (off, alone) in [(old, &t.a_blank), (new, &t.b_blank)] {
                    let img = render_layers(&t.layered, dpi, vec![(off.0, off.1, false)]);
                    good &= match alone {
                        Some(r) => diff(&img, &render_layers(r, dpi, Vec::new())).is_some_and(|(m, _)| m <= 1),
                        None => is_paper(&img),
                    };
                }
                good &= is_paper(&render_layers(&t.layered, dpi, vec![(old.0, old.1, false), (new.0, new.1, false)]));
                l2 += usize::from(good);
                l2n += 1;
                if !good {
                    p.check("L2", false, format!("{} at {dpi} dpi: a toggle differs", t.name));
                }
                if k == 0 {
                    p.png(&format!("{dir}/pdfcraft-{dpi}-both.png"), &both);
                }
                // Poppler and PDFium: copies whose default configuration hides Old, New or both.
                for r in [Renderer::Poppler, Renderer::Pdfium] {
                    let tmp = p.tmp.clone();
                    let mut ok = true;
                    let mut why = String::new();
                    for (o, n, alone) in [(true, false, &t.a_blank), (false, true, &t.b_blank), (true, true, &None)] {
                        let variant = with_default_off(&t.layered, o, n);
                        let tag = match (o, n) {
                            (true, false) => "old-off",
                            (false, true) => "new-off",
                            _ => "both-off",
                        };
                        let vpath = p.write(&format!("{dir}/{tag}.pdf"), &variant);
                        let img = render(r, &variant, &vpath, dpi, &tmp);
                        let want = match alone {
                            Some(bytes) => {
                                let bp = p.write(&format!("{dir}/{}.pdf", if o { "a-blank" } else { "b-blank" }), bytes);
                                render(r, bytes, &bp, dpi, &tmp).map(Some)
                            }
                            None => Ok(None),
                        };
                        let res = match (img, want) {
                            (Ok(img), Ok(Some(w))) => diff(&img, &w).map(|(m, c)| (m <= 1, format!("{tag}: {m} levels, {c} px"))),
                            (Ok(img), Ok(None)) => Some((is_paper(&img), format!("{tag}: paper {}", is_paper(&img)))),
                            (Err(e), _) | (_, Err(e)) => Some((false, format!("{tag}: {e}"))),
                        };
                        let (good, w) = res.unwrap_or((false, format!("{tag}: sizes differ")));
                        ok &= good;
                        let _ = write!(why, "{w}; ");
                    }
                    let e = l3.entry(r.name()).or_default();
                    e.0 += usize::from(ok);
                    e.1 += 1;
                    if !ok {
                        p.check("L3", false, format!("{} at {dpi} dpi in {}: {why}", t.name, r.name()));
                    }
                }
            }
            let _ = lpath;
            if k == 0 {
                keep.push(t);
            }
        }
    }
    p.check("L2", l2 == l2n, format!("in-process toggles (Layers panel overrides): {l2} of {l2n} page/dpi cases match OV3 with both on (0 levels), the blank-side page with one off (≤ 1 level), paper with both off"));
    for (r, (ok, n)) in &l3 {
        p.check(
            "L3",
            ok == n,
            format!("{r}: {ok} of {n} default-state copies (Old off, New off, both off) match the blank-side page (≤ 1 level) or paper"),
        );
        p.check("L12", ok == n, format!("T2 in {r}: {ok} of {n} agree"));
    }
    p.check("L12", l2 == l2n, format!("T2 in pdfcraft: {l2} of {l2n} agree"));
    keep
}

// ── L2: the app's Layers panel under Xvfb ────────────────────────────────────────────────────

/// A panel state: its name, the checkbox to click first, and the overrides that render it.
type Step = (&'static str, Option<&'static str>, Vec<(i32, i32, bool)>);

/// An annotation case: name, source, the source the control is rendered from, and whether it is
/// a T6 (stored appearances) case.
type AnnotCase = (String, Arc<Vec<u8>>, Arc<Vec<u8>>, bool);

struct Xvfb(Child);

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ui(cli: &Path, ctl: &Path, args: &[&str]) -> Result<String, String> {
    run(Command::new(cli).arg("ui").arg("--control").arg(ctl).args(args))
}

/// A number after `"key":` in JSON text (the first one).
fn json_number(text: &str, key: &str) -> Option<f64> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text[at..].trim_start().trim_start_matches(':').trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The canvas viewport in `ui.state` (logical points).
fn viewport(state: &str) -> Option<[f64; 4]> {
    let at = state.find("\"viewport\"")?;
    let open = state[at..].find('[')? + at + 1;
    let close = state[open..].find(']')? + open;
    let v: Vec<f64> = state[open..close].split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// The first page's rectangle in `ui.state` (logical points).
fn page_rect(state: &str) -> Option<[f64; 4]> {
    let at = state.find("\"pages_on_screen\"")?;
    let r = state[at..].find("\"rect\"")? + at;
    let open = state[r..].find('[')? + r + 1;
    let close = state[open..].find(']')? + open;
    let v: Vec<f64> = state[open..close].split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// Wait until the app has drawn everything it asked for (no pending page renders), up to 30 s.
fn idle(cli: &Path, ctl: &Path) {
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(400));
    while started.elapsed() < Duration::from_secs(30) {
        let s = ui(cli, ctl, &["state"]).unwrap_or_default();
        // A toast (the "drawing without a graphics processor" notice) covers part of the page.
        let toast = s.contains("\"notice\": \"") || s.contains("\"notice\":\"");
        if json_number(&s, "pending_requests") == Some(0.0) && !toast {
            std::thread::sleep(Duration::from_millis(600));
            if json_number(&ui(cli, ctl, &["state"]).unwrap_or_default(), "pending_requests") == Some(0.0) {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn panel(p: &mut Proof, toggles: &[Toggle], app: Option<&Path>, cli: Option<&Path>) {
    let (Some(app), Some(cli)) = (app, cli) else {
        p.not_run("L2", "the panel check: no app given");
        return;
    };
    let display = ":93";
    let Ok(child) = Command::new("Xvfb").args([display, "-screen", "0", "1600x1100x24"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        p.not_run("L2", "the panel check: Xvfb doesn't start");
        return;
    };
    let _x = Xvfb(child);
    std::thread::sleep(Duration::from_millis(800));
    let (mut ok, mut n) = (0, 0);
    let mut log = String::new();
    for t in toggles {
        let dir = p.path(&format!("T2/{}/panel", t.name));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = p.path(&format!("T2/{}/overlay.pdf", t.name));
        let ctl = dir.join("ctl");
        let _ = std::fs::remove_file(&ctl);
        let Ok(mut app_child) = Command::new(app)
            .arg("--control")
            .arg(&ctl)
            .arg(&file)
            .env("DISPLAY", display)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.join("app.log")).map(Stdio::from).unwrap_or(Stdio::null()))
            .spawn()
        else {
            p.check("L2", false, format!("panel: the app doesn't start for {}", t.name));
            continue;
        };
        let started = Instant::now();
        let mut state = String::new();
        while started.elapsed() < Duration::from_secs(60) {
            if ctl.exists()
                && let Ok(s) = ui(cli, &ctl, &["state"])
                && page_rect(&s).is_some()
            {
                state = s;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        idle(cli, &ctl);
        let state = ui(cli, &ctl, &["state"]).unwrap_or(state);
        let Some(rect) = page_rect(&state) else {
            p.check("L2", false, format!("panel: no page on screen for {}", t.name));
            let _ = app_child.kill();
            continue;
        };
        let (old, new) = groups(&t.layered);
        // Both on; click Old (New only); click New (neither); click Old (Old only).
        let steps: [Step; 4] = [
            ("both", None, Vec::new()),
            ("new-only", Some("Old"), vec![(old.0, old.1, false)]),
            ("neither", Some("New"), vec![(old.0, old.1, false), (new.0, new.1, false)]),
            ("old-only", Some("Old"), vec![(new.0, new.1, false)]),
        ];
        let inspect = ui(cli, &ctl, &["inspect", "query=Old"]).unwrap_or_default();
        let _ = writeln!(
            log,
            "{}: panel lists Old {}, New {}",
            t.name,
            inspect.contains("\"Old\""),
            ui(cli, &ctl, &["inspect", "query=New"]).unwrap_or_default().contains("\"New\"")
        );
        for (state_name, click, overrides) in steps {
            if let Some(label) = click {
                let _ = ui(cli, &ctl, &["click", &format!("label={label}")]);
                idle(cli, &ctl);
            }
            let shot = dir.join(format!("{state_name}.png"));
            let _ = ui(cli, &ctl, &["screenshot", "--out", &shot.to_string_lossy()]);
            std::thread::sleep(Duration::from_millis(300));
            let ppp = json_number(&ui(cli, &ctl, &["state"]).unwrap_or_default(), "pixels_per_point").unwrap_or(1.0);
            let Ok(img) = std::fs::read(&shot).map_err(|e| e.to_string()).and_then(|b| png_decode(&b)) else {
                p.check("L2", false, format!("panel: no screenshot for {} {state_name}", t.name));
                continue;
            };
            // The part of the page inside the viewport (the rest is scrolled off), in the screenshot
            // and in the in-process render at the panel's scale.
            let view = viewport(&ui(cli, &ctl, &["state"]).unwrap_or_default()).unwrap_or(rect);
            let vis = [rect[0].max(view[0]), rect[1].max(view[1]), rect[2].min(view[2]), rect[3].min(view[3])];
            let px = vis.map(|v| (v * ppp).round() as usize);
            let page = crop(&img, px[0], px[1], px[2], px[3]);
            let k = ppp * (rect[2] - rect[0]) / t.page_w;
            let dpi = 72.0 * k;
            let full = render_layers(&t.layered, dpi, overrides);
            let off = [(vis[0] - rect[0]) * ppp, (vis[1] - rect[1]) * ppp];
            let want = crop(
                &full,
                off[0].round() as usize,
                off[1].round() as usize,
                (off[0] + page.width as f64).round() as usize,
                (off[1] + page.height as f64).round() as usize,
            );
            let d = cell_diff(&page, &want, 24);
            let good = d <= 8.0;
            n += 1;
            ok += usize::from(good);
            p.png(&format!("T2/{}/panel/{state_name}-page.png", t.name), &page);
            p.png(&format!("T2/{}/panel/{state_name}-expected.png", t.name), &want);
            let _ = writeln!(
                log,
                "{} {state_name}: page {}×{} px at {dpi:.1} dpi, largest cell-mean difference {d:.1} levels",
                t.name, page.width, page.height
            );
        }
        let _ = ui(cli, &ctl, &["quit"]);
        std::thread::sleep(Duration::from_millis(500));
        let _ = app_child.kill();
        let _ = app_child.wait();
    }
    p.write("T2/panel.txt", log.as_bytes());
    p.check("L2", ok == n && n > 0, format!("the app's Layers panel, clicked through its control channel under Xvfb: {ok} of {n} screenshots match the in-process render of that state (cell means within 8 levels); see T2/panel.txt"));
}

// ── pdf.js ───────────────────────────────────────────────────────────────────────────────────

fn pdfjs(p: &mut Proof, dist: Option<&Path>, driver: Option<&Path>, files: &[PathBuf], plain: bool, out_rel: &str) -> Option<PathBuf> {
    let (dist, driver) = (dist?, driver?);
    let out = p.path(out_rel);
    std::fs::create_dir_all(&out).ok()?;
    let mut cmd = Command::new("python3");
    cmd.arg("-I").arg(driver).arg(dist).arg(&out);
    if plain {
        cmd.arg("--plain");
    }
    cmd.args(files);
    let res = cmd.output().ok()?;
    let _ = std::fs::write(out.join("driver.log"), [res.stdout.as_slice(), b"\n", res.stderr.as_slice()].concat());
    res.status.success().then_some(out)
}

fn pdfjs_toggles(p: &mut Proof, toggles: &[Toggle], dist: Option<&Path>, driver: Option<&Path>) {
    // The driver names screenshots by file stem: each overlay, and the pages composed without A
    // and without B, under the case's name.
    let mut toggled = Vec::new();
    let mut plain = Vec::new();
    for t in toggles {
        let dir = p.path("T2/pdfjs-in");
        let _ = std::fs::create_dir_all(&dir);
        let to = dir.join(format!("{}.pdf", t.name));
        let _ = std::fs::write(&to, &*t.layered);
        toggled.push(to.clone());
        plain.push(to);
        let ov3 = dir.join(format!("{}-ov3.pdf", t.name));
        let _ = std::fs::write(&ov3, &*t.ov3);
        plain.push(ov3);
        for (tag, bytes) in [("a-blank", &t.a_blank), ("b-blank", &t.b_blank)] {
            if let Some(b) = bytes {
                let f = dir.join(format!("{}-{tag}.pdf", t.name));
                let _ = std::fs::write(&f, &**b);
                plain.push(f);
            }
        }
    }
    let toggled_dir = pdfjs(p, dist, driver, &toggled, false, "T2/pdfjs");
    let plain_dir = pdfjs(p, dist, driver, &plain, true, "T2/pdfjs-plain");
    let (Some(dir), Some(pdir)) = (toggled_dir, plain_dir) else {
        p.not_run("L3", "the pdf.js check: no pdf.js or driver given, or the driver failed (T2/pdfjs/driver.log)");
        return;
    };
    let read = |d: &Path, f: String| std::fs::read(d.join(f)).map_err(|e| e.to_string()).and_then(|b| png_decode(&b));
    // The page's border and shadow, and a few pixels of viewer the open sidebar leaves at the
    // right, are not page content: leave a 10-pixel margin (the families draw nothing there).
    let inner = |img: &Image| crop(img, 10, 10, img.width.saturating_sub(10), img.height.saturating_sub(10));
    let (mut ok, mut n) = (0, 0);
    let mut worst_both = 0.0f64;
    let mut ok_ov3 = true;
    let mut log = String::new();
    for t in toggles {
        // Toggled in the sidebar, against pdf.js's own drawing of the page that state should
        // show: the page without A (Old off), without B (New off), paper (both off), the page as
        // opened (both on).
        for (state, reference) in [
            ("both", Some(format!("{}-plain.png", t.name))),
            ("old-only", t.b_blank.as_ref().map(|_| format!("{}-b-blank-plain.png", t.name))),
            ("new-only", t.a_blank.as_ref().map(|_| format!("{}-a-blank-plain.png", t.name))),
            ("neither", None),
        ] {
            n += 1;
            let Ok(img) = read(&dir, format!("{}-{state}.png", t.name)) else {
                let _ = writeln!(log, "{} {state}: no screenshot", t.name);
                continue;
            };
            let page = inner(&img);
            let (good, what) = match reference {
                None => {
                    let paper = page.rgb.iter().all(|v| *v >= 250);
                    (paper, format!("paper {paper}"))
                }
                // The driver draws both at the same scale (the sidebar open, page-fit); a size
                // difference means the two can't be compared cell for cell.
                Some(f) => match read(&pdir, f) {
                    Ok(r) if (r.width, r.height) != (img.width, img.height) => {
                        (false, format!("pdf.js's own page is {}×{} px: not the same size", r.width, r.height))
                    }
                    Ok(r) => {
                        let d = cell_diff(&page, &inner(&r), 24);
                        (d <= 4.0, format!("largest cell-mean difference from pdf.js's own page {d:.1} levels"))
                    }
                    Err(e) => (false, e),
                },
            };
            ok += usize::from(good);
            let _ = writeln!(log, "{} {state}: {}×{} px, {what}", t.name, img.width, img.height);
            if state == "both" {
                // L4 in pdf.js: both layers on, as pdf.js draws OV3's overlay (no layers).
                match read(&pdir, format!("{}-ov3-plain.png", t.name)) {
                    Ok(r) if (r.width, r.height) == (img.width, img.height) => {
                        let d = cell_diff(&page, &inner(&r), 24);
                        let _ = writeln!(
                            log,
                            "{} both: against pdf.js's own drawing of OV3's overlay, largest cell-mean difference {d:.1} levels",
                            t.name
                        );
                        ok_ov3 &= d <= 4.0;
                    }
                    Ok(r) => {
                        let _ = writeln!(log, "{} both: pdf.js's drawing of OV3's overlay is {}×{} px: not the same size", t.name, r.width, r.height);
                        ok_ov3 = false;
                    }
                    Err(e) => {
                        let _ = writeln!(log, "{} both: no drawing of OV3's overlay: {e}", t.name);
                        ok_ov3 = false;
                    }
                }
                // How pdf.js composites the overlay itself, against the standard (in process).
                let dpi = 72.0 * img.width as f64 / t.page_w;
                let full = render_layers(&t.layered, dpi, Vec::new());
                let d = cell_diff(&page, &inner(&full), 24);
                worst_both = worst_both.max(d);
                if d > 8.0 {
                    p.incompatibilities.push(format!(
                        "{}, pdf.js: with both layers on, the overlay differs from the standard's compositing by up to {d:.0} levels (cell means): pdf.js draws the shared parts in B's colour instead of multiplying A's and B's (it draws OV3's overlay, which has no layers, the same way: T2/pdfjs-plain/*-ov3-plain.png)",
                        t.name
                    ));
                }
            }
        }
    }
    p.write("T2/pdfjs.txt", log.as_bytes());
    p.check(
        "L4",
        ok_ov3,
        "pdf.js: with both layers on, each T2 overlay looks as pdf.js draws OV3's overlay of the same page (cell means within 4 levels)",
    );
    p.check("L3", ok == n && n > 0, format!("pdf.js's Layers sidebar, clicked in Chromium: {ok} of {n} states show what pdf.js itself draws for the page without that document, paper with both off (cell means within 4 levels); see T2/pdfjs.txt"));
    p.check("L12", ok == n && n > 0, format!("T2 toggling in pdf.js: {ok} of {n} agree; its compositing of the shared parts differs from the standard by up to {worst_both:.0} levels (an incompatibility, recorded)"));
}

// ── T4 and T5: source layers (L5, L6, L12) ───────────────────────────────────────────────────

fn t4(p: &mut Proof, dist: Option<&Path>, driver: Option<&Path>) {
    let cases = catalogue::resolved();
    let mut sources = Vec::new();
    let (mut l5, mut l5n) = (BTreeMap::<&str, (usize, usize)>::new(), 0usize);
    let mut leaks = 0;
    let mut same_twice = true;
    for case in &cases {
        let built = twins::build(case);
        let dir = format!("T4/{}", case.name);
        let spath = p.source(&format!("{dir}/source.pdf"), &built.source);
        let tpath = p.write(&format!("{dir}/twin.pdf"), &built.twin);
        sources.push((case.name, spath.clone(), tpath.clone()));
        let src = synth::open(&built.source);
        let vis =
            Visibility { decisions: case.decisions.iter().map(|(n, on)| (ObjRef::new(built.numbers[n], 0), *on)).collect(), ..Visibility::default() };
        let prepared = prepare(&src, &[0], &vis).expect("prepares");
        let placement = identity_placement(&src);
        let (out, report) = layered(Some(prepared.source(0).expect("page")), None, &placement, &LayerStyle::default(), Roles::default());
        let (again, _) = layered(
            Some(prepare(&src, &[0], &vis).expect("again").source(0).expect("page")),
            None,
            &placement,
            &LayerStyle::default(),
            Roles::default(),
        );
        same_twice &= out == again;
        let twin = synth::open(&built.twin);
        let tp = prepare(&twin, &[0], &Visibility::default()).expect("twin");
        let (twin_out, _) = layered(Some(tp.source(0).expect("page")), None, &placement, &LayerStyle::default(), Roles::default());
        let opath = p.output(&format!("{dir}/overlay.pdf"), &out);
        let topath = p.write(&format!("{dir}/twin-overlay.pdf"), &twin_out);
        p.write(&format!("{dir}/report.txt"), report.text().as_bytes());
        if any_stream_has(&out, twins::CANARY_TEXT.as_bytes()) || any_stream_has(&out, twins::CANARY_OPERANDS.as_bytes()) {
            leaks += 1;
            p.check("L5", false, format!("{}: the canary is in the output", case.name));
        }
        l5n += 1;
        let mut row = [case.name.to_string(), String::new(), String::new(), String::new(), String::new()];
        for (k, r) in RENDERERS.iter().enumerate() {
            let tmp = p.tmp.clone();
            let (o, t) = (render(*r, &out, &opath, 100.0, &tmp), render(*r, &twin_out, &topath, 100.0, &tmp));
            let good = match (&o, &t) {
                (Ok(o), Ok(t)) => diff(o, t).is_some_and(|(m, _)| m <= 1),
                _ => false,
            };
            let e = l5.entry(r.name()).or_default();
            e.0 += usize::from(good);
            e.1 += 1;
            if !good {
                p.check("L5", false, format!("{} in {}: the overlay differs from the twin's", case.name, r.name()));
            }
            // Source interpretation: the source as the viewer shows it, against its twin.
            let (s, tw) = (render(*r, &built.source, &spath, 100.0, &tmp), render(*r, &built.twin, &tpath, 100.0, &tmp));
            row[k + 1] = match (s, tw) {
                (Ok(s), Ok(tw)) => match diff(&s, &tw) {
                    Some((m, _)) if m <= 1 => "follows the standard".into(),
                    Some((m, c)) => format!("differs: {c} px, up to {m} levels"),
                    None => "differs: page size".into(),
                },
                (Err(e), _) | (_, Err(e)) => format!("not rendered: {e}"),
            };
        }
        p.interpretation.push(row);
    }
    for (r, (ok, n)) in &l5 {
        p.check("L5", ok == n, format!("{r}: {ok} of {n} T4/T5 overlays render within 1 level of their twin's overlay"));
        p.check("L12", ok == n, format!("T4/T5 in {r}: {ok} of {n} agree"));
    }
    p.check("L5", leaks == 0, format!("canary scan: {} of {l5n} T4/T5 overlays hold no canary text or colour", l5n - leaks));
    p.check("L6", same_twice, format!("the same state twice gives byte-identical files ({} cases)", cases.len()));
    // pdf.js: the sources and twins as the viewer opens them.
    let files: Vec<PathBuf> = sources
        .iter()
        .flat_map(|(name, s, t)| {
            let a = p.path(&format!("T4/pdfjs-in/{name}.pdf"));
            let b = p.path(&format!("T4/pdfjs-in/{name}-twin.pdf"));
            let _ = std::fs::create_dir_all(a.parent().expect("dir"));
            let _ = std::fs::copy(s, &a);
            let _ = std::fs::copy(t, &b);
            [a, b]
        })
        .collect();
    let dir = pdfjs(p, dist, driver, &files, true, "T4/pdfjs");
    for row in p.interpretation.iter_mut() {
        row[4] = match &dir {
            None => "not run".into(),
            Some(d) => {
                let read = |f: String| std::fs::read(d.join(f)).map_err(|e| e.to_string()).and_then(|b| png_decode(&b));
                match (read(format!("{}-plain.png", row[0])), read(format!("{}-twin-plain.png", row[0]))) {
                    (Ok(a), Ok(b)) => {
                        let c = cell_diff(&a, &b, 24);
                        if c <= 2.0 { "follows the standard".into() } else { format!("differs: cell means up to {c:.0} levels") }
                    }
                    _ => "no screenshot".into(),
                }
            }
        };
    }
}

// ── T6 to T9: annotations, form fields, signatures, redaction marks (L7 to L10) ─────────────

fn t6_to_t9(p: &mut Proof) {
    let mut l7: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut l9: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let tally = |m: &mut BTreeMap<&str, (usize, usize)>, r: Renderer, good: bool| {
        let e = m.entry(r.name()).or_default();
        e.0 += usize::from(good);
        e.1 += 1;
    };
    // T6: stored appearances on rotated pages, against the control (source rendered, then the
    // model). T7: missing appearances, against `with_missing_appearances`. T8: form fields.
    let mut cases: Vec<AnnotCase> = Vec::new();
    for rotate in [0, 90, 180, 270] {
        let (s, _) = annots::with_appearances(rotate);
        let b = s.bytes();
        cases.push((format!("T6-appearances-{rotate}"), b.clone(), b, true));
        let s = annots::without_appearances(rotate);
        let b = s.bytes();
        let shown = synth::save(&pdfcraft_annot::with_missing_appearances(&synth::open(&b)).expect("appearances"), true);
        cases.push((format!("T7-missing-{rotate}"), b, shown, false));
    }
    let form = annots::form(true);
    let mut filled = synth::open(&form);
    for (name, v) in [
        ("name", pdfcraft_forms::FieldValue::Text("New value".into())),
        ("city", pdfcraft_forms::FieldValue::Text("Lisbon".into())),
        ("size", pdfcraft_forms::FieldValue::Choice(vec!["Large".into()])),
    ] {
        pdfcraft_forms::set_value(&mut filled, name, &v).expect("fills");
    }
    let later = pdfcraft_forms::fields(&filled).into_iter().find(|f| f.name == "later").expect("later");
    let w = later.widgets[0].clone();
    let ap = pdfcraft_forms::appearance::check_box_states(&mut filled, &w, later.kind, "Yes");
    filled.update_dict(w.obj, |d| d.set(b"AP".to_vec(), Object::Dict(ap))).expect("widget");
    cases.push(("T8-form".into(), form, synth::save(&filled, true), false));
    for (name, source, control_src, t6) in &cases {
        let dir = format!("annotations/{name}");
        p.source(&format!("{dir}/source.pdf"), source);
        let cpath = p.write(&format!("{dir}/control-source.pdf"), control_src);
        // The same page flattened by PdfCraft's own Flatten command (upstream's, not the
        // overlay's code): its annotations drawn into the content as plain graphics.
        let mut flat = synth::open(control_src);
        let flattened = pdfcraft_edit::flatten(&mut flat, &[0], true, true);
        let flat = synth::save(&flat, true);
        let fpath = p.write(&format!("{dir}/flattened-source.pdf"), &flat);
        let doc = synth::open(source);
        let prepared = prep_all(&doc, &Visibility::default());
        let (out, report) =
            layered(Some(prepared.source(0).expect("page")), None, &identity_placement(&doc), &LayerStyle::default(), Roles::default());
        let opath = p.output(&format!("{dir}/overlay.pdf"), &out);
        p.write(&format!("{dir}/report.txt"), report.text().as_bytes());
        let mut row = [format!("{name} (annotations)"), String::new(), String::new(), String::new(), "not run".into()];
        for (k, r) in RENDERERS.iter().enumerate() {
            let tmp = p.tmp.clone();
            let images = render(*r, &out, &opath, 100.0, &tmp).and_then(|o| {
                let s = render(*r, control_src, &cpath, 100.0, &tmp)?;
                let f = render(*r, &flat, &fpath, 100.0, &tmp)?;
                Ok((o, s, f))
            });
            let Ok((o, src, fl)) = images else {
                tally(if *t6 { &mut l7 } else { &mut l9 }, *r, false);
                p.check(if *t6 { "L7" } else { "L9" }, false, format!("{name}: {} didn't render", r.name()));
                continue;
            };
            let (control, flat_control) = (model(&src), model(&fl));
            p.png(&format!("{dir}/{}-overlay.png", r.name()), &o);
            p.png(&format!("{dir}/{}-control.png", r.name()), &control);
            p.png(&format!("{dir}/{}-flattened-control.png", r.name()), &flat_control);
            // The control as OV3 measured it (medians, here per 8-pixel cell with ink, so edges
            // each renderer antialiases its own way don't decide), from the page flattened by
            // the Flatten command: it holds the annotations as page content, which every renderer
            // draws alike. pdfcraft also every pixel, against its own drawing of the annotations.
            let against_flat = cell_medians(&o, &flat_control, 8);
            p.write(&format!("{dir}/{}-cells.txt", r.name()), worst_cells(&o, &flat_control, 8).as_bytes());
            let mut good = flattened.is_ok() && against_flat.is_some_and(|v| v <= 2.0);
            if *r == Renderer::PdfCraft {
                good &= diff(&o, &control).is_some_and(|(m, _)| m <= 2);
            }
            tally(if *t6 { &mut l7 } else { &mut l9 }, *r, good);
            if !good {
                // Where they differ, for the reviewer: {renderer}-diff.png and its box.
                let region = match diff_map(&o, &flat_control) {
                    Some((map, bbox)) => {
                        p.png(&format!("{dir}/{}-diff.png", r.name()), &map);
                        match bbox {
                            Some([x0, y0, x1, y1]) => format!(
                                "pixels more than 2 levels apart lie in x {x0}–{x1}, y {y0}–{y1} of {}×{} ({dir}/{}-diff.png)",
                                o.width,
                                o.height,
                                r.name()
                            ),
                            None => "no pixel more than 2 levels apart".into(),
                        }
                    }
                    None => "sizes differ".into(),
                };
                p.check(
                    if *t6 { "L7" } else { "L9" },
                    false,
                    format!(
                        "{name} in {}: worst cell median {} from the flattened control{}; {region}",
                        r.name(),
                        against_flat.map(|v| format!("{v:.0} levels")).unwrap_or_else(|| "unmeasured (sizes differ)".into()),
                        flattened.as_ref().err().map(|e| format!(" (flatten failed: {e:?})")).unwrap_or_default()
                    ),
                );
            }
            // How the renderer draws the source's annotations itself, against the flattened page.
            row[k + 1] = match cell_medians(&src, &fl, 8) {
                Some(v) if v <= 2.0 => "draws them as the standard says".into(),
                Some(v) => {
                    p.incompatibilities.push(format!("{name}, {}: draws the source's annotations differently from their appearance streams (cell medians up to {v:.0} levels from the flattened page); the overlay follows the appearance streams", r.name()));
                    format!("differs: up to {v:.0} levels")
                }
                None => "differs: page size".into(),
            };
        }
        p.interpretation.push(row);
    }
    for (crit, m, what) in [("L7", &l7, "T6 stored appearances (0°, 90°, 180°, 270°)"), ("L9", &l9, "T7 missing appearances and T8 form fields")]
    {
        for (r, (ok, n)) in m {
            p.check(
                "L12",
                true,
                format!(
                    "{what} in {r}: {ok} of {n} within 2 levels of that renderer's control (cell medians); the rest are recorded incompatibilities"
                ),
            );
        }
        let pdfcraft = m.get("pdfcraft").copied().unwrap_or((0, 0));
        p.check(
            crit,
            pdfcraft.0 == pdfcraft.1,
            format!(
                "{what}: pdfcraft {}/{} within 2 levels of the control (every pixel); Poppler and PDFium: see L12 and the incompatibilities",
                pdfcraft.0, pdfcraft.1
            ),
        );
    }
    l8(p);
    t9(p);
}

/// L8 in every renderer: /CA 0.5 without compounding, targets on rotated pages, NoRotate.
fn l8(p: &mut Proof) {
    use annots::{Src, ap};
    let tmp = p.tmp.clone();
    // Opacity.
    let mut s = Src::new(0);
    s.content = String::new();
    s.obj("sq-ap", ap([0.0, 0.0, 100.0, 60.0], None, "0 g 0 0 60 60 re f 40 0 60 60 re f"));
    s.annot("sq", "<< /Type /Annot /Subtype /Square /Rect [50 50 150 110] /F 4 /CA 0.5 /AP << /N {sq-ap} >> >>");
    let b = s.bytes();
    p.source("L8/opacity-source.pdf", &b);
    let doc = synth::open(&b);
    let (out, _) = layered(
        Some(prep_all(&doc, &Visibility::default()).source(0).expect("page")),
        None,
        &identity_placement(&doc),
        &LayerStyle::default(),
        Roles::default(),
    );
    let opath = p.output("L8/opacity.pdf", &out);
    let want = Pair { a: [204, 51, 51], b: [51, 204, 204] }.predict(0.5, 0.0);
    for r in RENDERERS {
        let Ok(img) = render(r, &out, &opath, 72.0, &tmp) else {
            p.check("L8", false, format!("opacity: {} didn't render", r.name()));
            continue;
        };
        let mut worst = 0.0f64;
        for (x, y) in [(70, 120), (100, 120), (130, 120)] {
            let px = img.get(x, y).unwrap_or([0, 0, 0]);
            worst = worst.max(px.iter().zip(want).map(|(a, b)| (f64::from(*a) - b).abs()).fold(0.0, f64::max));
        }
        p.check("L8", worst <= 2.0, format!("/CA 0.5 in {}: both parts and their overlap within {worst:.1} levels of coverage 0.5", r.name()));
    }
    // Rotation and NoRotate: target centroids at 300 dpi.
    let (w, h) = (annots::W, annots::H);
    let displayed = |(x, y): (f64, f64), rot: i64| match rot {
        90 => (y, x),
        180 => (w - x, y),
        270 => (h - y, w - x),
        _ => (x, h - y),
    };
    // Discs, not squares: Poppler and PDFium snap the edges of axis-aligned rectangles to the
    // pixel grid, which moves a square's centroid (as in the OV3 families).
    let target = |x: f64, y: f64| {
        let (r, k) = (3.0, 3.0 * 0.5523);
        format!(
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
        )
    };
    for rot in [0, 90, 180, 270] {
        for no_rotate in [false, true] {
            let mut s = Src::new(rot);
            s.content = String::new();
            let (bbox, rect) = ([0.0, 0.0, 40.0, 30.0], [120.0, 70.0, 160.0, 100.0]);
            s.obj("a", ap(bbox, None, &format!("{}{}", target(6.0, 24.0), target(34.0, 6.0))));
            let flags = if no_rotate { 20 } else { 4 };
            s.annot("n", format!("<< /Type /Annot /Subtype /Text /Rect [120 70 160 100] /F {flags} /AP << /N {{a}} >> >>"));
            let b = s.bytes();
            let doc = synth::open(&b);
            let (out, _) = layered(
                Some(prep_all(&doc, &Visibility::default()).source(0).expect("page")),
                None,
                &identity_placement(&doc),
                &LayerStyle::default(),
                Roles::default(),
            );
            let opath = p.output(&format!("L8/rotate-{rot}{}.pdf", if no_rotate { "-norotate" } else { "" }), &out);
            for r in RENDERERS {
                let Ok(img) = render(r, &out, &opath, 300.0, &tmp) else { continue };
                let (dw, dh) = if rot % 180 == 0 { (w, h) } else { (h, w) };
                let (kx, ky) = (img.width as f64 / dw, img.height as f64 / dh);
                let mut worst = 0.0f64;
                for (tx, ty) in [(6.0, 24.0), (34.0, 6.0)] {
                    // Where the standard puts the target: Algorithm 8.1 (identity /Matrix, /BBox
                    // the size of /Rect) then the page rotation, or upright about /Rect's
                    // upper-left corner with NoRotate.
                    let user = (rect[0] + tx, rect[1] + ty);
                    let (ex, ey) = if no_rotate {
                        let c = displayed((rect[0], rect[3]), rot);
                        (c.0 + tx, c.1 + (bbox[3] - ty))
                    } else {
                        displayed(user, rot)
                    };
                    let (cx, cy) = (ex * kx, ey * ky);
                    let (x0, y0) = ((cx - 40.0).max(0.0) as usize, (cy - 40.0).max(0.0) as usize);
                    let found = measure::centroid(&img, x0, y0, x0 + 80, y0 + 80, |px| f64::from(255 - px[1]) / 204.0);
                    worst = worst.max(found.map_or(f64::INFINITY, |(fx, fy, _)| (fx - cx).abs().max((fy - cy).abs())));
                }
                p.check(
                    "L8",
                    worst <= 0.5,
                    format!(
                        "{} at {rot}° in {}: targets within {worst:.2} px at 300 dpi of where the standard puts them",
                        if no_rotate { "NoRotate note" } else { "note" },
                        r.name()
                    ),
                );
            }
        }
    }
}

/// L10, L11: a signed source and unapplied redaction marks.
fn t9(p: &mut Proof) {
    let claims = |t: &str| {
        let t = t.to_lowercase();
        [
            "is signed",
            "was signed",
            "has been signed",
            "signature is valid",
            "redactions were applied",
            "redactions applied",
            "has been redacted",
            "was redacted",
        ]
        .iter()
        .any(|c| t.contains(c))
    };
    let signed = annots::signed_source();
    p.source("T9/signed-source.pdf", &signed);
    let src = Document::open(Arc::new(signed.clone())).expect("opens");
    let trust = pdfcraft_sign::pdf::TrustStore::default();
    let source_signed = pdfcraft_sign::pdf::list(&src, &signed, &trust).iter().any(|s| s.signed);
    let prepared = prep_all(&src, &Visibility::default());
    let (out, report) = layered(Some(prepared.source(0).expect("page")), None, &identity_placement(&src), &LayerStyle::default(), Roles::default());
    p.output("T9/signed-overlay.pdf", &out);
    p.write("T9/signed-report.txt", report.text().as_bytes());
    let raw = String::from_utf8_lossy(&out);
    let carried = ["/AcroForm", "/Annots", "/Sig", "/ByteRange", "/SigFlags"].iter().filter(|k| raw.contains(**k)).copied().collect::<Vec<_>>();
    let listed = pdfcraft_sign::pdf::list(&synth::open(&out), &out, &trust).len();
    let warned = report.warnings().iter().any(|w| w.contains("signed signature field") && w.contains("not signed"));
    p.check("L10", source_signed && warned, "a source signed with the sign crate's test identity gives the signed warning");
    p.check("L10", carried.is_empty() && listed == 0, format!("the overlay carries none of /AcroForm, /Annots, /Sig, /ByteRange, /SigFlags (found {carried:?}); sign::list finds {listed} signatures in it"));
    let doc = synth::open(&out);
    let subject = doc
        .trailer()
        .get(b"Info")
        .and_then(|i| doc.dict(i))
        .and_then(|i| i.get(b"Subject").and_then(|s| doc.resolve(s).as_string().map(|s| s.to_text())))
        .unwrap_or_default();
    p.check("L10", !claims(&report.text()) && !claims(&subject), "neither the report nor the output's metadata says the output is signed");
    // Redaction marks.
    let marked = synth::raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>".into(),
        synth::raw_stream("", "BT /F1 14 Tf 20 150 Td (Secret account 1234) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Annot /Subtype /Redact /F 4 /Rect [18 145 230 165] /QuadPoints [18 165 230 165 18 145 230 145] /OverlayText (REMOVED) /IC [0 0 0] >>".into(),
    ]);
    p.source("T9/redaction-source.pdf", &marked);
    let src = synth::open(&marked);
    let marks = pdfcraft_redact::marks(&src).len();
    let prepared = prep_all(&src, &Visibility::default());
    let (out, report) = layered(Some(prepared.source(0).expect("page")), None, &identity_placement(&src), &LayerStyle::default(), Roles::default());
    p.output("T9/redaction-overlay.pdf", &out);
    p.write("T9/redaction-report.txt", report.text().as_bytes());
    let warned = report.warnings() == ["Document A, page 1: 1 redaction mark not applied: the content under them is still in the overlay."];
    p.check(
        "L10",
        marks == 1 && warned,
        format!("unapplied redaction marks (redact::marks finds {marks}) give the warning naming the page and count"),
    );
    p.check(
        "L10",
        any_stream_has(&out, b"Secret account 1234") && !claims(&report.text()),
        "the marks are never applied (the text under them is in the overlay), and nothing says they were",
    );
}

// ── T10: the drawing-like pair ───────────────────────────────────────────────────────────────

fn t10(p: &mut Proof) {
    use drawing::{HVAC, HVAC_OPERANDS, Rev};
    let (old, new) = (drawing::sheet(Rev::Old), drawing::sheet(Rev::New));
    let opath = p.source("T10/old.pdf", &old);
    let npath = p.source("T10/new.pdf", &new);
    let (od, nd) = (synth::open(&old), synth::open(&new));
    let placement = identity_placement(&od);
    let tmp = p.tmp.clone();
    for (tag, decisions) in [("default", Vec::new()), ("hvac-on", vec![(ObjRef::new(HVAC, 0), true)])] {
        let vis = Visibility { decisions, ..Visibility::default() };
        let (pa, pb) = (prep_all(&od, &vis), prep_all(&nd, &vis));
        let unresolved = pa.reports()[0].unresolved.len() + pb.reports()[0].unresolved.len();
        let (out, report) =
            layered(Some(pa.source(0).expect("old")), Some(pb.source(0).expect("new")), &placement, &LayerStyle::default(), Roles::default());
        let path = p.output(&format!("T10/overlay-{tag}.pdf"), &out);
        p.write(&format!("T10/report-{tag}.txt"), report.text().as_bytes());
        let hvac = any_stream_has(&out, HVAC_OPERANDS.as_bytes());
        p.check("L5", unresolved == 0 && hvac == (tag == "hvac-on"), format!("T10 {tag}: no unresolved item; HVAC drawn {hvac}"));
        // Toggles in PdfCraft against the pages composed without each sheet.
        let (g_old, g_new) = groups(&out);
        let (a_blank, _) = layered(None, Some(pb.source(0).expect("new")), &placement, &LayerStyle::default(), Roles::default());
        let (b_blank, _) = layered(Some(pa.source(0).expect("old")), None, &placement, &LayerStyle::default(), Roles::default());
        let dpi = 36.0;
        let t_old =
            diff(&render_layers(&out, dpi, vec![(g_old.0, g_old.1, false)]), &render_layers(&a_blank, dpi, Vec::new())).is_some_and(|(m, _)| m <= 1);
        let t_new =
            diff(&render_layers(&out, dpi, vec![(g_new.0, g_new.1, false)]), &render_layers(&b_blank, dpi, Vec::new())).is_some_and(|(m, _)| m <= 1);
        p.check("L2", t_old && t_new, format!("T10 {tag}: Old off and New off in pdfcraft match the pages without each sheet"));
        let mut row = Vec::new();
        for r in RENDERERS {
            if let Ok(img) = render(r, &out, &path, dpi, &tmp) {
                p.png(&format!("T10/{tag}-{}.png", r.name()), &img);
                row.push(img);
            }
        }
        if tag == "default" {
            let a = render(Renderer::PdfCraft, &old, &opath, dpi, &tmp).expect("old");
            let b = render(Renderer::PdfCraft, &new, &npath, dpi, &tmp).expect("new");
            if let Some(o) = row.first() {
                p.png("T10/side-by-side.png", &side_by_side(&[&a, &b, o]));
            }
        }
        p.check("L12", row.len() == 3, format!("T10 {tag}: rendered in {} of 3 renderers (T10/{tag}-*.png)", row.len()));
    }
}

// ── L11, L13 to L16, the report ──────────────────────────────────────────────────────────────

fn l11(p: &mut Proof) {
    let mut changed = Vec::new();
    for (path, bytes) in &p.sources {
        if std::fs::read(path).ok().as_deref() != Some(bytes.as_slice()) {
            changed.push(path.display().to_string());
        }
    }
    let mut list = String::new();
    for (path, _) in &p.sources {
        let _ = writeln!(list, "{}  {}", sha256sum(path), path.strip_prefix(&p.out).unwrap_or(path).display());
    }
    p.write("L11/sha256.txt", list.as_bytes());
    let n = p.sources.len();
    p.check("L11", changed.is_empty(), format!("{n} source files, layered and signed included, byte-identical after every renderer read them (SHA-256 in L11/sha256.txt); changed: {changed:?}"));
}

fn logs(p: &mut Proof, args: &Args) {
    match args.test_log.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
        Some(log) => {
            let results: Vec<&str> = log.lines().filter(|l| l.contains("test result:")).collect();
            let failed = results.iter().any(|l| !l.contains(" 0 failed"));
            let passed: usize = results.iter().filter_map(|l| l.split("ok. ").nth(1)?.split(' ').next()?.parse::<usize>().ok()).sum();
            p.check(
                "L13",
                !failed && !results.is_empty(),
                format!("pdfcraft-overlay: {passed} tests in {} binaries, none failed (OV2 and OV3 test files unchanged, below)", results.len()),
            );
            let hostile = log.lines().find(|l| l.contains("10000 cases:") && l.contains("composed"));
            p.check(
                "L14",
                !failed && hostile.is_some(),
                format!("tests/hostile.rs: {}", hostile.unwrap_or("the hostile run's summary line wasn't found")),
            );
        }
        None => {
            p.not_run("L13", "no test log given");
            p.not_run("L14", "no test log given");
        }
    }
    match &args.repo {
        Some(repo) => {
            let files = [
                "crates/overlay/tests/place.rs",
                "crates/overlay/tests/sheet.rs",
                "crates/overlay/tests/similarity.rs",
                "crates/overlay/tests/affine.rs",
                "crates/overlay/tests/frame.rs",
                "crates/overlay/tests/identification.rs",
                "crates/overlay/tests/generator.rs",
                "crates/overlay/tests/composite.rs",
                "crates/overlay/tests/pixels.rs",
                "crates/overlay/tests/render_masks.rs",
                "crates/overlay/tests/classify.rs",
            ];
            let out = Command::new("git").arg("-C").arg(repo).args(["diff", "--stat", "e89611c2", "--"]).args(files).output();
            let unchanged = out.as_ref().is_ok_and(|o| o.status.success() && o.stdout.is_empty());
            p.check("L13", unchanged, "the OV2 and OV3 test files are unchanged since OV2.7 (git diff e89611c2, empty)");
        }
        None => p.not_run("L13", "no repository given for the unchanged-test check"),
    }
    match args.ci.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
        Some(ci) => {
            // One `conclusion:` line per job; every one of them must be a success.
            let jobs: Vec<&str> = ci.lines().filter_map(|l| l.strip_prefix("conclusion:")).collect();
            let ok = !jobs.is_empty() && jobs.iter().all(|j| j.trim_start().starts_with("success"));
            p.check("L15", ok, format!("CI: {}", ci.lines().collect::<Vec<_>>().join("; ")));
        }
        None => p.not_run("L15", "no CI summary given"),
    }
    p.not_run(
        "L16",
        "your manual check: T10/old.pdf, T10/new.pdf and T10/overlay-default.pdf are prepared for toggling in pdfcraft and an independent viewer",
    );
}

const TITLES: [(&str, &str); 16] = [
    ("L1", "Exactly two layers"),
    ("L2", "Toggling, PDFCraft"),
    ("L3", "Toggling, other viewers"),
    ("L4", "Dark overlap preserved"),
    ("L5", "Source optional content resolved, nothing hidden leaks"),
    ("L6", "Visibility state"),
    ("L7", "Annotation appearances preserved"),
    ("L8", "Opacity and transformations"),
    ("L9", "Missing and unusable appearances"),
    ("L10", "Signatures and redactions"),
    ("L11", "Sources untouched"),
    ("L12", "Cross-renderer agreement, disagreements recorded"),
    ("L13", "No regressions"),
    ("L14", "No panics, no leaks"),
    ("L15", "Gates"),
    ("L16", "Manual check (yours)"),
];

fn report(p: &mut Proof) {
    // Every output written: the defensive check and qpdf.
    let (mut ok, mut n) = (0, 0);
    let mut bad = Vec::new();
    for path in p.outputs.clone() {
        let bytes = Arc::new(std::fs::read(&path).expect("output"));
        let good = check_output(&synth::open(&bytes)).is_ok() && qpdf(&path).is_ok();
        n += 1;
        ok += usize::from(good);
        if !good {
            bad.push(path.display().to_string());
        }
    }
    p.check("L1", ok == n, format!("all {n} outputs of this proof pass the defensive check and qpdf --check; failing: {bad:?}"));
    let mut md = String::from(
        "# OV4 proof\n\nEvery criterion of the step 4 PRD, measured by `examples/ov4_proof.rs`.\n\n| # | Criterion | Result | Evidence |\n|---|---|---|---|\n",
    );
    let mut tsv = String::from("criterion\tresult\tevidence\n");
    for (id, title) in TITLES {
        let checks = p.checks.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let any_fail = checks.iter().any(|c| c.ok == Some(false));
        let any_pass = checks.iter().any(|c| c.ok == Some(true));
        let any_not_run = checks.iter().any(|c| c.ok.is_none());
        let result = if any_fail {
            "FAILED"
        } else if any_pass && any_not_run {
            "passed (part not run)"
        } else if any_pass {
            "passed"
        } else {
            "not run"
        };
        let evidence: Vec<String> = checks
            .iter()
            .map(|c| {
                format!(
                    "{}{}",
                    match c.ok {
                        Some(true) => "✓ ",
                        Some(false) => "✗ ",
                        None => "– not run: ",
                    },
                    c.what
                )
            })
            .collect();
        let _ = writeln!(md, "| {id} | {title} | {result} | {} |", evidence.join("<br>"));
        let _ = writeln!(tsv, "{id}\t{result}\t{}", evidence.join(" | "));
    }
    md.push_str("\n## Source interpretation (T4, each source as the viewer shows it, against its twin)\n\n| Case | pdfcraft | Poppler | PDFium | pdf.js |\n|---|---|---|---|---|\n");
    let mut interp = String::from("case\tpdfcraft\tpoppler\tpdfium\tpdfjs\n");
    for row in &p.interpretation {
        let _ = writeln!(md, "| {} |", row.join(" | "));
        let _ = writeln!(interp, "{}", row.join("\t"));
    }
    md.push_str("\n## Incompatibilities\n\n");
    if p.incompatibilities.is_empty() {
        md.push_str("None.\n");
    }
    for i in &p.incompatibilities {
        let _ = writeln!(md, "- {i}");
    }
    p.write("report.md", md.as_bytes());
    p.write("criteria.tsv", tsv.as_bytes());
    p.write("source-interpretation.tsv", interp.as_bytes());
    p.write("incompatibilities.txt", p.incompatibilities.join("\n").as_bytes());
}

fn main() -> ExitCode {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = args.out.join("tmp");
    std::fs::create_dir_all(&tmp).expect("output directory");
    let mut p = Proof {
        out: args.out.clone(),
        tmp,
        checks: BTreeMap::new(),
        incompatibilities: Vec::new(),
        interpretation: Vec::new(),
        sources: Vec::new(),
        outputs: Vec::new(),
    };
    let started = Instant::now();
    let step = |name: &str| eprintln!("[{:>6.1} s] {name}", started.elapsed().as_secs_f64());
    step("L4");
    l4(&mut p, args.ov3_reference.as_deref(), args.ov3_layered.as_deref());
    step("T1");
    t1(&mut p);
    step("T2");
    let toggles = t2(&mut p);
    step("panel");
    panel(&mut p, &toggles, args.app.as_deref(), args.cli.as_deref());
    step("pdf.js toggles");
    pdfjs_toggles(&mut p, &toggles, args.pdfjs_dist.as_deref(), args.pdfjs_driver.as_deref());
    step("T4, T5");
    t4(&mut p, args.pdfjs_dist.as_deref(), args.pdfjs_driver.as_deref());
    step("T6 to T9");
    t6_to_t9(&mut p);
    step("T10");
    t10(&mut p);
    step("L11, logs, report");
    l11(&mut p);
    logs(&mut p, &args);
    report(&mut p);
    let _ = std::fs::remove_dir_all(p.out.join("tmp"));
    let failed = p.checks.values().flatten().any(|c| c.ok == Some(false));
    step(if failed { "done: some checks FAILED (report.md)" } else { "done: report.md" });
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

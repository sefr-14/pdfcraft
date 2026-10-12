//! SYNC study (upstream integration, 12 Oct 2026): what upstream's renderer changes to shadings do
//! to PDF vector shadings and gradients, alone and through the overlay compositor. Upstream drew
//! axial and radial shadings as native gradients from 7f1b632 (a hayro 0.8 backport); function-based
//! shadings and soft masks also go through the vendored renderer. This study is not part of the
//! OV3 acceptance suite and changes nothing in it.
//!
//! ```text
//! cargo run -p pdfcraft-overlay --example sync_shading_study -- [--out DIR] [--dpi 150]
//! ```
//!
//! Writes `sources/shadings.pdf` (one shading case per page), `overlays/<pair>.pdf` (pairs of
//! those pages through `compose`), `renders/<page>-<renderer>.ppm` and `.png`, and `study.csv`:
//! for every page, PdfCraft against Poppler and against PDFium, as the mean, 99th percentile and
//! largest per-pixel difference (the largest channel difference, in 8-bit levels) and the share
//! of pixels more than 8 levels apart. Run it on two builds and compare their PdfCraft renders
//! byte for byte to see what a renderer change moved.
//!
//! Poppler (`pdftoppm`) and PDFium (`python3 -m pypdfium2_cli`) run as separate processes, as in
//! the OV3 proof.

#[path = "../tests/common/measure.rs"]
mod measure;
#[path = "../tests/common/synth.rs"]
mod synth;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use measure::Image;
use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Object, Stream};
use pdfcraft_overlay::{Placement, Point, Source, Style, compose};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use synth::{Page, Res};

const W: f64 = 300.0;
const H: f64 = 200.0;

fn name(n: &str) -> Object {
    Object::name(n)
}

fn reals(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

fn dict(entries: Vec<(&str, Object)>) -> Dict {
    let mut d = Dict::new();
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v);
    }
    d
}

/// An exponential (Type 2) function from `c0` to `c1`.
fn exp_fn(c0: &[f64], c1: &[f64], n: f64) -> Object {
    Object::Dict(dict(vec![
        ("FunctionType", Object::Int(2)),
        ("Domain", reals(&[0.0, 1.0])),
        ("C0", reals(c0)),
        ("C1", reals(c1)),
        ("N", Object::Real(n)),
    ]))
}

/// A stitching (Type 3) function through `stops` colours at even spacing.
fn stitched(stops: &[[f64; 3]]) -> Object {
    let parts: Vec<Object> = stops.windows(2).map(|w| exp_fn(&w[0], &w[1], 1.0)).collect();
    let k = parts.len();
    let bounds: Vec<f64> = (1..k).map(|i| i as f64 / k as f64).collect();
    let encode: Vec<f64> = (0..k).flat_map(|_| [0.0, 1.0]).collect();
    Object::Dict(dict(vec![
        ("FunctionType", Object::Int(3)),
        ("Domain", reals(&[0.0, 1.0])),
        ("Functions", Object::Array(parts)),
        ("Bounds", reals(&bounds)),
        ("Encode", reals(&encode)),
    ]))
}

fn axial(space: &str, coords: [f64; 4], function: Object, extend: [bool; 2]) -> Dict {
    dict(vec![
        ("ShadingType", Object::Int(2)),
        ("ColorSpace", name(space)),
        ("Coords", reals(&coords)),
        ("Function", function),
        ("Extend", Object::Array(vec![Object::Bool(extend[0]), Object::Bool(extend[1])])),
    ])
}

fn radial(space: &str, coords: [f64; 6], function: Object, extend: [bool; 2]) -> Dict {
    dict(vec![
        ("ShadingType", Object::Int(3)),
        ("ColorSpace", name(space)),
        ("Coords", reals(&coords)),
        ("Function", function),
        ("Extend", Object::Array(vec![Object::Bool(extend[0]), Object::Bool(extend[1])])),
    ])
}

/// The cases, one page each: (name, what it checks, page).
fn cases() -> Vec<(&'static str, &'static str, Page)> {
    let red_blue = || exp_fn(&[0.8, 0.1, 0.1], &[0.1, 0.2, 0.8], 1.0);
    let clip = "q 20 20 260 160 re W n";
    let mut out = Vec::new();

    out.push((
        "s01-axial-rgb-sh",
        "axial, DeviceRGB, sh under a clip, both ends extended",
        Page::new(W, H, format!("{clip} /Sh sh Q")).with_resource(
            "Shading",
            "Sh",
            Res::Dict(axial("DeviceRGB", [40.0, 0.0, 260.0, 0.0], red_blue(), [true, true])),
        ),
    ));
    out.push((
        "s02-axial-gray-pattern",
        "axial, DeviceGray, as a PatternType 2 fill, not extended",
        Page::new(W, H, "/Pattern cs /P scn 20 20 260 160 re f").with_resource(
            "Pattern",
            "P",
            Res::Dict(dict(vec![
                ("PatternType", Object::Int(2)),
                ("Shading", Object::Dict(axial("DeviceGray", [0.0, 40.0, 0.0, 160.0], exp_fn(&[0.05], &[0.95], 1.0), [false, false]))),
                ("Matrix", reals(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
            ])),
        ),
    ));
    out.push((
        "s03-radial-concentric",
        "radial, concentric, both ends extended",
        Page::new(W, H, format!("{clip} /Sh sh Q")).with_resource(
            "Shading",
            "Sh",
            Res::Dict(radial("DeviceRGB", [150.0, 100.0, 5.0, 150.0, 100.0, 90.0], red_blue(), [true, true])),
        ),
    ));
    out.push((
        "s04-radial-two-circle",
        "radial, two circles with an off-centre focus",
        Page::new(W, H, format!("{clip} /Sh sh Q")).with_resource(
            "Shading",
            "Sh",
            Res::Dict(radial("DeviceRGB", [110.0, 120.0, 0.0, 160.0, 100.0, 80.0], exp_fn(&[1.0, 0.9, 0.2], &[0.1, 0.5, 0.2], 1.0), [true, true])),
        ),
    ));
    out.push((
        "s05-axial-stitched",
        "axial, four colour stops through a stitching function, diagonal",
        Page::new(W, H, format!("{clip} /Sh sh Q")).with_resource(
            "Shading",
            "Sh",
            Res::Dict(axial(
                "DeviceRGB",
                [20.0, 20.0, 280.0, 180.0],
                stitched(&[[0.9, 0.9, 0.2], [0.2, 0.7, 0.3], [0.2, 0.3, 0.8], [0.6, 0.1, 0.5]]),
                [false, false],
            )),
        ),
    ));
    out.push((
        "s06-function-based",
        "function-based (Type 1) with a PostScript function, which native gradients don't cover",
        Page::new(W, H, format!("{clip} /Sh sh Q"))
            .with_resource(
                "Shading",
                "Sh",
                Res::Dict(dict(vec![
                    ("ShadingType", Object::Int(1)),
                    ("ColorSpace", name("DeviceRGB")),
                    ("Domain", reals(&[0.0, 1.0, 0.0, 1.0])),
                    ("Matrix", reals(&[260.0, 0.0, 0.0, 160.0, 20.0, 20.0])),
                    ("Function", Object::String(pdfcraft_cos::PdfString::literal(b"ref:Function/F".to_vec()))),
                ])),
            )
            .with_resource(
                "Function",
                "F",
                Res::Stream(
                    dict(vec![
                        ("FunctionType", Object::Int(4)),
                        ("Domain", reals(&[0.0, 1.0, 0.0, 1.0])),
                        ("Range", reals(&[0.0, 1.0, 0.0, 1.0, 0.0, 1.0])),
                    ]),
                    b"{ 2 copy add 2 div }".to_vec(),
                ),
            ),
    ));
    out.push((
        "s07-luminosity-fade",
        "a luminosity soft mask whose group is one axial sh: the InDesign fade upstream sped up",
        Page::new(W, H, "q /Fade gs 0.1 0.3 0.6 rg 20 20 260 160 re f Q")
            .with_resource("ExtGState", "Fade", Res::Dict(dict(vec![("Type", name("ExtGState")), ("SMask", synth::mask_ref("Luminosity", "M"))])))
            .with_resource(
                "XObject",
                "M",
                Res::Stream(
                    dict(vec![
                        ("Type", name("XObject")),
                        ("Subtype", name("Form")),
                        ("BBox", reals(&[0.0, 0.0, W, H])),
                        ("Group", Object::Dict(dict(vec![("Type", name("Group")), ("S", name("Transparency")), ("CS", name("DeviceGray"))]))),
                        (
                            "Resources",
                            Object::Dict(dict(vec![(
                                "Shading",
                                Object::Dict(dict(vec![(
                                    "Sh",
                                    Object::Dict(axial("DeviceGray", [20.0, 0.0, 280.0, 0.0], exp_fn(&[1.0], &[0.0], 1.0), [true, true])),
                                )])),
                            )])),
                        ),
                    ]),
                    b"/Sh sh".to_vec(),
                ),
            ),
    ));
    out.push((
        "s08-lines-over-gradient",
        "thin black line work (0.1 to 0.5 mm) over a light axial background, as a shaded drawing",
        Page::new(
            W,
            H,
            format!(
                "{clip} /Sh sh Q 0 G 0.28 w 30 40 m 270 40 l S 0.57 w 30 80 m 270 160 l S 1.42 w 30 160 m 270 120 l S 0.28 w 150 25 m 150 175 l S"
            ),
        )
        .with_resource(
            "Shading",
            "Sh",
            Res::Dict(axial("DeviceRGB", [0.0, 20.0, 0.0, 180.0], exp_fn(&[0.95, 0.95, 0.85], &[0.75, 0.85, 0.95], 1.0), [true, true])),
        ),
    ));
    out
}

/// Overlay pairs: (name, A page, B page), pages 0-based into `cases()`.
const PAIRS: [(&str, usize, usize); 4] =
    [("o1-axial-vs-stitched", 0, 4), ("o2-radial-vs-two-circle", 2, 3), ("o3-fade-vs-lines", 6, 7), ("o4-function-vs-pattern", 5, 1)];

/// One page to render: its label, what it checks, the PDF and the 0-based page.
struct Job {
    label: String,
    what: String,
    bytes: Arc<Vec<u8>>,
    path: PathBuf,
    page: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Renderer {
    PdfCraft,
    Poppler,
    Pdfium,
}

impl Renderer {
    fn name(self) -> &'static str {
        match self {
            Renderer::PdfCraft => "pdfcraft",
            Renderer::Poppler => "poppler",
            Renderer::Pdfium => "pdfium",
        }
    }
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

fn render(renderer: Renderer, pdf: &Arc<Vec<u8>>, path: &Path, page: usize, dpi: f64, tmp: &Path) -> Result<Image, String> {
    match renderer {
        Renderer::PdfCraft => {
            let mut r = PageRenderer::new(pdf.clone(), RenderConfig::default());
            let out = r.render(RenderRequest { page, scale: (dpi / 72.0) as f32, ..RenderRequest::default() });
            if let Some(e) = out.error {
                return Err(format!("PdfCraft: {e}"));
            }
            Ok(Image::from_premultiplied_rgba(out.width as usize, out.height as usize, &out.rgba))
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
            Image::from_pnm(&bytes)
        }
    }
}

/// Per-pixel difference (largest channel difference, levels) over the common area: mean, 99th
/// percentile, maximum, and the share of pixels more than 8 levels apart.
fn compare(a: &Image, b: &Image) -> (f64, u8, u8, f64) {
    let (w, h) = (a.width.min(b.width), a.height.min(b.height));
    let mut hist = [0u64; 256];
    let (mut sum, mut over) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let (Some(p), Some(q)) = (a.get(x, y), b.get(x, y)) else { continue };
            let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
            hist[usize::from(d)] += 1;
            sum += u64::from(d);
            if d > 8 {
                over += 1;
            }
        }
    }
    let n = (w * h).max(1) as u64;
    let target = n * 99 / 100;
    let (mut seen, mut p99) = (0u64, 0u8);
    for (level, count) in hist.iter().enumerate() {
        seen += count;
        if seen >= target {
            p99 = level as u8;
            break;
        }
    }
    let max = hist.iter().rposition(|c| *c > 0).unwrap_or(0) as u8;
    (sum as f64 / n as f64, p99, max, over as f64 / n as f64)
}

fn ppm(img: &Image) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", img.width, img.height).into_bytes();
    out.extend_from_slice(&img.rgb);
    out
}

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

fn png(img: &Image) -> Vec<u8> {
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

fn write(path: &Path, bytes: &[u8]) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).expect("create directory");
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn main() {
    let mut out = PathBuf::from("sync-shading-study");
    let mut dpi = 150.0;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(args.next().expect("--out DIR")),
            "--dpi" => dpi = args.next().and_then(|v| v.parse().ok()).expect("--dpi NUMBER"),
            other => panic!("unknown argument {other}"),
        }
    }
    let tmp = out.join("tmp");
    std::fs::create_dir_all(&tmp).expect("create tmp");

    let cases = cases();
    let pages: Vec<Page> = cases.iter().map(|(_, _, p)| p.clone()).collect();
    let sources = synth::pdf(&pages);
    let sources_path = out.join("sources/shadings.pdf");
    write(&sources_path, &sources);
    let sources_doc = synth::open(&sources);

    let identity = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let placement = Placement {
        width: W,
        height: H,
        unit: 1.0,
        a: identity,
        b: identity,
        b_outline: [Point::new(0.0, 0.0), Point::new(W, 0.0), Point::new(W, H), Point::new(0.0, H)],
        b_clipped: false,
    };
    let mut overlays = Vec::new();
    for (pair, a, b) in PAIRS {
        let doc = compose(Some(Source { doc: &sources_doc, page: a }), Some(Source { doc: &sources_doc, page: b }), &placement, &Style::default())
            .unwrap_or_else(|e| panic!("{pair}: {e}"));
        let bytes = synth::save(&doc, false);
        let path = out.join(format!("overlays/{pair}.pdf"));
        write(&path, &bytes);
        overlays.push((pair, bytes, path));
    }

    let mut jobs: Vec<Job> = Vec::new();
    for (i, (case, what, _)) in cases.iter().enumerate() {
        jobs.push(Job { label: case.to_string(), what: what.to_string(), bytes: sources.clone(), path: sources_path.clone(), page: i });
    }
    for (pair, bytes, path) in &overlays {
        let (_, a, b) = PAIRS.iter().find(|(p, _, _)| p == pair).copied().expect("listed");
        let what = format!("overlay of {} (A) and {} (B)", cases[a].0, cases[b].0);
        jobs.push(Job { label: pair.to_string(), what, bytes: bytes.clone(), path: path.clone(), page: 0 });
    }

    let mut csv = String::from("page,what,pdfcraft_vs,mean_levels,p99_levels,max_levels,share_over_8\n");
    let mut md = String::from("# Shading study\n\n");
    let _ = writeln!(
        md,
        "PdfCraft against Poppler and PDFium at {dpi} dpi. Differences are the largest channel difference per pixel, in 8-bit levels.\n"
    );
    md.push_str("| Page | PdfCraft vs | Mean | 99th pct | Max | Share > 8 |\n| --- | --- | --- | --- | --- | --- |\n");
    for Job { label, what, bytes, path, page } in &jobs {
        let mut imgs = Vec::new();
        for r in [Renderer::PdfCraft, Renderer::Poppler, Renderer::Pdfium] {
            let img = render(r, bytes, path, *page, dpi, &tmp).unwrap_or_else(|e| panic!("{label} {}: {e}", r.name()));
            write(&out.join(format!("renders/{label}-{}.ppm", r.name())), &ppm(&img));
            write(&out.join(format!("renders/{label}-{}.png", r.name())), &png(&img));
            imgs.push((r, img));
        }
        let base = &imgs[0].1;
        for (r, img) in imgs.iter().skip(1) {
            let (mean, p99, max, over) = compare(base, img);
            let _ = writeln!(csv, "{label},\"{what}\",{},{mean:.3},{p99},{max},{over:.5}", r.name());
            let _ = writeln!(md, "| {label} | {} | {mean:.2} | {p99} | {max} | {:.2} % |", r.name(), over * 100.0);
        }
    }
    write(&out.join("study.csv"), csv.as_bytes());
    write(&out.join("study.md"), md.as_bytes());
    let _ = std::fs::remove_dir_all(&tmp);
    print!("{md}");
}

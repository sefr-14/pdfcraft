//! The step 3 compositing proof (OV3.4, OV3.5): generate the synthetic suite, render every
//! overlay page in PdfCraft, Poppler and PDFium, measure it against the expected values, and
//! write the proof package.
//!
//! ```text
//! cargo run -p pdfcraft-overlay --example ov3_proof -- [--out DIR] [--only SET,…] [--renderers pdfcraft,poppler,pdfium]
//! ```
//!
//! Poppler (`pdftoppm`) and PDFium (the pypdfium2 command line, `python3 -m pypdfium2_cli`) run
//! as separate processes only, as the repo's other oracles do. They are not needed by the
//! crate's tests, and the Windows workflow doesn't run this.

#[path = "../tests/common/evaluate.rs"]
mod evaluate;
#[path = "../tests/common/families.rs"]
mod families;
#[path = "../tests/common/measure.rs"]
mod measure;
#[path = "../tests/common/synth.rs"]
mod synth;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::time::Instant;

use evaluate::{LineRow, PatchRow, Renderer, TargetRow, measure_lines, measure_patches, measure_targets, render, run};
use families::{LineKind, Set};
use measure::Image;
use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};
use pdfcraft_overlay::Side;

struct Args {
    out: PathBuf,
    only: Option<Vec<String>>,
    renderers: Vec<Renderer>,
}

fn args() -> Result<Args, String> {
    let mut out = PathBuf::from("target/ov3-proof");
    let mut only = None;
    let mut renderers = vec![Renderer::PdfCraft, Renderer::Poppler, Renderer::Pdfium];
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(it.next().ok_or("--out needs a directory")?),
            "--only" => only = Some(it.next().ok_or("--only needs set names")?.split(',').map(String::from).collect()),
            "--renderers" => {
                renderers = it
                    .next()
                    .ok_or("--renderers needs names")?
                    .split(',')
                    .map(|r| match r {
                        "pdfcraft" => Ok(Renderer::PdfCraft),
                        "poppler" => Ok(Renderer::Poppler),
                        "pdfium" => Ok(Renderer::Pdfium),
                        other => Err(format!("unknown renderer {other}")),
                    })
                    .collect::<Result<_, _>>()?
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args { out, only, renderers })
}

// ── Hashing and PNG, without a third-party crate (D10) ──────────────────────────────────────

/// SHA-256 (FIPS 180-4) in hex. Checked against the standard's vectors at start-up, and against
/// `sha256sum` where that exists.
fn sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut h: [u32; 8] = [0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (word, b) in w.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for (k, wi) in K.iter().zip(w.iter()) {
            let t1 = hh
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(*k)
                .wrapping_add(*wi);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(v);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// The FIPS 180-4 example vectors.
fn sha256_self_test() -> Result<(), String> {
    let vectors: [(&[u8], &str); 3] = [
        (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
    ];
    for (input, want) in vectors {
        let got = sha256(input);
        if got != want {
            return Err(format!("SHA-256 self-test failed for {:?}: {got}", String::from_utf8_lossy(input)));
        }
    }
    Ok(())
}

/// CRC-32 (ISO 3309, as PNG uses it) over the concatenation of `parts`.
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

/// An 8-bit RGB PNG of `img`, losslessly: the renderer's own pixels, compressed with the zlib
/// encoder PdfCraft uses for PDF streams.
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

fn write(path: &Path, bytes: &[u8]) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).expect("create directory");
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn write_png(path: &Path, img: &Image) {
    write(path, &png_bytes(img));
}

// ── Vector check, sources, showcase ──────────────────────────────────────────────────────────

/// Criterion 10 for one overlay file: every page's source forms hold their source page's content
/// byte for byte, no image is added, and the page draws only q, Q, gs, rg, re and f.
fn vector_check(set: &Set) -> (bool, String) {
    let out = synth::open(&set.overlay);
    let (a, b) = (synth::open(&set.a), synth::open(&set.b));
    let images =
        |d: &Document| d.scan_objects().filter(|(_, o)| matches!(&**o, Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Image"[..]))).count();
    let mut ok = true;
    let mut text = String::new();
    let forms: Vec<Vec<u8>> = out
        .scan_objects()
        .filter_map(|(_, o)| match &*o {
            Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Form"[..]) => s.decoded().ok(),
            _ => None,
        })
        .collect();
    for case in &set.cases {
        for (doc, page) in [(&a, case.a_page), (&b, case.b_page)] {
            if let Some(p) = page {
                let content = synth::page_content(doc, p);
                let found = forms.contains(&content);
                ok &= found;
                if !found {
                    let _ = writeln!(text, "- {}: source content not found unchanged", case.id);
                }
            }
        }
    }
    let (ia, ib, io) = (images(&a), images(&b), images(&out));
    let image_ok = io == ia + ib || (io <= ia + ib);
    ok &= image_ok;
    let pages = pdfcraft_model::pages(&out);
    let mut ops_ok = true;
    for p in &pages {
        if let Some(c) = p.dict.get(b"Contents")
            && let Object::Stream(s) = &*out.resolve(c)
        {
            let parsed = pdfcraft_content::parse(&s.decoded().unwrap_or_default());
            ops_ok &= parsed.ops.iter().all(|o| [&b"q"[..], b"Q", b"gs", b"rg", b"re", b"f"].contains(&o.op.as_slice()));
        }
    }
    ok &= ops_ok;
    let _ = writeln!(
        text,
        "- {}: {} pages; source content found unchanged in forms: {}; images in sources {} + {}, in overlay {}; page operators only q Q gs rg re f: {}",
        set.name,
        pages.len(),
        if text.is_empty() { "all" } else { "NOT all" },
        ia,
        ib,
        io,
        ops_ok
    );
    (ok, text)
}

/// Criterion 11: composing from the written source files leaves them byte-identical.
fn sources_unchanged(set: &Set, dir: &Path) -> (bool, String) {
    let (ha, hb) = (sha256(&set.a), sha256(&set.b));
    let (a, b) = (synth::open(&set.a), synth::open(&set.b));
    let mut out = Document::new_empty();
    for case in &set.cases {
        let sa = case.a_page.map(|page| pdfcraft_overlay::Source { doc: &a, page });
        let sb = case.b_page.map(|page| pdfcraft_overlay::Source { doc: &b, page });
        let _ = pdfcraft_overlay::compose_page(&mut out, sa, sb, &case.placement, &case.style);
    }
    let _ = synth::save(&out, true);
    let in_memory = sha256(a.bytes()) == ha && sha256(b.bytes()) == hb && !a.is_modified() && !b.is_modified();
    // The files on disk, after every renderer has read them, hashed here and by sha256sum.
    let file = |side: &str| dir.join(format!("sources/{}-{side}.pdf", set.name));
    let on_disk = ["A", "B"].iter().zip([&ha, &hb]).all(|(side, h)| std::fs::read(file(side)).is_ok_and(|bytes| sha256(&bytes) == **h));
    let tool: Vec<Option<String>> = ["A", "B"]
        .iter()
        .map(|side| run(Command::new("sha256sum").arg(file(side))).ok().and_then(|s| s.split_whitespace().next().map(String::from)))
        .collect();
    let tool_text = match (&tool[0], &tool[1]) {
        (Some(ta), Some(tb)) if *ta == ha && *tb == hb => "sha256sum agrees",
        (Some(_), Some(_)) => "sha256sum DISAGREES",
        _ => "sha256sum not available",
    };
    let ok = in_memory && on_disk && tool_text != "sha256sum DISAGREES";
    let text = format!(
        "- {}: A {}…, B {}…; in memory {}; files on disk {}; {tool_text}: {}",
        set.name,
        &ha[..16],
        &hb[..16],
        if in_memory { "unchanged" } else { "CHANGED" },
        if on_disk { "unchanged" } else { "CHANGED" },
        if ok { "pass" } else { "FAIL" }
    );
    (ok, text)
}

fn ascii(s: &str) -> String {
    s.replace('°', " deg").replace('×', "x").replace(['−', '–', '—'], "-").chars().filter(|c| c.is_ascii()).collect()
}

fn pdf_text(s: &str) -> String {
    ascii(s).replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

/// The showcase: every overlay page of the suite in one file, each with a label in plain text
/// over its top margin (the label is the only thing added; it is not part of any measured case).
fn showcase(sets: &[Set]) -> Arc<Vec<u8>> {
    let mut out = Document::new_empty();
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let font = out.add(Object::Dict(font));
    for set in sets {
        let (a, b) = (synth::open(&set.a), synth::open(&set.b));
        for case in &set.cases {
            let sa = case.a_page.map(|page| pdfcraft_overlay::Source { doc: &a, page });
            let sb = case.b_page.map(|page| pdfcraft_overlay::Source { doc: &b, page });
            let page = pdfcraft_overlay::compose_page(&mut out, sa, sb, &case.placement, &case.style).expect("composes");
            let h = case.placement.page_box()[3];
            let size = (9.0 / case.placement.unit).max(4.0);
            let label = format!("{}  {}", case.id, case.title);
            let text = format!("q 0 g BT /Lbl {size} Tf 12 {} Td ({}) Tj ET Q", h - 2.0 * size, pdf_text(&label));
            let stream = out.add(Object::Stream(Stream::flate(Dict::new(), text.as_bytes())));
            out.update_dict(page, |d| {
                let first = d.get(b"Contents").cloned().unwrap_or(Object::Null);
                d.set(b"Contents".to_vec(), Object::Array(vec![first, Object::Ref(stream)]));
                let mut res = d.get(b"Resources").and_then(|r| r.as_dict().cloned()).unwrap_or_default();
                let mut fonts = Dict::new();
                fonts.set(b"Lbl".to_vec(), Object::Ref(font));
                res.set(b"Font".to_vec(), Object::Dict(fonts));
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })
            .expect("page");
        }
    }
    let mut info = Dict::new();
    info.set(b"Title".to_vec(), Object::String(PdfString::text("PdfCraft overlay step 3: compositing proof showcase")));
    let info = out.add(Object::Dict(info));
    out.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    synth::save(&out, true)
}

/// Three renders side by side, then the largest difference between any two of them per pixel,
/// amplified 8× (black: no difference).
fn side_by_side(renders: &[(Renderer, &Image)]) -> Option<Image> {
    let (w, h) = (renders.iter().map(|(_, i)| i.width).min()?, renders.iter().map(|(_, i)| i.height).min()?);
    let gap = 12;
    let panels = renders.len() + 1;
    let width = panels * w + (panels - 1) * gap;
    let mut rgb = vec![128u8; width * h * 3];
    for (k, (_, img)) in renders.iter().enumerate() {
        for y in 0..h {
            let src = &img.rgb[(y * img.width) * 3..(y * img.width + w) * 3];
            let at = (y * width + k * (w + gap)) * 3;
            rgb[at..at + w * 3].copy_from_slice(src);
        }
    }
    let k = renders.len();
    for y in 0..h {
        for x in 0..w {
            let mut worst = 0u8;
            for i in 0..renders.len() {
                for j in i + 1..renders.len() {
                    let (p, q) = (renders[i].1.get(x, y)?, renders[j].1.get(x, y)?);
                    for c in 0..3 {
                        worst = worst.max(p[c].abs_diff(q[c]));
                    }
                }
            }
            let v = worst.saturating_mul(8);
            let at = (y * width + k * (w + gap) + x) * 3;
            rgb[at..at + 3].copy_from_slice(&[v, v, v]);
        }
    }
    Some(Image { width, height: h, rgb })
}

// ── Reporting ────────────────────────────────────────────────────────────────────────────────

fn fmt3(v: [f64; 3]) -> String {
    format!("{:.1}, {:.1}, {:.1}", v[0], v[1], v[2])
}

fn main() -> ExitCode {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = sha256_self_test() {
        eprintln!("{e}");
        return ExitCode::from(1);
    }
    let started = Instant::now();
    let out = args.out.clone();
    let tmp = out.join("tmp");
    std::fs::create_dir_all(&tmp).expect("output directory");

    // Generate twice: the suite must be byte-identical (criterion 13).
    let mut sets = families::all();
    let again = families::all();
    let mut determinism = Vec::new();
    for (s, t) in sets.iter().zip(&again) {
        determinism.push((s.name, s.a == t.a && s.b == t.b && s.overlay == t.overlay));
    }
    drop(again);
    if let Some(only) = &args.only {
        sets.retain(|s| only.iter().any(|o| o == s.name));
    }
    for s in &sets {
        write(&out.join(format!("sources/{}-A.pdf", s.name)), &s.a);
        write(&out.join(format!("sources/{}-B.pdf", s.name)), &s.b);
        write(&out.join(format!("overlays/{}.pdf", s.name)), &s.overlay);
    }
    let show = showcase(&sets);
    write(&out.join("showcase.pdf"), &show);

    let versions: Vec<(String, String)> = vec![
        ("pdftoppm".into(), run(Command::new("pdftoppm").arg("-v")).unwrap_or_else(|e| e).lines().next().unwrap_or_default().to_string()),
        ("pdfium".into(), run(Command::new("python3").args(["-m", "pypdfium2_cli", "--version"])).unwrap_or_else(|e| e).trim().replace('\n', "; ")),
        ("pdfcraft".into(), run(Command::new("git").args(["rev-parse", "--short", "HEAD"])).unwrap_or_else(|e| e).trim().to_string()),
    ];

    let mut patches = Vec::new();
    let mut targets = Vec::new();
    let mut lines = Vec::new();
    let mut render_notes = Vec::new();
    let mut timings = Vec::new();
    for set in &sets {
        let path = out.join(format!("overlays/{}.pdf", set.name));
        for case in &set.cases {
            for &dpi in &case.dpis {
                let mut kept: Vec<(Renderer, Image)> = Vec::new();
                for &renderer in &args.renderers {
                    let r = match render(renderer, &set.overlay, &path, case.page, dpi, case, &tmp) {
                        Ok(r) => r,
                        Err(e) => {
                            render_notes.push(format!("{} p{} {} at {dpi} dpi: render failed: {e}", set.name, case.page + 1, renderer.name()));
                            continue;
                        }
                    };
                    if (r.effective_dpi - r.dpi).abs() > 0.5 {
                        render_notes.push(format!(
                            "{} ({}) {} at {dpi} dpi: rendered at {:.1} dpi, the renderer's limit ({} × {} px)",
                            case.id,
                            set.name,
                            renderer.name(),
                            r.effective_dpi,
                            r.img.width,
                            r.img.height
                        ));
                    }
                    if case.placement.unit != 1.0 || !r.honours_unit {
                        render_notes.push(format!(
                            "{} {} at {dpi} dpi: page /UserUnit {} {} ({} × {} px)",
                            case.id,
                            renderer.name(),
                            case.placement.unit,
                            if r.honours_unit { "applied" } else { "ignored" },
                            r.img.width,
                            r.img.height
                        ));
                    }
                    timings.push((case.id.clone(), renderer, dpi, r.millis, r.img.width, r.img.height));
                    measure_patches(set.name, case, &r, renderer, &mut patches);
                    measure_targets(set.name, case, &r, renderer, &mut targets);
                    measure_lines(case, &r, renderer, &mut lines);
                    if dpi <= 150.0 || set.name == "f01-overlap" || set.name == "f17-light-tones" {
                        write_png(&out.join(format!("renders/{}dpi/{}/{}.png", dpi as u32, renderer.name(), case.id)), &r.img);
                    }
                    kept.push((renderer, r.img));
                }
                if kept.len() > 1 && dpi <= 150.0 {
                    let refs: Vec<(Renderer, &Image)> = kept.iter().map(|(r, i)| (*r, i)).collect();
                    if let Some(img) = side_by_side(&refs) {
                        write_png(&out.join(format!("compare/{}dpi/{}.png", dpi as u32, case.id)), &img);
                    }
                }
            }
        }
    }

    // ── Results ──
    let mut md = String::new();
    let _ = writeln!(md, "# Step 3 compositing proof: results\n");
    let _ = writeln!(md, "Generated by `cargo run -p pdfcraft-overlay --example ov3_proof` in {:.0} s.\n", started.elapsed().as_secs_f64());
    for (k, v) in &versions {
        let _ = writeln!(md, "- {k}: {v}");
    }
    let renderer_names: Vec<&str> = args.renderers.iter().map(|r| r.name()).collect();
    let _ = writeln!(md, "- renderers: {}", renderer_names.join(", "));
    let _ = writeln!(md);

    // Criterion table.
    let mut crit: BTreeMap<String, (usize, usize, f64, String)> = BTreeMap::new();
    for p in &patches {
        if let Some(ok) = p.pass {
            let key = p.criterion.split_whitespace().next().unwrap_or("").to_string();
            let e = crit.entry(key).or_insert((0, 0, 0.0, String::new()));
            e.0 += 1;
            if !ok {
                e.1 += 1;
            }
            if p.error > e.2 {
                e.2 = p.error;
                e.3 = format!("{} / {} ({} at {} dpi): {:.1} levels", p.case, p.patch, p.renderer.name(), p.dpi, p.error);
            }
        }
    }
    let _ = writeln!(md, "## Patches by criterion\n\n| Criterion | Checks | Failures | Worst error | Where |\n| --- | --- | --- | --- | --- |");
    for (k, (n, f, w, where_)) in &crit {
        let _ = writeln!(md, "| {k} | {n} | {f} | {w:.2} | {where_} |");
    }

    // Light tones (criterion 6): agreement between renderers too.
    let _ = writeln!(
        md,
        "\n## Light tones (criterion 6)\n\n| Case | Patch | Expected m | Recovered m per renderer (dpi) | Worst error, levels | Renderer spread, levels |\n| --- | --- | --- | --- | --- | --- |"
    );
    let mut by_patch: BTreeMap<(String, String, u32), Vec<&PatchRow>> = BTreeMap::new();
    for p in patches.iter().filter(|p| p.mask.is_some()) {
        by_patch.entry((p.case.clone(), p.patch.clone(), p.dpi as u32)).or_default().push(p);
    }
    let mut spread_fail = 0;
    for ((case, patch, dpi), rows) in &by_patch {
        let ms: Vec<f64> = rows.iter().filter_map(|r| r.mask.map(|m| m.0)).collect();
        let spread = (ms.iter().cloned().fold(f64::MIN, f64::max) - ms.iter().cloned().fold(f64::MAX, f64::min)) * 255.0;
        if spread > 1.0 + 1e-9 {
            spread_fail += 1;
        }
        let worst = rows.iter().map(|r| r.error).fold(0.0, f64::max);
        let per: Vec<String> = rows.iter().map(|r| format!("{} {:.4}", r.renderer.name(), r.mask.map(|m| m.0).unwrap_or(f64::NAN))).collect();
        let want = rows.first().and_then(|r| r.mask.map(|m| m.1)).unwrap_or(f64::NAN);
        let _ = writeln!(md, "| {case} | {patch} | {want:.4} | {} ({dpi}) | {worst:.2} | {spread:.2} |", per.join(", "));
    }
    let _ = writeln!(md, "\nRenderer spread above 1 level: {spread_fail} patches.");

    // Report-only patches (CMYK, separation, hatching at t < 1): values per renderer.
    let _ = writeln!(
        md,
        "\n## Report-only patches (no single expected value)\n\n| Case | Patch | dpi | {} | Spread, levels |\n| --- | --- | --- | {} --- |",
        renderer_names.join(" | "),
        "--- | ".repeat(renderer_names.len())
    );
    let mut report: BTreeMap<(String, String, u32), Vec<&PatchRow>> = BTreeMap::new();
    for p in patches.iter().filter(|p| p.pass.is_none()) {
        report.entry((p.case.clone(), p.patch.clone(), p.dpi as u32)).or_default().push(p);
    }
    for ((case, patch, dpi), rows) in &report {
        let mut cells = Vec::new();
        for r in &args.renderers {
            cells.push(rows.iter().find(|x| x.renderer == *r).map(|x| fmt3(x.value)).unwrap_or_else(|| "-".into()));
        }
        let spread = (0..3)
            .map(|c| {
                let v: Vec<f64> = rows.iter().map(|r| r.value[c]).collect();
                v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min)
            })
            .fold(0.0, f64::max);
        let _ = writeln!(md, "| {case} | {patch} | {dpi} | {} | {spread:.1} |", cells.join(" | "));
    }

    // Geometry (criterion 8).
    let tol_pt = 0.5 * 72.0 / 300.0;
    let on: Vec<&TargetRow> = targets.iter().filter(|t| t.on_page).collect();
    let worst = on.iter().filter_map(|t| t.error_pt.map(|e| (e, *t))).fold((0.0, None), |acc, (e, t)| if e > acc.0 { (e, Some(t)) } else { acc });
    let missing = on.iter().filter(|t| t.measured.is_none()).count();
    let failures = on.iter().filter(|t| t.error_pt.is_some_and(|e| e > tol_pt)).count();
    let _ = writeln!(md, "\n## Geometry (criterion 8)\n");
    let _ = writeln!(md, "Tolerance: 0.5 px at 300 dpi = {tol_pt:.3} pt, applied at every dpi.\n");
    let _ = writeln!(md, "- Targets measured: {} on the page, {} expected off the page (clipped).", on.len(), targets.len() - on.len());
    let _ = writeln!(md, "- Targets not found: {missing}. Targets past the tolerance: {failures}.");
    if let (w, Some(t)) = worst {
        let _ = writeln!(
            md,
            "- Worst: {w:.4} pt ({:.3} px at 300 dpi), {} {} ({} at {} dpi).",
            w * 300.0 / 72.0,
            t.case,
            t.target,
            t.renderer.name(),
            t.dpi
        );
    }
    // Coincidence of partners.
    let mut coincide_worst = (0.0f64, String::new());
    let mut coincide_fail = 0;
    let mut coincide_n = 0;
    for t in on.iter().filter(|t| t.side == Side::A) {
        if let Some(pname) = &t.partner
            && let Some(p) = on.iter().find(|u| u.case == t.case && &u.target == pname && u.renderer == t.renderer && u.dpi == t.dpi)
            && let (Some(m), Some(n)) = (t.measured, p.measured)
        {
            coincide_n += 1;
            let unit = 1.0;
            let d = ((m.0 - n.0).powi(2) + (m.1 - n.1).powi(2)).sqrt() * unit;
            if d > tol_pt {
                coincide_fail += 1;
            }
            if d > coincide_worst.0 {
                coincide_worst = (d, format!("{} {}/{} ({} at {} dpi)", t.case, t.target, pname, t.renderer.name(), t.dpi));
            }
        }
    }
    let _ = writeln!(
        md,
        "- A and B targets that must coincide: {coincide_n} pairs; past the tolerance: {coincide_fail}; worst {:.4} pt, {}.",
        coincide_worst.0, coincide_worst.1
    );
    let _ = writeln!(md, "\n| Case | Renderer | dpi | Worst target error, pt | Worst A–B distance, pt |\n| --- | --- | --- | --- | --- |");
    let mut per_case: BTreeMap<(String, &'static str, u32), (f64, f64)> = BTreeMap::new();
    for t in &on {
        let e = per_case.entry((t.case.clone(), t.renderer.name(), t.dpi as u32)).or_insert((0.0, 0.0));
        e.0 = e.0.max(t.error_pt.unwrap_or(f64::INFINITY));
        if t.side == Side::A
            && let Some(pname) = &t.partner
            && let Some(p) = on.iter().find(|u| u.case == t.case && &u.target == pname && u.renderer == t.renderer && u.dpi == t.dpi)
            && let (Some(m), Some(n)) = (t.measured, p.measured)
        {
            e.1 = e.1.max(((m.0 - n.0).powi(2) + (m.1 - n.1).powi(2)).sqrt());
        }
    }
    for ((case, r, dpi), (e, d)) in &per_case {
        let _ = writeln!(md, "| {case} | {r} | {dpi} | {e:.4} | {d:.4} |");
    }

    // Lines: criteria 3, 4 and the displacement table (5).
    let mut vis_fail = Vec::new();
    let mut zero_hits = (0usize, 0usize, 0usize);
    let mut zero_old = 0usize;
    type Key = (&'static str, u32, &'static str, i64, i64);
    let key = |l: &LineRow| -> Key { (l.renderer.name(), l.dpi as u32, l.pair, l.orientation as i64, (l.width_mm * 100.0).round() as i64) };
    let mut groups: BTreeMap<Key, Vec<&LineRow>> = BTreeMap::new();
    for l in &lines {
        groups.entry(key(l)).or_default().push(l);
    }
    let mut table = String::new();
    let _ = writeln!(table, "renderer,dpi,pair,orientation_deg,width_mm,min_distinguishable_mm_d1,min_distinguishable_mm_nearest_colour");
    let mut md_table = String::from(
        "| Renderer | dpi | Pair | Angle | Width, mm | Minimum displacement, mm (D1) | Nearest-colour rule |\n| --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for (k, rows) in &groups {
        // Visibility of one-document lines at every offset.
        for kind in [LineKind::AOnly, LineKind::BOnly] {
            let ok =
                (0..8).all(|o| rows.iter().any(|l| l.kind == kind && l.offset == o && if kind == LineKind::AOnly { l.d1.a > 0 } else { l.d1.b > 0 }));
            if !ok {
                vis_fail.push(format!("{k:?} {kind:?}"));
            }
        }
        for l in rows.iter().filter(|l| l.kind == LineKind::Shared) {
            zero_hits.0 += l.d1.a + l.d1.b;
            zero_hits.1 += l.d1.ambiguous();
            zero_hits.2 += 1;
            zero_old += l.nearest.a + l.nearest.b;
        }
        let min_d = |old: bool| -> String {
            for d in families::DISPLACEMENTS_MM.iter().skip(1) {
                let all = (0..8).all(|o| {
                    rows.iter().any(|l| {
                        l.kind == LineKind::Displaced(*d) && l.offset == o && {
                            let c = if old { &l.nearest } else { &l.d1 };
                            c.a > 0 && c.b > 0
                        }
                    })
                });
                if all {
                    return format!("{d}");
                }
            }
            "not distinguished up to 0.5".into()
        };
        let (a, b) = (min_d(false), min_d(true));
        let _ = writeln!(table, "{},{},{},{},{},{},{}", k.0, k.1, k.2, k.3, k.4 as f64 / 100.0, a, b);
        let _ = writeln!(md_table, "| {} | {} | {} | {}° | {:.2} | {a} | {b} |", k.0, k.1, k.2, k.3, k.4 as f64 / 100.0);
    }
    write(&out.join("displacement.csv"), table.as_bytes());
    let _ = writeln!(md, "\n## Lines (criteria 3 to 5)\n");
    let _ = writeln!(md, "- Conditions: {} (renderer × dpi × pair × angle × width).", groups.len());
    let _ = writeln!(
        md,
        "- Criterion 3, a one-document line visible at every offset: {} failures{}.",
        vis_fail.len(),
        if vis_fail.is_empty() { String::new() } else { format!(": {}", vis_fail.join("; ")) }
    );
    let _ = writeln!(
        md,
        "- Criterion 4, identical lines: {} A or B pixels in {} profiles under D1 ({} reported ambiguous); the nearest-colour rule finds {} A or B pixels in the same profiles.",
        zero_hits.0, zero_hits.2, zero_hits.1, zero_old
    );
    let _ = writeln!(md, "\n### Minimum distinguishable displacement (criterion 5)\n\n{md_table}");

    // Facts checked without rendering.
    let _ = writeln!(md, "\n## Checks on the files\n");
    let mut checks_fail = 0;
    for set in &sets {
        for case in &set.cases {
            for (what, ok, detail) in &case.checks {
                if !ok {
                    checks_fail += 1;
                    let _ = writeln!(md, "- FAILED {} {what}: {detail}", case.id);
                }
            }
        }
    }
    let n_checks: usize = sets.iter().map(|s| s.cases.iter().map(|c| c.checks.len()).sum::<usize>()).sum();
    let _ = writeln!(md, "- {n_checks} checks (page box and unit equal the placement's, criterion 9a), {checks_fail} failed.");
    let mut vec_md = String::from("# Vector check (criterion 10)\n\n");
    let mut vec_ok = true;
    for set in &sets {
        let (ok, text) = vector_check(set);
        vec_ok &= ok;
        vec_md.push_str(&text);
    }
    write(&out.join("vector-check.md"), vec_md.as_bytes());
    let _ = writeln!(md, "- Vector check (criterion 10): {}. See vector-check.md.", if vec_ok { "pass" } else { "FAIL" });
    let mut src_ok = true;
    let mut src_md = String::new();
    for set in &sets {
        let (ok, text) = sources_unchanged(set, &out);
        src_ok &= ok;
        let _ = writeln!(src_md, "{text}");
    }
    let _ = writeln!(md, "- Sources unchanged (criterion 11): {}.\n{src_md}", if src_ok { "pass" } else { "FAIL" });
    let det_ok = determinism.iter().all(|(_, ok)| *ok);
    let _ = writeln!(md, "- Determinism (criterion 13): two generations byte-identical: {}.", if det_ok { "pass" } else { "FAIL" });

    // Renders that hit a limit or a /UserUnit question.
    let _ = writeln!(md, "\n## Render notes\n");
    for n in &render_notes {
        let _ = writeln!(md, "- {n}");
    }

    // Incompatibilities: patch cases where renderers disagree beyond the tolerance.
    let mut inc = String::from(
        "# Renderer incompatibilities\n\nPatches where at least one renderer misses the expected value while another meets it, or where renderers differ by more than the tolerance.\n\n| Case | Patch | dpi | Expected | Values | Errors, levels |\n| --- | --- | --- | --- | --- | --- |\n",
    );
    let mut n_inc = 0;
    let mut grouped: BTreeMap<(String, String, u32), Vec<&PatchRow>> = BTreeMap::new();
    for p in patches.iter().filter(|p| p.pass.is_some()) {
        grouped.entry((p.case.clone(), p.patch.clone(), p.dpi as u32)).or_default().push(p);
    }
    for ((case, patch, dpi), rows) in &grouped {
        let any_pass = rows.iter().any(|r| r.pass == Some(true));
        let any_fail = rows.iter().any(|r| r.pass == Some(false));
        if any_fail {
            n_inc += 1;
            let vals: Vec<String> = rows.iter().map(|r| format!("{} ({})", r.renderer.name(), fmt3(r.value))).collect();
            let errs: Vec<String> = rows.iter().map(|r| format!("{} {:.1}", r.renderer.name(), r.error)).collect();
            let exp = rows.first().and_then(|r| r.expected).map(fmt3).unwrap_or_else(|| "mask".into());
            let _ = writeln!(
                inc,
                "| {case}{} | {patch} | {dpi} | {exp} | {} | {} |",
                if any_pass { "" } else { " (all fail)" },
                vals.join("; "),
                errs.join("; ")
            );
        }
    }
    write(&out.join("incompatibilities.md"), inc.as_bytes());
    let _ = writeln!(md, "\nPatch cases with a failure: {n_inc}. See incompatibilities.md.");

    // CSVs.
    let mut csv =
        String::from("set,case,patch,criterion,renderer,dpi,r,g,b,expected_r,expected_g,expected_b,mask,expected_mask,tolerance,error,pass\n");
    for p in &patches {
        let e = p.expected.unwrap_or([f64::NAN; 3]);
        let (m, me) = p.mask.unwrap_or((f64::NAN, f64::NAN));
        let _ = writeln!(
            csv,
            "{},{},\"{}\",{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.5},{:.5},{},{:.3},{}",
            p.set,
            p.case,
            p.patch,
            p.criterion,
            p.renderer.name(),
            p.dpi,
            p.value[0],
            p.value[1],
            p.value[2],
            e[0],
            e[1],
            e[2],
            m,
            me,
            p.tol,
            p.error,
            p.pass.map_or("report".to_string(), |b| b.to_string())
        );
    }
    write(&out.join("patches.csv"), csv.as_bytes());
    let mut tcsv = String::from("set,case,target,side,renderer,dpi,expected_x,expected_y,measured_x,measured_y,error_pt,on_page\n");
    for t in &targets {
        let m = t.measured.unwrap_or((f64::NAN, f64::NAN));
        let _ = writeln!(
            tcsv,
            "{},{},{},{:?},{},{},{:.4},{:.4},{:.4},{:.4},{:.5},{}",
            t.set,
            t.case,
            t.target,
            t.side,
            t.renderer.name(),
            t.dpi,
            t.expected.0,
            t.expected.1,
            m.0,
            m.1,
            t.error_pt.unwrap_or(f64::NAN),
            t.on_page
        );
    }
    write(&out.join("targets.csv"), tcsv.as_bytes());
    let mut lcsv = String::from(
        "renderer,dpi,pair,orientation_deg,width_mm,kind,offset,d1_a,d1_b,d1_shared,d1_paper,d1_near,d1_under,d1_unexplained,nearest_a,nearest_b,nearest_shared,nearest_paper\n",
    );
    for l in &lines {
        let _ = writeln!(
            lcsv,
            "{},{},{},{},{},{:?},{},{},{},{},{},{},{},{},{},{},{},{}",
            l.renderer.name(),
            l.dpi,
            l.pair,
            l.orientation,
            l.width_mm,
            l.kind,
            l.offset,
            l.d1.a,
            l.d1.b,
            l.d1.shared,
            l.d1.paper,
            l.d1.near,
            l.d1.under,
            l.d1.unexplained,
            l.nearest.a,
            l.nearest.b,
            l.nearest.shared,
            l.nearest.paper
        );
    }
    write(&out.join("lines.csv"), lcsv.as_bytes());

    // Run record.
    let mut run_md = String::from("# Run record\n\n");
    for (k, v) in &versions {
        let _ = writeln!(run_md, "- {k}: {v}");
    }
    let _ = writeln!(run_md, "- seeds: SplitMix64 0x5CA0_0003 (scan image); the rest of the suite is fixed geometry");
    let _ = writeln!(
        run_md,
        "- commands: pdftoppm -r DPI -f N -l N -singlefile; python3 -m pypdfium2_cli render --scale DPI/72 --format ppm --no-x-channel; PdfCraft in process (PageRenderer, scale DPI/72), each renderer's default antialiasing"
    );
    if let Ok(status) = std::fs::read_to_string("/proc/self/status")
        && let Some(l) = status.lines().find(|l| l.starts_with("VmHWM"))
    {
        let _ = writeln!(run_md, "- peak memory of this process (PdfCraft renders in process): {}", l.trim_start_matches("VmHWM:").trim());
    }
    let _ = writeln!(run_md, "\n| Case | Renderer | dpi | Time, ms | Pixels |\n| --- | --- | --- | --- | --- |");
    for (case, r, dpi, ms, w, h) in &timings {
        if case.contains("arch") || case.contains("lines") {
            let _ = writeln!(run_md, "| {case} | {} | {dpi} | {ms} | {w} × {h} |", r.name());
        }
    }
    write(&out.join("run.md"), run_md.as_bytes());
    write(&out.join("results.md"), md.as_bytes());
    let _ = std::fs::remove_dir_all(&tmp);
    println!("{}", md);
    ExitCode::SUCCESS
}

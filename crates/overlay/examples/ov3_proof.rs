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

use evaluate::{LineRow, PatchRow, Render, Renderer, TargetRow, measure_lines, measure_patches, measure_targets, render, run};
use families::{Case, Expect, LineKind, Set};
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
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (word, b) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*b);
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

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Int(i) => Some(*i as f64),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn numbers(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    match &*doc.resolve(o?) {
        Object::Array(a) => a.iter().map(number).collect(),
        _ => None,
    }
}

/// Criterion 7 for one overlay file: on every page, the layers' soft masks share one transfer
/// function object, and it is a Type 2 function with Domain [0 1], C0 [1], C1 [1 − 1/t] exactly
/// (t from the case), N 1. The preview is PdfCraft's render of this same file.
fn transfer_check(set: &Set) -> (bool, String) {
    let doc = synth::open(&set.overlay);
    let pages = pdfcraft_model::pages(&doc);
    let mut bad = Vec::new();
    for case in &set.cases {
        let Some(page) = pages.get(case.page) else {
            bad.push(format!("{}: no page {}", case.id, case.page + 1));
            continue;
        };
        let dict = |o: Option<&Object>| o.and_then(|o| doc.resolve(o).as_dict().cloned());
        let states = dict(dict(page.dict.get(b"Resources")).as_ref().and_then(|r| r.get(b"ExtGState")));
        let mut refs = Vec::new();
        for (_, gs) in states.iter().flat_map(|d| d.iter()) {
            match dict(dict(Some(gs)).as_ref().and_then(|g| g.get(b"SMask"))).as_ref().and_then(|m| m.get(b"TR")) {
                Some(Object::Ref(r)) => refs.push(*r),
                _ => bad.push(format!("{}: a soft mask without an indirect /TR", case.id)),
            }
        }
        let want = usize::from(case.a_page.is_some()) + usize::from(case.b_page.is_some());
        refs.dedup();
        let [tr] = refs.as_slice() else {
            bad.push(format!("{}: {} transfer functions for {want} layers", case.id, refs.len()));
            continue;
        };
        let f = doc.get(*tr);
        let Some(f) = f.as_dict() else {
            bad.push(format!("{}: /TR is not a dictionary", case.id));
            continue;
        };
        let t = case.style.threshold;
        let ok = f.get(b"FunctionType").and_then(number) == Some(2.0)
            && numbers(&doc, f.get(b"Domain")) == Some(vec![0.0, 1.0])
            && numbers(&doc, f.get(b"C0")) == Some(vec![1.0])
            && numbers(&doc, f.get(b"C1")) == Some(vec![1.0 - 1.0 / t])
            && f.get(b"N").and_then(number) == Some(1.0);
        if !ok {
            bad.push(format!("{}: /TR is not FunctionType 2, Domain [0 1], C0 [1], C1 [{}], N 1", case.id, 1.0 - 1.0 / t));
        }
    }
    let ok = bad.is_empty();
    let text = if ok {
        format!("- {}: {} pages, one shared Type 2 function per page with C1 = 1 − 1/t exactly: pass", set.name, set.cases.len())
    } else {
        format!("- {}: FAIL: {}", set.name, bad.join("; "))
    };
    (ok, text)
}

/// A zoomed mosaic of one width's cells on a line-study page, for every renderer side by side:
/// 8 columns (sub-pixel offsets 0 to 7) by 9 rows (displacement 0, 0.025, 0.05, 0.1, 0.15, 0.25,
/// 0.5 mm, A only, B only), each tile 12 × 12 pt around the cell centre, enlarged to about
/// 100 px with nearest-neighbour pixels.
fn zoom_mosaic(case: &Case, width_mm: f64, renders: &[(Renderer, Render)]) -> Option<Image> {
    let tile_pt = 12.0;
    let (cols, rows, gap, panel_gap) = (8usize, 9usize, 10usize, 40usize);
    let kinds: Vec<LineKind> = families::DISPLACEMENTS_MM
        .iter()
        .map(|d| if *d == 0.0 { LineKind::Shared } else { LineKind::Displaced(*d) })
        .chain([LineKind::AOnly, LineKind::BOnly])
        .collect();
    let first = &renders.first()?.1;
    let src = (tile_pt * first.scale_x).round().max(1.0) as usize;
    let zoom = (100 / src).max(1);
    let tile = src * zoom;
    let panel_w = cols * tile + (cols - 1) * gap;
    let panel_h = rows * tile + (rows - 1) * gap;
    let width = renders.len() * panel_w + (renders.len() - 1) * panel_gap;
    // Light gray between tiles, darker between renderers, so neither reads as a drawn line.
    let mut rgb = vec![232u8; width * panel_h * 3];
    for k in 1..renders.len() {
        for y in 0..panel_h {
            let x0 = k * (panel_w + panel_gap) - panel_gap;
            let at = (y * width + x0) * 3;
            rgb[at..at + panel_gap * 3].fill(190);
        }
    }
    for (k, (_, r)) in renders.iter().enumerate() {
        for (row, kind) in kinds.iter().enumerate() {
            for col in 0..cols {
                let Some(l) = case.lines.iter().find(|l| l.width_mm == width_mm && l.kind == *kind && usize::from(l.offset) == col) else { continue };
                let (cx, cy) = r.to_px(l.centre.0, l.centre.1);
                let (x0, y0) = ((cx - src as f64 / 2.0).round().max(0.0) as usize, (cy - src as f64 / 2.0).round().max(0.0) as usize);
                for ty in 0..tile {
                    for tx in 0..tile {
                        let px = r.img.get(x0 + tx / zoom, y0 + ty / zoom).unwrap_or([255, 255, 255]);
                        let (ox, oy) = (k * (panel_w + panel_gap) + col * (tile + gap) + tx, row * (tile + gap) + ty);
                        let at = (oy * width + ox) * 3;
                        rgb[at..at + 3].copy_from_slice(&px);
                    }
                }
            }
        }
    }
    Some(Image { width, height: panel_h, rgb })
}

/// The case map (PRD deliverable): every probe with its file, page, region, expected value,
/// tolerance and criterion, as CSV, and one row per overlay page as a readable table.
fn case_map(sets: &[Set]) -> (String, String) {
    let mut csv = String::from(
        "set,case,overlay_file,overlay_page,a_file,a_page,b_file,b_page,threshold,probe_kind,probe,region_or_centre,expected,tolerance,criterion,dpis\n",
    );
    let mut md = String::from(
        "# Case map\n\nOne row per overlay page. Every probe (patch, target, line) with its region, expected value, tolerance and criterion is in cases.csv. Coordinates are the output page's user space units, origin bottom left.\n\n| Case | Title | Overlay page | A page | B page | t | Patches | Targets | Lines | dpi |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let page = |p: Option<usize>| p.map_or("-".to_string(), |p| (p + 1).to_string());
    for set in sets {
        for c in &set.cases {
            let dpis: Vec<String> = c.dpis.iter().map(|d| format!("{d}")).collect();
            let head = format!(
                "{},{},overlays/{}.pdf,{},sources/{}-A.pdf,{},sources/{}-B.pdf,{},{}",
                set.name,
                c.id,
                set.name,
                c.page + 1,
                set.name,
                page(c.a_page),
                set.name,
                page(c.b_page),
                c.style.threshold
            );
            for p in &c.patches {
                let (expected, tol) = match &p.expect {
                    Expect::Colour { rgb, tol } => (format!("median {}", fmt3(*rgb)), format!("{tol} levels")),
                    Expect::Mean { rgb, tol } => (format!("mean {}", fmt3(*rgb)), format!("{tol} levels")),
                    Expect::Mask { m, tol } => (format!("mask m = {m:.4}"), format!("{tol} levels")),
                    Expect::Report => ("report only".to_string(), "-".to_string()),
                };
                let r = p.rect;
                let _ = writeln!(
                    csv,
                    "{head},patch,\"{}\",{} {} {} {},\"{expected}\",{tol},{},{}",
                    p.name,
                    r[0],
                    r[1],
                    r[2],
                    r[3],
                    p.criterion,
                    dpis.join(" ")
                );
            }
            for t in &c.targets {
                let _ = writeln!(
                    csv,
                    "{head},target,{} ({:?}{}),{:.4} {:.4},\"{}\",0.5 px at 300 dpi,8,{}",
                    t.name,
                    t.side,
                    t.partner.as_ref().map_or(String::new(), |p| format!(", coincides with {p}")),
                    t.centre.0,
                    t.centre.1,
                    if t.on_page { "centroid at the centre" } else { "absent (off the page)" },
                    dpis.join(" ")
                );
            }
            for l in &c.lines {
                let _ = writeln!(
                    csv,
                    "{head},line,{:?} {} mm {}deg offset {},{:.4} {:.4},\"{}\",D1 rule,{},{}",
                    l.kind,
                    l.width_mm,
                    l.orientation_deg,
                    l.offset,
                    l.centre.0,
                    l.centre.1,
                    match l.kind {
                        LineKind::Shared => "no A or B pixel",
                        LineKind::AOnly => "an A pixel on the profile",
                        LineKind::BOnly => "a B pixel on the profile",
                        LineKind::Displaced(_) => "reported: A and B pixels on the profile",
                    },
                    match l.kind {
                        LineKind::Shared => "4",
                        LineKind::AOnly | LineKind::BOnly => "3",
                        LineKind::Displaced(_) => "5",
                    },
                    dpis.join(" ")
                );
            }
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                c.id,
                c.title.replace('|', "/"),
                c.page + 1,
                page(c.a_page),
                page(c.b_page),
                c.style.threshold,
                c.patches.len(),
                c.targets.len(),
                c.lines.len(),
                dpis.join(", ")
            );
        }
    }
    (csv, md)
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
        (
            "pdfcraft".into(),
            run(Command::new("git").args(["describe", "--always", "--dirty", "--abbrev=7"])).unwrap_or_else(|e| e).trim().to_string(),
        ),
    ];

    let mut patches = Vec::new();
    let mut controls: Vec<PatchRow> = Vec::new();
    let mut no_control: Vec<String> = Vec::new();
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
                    if !case.patches.is_empty() {
                        match evaluate::control(set, case, renderer, &r, &out, &tmp) {
                            Some(c) => measure_patches(set.name, case, &c, renderer, &mut controls),
                            None => no_control.push(format!("{} ({} at {dpi} dpi)", case.id, renderer.name())),
                        }
                    }
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
    let mut head = String::new();
    let _ = writeln!(head, "# Step 3 compositing proof: results\n");
    let _ = writeln!(head, "Generated by `cargo run -p pdfcraft-overlay --example ov3_proof` in {:.0} s.\n", started.elapsed().as_secs_f64());
    for (k, v) in &versions {
        let _ = writeln!(head, "- {k}: {v}");
    }
    let renderer_names: Vec<&str> = args.renderers.iter().map(|r| r.name()).collect();
    let _ = writeln!(head, "- renderers: {}", renderer_names.join(", "));
    let _ = writeln!(head);
    // One row per criterion: (criterion, verdict, measured).
    let mut summary: Vec<(&str, String, String)> = Vec::new();
    let verdict = |ok: bool| if ok { "pass".to_string() } else { "FAIL".to_string() };
    // A criterion this run measured nothing for (a partial run with --only) is "not run", not a failure.
    let judged = |measured: bool, ok: bool| if measured { verdict(ok) } else { "not run".to_string() };
    let mut md = String::new();

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
    let crit_row = |k: &str| crit.get(k).cloned().unwrap_or((0, 0, 0.0, String::new()));

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
    for (k, label) in [("1", "1 Overlap and exclusive colour"), ("2", "2 Synthetic suite (patches)")] {
        let (n, f, w, where_) = crit_row(k);
        summary.push((label, judged(n > 0, f == 0), format!("{n} checks, {f} failed; worst {w:.2} levels ({where_})")));
    }
    {
        let (n, f, w, where_) = crit_row("6");
        summary.push((
            "6 Light tones",
            judged(n > 0, f == 0 && spread_fail == 0),
            format!(
                "{n} checks against m_t(L), {f} failed, worst {w:.2} levels ({where_}); renderers apart by more than 1 level on {spread_fail} patches"
            ),
        ));
    }

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
            let unit = sets.iter().flat_map(|s| s.cases.iter()).find(|c| c.id == t.case).map_or(1.0, |c| c.placement.unit);
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
    summary.push((
        "8 Geometry in the output",
        judged(!on.is_empty(), missing == 0 && failures == 0 && coincide_fail == 0),
        format!(
            "{} targets: {missing} not found, {failures} past {tol_pt:.3} pt, worst {:.4} pt; {coincide_n} A–B pairs: {coincide_fail} apart by more, worst {:.4} pt",
            on.len(),
            worst.0,
            coincide_worst.0
        ),
    ));
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
            let unit = sets.iter().flat_map(|s| s.cases.iter()).find(|c| c.id == t.case).map_or(1.0, |c| c.placement.unit);
            e.1 = e.1.max(((m.0 - n.0).powi(2) + (m.1 - n.1).powi(2)).sqrt() * unit);
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
    let mut non_monotonic: Vec<String> = Vec::new();
    let mut non_monotonic_old = 0usize;
    let mut worst_d: Vec<(f64, Key)> = Vec::new();
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
        // A displacement is distinguished when every one of the 8 offsets shows both colours
        // (D4: the worst offset counts). The minimum is the smallest displacement from which
        // every larger one in the study is distinguished too; where a smaller one is distinguished
        // but a larger one isn't, the condition is listed as non-monotonic with its raw results.
        let distinguished = |d: f64, old: bool| {
            (0..8).all(|o| {
                rows.iter().any(|l| {
                    l.kind == LineKind::Displaced(d) && l.offset == o && {
                        let c = if old { &l.nearest } else { &l.d1 };
                        c.a > 0 && c.b > 0
                    }
                })
            })
        };
        let steps: Vec<f64> = families::DISPLACEMENTS_MM.iter().skip(1).copied().collect();
        let min_d = |old: bool| -> (String, Option<String>) {
            let pass: Vec<bool> = steps.iter().map(|d| distinguished(*d, old)).collect();
            let from = (0..steps.len()).find(|i| pass[*i..].iter().all(|p| *p));
            let value = from.map_or("not distinguished up to 0.5".to_string(), |i| format!("{}", steps[i]));
            let first = pass.iter().position(|p| *p);
            let odd = (first != from)
                .then(|| steps.iter().zip(&pass).map(|(d, p)| format!("{d}: {}", if *p { "yes" } else { "no" })).collect::<Vec<_>>().join(", "));
            (value, odd)
        };
        let ((a, odd_a), (b, odd_b)) = (min_d(false), min_d(true));
        if let Some(o) = odd_a {
            non_monotonic.push(format!("{} {} dpi, {}, {}°, {:.2} mm (D1): {o}", k.0, k.1, k.2, k.3, k.4 as f64 / 100.0));
        }
        if odd_b.is_some() {
            non_monotonic_old += 1;
        }
        if a == "not distinguished up to 0.5" {
            worst_d.push((f64::INFINITY, *k));
        } else if let Ok(v) = a.parse::<f64>() {
            worst_d.push((v, *k));
        }
        let _ = writeln!(table, "{},{},{},{},{},{},{}", k.0, k.1, k.2, k.3, k.4 as f64 / 100.0, a, b);
        let _ = writeln!(md_table, "| {} | {} | {} | {}° | {:.2} | {a} | {b} |", k.0, k.1, k.2, k.3, k.4 as f64 / 100.0);
    }
    write(&out.join("displacement.csv"), table.as_bytes());
    let expected_conditions = args.renderers.len() * 2 * 2 * families::ORIENTATIONS.len() * families::WIDTHS_MM.len();
    let _ = writeln!(md, "\n## Lines (criteria 3 to 5)\n");
    let _ = writeln!(md, "- Conditions: {} of {expected_conditions} (renderer × dpi × pair × angle × width).", groups.len());
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
    let _ = writeln!(
        md,
        "- Criterion 5: the minimum is the smallest displacement from which every larger one is distinguished at all 8 offsets. Non-monotonic conditions under D1: {}{}. Under the nearest-colour rule: {non_monotonic_old}.",
        non_monotonic.len(),
        if non_monotonic.is_empty() { String::new() } else { format!(" ({})", non_monotonic.join("; ")) }
    );
    summary.push((
        "3 Thin lines visible",
        judged(!groups.is_empty(), groups.len() == expected_conditions && vis_fail.is_empty()),
        format!("{} conditions × A-only and B-only × 8 offsets: {} failures", groups.len(), vis_fail.len()),
    ));
    summary.push((
        "4 Zero displacement",
        judged(zero_hits.2 > 0, zero_hits.0 == 0),
        format!(
            "{} A or B pixels in {} identical-line profiles ({} ambiguous); the old nearest-colour rule finds {}",
            zero_hits.0, zero_hits.2, zero_hits.1, zero_old
        ),
    ));
    summary.push((
        "5 Displacement study",
        judged(!groups.is_empty(), groups.len() == expected_conditions),
        format!("reported for {} of {expected_conditions} conditions; {} non-monotonic under D1 (listed)", groups.len(), non_monotonic.len()),
    ));
    let _ = writeln!(md, "\n### Minimum distinguishable displacement (criterion 5)\n\n{md_table}");

    // Zoomed side by sides of the worst conditions: the largest minimum overall, the largest for
    // each renderer and dpi, and every non-monotonic condition.
    let overall = worst_d.iter().map(|(v, _)| *v).fold(0.0, f64::max);
    let mut chosen: Vec<Key> = worst_d.iter().filter(|(v, _)| *v == overall).map(|(_, k)| *k).collect();
    for r in &args.renderers {
        for dpi in [150u32, 300] {
            let mine: Vec<&(f64, Key)> = worst_d.iter().filter(|(_, k)| k.0 == r.name() && k.1 == dpi).collect();
            let top = mine.iter().map(|(v, _)| *v).fold(0.0, f64::max);
            if let Some((_, k)) = mine.iter().find(|(v, _)| *v == top) {
                chosen.push(*k);
            }
        }
    }
    for n in &non_monotonic {
        if let Some((_, k)) =
            worst_d.iter().find(|(_, k)| n.starts_with(&format!("{} {} dpi, {}, {}°, {:.2} mm", k.0, k.1, k.2, k.3, k.4 as f64 / 100.0)))
        {
            chosen.push(*k);
        }
    }
    // One mosaic per page, dpi and width, showing every renderer.
    let mut mosaics: Vec<(String, u32, i64)> = Vec::new();
    for k in &chosen {
        let id = format!("F01-08-lines-{}-{}deg", k.2, k.3);
        if !mosaics.contains(&(id.clone(), k.1, k.4)) {
            mosaics.push((id, k.1, k.4));
        }
    }
    let mut zoom_list = Vec::new();
    if let Some(set) = sets.iter().find(|s| s.name == "f01-08-lines") {
        let path = out.join(format!("overlays/{}.pdf", set.name));
        for (id, dpi, w) in &mosaics {
            let Some(case) = set.cases.iter().find(|c| &c.id == id) else { continue };
            let renders: Vec<(Renderer, Render)> = args
                .renderers
                .iter()
                .filter_map(|r| render(*r, &set.overlay, &path, case.page, f64::from(*dpi), case, &tmp).ok().map(|x| (*r, x)))
                .collect();
            let width_mm = *w as f64 / 100.0;
            if let Some(img) = zoom_mosaic(case, width_mm, &renders) {
                let name = format!("compare/zoom/{id}-{dpi}dpi-{width_mm}mm.png");
                write_png(&out.join(&name), &img);
                zoom_list.push(name);
            }
        }
    }
    let _ = writeln!(
        md,
        "\nZoomed side by sides of the worst conditions ({}), each renderer a panel in the order {}: columns are the 8 sub-pixel offsets, rows the displacements 0, 0.025, 0.05, 0.1, 0.15, 0.25, 0.5 mm, then A only and B only; each tile is 12 × 12 pt around the cell centre.\n",
        zoom_list.len(),
        renderer_names.join(", ")
    );
    for z in &zoom_list {
        let _ = writeln!(md, "- {z}");
    }

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
    let mut tr_ok = true;
    let mut tr_md = String::new();
    for set in &sets {
        let (ok, text) = transfer_check(set);
        tr_ok &= ok;
        let _ = writeln!(tr_md, "  {text}");
    }
    let _ = writeln!(md, "- Transfer function (criterion 7): {}.\n{tr_md}", if tr_ok { "pass" } else { "FAIL" });
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
    summary.push((
        "7 Same function in preview and export",
        verdict(tr_ok),
        "one Type 2 function per page with C1 = 1 − 1/t exactly, read back from every overlay file; the preview is PdfCraft's render of the same file".into(),
    ));
    {
        let (n, f, w, where_) = crit_row("9b");
        summary.push((
            "9a Expansion and clipping",
            judged(n_checks > 0, checks_fail == 0),
            format!("{n_checks} page box and unit checks, {checks_fail} failed"),
        ));
        summary.push(("9b Coloured content", judged(n > 0, f == 0), format!("{n} checks, {f} failed; worst {w:.2} levels ({where_})")));
    }
    summary.push((
        "10 Vectors kept",
        verdict(vec_ok),
        "source content unchanged inside forms, no image added, page operators q Q gs rg re f only (vector-check.md)".into(),
    ));
    summary.push(("11 Sources unchanged", verdict(src_ok), "SHA-256 in memory and on disk after every render, cross-checked with sha256sum".into()));
    summary.push((
        "12 No panics",
        "see cargo test".into(),
        "not measured by this run: tests/composite.rs hostile_input_never_panics, 10,000 seeded cases".into(),
    ));
    summary.push(("13 Reproducible", verdict(det_ok), "two generations byte-identical; seeds and versions in run.md".into()));
    summary.push(("14 No regressions", "see cargo test".into(), "not measured by this run: the OV2 tests and API".into()));
    summary.push(("15 Gates", "see the gate log".into(), "not measured by this run".into()));

    // The compositing control: the overlay against the model applied to each renderer's own
    // render of the sources.
    let _ = writeln!(md, "\n## Compositing control (diagnostic, not a criterion)\n");
    let _ = writeln!(
        md,
        "Each source page is rendered alone by the same renderer at the same dpi and put through the model pixel by pixel (m_t of PDF luminosity, layer colours multiplied). The overlay should match it whatever the renderer draws, so a difference here is in the compositing; a difference from the expected value that the control shares is in how the renderer draws the source.\n"
    );
    let ctl_key = |p: &PatchRow| (p.case.clone(), p.patch.clone(), p.renderer, p.dpi as u32);
    let ctl: BTreeMap<_, &PatchRow> = controls.iter().map(|c| (ctl_key(c), c)).collect();
    let diff = |p: &PatchRow| ctl.get(&ctl_key(p)).map(|c| (0..3).map(|i| (p.value[i] - c.value[i]).abs()).fold(0.0, f64::max));
    let _ = writeln!(
        md,
        "| Renderer | Patches with a control | Worst overlay − control, levels | Where | Above 2 levels |\n| --- | --- | --- | --- | --- |"
    );
    let (mut ctl_n, mut ctl_above, mut ctl_worst) = (0usize, 0usize, 0.0f64);
    for r in &args.renderers {
        let ds: Vec<(f64, &PatchRow)> = patches.iter().filter(|p| p.renderer == *r).filter_map(|p| diff(p).map(|d| (d, p))).collect();
        let worst = ds.iter().fold((0.0, None), |acc, (d, p)| if *d > acc.0 { (*d, Some(*p)) } else { acc });
        let above = ds.iter().filter(|(d, _)| *d > 2.0).count();
        let where_ = worst.1.map_or(String::new(), |p| format!("{} / {} at {} dpi", p.case, p.patch, p.dpi));
        let _ = writeln!(md, "| {} | {} | {:.2} | {where_} | {above} |", r.name(), ds.len(), worst.0);
        (ctl_n, ctl_above, ctl_worst) = (ctl_n + ds.len(), ctl_above + above, ctl_worst.max(worst.0));
    }
    // The assessment R-a and R-b were accepted on (12 Oct). It doesn't replace criteria 2 and 6,
    // which keep their own rows and measurements.
    summary.push((
        "Source-controlled compositing (basis of the accepted exceptions R-a and R-b)",
        judged(ctl_n > 0, ctl_above == 0),
        format!("{ctl_n} patches across the renderers; worst overlay − control {ctl_worst:.2} levels; {ctl_above} above 2 levels"),
    ));
    if !no_control.is_empty() {
        let _ = writeln!(md, "\nNo control (placement not the identity, unit not 1, or renders not aligned): {}.", no_control.join(", "));
    }

    // Renders that hit a limit or a /UserUnit question.
    let _ = writeln!(md, "\n## Render notes\n");
    for n in &render_notes {
        let _ = writeln!(md, "- {n}");
    }

    // Incompatibilities: patch cases where renderers disagree beyond the tolerance.
    let mut inc = String::from(
        "# Renderer incompatibilities\n\nPatches where at least one renderer misses the expected value. \"All fail\" means no renderer meets it. The control column is each renderer's own render of the sources put through the model (see results.md, Compositing control): when the overlay matches its control but not the expected value, the renderer draws the source differently from the expected value, before any compositing.\n\n| Case | Patch | dpi | Expected | Values | Errors, levels | Control | Overlay − control, levels |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n",
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
            let ctl_vals: Vec<String> = rows
                .iter()
                .map(|r| ctl.get(&ctl_key(r)).map_or(format!("{} -", r.renderer.name()), |c| format!("{} ({})", r.renderer.name(), fmt3(c.value))))
                .collect();
            let ctl_diff: Vec<String> =
                rows.iter().map(|r| diff(r).map_or(format!("{} -", r.renderer.name()), |d| format!("{} {d:.1}", r.renderer.name()))).collect();
            let _ = writeln!(
                inc,
                "| {case}{} | {patch} | {dpi} | {exp} | {} | {} | {} | {} |",
                if any_pass { "" } else { " (all fail)" },
                vals.join("; "),
                errs.join("; "),
                ctl_vals.join("; "),
                ctl_diff.join("; ")
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
    let (map_csv, map_md) = case_map(&sets);
    write(&out.join("cases.csv"), map_csv.as_bytes());
    write(&out.join("cases.md"), map_md.as_bytes());

    // Run record.
    let mut run_md = String::from("# Run record\n\n");
    for (k, v) in &versions {
        let _ = writeln!(run_md, "- {k}: {v}");
    }
    let _ = writeln!(run_md, "- seeds: SplitMix64 0x5CA0_0003 (scan image); the rest of the suite is fixed geometry");
    let _ = writeln!(
        run_md,
        "- commands: pdftoppm -r DPI -f N -l N -singlefile; python3 -m pypdfium2_cli render --pages N --scale DPI/72 --format ppm --no-x-channel --linear; PdfCraft in process (PageRenderer, scale DPI/72), each renderer's default antialiasing"
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
    // Criteria 2 and 6 keep their original verdicts and measurements. Where they fail, the row says
    // that the failure is an accepted, documented renderer-dependent exception (12 Oct), not a pass.
    for (c, v, _) in &mut summary {
        let exception = match c.split_whitespace().next() {
            Some("2") => Some("R-a"),
            Some("6") => Some("R-b"),
            _ => None,
        };
        if let Some(x) = exception
            && v == "FAIL"
        {
            *v = format!("FAIL (accepted documented exception {x}, 12 Oct)");
        }
    }
    // In the PRD's order: by number, then letter.
    summary.sort_by_key(|(c, _, _)| {
        let digits: String = c.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        (digits.parse::<u32>().unwrap_or(u32::MAX), c.chars().nth(digits.len()).filter(|ch| ch.is_ascii_lowercase()))
    });
    let mut table = String::from("## Criteria\n\n| Criterion | Result | Measured |\n| --- | --- | --- |\n");
    for (c, v, m) in &summary {
        let _ = writeln!(table, "| {c} | {v} | {} |", m.replace('|', "/"));
    }
    let _ = writeln!(table, "\nThe case map is in cases.md and cases.csv; per-probe values in patches.csv, targets.csv and lines.csv.\n");
    let results = format!("{head}{table}{md}");
    write(&out.join("results.md"), results.as_bytes());
    let _ = std::fs::remove_dir_all(&tmp);
    println!("{results}");
    ExitCode::SUCCESS
}

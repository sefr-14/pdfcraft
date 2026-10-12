//! Reading rendered pixels (OV3.3, OV3.4): images, recovered mask coverage, pixel classes,
//! profiles and centroids. Self-contained, so the proof example can include it by path.
//!
//! A pixel of the overlay prints, per channel c, as P_c = 255 (1 − αA a_c)(1 − αB b_c), where
//! αA and αB are the two documents' mask values (coverage) and a_c = (255 − A_c) / 255 for layer
//! colour A (b_c likewise for B). Classification works on (αA, αB) recovered from the three
//! channels (step 3 PRD, decision D1), not on the nearest colour.

#![allow(dead_code)]

/// An RGB image, 8 bits per channel, row-major, top row first.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn get(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y * self.width + x) * 3;
        Some([self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]])
    }

    /// Premultiplied RGBA (as PdfCraft's renderer returns it) composited onto white.
    pub fn from_premultiplied_rgba(width: usize, height: usize, rgba: &[u8]) -> Image {
        let mut rgb = Vec::with_capacity(width * height * 3);
        for px in rgba.as_chunks::<4>().0 {
            let back = 255 - px[3];
            rgb.extend([px[0].saturating_add(back), px[1].saturating_add(back), px[2].saturating_add(back)]);
        }
        Image { width, height, rgb }
    }

    /// A binary PPM (P6, maxval 255) or PAM (P7, RGB or RGB_ALPHA, maxval 255; alpha is taken
    /// as straight and composited onto white).
    pub fn from_pnm(bytes: &[u8]) -> Result<Image, String> {
        let magic = bytes.get(..2).ok_or("empty file")?;
        match magic {
            b"P6" => {
                let (fields, at) = header_fields(bytes, 2, 3)?;
                let (w, h, max) = (fields[0], fields[1], fields[2]);
                if max != 255 {
                    return Err(format!("maxval {max}"));
                }
                let data = bytes.get(at..at + w * h * 3).ok_or("short PPM")?;
                Ok(Image { width: w, height: h, rgb: data.to_vec() })
            }
            b"P7" => {
                let text_end = find(bytes, b"ENDHDR\n").ok_or("PAM without ENDHDR")?;
                let header = std::str::from_utf8(&bytes[..text_end]).map_err(|e| e.to_string())?;
                let mut w = 0;
                let mut h = 0;
                let mut depth = 0;
                for line in header.lines() {
                    let mut it = line.split_whitespace();
                    match (it.next(), it.next()) {
                        (Some("WIDTH"), Some(v)) => w = v.parse().map_err(|_| "width")?,
                        (Some("HEIGHT"), Some(v)) => h = v.parse().map_err(|_| "height")?,
                        (Some("DEPTH"), Some(v)) => depth = v.parse().map_err(|_| "depth")?,
                        (Some("MAXVAL"), Some(v)) if v != "255" => return Err(format!("maxval {v}")),
                        _ => {}
                    }
                }
                let at = text_end + b"ENDHDR\n".len();
                let data = bytes.get(at..at + w * h * depth).ok_or("short PAM")?;
                let mut rgb = Vec::with_capacity(w * h * 3);
                for px in data.chunks_exact(depth) {
                    match depth {
                        3 => rgb.extend_from_slice(px),
                        4 => {
                            let a = u32::from(px[3]);
                            for c in &px[..3] {
                                rgb.push(((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8);
                            }
                        }
                        _ => return Err(format!("depth {depth}")),
                    }
                }
                Ok(Image { width: w, height: h, rgb })
            }
            other => Err(format!("not a PPM or PAM: {:?}", String::from_utf8_lossy(other))),
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `n` whitespace-separated decimal fields after `at`, skipping `#` comments; returns them and
/// the offset just past the single whitespace byte that ends the last one.
fn header_fields(b: &[u8], mut at: usize, n: usize) -> Result<(Vec<usize>, usize), String> {
    let mut out = Vec::new();
    while out.len() < n {
        while at < b.len() && (b[at].is_ascii_whitespace() || b[at] == b'#') {
            if b[at] == b'#' {
                while at < b.len() && b[at] != b'\n' {
                    at += 1;
                }
            }
            at += 1;
        }
        let start = at;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
        let s = std::str::from_utf8(&b[start..at]).map_err(|e| e.to_string())?;
        out.push(s.parse().map_err(|_| format!("bad header field {s:?}"))?);
    }
    Ok((out, at + 1))
}

/// The overlay's two layer colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub a: [u8; 3],
    pub b: [u8; 3],
}

impl Pair {
    pub const RED_CYAN: Pair = Pair { a: [204, 51, 51], b: [51, 204, 204] };
    pub const GREEN_MAGENTA: Pair = Pair { a: [51, 204, 51], b: [204, 51, 204] };
    pub const AZURE_ORANGE: Pair = Pair { a: [0, 128, 255], b: [255, 127, 0] };
    pub const SPRING_ROSE: Pair = Pair { a: [0, 191, 96], b: [255, 64, 159] };

    /// The colour the model predicts for coverages (αA, αB), unrounded, per channel 0 to 255.
    pub fn predict(&self, alpha_a: f64, alpha_b: f64) -> [f64; 3] {
        let mut p = [0.0; 3];
        for (c, out) in p.iter_mut().enumerate() {
            let a = (255.0 - f64::from(self.a[c])) / 255.0;
            let b = (255.0 - f64::from(self.b[c])) / 255.0;
            *out = 255.0 * (1.0 - alpha_a * a) * (1.0 - alpha_b * b);
        }
        p
    }

    /// The overlap colour A × B / 255 per channel.
    pub fn overlap(&self) -> [f64; 3] {
        self.predict(1.0, 1.0)
    }

    /// The smallest coverage of `colour` that differs from paper by 64 levels in some channel:
    /// 64 / max(255 − colour). The main PRD's visibility starting value, as a coverage.
    pub fn visible_coverage(colour: [u8; 3]) -> f64 {
        let reach = colour.iter().map(|c| 255.0 - f64::from(*c)).fold(0.0, f64::max);
        if reach > 0.0 { 64.0 / reach } else { f64::INFINITY }
    }
}

/// (αA, αB) recovered from one pixel, with how well they explain it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recovered {
    pub alpha_a: f64,
    pub alpha_b: f64,
    /// RMS difference between the pixel and the model's colour for these coverages, in levels.
    pub residual: f64,
    /// Standard deviation of αA − αB from one level of noise per channel; infinite when the two
    /// coverages can't be told apart from this pixel (underdetermined).
    pub sigma_diff: f64,
}

/// One level, in the model's 0-to-1 units: the noise allowance per channel.
pub const LEVEL: f64 = 1.0 / 255.0;

/// Recover (αA, αB) in [0, 1]² by least squares over the three channels: for each αA the best αB
/// is linear and closed-form, and αA is found by a dense grid then a golden-section refinement.
pub fn recover(pixel: [u8; 3], pair: Pair) -> Recovered {
    let q: [f64; 3] = pixel.map(|v| f64::from(v) / 255.0);
    let a: [f64; 3] = pair.a.map(|v| (255.0 - f64::from(v)) / 255.0);
    let b: [f64; 3] = pair.b.map(|v| (255.0 - f64::from(v)) / 255.0);
    let best_b = |aa: f64| -> (f64, f64) {
        let u: [f64; 3] = [1.0 - aa * a[0], 1.0 - aa * a[1], 1.0 - aa * a[2]];
        let (mut num, mut den) = (0.0, 0.0);
        for c in 0..3 {
            num += (u[c] - q[c]) * u[c] * b[c];
            den += (u[c] * b[c]).powi(2);
        }
        let ab = if den > 0.0 { (num / den).clamp(0.0, 1.0) } else { 0.0 };
        let err: f64 = (0..3).map(|c| (u[c] * (1.0 - ab * b[c]) - q[c]).powi(2)).sum();
        (ab, err)
    };
    let n = 400;
    let (mut best, mut best_err) = (0.0, f64::INFINITY);
    for i in 0..=n {
        let aa = i as f64 / n as f64;
        let (_, e) = best_b(aa);
        if e < best_err {
            (best, best_err) = (aa, e);
        }
    }
    let (mut lo, mut hi) = ((best - 1.0 / n as f64).max(0.0), (best + 1.0 / n as f64).min(1.0));
    let g = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..60 {
        let (x1, x2) = (hi - g * (hi - lo), lo + g * (hi - lo));
        if best_b(x1).1 <= best_b(x2).1 {
            hi = x2;
        } else {
            lo = x1;
        }
    }
    let mid = (lo + hi) / 2.0;
    let alpha_a = if best_b(mid).1 <= best_err { mid } else { best };
    let (alpha_b, err) = best_b(alpha_a);
    // Uncertainty of αA − αB from one level of noise per channel, through the Jacobian.
    let (mut saa, mut sab, mut sbb) = (0.0, 0.0, 0.0);
    for c in 0..3 {
        let ja = -a[c] * (1.0 - alpha_b * b[c]);
        let jb = -b[c] * (1.0 - alpha_a * a[c]);
        saa += ja * ja;
        sab += ja * jb;
        sbb += jb * jb;
    }
    let det = saa * sbb - sab * sab;
    let scale = saa.max(sbb);
    let sigma_diff = if scale > 0.0 && det > 1e-9 * scale * scale {
        let (caa, cab, cbb) = (sbb / det, -sab / det, saa / det);
        (LEVEL * LEVEL * (caa + cbb - 2.0 * cab)).max(0.0).sqrt()
    } else {
        f64::INFINITY
    };
    Recovered { alpha_a, alpha_b, residual: (err / 3.0).sqrt() * 255.0, sigma_diff }
}

/// What a pixel shows (decision D1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Neither document reaches the visibility coverage.
    Paper,
    /// A's coverage exceeds B's by at least A's visibility coverage: a difference showing A.
    A,
    /// The same for B.
    B,
    /// Ink from both, or from one below the difference threshold: not a difference.
    Shared,
    /// The decision between A or B and the rest is within noise of the threshold.
    NearThreshold,
    /// The two coverages can't be separated from this pixel's colour.
    Underdetermined,
    /// The colour isn't one the two layer colours can produce (residual above the limit).
    Unexplained,
}

impl Class {
    pub fn is_ambiguous(self) -> bool {
        matches!(self, Class::NearThreshold | Class::Underdetermined | Class::Unexplained)
    }
}

/// The largest residual, in levels, a pixel may have and still be explained by the model.
pub const MAX_RESIDUAL: f64 = 4.0;
/// How many standard deviations of noise a difference must clear its threshold by.
pub const K_SIGMA: f64 = 2.0;

/// Classify by recovered coverage: A when αA − αB ≥ δA (A's visibility coverage), B when
/// αB − αA ≥ δB. A decision within K_SIGMA·σ of its threshold is NearThreshold, never A or B.
pub fn classify(pixel: [u8; 3], pair: Pair) -> (Class, Recovered) {
    let r = recover(pixel, pair);
    if r.residual > MAX_RESIDUAL {
        return (Class::Unexplained, r);
    }
    if !r.sigma_diff.is_finite() {
        return (Class::Underdetermined, r);
    }
    let (da, db) = (Pair::visible_coverage(pair.a), Pair::visible_coverage(pair.b));
    let diff = r.alpha_a - r.alpha_b;
    let margin = K_SIGMA * r.sigma_diff;
    if (diff - da).abs() < margin || (-diff - db).abs() < margin {
        return (Class::NearThreshold, r);
    }
    let class = if diff >= da {
        Class::A
    } else if -diff >= db {
        Class::B
    } else if r.alpha_a >= da || r.alpha_b >= db {
        Class::Shared
    } else {
        Class::Paper
    };
    (class, r)
}

/// The main PRD's original rule: the nearest of A, B, the overlap colour and paper in RGB; A or B
/// only when the pixel also differs from paper by at least 64 levels in some channel.
pub fn nearest_colour(pixel: [u8; 3], pair: Pair) -> Class {
    let p = pixel.map(f64::from);
    let dist = |c: [f64; 3]| (0..3).map(|i| (p[i] - c[i]).powi(2)).sum::<f64>();
    let a = pair.a.map(f64::from);
    let b = pair.b.map(f64::from);
    let candidates = [(Class::A, dist(a)), (Class::B, dist(b)), (Class::Shared, dist(pair.overlap())), (Class::Paper, dist([255.0; 3]))];
    let (class, _) = candidates.iter().fold((Class::Paper, f64::INFINITY), |acc, (c, d)| if *d < acc.1 { (*c, *d) } else { acc });
    let far = pixel.iter().any(|v| 255 - v >= 64);
    match class {
        Class::A | Class::B if !far => Class::Paper,
        c => c,
    }
}

/// Every pixel the segment from `from` to `to` passes through or touches (its supercover), in
/// pixel units with pixel (x, y) spanning [x, x + 1] × [y, y + 1], ordered along the segment.
///
/// A walk that samples points along the segment and takes the pixel under each one steps
/// diagonally from one pixel to the next where the segment crosses a pixel corner, and so skips
/// the two pixels beside that corner. At 45° that is every other pixel, including the one a line
/// lies on (found in the first full run of the line study). The supercover can't skip one.
pub fn profile(img: &Image, from: (f64, f64), to: (f64, f64)) -> Vec<(usize, usize)> {
    // Squares are grown by this much so a segment through a corner meets all four pixels there,
    // however the corner's coordinates round.
    const EPS: f64 = 1e-6;
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let lo = |a: f64, b: f64| (a.min(b) - 1.0).floor().max(0.0) as usize;
    let hi = |a: f64, b: f64| (a.max(b) + 1.0).floor().max(0.0) as usize;
    let mut hits: Vec<(f64, usize, usize)> = Vec::new();
    for y in lo(from.1, to.1)..=hi(from.1, to.1).min(img.height.saturating_sub(1)) {
        for x in lo(from.0, to.0)..=hi(from.0, to.0).min(img.width.saturating_sub(1)) {
            // Liang–Barsky: the part of t ∈ [0, 1] inside the grown square, if any.
            let (xf, yf) = (x as f64, y as f64);
            let mut span = (0.0_f64, 1.0_f64);
            let inside = [(-dx, from.0 - xf + EPS), (dx, xf + 1.0 + EPS - from.0), (-dy, from.1 - yf + EPS), (dy, yf + 1.0 + EPS - from.1)]
                .iter()
                .all(|&(p, q)| clip(p, q, &mut span));
            if inside {
                hits.push((span.0, x, y));
            }
        }
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.2, a.1).cmp(&(b.2, b.1))));
    hits.into_iter().map(|(_, x, y)| (x, y)).collect()
}

/// One Liang–Barsky step: keep the part of `span` where p·t ≤ q. False when nothing is left.
fn clip(p: f64, q: f64, span: &mut (f64, f64)) -> bool {
    if p == 0.0 {
        return q >= 0.0;
    }
    let r = q / p;
    if p < 0.0 {
        span.0 = span.0.max(r);
    } else {
        span.1 = span.1.min(r);
    }
    span.0 <= span.1
}

/// Per-channel median of the pixels in [x0, x1) × [y0, y1) (pixel indices, clamped).
pub fn median(img: &Image, x0: usize, y0: usize, x1: usize, y1: usize) -> Option<[u8; 3]> {
    let mut ch: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            let p = img.get(x, y)?;
            for c in 0..3 {
                ch[c].push(p[c]);
            }
        }
    }
    if ch[0].is_empty() {
        return None;
    }
    let mut out = [0u8; 3];
    for c in 0..3 {
        ch[c].sort_unstable();
        out[c] = ch[c][ch[c].len() / 2];
    }
    Some(out)
}

/// The weighted centroid, in pixel units, of `weight(pixel)` over [x0, x1) × [y0, y1); pixel
/// centres at +0.5. `None` when the weights sum to zero.
pub fn centroid(img: &Image, x0: usize, y0: usize, x1: usize, y1: usize, weight: impl Fn([u8; 3]) -> f64) -> Option<(f64, f64, f64)> {
    let (mut sw, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            let w = weight(img.get(x, y)?);
            sw += w;
            sx += w * (x as f64 + 0.5);
            sy += w * (y as f64 + 0.5);
        }
    }
    (sw > 0.0).then(|| (sx / sw, sy / sw, sw))
}

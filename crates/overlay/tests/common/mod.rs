//! Shared test helpers: synthetic PDFs, a seeded generator, tolerances.

#![allow(dead_code)]

use std::sync::Arc;

use pdfcraft_cos::Document;

pub mod families;
pub mod measure;
pub mod synth;

/// A PDF whose page tree node carries `tree` and whose pages carry one entry of `pages` each.
pub fn doc(tree: &str, pages: &[&str]) -> Document {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {n} {tree} >>", kids.join(" "))];
    for p in pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R {p} >>"));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("synthetic PDF opens")
}

/// SplitMix64: a small seeded generator, so every run tests the same cases.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// One of `values`, chosen uniformly.
    pub fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[(self.next_u64() % values.len() as u64) as usize]
    }

    /// A value that is sometimes hostile: NaN, infinities, huge, tiny, zero or ordinary.
    pub fn wild(&mut self) -> f64 {
        match self.next_u64() % 10 {
            0 => f64::NAN,
            1 => f64::INFINITY,
            2 => f64::NEG_INFINITY,
            3 => self.pick(&[1e300, -1e300, f64::MAX, f64::MIN, f64::MIN_POSITIVE, -0.0, 0.0, 5e-324]),
            4 => self.range(-1e12, 1e12),
            _ => self.range(-5000.0, 5000.0),
        }
    }
}

pub fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[track_caller]
pub fn assert_close(a: f64, b: f64, tol: f64, what: &str) {
    assert!(close(a, b, tol), "{what}: {a} vs {b} (tolerance {tol}, off by {})", (a - b).abs());
}

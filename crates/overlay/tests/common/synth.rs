//! Synthetic source PDFs for the step 3 tests and proof (OV3.2). Self-contained, so the proof
//! example can include it by path. Everything is deterministic: the same inputs always give the
//! same bytes.

#![allow(dead_code)]

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};

/// Points per millimetre.
pub const MM: f64 = 72.0 / 25.4;

/// A resource a page uses, under `/Resources /<category> /<name>`.
#[derive(Clone, Debug)]
pub enum Res {
    /// A direct dictionary (an ExtGState, a colour space array is given as `Obj`).
    Dict(Dict),
    /// Any direct object.
    Obj(Object),
    /// A stream with its dictionary and its (unencoded) data, written with Flate.
    Stream(Dict, Vec<u8>),
}

/// One source page.
#[derive(Clone, Debug)]
pub struct Page {
    pub media: [f64; 4],
    pub crop: Option<[f64; 4]>,
    pub rotate: i64,
    pub user_unit: Option<f64>,
    /// The page's own transparency group, if any (`/Group`).
    pub group: Option<Dict>,
    /// The content streams; more than one becomes a `/Contents` array.
    pub contents: Vec<String>,
    /// (category, name, resource).
    pub resources: Vec<(&'static str, String, Res)>,
}

impl Page {
    /// A page `w × h` points with its origin at (0, 0) and one content stream.
    pub fn new(w: f64, h: f64, content: impl Into<String>) -> Self {
        Page { media: [0.0, 0.0, w, h], crop: None, rotate: 0, user_unit: None, group: None, contents: vec![content.into()], resources: Vec::new() }
    }

    pub fn with_resource(mut self, category: &'static str, name: &str, res: Res) -> Self {
        self.resources.push((category, name.to_string(), res));
        self
    }
}

fn reals(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

/// Write `pages` as a PDF (classic cross-reference table, no object streams).
pub fn pdf(pages: &[Page]) -> Arc<Vec<u8>> {
    let mut doc = Document::new_empty();
    let tree = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.reference(b"Pages"))).expect("new documents have a page tree");
    let mut kids = Vec::new();
    for p in pages {
        let mut resources = Dict::new();
        for (cat, name, res) in &p.resources {
            let obj = match res {
                Res::Dict(d) => Object::Dict(d.clone()),
                Res::Obj(o) => o.clone(),
                Res::Stream(d, data) => Object::Ref(doc.add(Object::Stream(Stream::flate(d.clone(), data)))),
            };
            let mut sub = resources.get(cat.as_bytes()).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
            sub.set(name.as_bytes().to_vec(), obj);
            resources.set(cat.as_bytes().to_vec(), Object::Dict(sub));
        }
        let streams: Vec<Object> =
            p.contents.iter().map(|c| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes()))))).collect();
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Page"));
        d.set(b"Parent".to_vec(), Object::Ref(tree));
        d.set(b"MediaBox".to_vec(), reals(&p.media));
        if let Some(c) = p.crop {
            d.set(b"CropBox".to_vec(), reals(&c));
        }
        if p.rotate != 0 {
            d.set(b"Rotate".to_vec(), Object::Int(p.rotate));
        }
        if let Some(u) = p.user_unit {
            d.set(b"UserUnit".to_vec(), Object::Real(u));
        }
        if let Some(g) = &p.group {
            d.set(b"Group".to_vec(), Object::Dict(g.clone()));
        }
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        let contents = match streams.as_slice() {
            [one] => one.clone(),
            _ => Object::Array(streams),
        };
        d.set(b"Contents".to_vec(), contents);
        kids.push(Object::Ref(doc.add(Object::Dict(d))));
    }
    let count = kids.len() as i64;
    doc.update_dict(tree, |t| {
        t.set(b"Kids".to_vec(), Object::Array(kids));
        t.set(b"Count".to_vec(), Object::Int(count));
    })
    .expect("the page tree is a dictionary");
    save(&doc, false)
}

/// Save a document deterministically. `object_streams` packs objects as PdfCraft's own saves do.
pub fn save(doc: &Document, object_streams: bool) -> Arc<Vec<u8>> {
    let opts = SaveOptions { object_streams, ..SaveOptions::default() };
    Arc::new(write_full(doc, &opts).expect("a document built here saves"))
}

pub fn open(bytes: &Arc<Vec<u8>>) -> Document {
    Document::open(bytes.clone()).expect("a document built here opens")
}

/// A PDF from raw object bodies, numbered from 1, with object 1 the catalog. For hostile or
/// unusual structures the builder above won't write.
pub fn raw_pdf(objects: &[String]) -> Arc<Vec<u8>> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objects.len() + 1).as_bytes());
    Arc::new(out)
}

/// A raw stream object body with uncompressed `data`.
pub fn raw_stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// The page's decoded content streams joined as `page_as_form` joins them (each followed by a
/// newline), read through pdfcraft-model's page list.
pub fn page_content(doc: &Document, page: usize) -> Vec<u8> {
    let pages = pdfcraft_model::pages(doc);
    let p = &pages[page];
    let mut out = Vec::new();
    let Some(c) = p.dict.get(b"Contents") else { return out };
    let list = match &*doc.resolve(c) {
        Object::Array(a) => a.clone(),
        _ => vec![c.clone()],
    };
    for o in list {
        if let Object::Stream(s) = &*doc.resolve(&o) {
            out.extend(s.decoded().expect("synthetic streams decode"));
            out.push(b'\n');
        }
    }
    out
}

/// Object references held in a dictionary entry that is a reference.
pub fn reference(d: &Dict, key: &[u8]) -> ObjRef {
    d.reference(key).unwrap_or_else(|| panic!("{} is not a reference", String::from_utf8_lossy(key)))
}

/// A 8-bit image XObject dictionary for `w × h` pixels in `colour_space` (`DeviceGray` or
/// `DeviceRGB`), not interpolated.
pub fn image_dict(w: usize, h: usize, colour_space: &str) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"ColorSpace".to_vec(), Object::name(colour_space));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"Interpolate".to_vec(), Object::Bool(false));
    d
}

/// SplitMix64: a small seeded generator, so every run makes the same files.
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
}

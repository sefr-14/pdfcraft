//! Validate the downloaded zip against the signed manifest and extract it into the staging folder
//! only (PRD §8). The zip must hold exactly the signed files under its single root folder; every
//! entry name is checked before anything is written, sizes are counted while writing (headers
//! aren't trusted), and every extracted file is hashed from what was written.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use zip::CompressionMethod;

use crate::manifest::{FileEntry, Manifest};
use crate::{Error, hash, paths};

/// Most entries read (files plus folder entries).
const MAX_ENTRIES: usize = crate::manifest::MAX_FILES * 2 + 16;
/// Above this size a file may not compress better than [`MAX_RATIO`] (zip bombs).
const RATIO_FLOOR: u64 = 1024 * 1024;
const MAX_RATIO: u64 = 200;

/// Check every entry of `zip_path` against `manifest` without writing anything; the entries in
/// archive order (index, relative path).
pub fn inspect(zip_path: &Path, manifest: &Manifest) -> Result<Vec<(usize, String)>, Error> {
    let file = std::fs::File::open(zip_path).map_err(|e| Error::io("couldn't open", zip_path, &e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Archive(format!("not a readable zip: {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(Error::Archive(format!("{} entries", zip.len())));
    }
    let signed: HashMap<&str, &FileEntry> = manifest.package.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let root = format!("{}/", manifest.package.root);
    let mut folded = std::collections::HashSet::new();
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i).map_err(|e| Error::Archive(format!("entry {i}: {e}")))?;
        let name = std::str::from_utf8(entry.name_raw()).map_err(|_| Error::Archive(format!("entry {i}: name isn't UTF-8")))?.to_string();
        let Some(rel) = name.strip_prefix(&root) else {
            return Err(Error::Archive(format!("{name:?} is outside the package folder {:?}", manifest.package.root)));
        };
        if rel.is_empty() {
            continue; // the root folder itself
        }
        if entry.encrypted() {
            return Err(Error::Archive(format!("{name:?} is encrypted")));
        }
        if entry.is_symlink() {
            return Err(Error::Archive(format!("{name:?} is a link")));
        }
        if let Some(dir) = rel.strip_suffix('/') {
            // A folder entry: only the folders the signed files live in.
            paths::check_relative(dir)?;
            let prefix = format!("{dir}/");
            if !manifest.package.files.iter().any(|f| f.path.starts_with(&prefix)) {
                return Err(Error::Archive(format!("unexpected folder {dir:?}")));
            }
            if entry.size() != 0 {
                return Err(Error::Archive(format!("folder {dir:?} has contents")));
            }
            continue;
        }
        paths::check_relative(rel)?;
        if !paths::is_allowed(rel) {
            return Err(Error::Archive(format!("{rel:?} isn't part of PDF Overlay")));
        }
        if !folded.insert(paths::fold(rel)) {
            return Err(Error::Archive(format!("{rel:?} appears twice")));
        }
        let Some(signed) = signed.get(rel) else {
            return Err(Error::Archive(format!("{rel:?} isn't in the signed file list")));
        };
        if !matches!(entry.compression(), CompressionMethod::Stored | CompressionMethod::Deflated) {
            return Err(Error::Archive(format!("{rel:?} uses an unsupported compression")));
        }
        if entry.size() != signed.size {
            return Err(Error::Archive(format!("{rel:?} is {} bytes, signed as {}", entry.size(), signed.size)));
        }
        if entry.size() > RATIO_FLOOR && entry.size() / entry.compressed_size().max(1) > MAX_RATIO {
            return Err(Error::Archive(format!("{rel:?} compresses implausibly well")));
        }
        files.push((i, rel.to_string()));
    }
    if files.len() != manifest.package.files.len() {
        let present: std::collections::HashSet<&str> = files.iter().map(|(_, p)| p.as_str()).collect();
        let missing: Vec<&str> = manifest.package.files.iter().map(|f| f.path.as_str()).filter(|p| !present.contains(p)).collect();
        return Err(Error::Archive(format!("signed files missing from the package: {}", missing.join(", "))));
    }
    Ok(files)
}

/// Check `zip_path` and extract it into `dest`, which must not exist yet. On any failure `dest`
/// may hold some files; the caller removes the staging folder.
pub fn extract(zip_path: &Path, manifest: &Manifest, dest: &Path) -> Result<(), Error> {
    let entries = inspect(zip_path, manifest)?;
    std::fs::create_dir(dest).map_err(|e| Error::io("couldn't create", dest, &e))?;
    let signed: HashMap<&str, &FileEntry> = manifest.package.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let file = std::fs::File::open(zip_path).map_err(|e| Error::io("couldn't open", zip_path, &e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Archive(format!("not a readable zip: {e}")))?;
    for (i, rel) in entries {
        let Some(want) = signed.get(rel.as_str()) else {
            return Err(Error::Archive(format!("{rel:?} isn't in the signed file list")));
        };
        let target = rel.split('/').fold(dest.to_path_buf(), |p, c| p.join(c));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io("couldn't create", parent, &e))?;
        }
        let mut out =
            std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| Error::io("couldn't create", &target, &e))?;
        let entry = zip.by_index(i).map_err(|e| Error::Archive(format!("{rel:?}: {e}")))?;
        let mut limited = entry.take(want.size.saturating_add(1));
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = limited.read(&mut buf).map_err(|e| Error::Archive(format!("{rel:?}: {e}")))?;
            if n == 0 {
                break;
            }
            let chunk = buf.get(..n).unwrap_or_default();
            total = total.saturating_add(n as u64);
            if total > want.size {
                return Err(Error::Archive(format!("{rel:?} is larger than signed")));
            }
            h.update(chunk);
            out.write_all(chunk).map_err(|e| Error::io("couldn't write", &target, &e))?;
        }
        out.sync_all().map_err(|e| Error::io("couldn't write", &target, &e))?;
        drop(out);
        if total != want.size || hash::hex(&h.finalize()) != want.sha256 {
            return Err(Error::Archive(format!("{rel:?} doesn't match its signed hash")));
        }
        // Read back what landed on disk: that is what gets installed.
        if hash::file(&target).map_err(|e| Error::io("couldn't read", &target, &e))? != want.sha256 {
            return Err(Error::Archive(format!("{rel:?} changed on disk after extraction")));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manifest::tests::sample;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    pub(crate) fn temp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("overlay-update-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A zip of `entries` (name, bytes) and the manifest that signs `files` (path, bytes).
    pub(crate) fn package(root: &str, entries: &[(&str, &[u8])], files: &[(&str, &[u8])]) -> (Vec<u8>, Manifest) {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*name, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
                w.write_all(bytes).unwrap();
            }
        }
        let zip = w.finish().unwrap().into_inner();
        let mut m = sample();
        m.package.root = root.into();
        m.package.files = files.iter().map(|(p, b)| FileEntry { path: (*p).into(), size: b.len() as u64, sha256: hash::sha256(b) }).collect();
        m.package.unpacked_size = files.iter().map(|(_, b)| b.len() as u64).sum();
        m.package.size = zip.len() as u64;
        m.package.sha256 = hash::sha256(&zip);
        (zip, m)
    }

    const ROOT: &str = "pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable";

    fn write(dir: &Path, zip: &[u8]) -> std::path::PathBuf {
        let p = dir.join("package.zip");
        std::fs::write(&p, zip).unwrap();
        p
    }

    #[test]
    fn a_package_matching_its_manifest_is_extracted() {
        let dir = temp("ok");
        let files: &[(&str, &[u8])] = &[("pdfcraft.exe", b"new app"), ("licenses/README.txt", b"licences"), ("portable.txt", b"marker")];
        let entries: Vec<(&str, &[u8])> = vec![
            ("pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable/", b""),
            ("pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable/licenses/", b""),
            ("pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable/pdfcraft.exe", b"new app"),
            ("pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable/licenses/README.txt", b"licences"),
            ("pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable/portable.txt", b"marker"),
        ];
        let (zip, m) = package(ROOT, &entries, files);
        let dest = dir.join("new");
        extract(&write(&dir, &zip), &m, &dest).unwrap();
        assert_eq!(std::fs::read(dest.join("pdfcraft.exe")).unwrap(), b"new app");
        assert_eq!(std::fs::read(dest.join("licenses").join("README.txt")).unwrap(), b"licences");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn refused(entries: &[(&str, &[u8])], files: &[(&str, &[u8])]) -> String {
        let dir = temp("bad");
        let (zip, m) = package(ROOT, entries, files);
        let e = extract(&write(&dir, &zip), &m, &dir.join("new")).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        e.to_string()
    }

    fn r(name: &str) -> String {
        format!("{ROOT}/{name}")
    }

    #[test]
    fn entries_outside_the_signed_list_refuse_the_package() {
        let app: (&str, &[u8]) = ("pdfcraft.exe", b"app");
        type Case<'a> = (Vec<(String, &'a [u8])>, &'static str);
        let cases: Vec<Case> = vec![
            (vec![(r("pdfcraft.exe"), b"app"), (r("../evil.exe"), b"x")], "component"),
            (vec![(r("pdfcraft.exe"), b"app"), (r("PdfCraftData/app.ron"), b"x")], "isn't part"),
            (vec![(r("pdfcraft.exe"), b"app"), ("other-root/pdfcraft.exe".into(), b"x")], "outside the package folder"),
            (vec![(r("pdfcraft.exe"), b"app"), (r("NOTICE"), b"x")], "isn't in the signed"),
            (vec![(r("pdfcraft.exe"), b"app"), (r("NOTICE:ads"), b"x")], "reserves"),
            (vec![(r("pdfcraft.exe"), b"app"), (r("CON"), b"x")], "device"),
            (vec![(r("pdfcraft.exe"), b"app"), (r("misc/"), b"")], "unexpected folder"),
            (vec![(r("pdfcraft.exe"), b"appX")], "signed as"),
            (vec![], "missing"),
        ];
        for (entries, expected) in cases {
            let entries: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), *b)).collect();
            let e = refused(&entries, &[app]);
            assert!(e.contains(expected), "{e} should mention {expected}");
        }
    }

    #[test]
    fn case_insensitive_duplicates_are_refused() {
        let e = refused(&[(&r("NOTICE"), b"a"), (&r("notice"), b"a"), (&r("pdfcraft.exe"), b"app")], &[("NOTICE", b"a"), ("pdfcraft.exe", b"app")]);
        assert!(e.contains("isn't") || e.contains("twice"), "{e}");
    }

    #[test]
    fn a_corrupt_or_tampered_zip_is_refused() {
        let dir = temp("corrupt");
        let (mut zip, m) = package(ROOT, &[(&r("pdfcraft.exe"), b"the application bytes")], &[("pdfcraft.exe", b"the application bytes")]);
        let p = write(&dir, b"not a zip at all");
        assert!(extract(&p, &m, &dir.join("a")).unwrap_err().to_string().contains("readable zip"));
        // A damaged byte inside the first entry's data (after its 30-byte local header and name):
        // decompression, the CRC or the signed hash catches it.
        let at = 30 + r("pdfcraft.exe").len() + 2;
        zip[at] ^= 0xff;
        let p = write(&dir, &zip);
        assert!(extract(&p, &m, &dir.join("b")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zip_bomb_is_refused_before_extraction() {
        let big = vec![0u8; 4 * 1024 * 1024];
        let e = refused(&[(&r("pdfcraft.exe"), &big)], &[("pdfcraft.exe", &big)]);
        assert!(e.contains("implausibly"), "{e}");
    }
}

//! The manifest inside a channel envelope: which build the owner approved and exactly what its
//! package holds. Parsed only after the envelope's signature has been checked; unknown fields are
//! ignored, malformed known ones refuse the whole manifest.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::envelope::MAX_MANIFEST;

/// The most files a package may hold.
pub const MAX_FILES: usize = 64;
/// The largest package downloaded.
pub const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
/// The most bytes a package may unpack to.
pub const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
/// The longest release note kept (characters).
const MAX_NOTE: usize = 4000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub product: String,
    pub platform: String,
    pub package_kind: String,
    pub channel: String,
    /// The approved distribution sequence: the only thing that orders updates.
    pub seq: u64,
    /// Such as `0.15.0-ov.7` (shown, and the package version in the registry).
    pub distribution_version: String,
    /// Upstream's workspace version the build reports (shown only).
    pub workspace_version: String,
    /// The full 40-character source commit.
    pub commit: String,
    pub source_repository: String,
    pub build: BuildIdentity,
    /// RFC 3339.
    pub approved_at: String,
    /// RFC 3339.
    pub expires_at: String,
    pub min_updater_schema: u32,
    pub package: Package,
    #[serde(default)]
    pub notes: Notes,
}

/// The Windows build that made the package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub workflow: String,
    pub run_id: u64,
    /// The Actions artifact name, `pdfcraft-overlay-<version>-<sha7>-windows-x64-portable`.
    pub artifact: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub url: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    /// The single top-level folder of the zip.
    pub root: String,
    /// The sum of the files' sizes.
    pub unpacked_size: u64,
    pub files: Vec<FileEntry>,
}

/// One file of the package, relative to its root folder, with `/` separators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    #[serde(default)]
    pub en: Option<String>,
    #[serde(default)]
    pub es: Option<String>,
}

/// 64 lowercase hex characters.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// 40 lowercase hex characters.
pub fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A short plain name: letters, digits, `.`, `_`, `-`, `+`, at most `max` bytes, not starting with `.`.
fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.starts_with('.') && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'+'))
}

/// An RFC 3339 time.
pub fn time(s: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(s).map(|t| t.with_timezone(&Utc)).map_err(|_| Error::Manifest(format!("{s:?} isn't an RFC 3339 time")))
}

impl Manifest {
    /// Parse and check the form of every field (not whether it is meant for this app).
    pub fn parse(bytes: &[u8]) -> Result<Manifest, Error> {
        if bytes.len() > MAX_MANIFEST {
            return Err(Error::Manifest("too large".into()));
        }
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        m.check_form()?;
        Ok(m)
    }

    fn check_form(&self) -> Result<(), Error> {
        let bad = |what: &str| Err(Error::Manifest(format!("malformed {what}")));
        if self.seq == 0 {
            return bad("seq");
        }
        if !plain(&self.distribution_version, 64) {
            return bad("distribution_version");
        }
        if !plain(&self.workspace_version, 64) {
            return bad("workspace_version");
        }
        if !is_commit(&self.commit) {
            return bad("commit");
        }
        if !plain(&self.build.workflow, 64) || !plain(&self.build.artifact, 128) {
            return bad("build");
        }
        let approved = time(&self.approved_at)?;
        let expires = time(&self.expires_at)?;
        if expires <= approved {
            return bad("expires_at (not after approved_at)");
        }
        let p = &self.package;
        if !plain(&p.file, 160) || !p.file.ends_with(".zip") {
            return bad("package.file");
        }
        if !plain(&p.root, 160) {
            return bad("package.root");
        }
        if !is_sha256(&p.sha256) {
            return bad("package.sha256");
        }
        if p.size == 0 || p.size > MAX_PACKAGE {
            return Err(Error::Manifest(format!("package size {} outside 1..={MAX_PACKAGE}", p.size)));
        }
        if p.files.is_empty() || p.files.len() > MAX_FILES {
            return Err(Error::Manifest(format!("{} package files (1..={MAX_FILES})", p.files.len())));
        }
        let mut total: u64 = 0;
        for f in &p.files {
            if !is_sha256(&f.sha256) {
                return Err(Error::Manifest(format!("malformed sha256 for {:?}", f.path)));
            }
            total = total.checked_add(f.size).ok_or_else(|| Error::Manifest("unpacked size overflows".into()))?;
        }
        if total != p.unpacked_size || p.unpacked_size > MAX_UNPACKED {
            return Err(Error::Manifest(format!("unpacked size {} doesn't match its files ({total}) or exceeds {MAX_UNPACKED}", p.unpacked_size)));
        }
        for note in [&self.notes.en, &self.notes.es].into_iter().flatten() {
            if note.chars().count() > MAX_NOTE {
                return bad("notes (too long)");
            }
        }
        Ok(())
    }

    /// The approval time (checked by [`Manifest::parse`]).
    pub fn approved(&self) -> Result<DateTime<Utc>, Error> {
        time(&self.approved_at)
    }

    /// The expiry time (checked by [`Manifest::parse`]).
    pub fn expires(&self) -> Result<DateTime<Utc>, Error> {
        time(&self.expires_at)
    }

    /// The manifest's JSON bytes, as the publication tool signs them.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec_pretty(self).map_err(|e| Error::Manifest(e.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> Manifest {
        Manifest {
            product: "pdf-overlay".into(),
            platform: "windows-x86_64".into(),
            package_kind: "portable-zip".into(),
            channel: "stable".into(),
            seq: 7,
            distribution_version: "0.15.0-ov.7".into(),
            workspace_version: "0.15.0".into(),
            commit: "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d".into(),
            source_repository: "sefr-14/pdfcraft".into(),
            build: BuildIdentity {
                workflow: "overlay-windows.yml".into(),
                run_id: 39595600341,
                artifact: "pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable".into(),
            },
            approved_at: "2026-11-02T10:00:00Z".into(),
            expires_at: "2027-05-01T00:00:00Z".into(),
            min_updater_schema: 1,
            package: Package {
                url: "https://git.ferrive.me/api/packages/sefr/generic/pdf-overlay-windows-x64/0.15.0-ov.7/p.zip".into(),
                file: "p.zip".into(),
                size: 100,
                sha256: "a".repeat(64),
                root: "pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable".into(),
                unpacked_size: 3,
                files: vec![FileEntry { path: "pdfcraft.exe".into(), size: 3, sha256: "b".repeat(64) }],
            },
            notes: Notes::default(),
        }
    }

    #[test]
    fn a_well_formed_manifest_round_trips_and_ignores_unknown_fields() {
        let m = sample();
        assert_eq!(Manifest::parse(&m.to_bytes().unwrap()).unwrap(), m);
        let mut v = serde_json::to_value(&m).unwrap();
        v["future_field"] = serde_json::json!({"x": 1});
        assert_eq!(Manifest::parse(v.to_string().as_bytes()).unwrap(), m);
    }

    #[test]
    fn malformed_fields_refuse_the_manifest() {
        type Case = (fn(&mut Manifest), &'static str);
        let cases: Vec<Case> = vec![
            (|m| m.seq = 0, "seq"),
            (|m| m.commit = "1a2b3c4".into(), "commit"),
            (|m| m.commit = "1A2B3C4D5E6F708192A3B4C5D6E7F8091A2B3C4D".into(), "commit"),
            (|m| m.distribution_version = "../x".into(), "distribution_version"),
            (|m| m.approved_at = "yesterday".into(), "RFC 3339"),
            (|m| m.expires_at = "2026-01-01T00:00:00Z".into(), "expires_at"),
            (|m| m.package.file = "p.exe".into(), "package.file"),
            (|m| m.package.root = ".hidden".into(), "package.root"),
            (|m| m.package.sha256 = "zz".into(), "package.sha256"),
            (|m| m.package.size = 0, "package size"),
            (|m| m.package.size = MAX_PACKAGE + 1, "package size"),
            (|m| m.package.files.clear(), "package files"),
            (|m| m.package.unpacked_size = 4, "unpacked size"),
            (|m| m.package.files[0].sha256 = "x".into(), "malformed sha256"),
            (|m| m.notes.en = Some("x".repeat(MAX_NOTE + 1)), "notes"),
        ];
        for (change, expected) in cases {
            let mut m = sample();
            change(&mut m);
            let e = Manifest::parse(&m.to_bytes().unwrap()).unwrap_err().to_string();
            assert!(e.contains(expected), "{e} should mention {expected}");
        }
        assert!(Manifest::parse(b"{}").is_err());
        assert!(Manifest::parse(&vec![b' '; MAX_MANIFEST + 1]).is_err());
    }

    #[test]
    fn too_many_files_or_an_overflowing_total_are_refused() {
        let mut m = sample();
        m.package.files = (0..=MAX_FILES).map(|i| FileEntry { path: format!("f{i}"), size: 1, sha256: "b".repeat(64) }).collect();
        m.package.unpacked_size = (MAX_FILES + 1) as u64;
        assert!(Manifest::parse(&m.to_bytes().unwrap()).is_err());
        let mut m = sample();
        m.package.files = vec![
            FileEntry { path: "a".into(), size: u64::MAX, sha256: "b".repeat(64) },
            FileEntry { path: "b".into(), size: 2, sha256: "b".repeat(64) },
        ];
        assert!(Manifest::parse(&m.to_bytes().unwrap()).unwrap_err().to_string().contains("overflows"));
    }
}

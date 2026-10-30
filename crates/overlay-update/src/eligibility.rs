//! Whether a signed envelope is an update this copy may offer (PRD §7). Every rule must hold;
//! the first that doesn't is the error.

use chrono::{DateTime, Duration, Utc};

use crate::envelope::{self, TrustedKey};
use crate::manifest::Manifest;
use crate::{CHANNEL, Check, CheckContext, Error, Offer, PACKAGE_KIND, PLATFORM, PRODUCT, SOURCE_REPOSITORY, UPDATER_SCHEMA, paths};

/// Where updates may come from, compiled into the app.
#[derive(Clone, Copy, Debug)]
pub struct Trust<'a> {
    pub keys: &'a [TrustedKey],
    /// The only host packages are downloaded from (HTTPS).
    pub host: &'a str,
    /// The path every package URL starts with, such as `/api/packages/sefr/generic/pdf-overlay-windows-x64/`.
    pub package_prefix: &'a str,
}

/// What this copy is.
#[derive(Clone, Copy, Debug)]
pub struct Installed<'a> {
    /// The commit compiled into the running build.
    pub commit: &'a str,
    /// The sequence of the last update installed by UPD.1 (0 when none: a manual install).
    pub installed_seq: u64,
}

/// How far in the future an approval date may lie (clock skew).
const SKEW: Duration = Duration::days(1);

/// Open `envelope` and decide what it means for this copy.
pub fn evaluate(
    envelope: &[u8],
    trust: &Trust<'_>,
    installed: &Installed<'_>,
    ctx: CheckContext,
    now: DateTime<Utc>,
) -> Result<(Manifest, Check), Error> {
    let manifest_bytes = envelope::open(envelope, trust.keys)?;
    let m = Manifest::parse(&manifest_bytes)?;
    for (what, got, want) in [
        ("product", &m.product, PRODUCT),
        ("platform", &m.platform, PLATFORM),
        ("package kind", &m.package_kind, PACKAGE_KIND),
        ("channel", &m.channel, CHANNEL),
        ("source repository", &m.source_repository, SOURCE_REPOSITORY),
    ] {
        if got != want {
            return Err(Error::NotForThisApp(format!("{what} {got:?}, expected {want:?}")));
        }
    }
    if m.min_updater_schema > UPDATER_SCHEMA {
        return Err(Error::Unsupported(format!("updater schema {}", m.min_updater_schema)));
    }
    if now >= m.expires()? {
        return Err(Error::Expired);
    }
    if m.approved()? > now + SKEW {
        return Err(Error::NotYetValid);
    }
    if m.seq < ctx.highest_seen_seq {
        return Err(Error::Downgrade { offered: m.seq, floor: ctx.highest_seen_seq });
    }
    check_url(&m, trust)?;
    check_files(&m)?;
    let check = if m.commit == installed.commit || m.seq <= installed.installed_seq {
        Check::UpToDate { distribution_version: m.distribution_version.clone(), seen_seq: m.seq }
    } else {
        Check::Available(Offer {
            seq: m.seq,
            distribution_version: m.distribution_version.clone(),
            commit: m.commit.clone(),
            approved_at: m.approved()?,
            size: m.package.size,
            notes_en: m.notes.en.clone(),
            notes_es: m.notes.es.clone(),
            envelope: String::from_utf8_lossy(envelope).into_owned(),
        })
    };
    Ok((m, check))
}

/// The package URL must be exactly `https://<host><prefix><distribution_version>/<file>`.
fn check_url(m: &Manifest, trust: &Trust<'_>) -> Result<(), Error> {
    let expected = format!("https://{}{}{}/{}", trust.host, trust.package_prefix, m.distribution_version, m.package.file);
    if m.package.url != expected {
        return Err(Error::Url(m.package.url.clone()));
    }
    Ok(())
}

/// Every file is a safe, allowed path, none appears twice (ignoring case), and the application
/// is there.
fn check_files(m: &Manifest) -> Result<(), Error> {
    let mut seen = std::collections::HashSet::new();
    for f in &m.package.files {
        paths::check_relative(&f.path)?;
        if !paths::is_allowed(&f.path) {
            return Err(Error::Archive(format!("{:?} isn't part of PDF Overlay", f.path)));
        }
        if !seen.insert(paths::fold(&f.path)) {
            return Err(Error::Archive(format!("{:?} is listed twice", f.path)));
        }
    }
    if !m.package.files.iter().any(|f| f.path == paths::APP_EXE) {
        return Err(Error::Archive(format!("no {}", paths::APP_EXE)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::sample;
    use ed25519_dalek::SigningKey;

    const NOW: &str = "2026-11-10T00:00:00Z";
    const PREFIX: &str = "/api/packages/sefr/generic/pdf-overlay-windows-x64/";

    fn signing() -> SigningKey {
        SigningKey::from_bytes(&[3; 32])
    }

    fn keys() -> Vec<TrustedKey> {
        vec![TrustedKey { id: "test-a", key: signing().verifying_key().to_bytes() }]
    }

    fn sealed(m: &Manifest) -> String {
        envelope::seal(&m.to_bytes().unwrap(), "test-a", &signing()).unwrap()
    }

    fn run(m: &Manifest, commit: &str, installed_seq: u64, highest: u64) -> Result<Check, Error> {
        let keys = keys();
        let trust = Trust { keys: &keys, host: "git.ferrive.me", package_prefix: PREFIX };
        let installed = Installed { commit, installed_seq };
        let now = crate::manifest::time(NOW).unwrap();
        evaluate(sealed(m).as_bytes(), &trust, &installed, CheckContext { highest_seen_seq: highest }, now).map(|(_, c)| c)
    }

    const OLD: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn a_newer_approved_build_is_offered() {
        let m = sample();
        match run(&m, OLD, 0, 0).unwrap() {
            Check::Available(o) => {
                assert_eq!((o.seq, o.distribution_version.as_str(), o.size), (7, "0.15.0-ov.7", 100));
                assert_eq!(o.envelope, sealed(&m));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_same_or_an_older_build_is_up_to_date() {
        let m = sample();
        let up_to_date = Check::UpToDate { distribution_version: "0.15.0-ov.7".into(), seen_seq: 7 };
        assert_eq!(run(&m, &m.commit, 0, 0), Ok(up_to_date.clone()));
        assert_eq!(run(&m, OLD, 7, 0), Ok(up_to_date.clone()));
        assert_eq!(run(&m, OLD, 9, 0), Ok(up_to_date));
    }

    #[test]
    fn a_replayed_older_envelope_is_refused() {
        assert_eq!(run(&sample(), OLD, 0, 8), Err(Error::Downgrade { offered: 7, floor: 8 }));
        assert!(matches!(run(&sample(), OLD, 0, 7), Ok(Check::Available(_))));
    }

    #[test]
    fn manifests_for_something_else_are_refused() {
        let cases: Vec<fn(&mut Manifest)> = vec![
            |m| m.product = "pdfcraft".into(),
            |m| m.platform = "windows-aarch64".into(),
            |m| m.package_kind = "msi".into(),
            |m| m.channel = "beta".into(),
            |m| m.source_repository = "storytold/pdfcraft".into(),
        ];
        for change in cases {
            let mut m = sample();
            change(&mut m);
            assert!(matches!(run(&m, OLD, 0, 0), Err(Error::NotForThisApp(_))), "{m:?}");
        }
        let mut m = sample();
        m.min_updater_schema = 2;
        assert!(matches!(run(&m, OLD, 0, 0), Err(Error::Unsupported(_))));
    }

    #[test]
    fn expiry_and_future_approvals_are_refused() {
        let mut m = sample();
        m.expires_at = NOW.into();
        assert_eq!(run(&m, OLD, 0, 0), Err(Error::Expired));
        let mut m = sample();
        m.approved_at = "2026-11-11T00:00:01Z".into();
        assert_eq!(run(&m, OLD, 0, 0), Err(Error::NotYetValid));
        // Within a day of skew is accepted.
        m.approved_at = "2026-11-10T23:00:00Z".into();
        assert!(run(&m, OLD, 0, 0).is_ok());
    }

    #[test]
    fn only_the_approved_package_location_is_accepted() {
        for url in [
            "http://git.ferrive.me/api/packages/sefr/generic/pdf-overlay-windows-x64/0.15.0-ov.7/p.zip",
            "https://evil.example/api/packages/sefr/generic/pdf-overlay-windows-x64/0.15.0-ov.7/p.zip",
            "https://git.ferrive.me.evil.example/api/packages/sefr/generic/pdf-overlay-windows-x64/0.15.0-ov.7/p.zip",
            "https://git.ferrive.me/api/packages/other/generic/pdf-overlay-windows-x64/0.15.0-ov.7/p.zip",
            "https://git.ferrive.me/api/packages/sefr/generic/pdf-overlay-windows-x64/0.15.0-ov.6/p.zip",
            "https://git.ferrive.me/api/packages/sefr/generic/pdf-overlay-windows-x64/0.15.0-ov.7/p.zip?x=1",
            "https://github.com/storytold/pdfcraft/releases/download/v0.15.0/p.zip",
        ] {
            let mut m = sample();
            m.package.url = url.into();
            assert!(matches!(run(&m, OLD, 0, 0), Err(Error::Url(_))), "{url}");
        }
    }

    #[test]
    fn package_file_lists_are_checked() {
        let entry = |p: &str| crate::manifest::FileEntry { path: p.into(), size: 1, sha256: "b".repeat(64) };
        let lists: Vec<Vec<crate::manifest::FileEntry>> = vec![
            vec![entry("pdfcraft.exe"), entry("../evil.exe")],
            vec![entry("pdfcraft.exe"), entry("PdfCraftData/app.ron")],
            vec![entry("pdfcraft.exe"), entry("NOTICE"), entry("notice")],
            vec![entry("NOTICE")],
            vec![entry("pdfcraft.exe"), entry("helper.exe")],
        ];
        for files in lists {
            let mut m = sample();
            m.package.unpacked_size = files.len() as u64;
            m.package.files = files;
            assert!(matches!(run(&m, OLD, 0, 0), Err(Error::Archive(_))), "{:?}", m.package.files);
        }
    }

    #[test]
    fn a_tampered_envelope_is_refused() {
        let s = sealed(&sample()).replace("signature_b64\": \"", "signature_b64\": \"AA");
        let keys = keys();
        let trust = Trust { keys: &keys, host: "git.ferrive.me", package_prefix: PREFIX };
        let r = evaluate(
            s.as_bytes(),
            &trust,
            &Installed { commit: OLD, installed_seq: 0 },
            CheckContext::default(),
            crate::manifest::time(NOW).unwrap(),
        );
        assert!(r.is_err());
    }
}

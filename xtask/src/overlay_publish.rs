//! `cargo xtask overlay-publish`: the owner's approval of a PDF Overlay Windows build for automatic
//! updates (UPD.1, `docs/updates.md`). Run on the owner's own machine, never in CI.
//!
//! - `keygen --id <key id> --out <file>` makes an Ed25519 signing key (the file holds its secret
//!   seed; keep it offline) and prints the public key to compile into the app.
//! - `sign --zip <portable zip> --commit <40 hex> --run-id <n> --seq <n> --key <file> --key-id <id>
//!   --out <dir> [--sha256 <file>] [--expires-days 180] [--notes-en <file>] [--notes-es <file>]
//!   [--previous <channel.json>]` checks the zip from the Actions artifact with the same validator
//!   the app uses, writes the signed `channel.json` and a copy of the zip into `<dir>`, and prints
//!   the upload commands. It uploads nothing itself: signing is the approval, uploading is a
//!   separate, deliberate step.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, anyhow, bail, ensure};
use ed25519_dalek::SigningKey;
use pdfcraft_overlay_update::eligibility::{self, Installed, Trust};
use pdfcraft_overlay_update::envelope::{self, TrustedKey};
use pdfcraft_overlay_update::manifest::{self, BuildIdentity, FileEntry, Manifest, Notes, Package};
use pdfcraft_overlay_update::{CHANNEL, Check, CheckContext, MANIFEST_SCHEMA, PACKAGE_KIND, PLATFORM, PRODUCT, SOURCE_REPOSITORY, archive, hash};

const HOST: &str = "git.ferrive.me";
const OWNER: &str = "sefr";
const PACKAGE: &str = "pdf-overlay-windows-x64";
const CHANNEL_PACKAGE: &str = "pdf-overlay-channel";
const NAME_PREFIX: &str = "pdfcraft-overlay-";
const NAME_SUFFIX: &str = "-windows-x64-portable.zip";

pub fn run(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(&Flags::parse(&args[1..])?),
        Some("sign") => sign(&Flags::parse(&args[1..])?),
        _ => bail!(
            "usage: cargo xtask overlay-publish keygen --id <key id> --out <file>\n       cargo xtask overlay-publish sign --zip <zip> --commit <sha> --run-id <n> --seq <n> --key <file> --key-id <id> --out <dir> [--sha256 <file>] [--expires-days 180] [--notes-en <file>] [--notes-es <file>] [--previous <channel.json>]"
        ),
    }
}

/// `--name value` pairs.
struct Flags(Vec<(String, String)>);

impl Flags {
    fn parse(args: &[String]) -> anyhow::Result<Flags> {
        let mut out = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let name = a.strip_prefix("--").ok_or_else(|| anyhow!("unexpected argument {a:?}"))?;
            let value = it.next().ok_or_else(|| anyhow!("--{name} needs a value"))?;
            out.push((name.to_string(), value.clone()));
        }
        Ok(Flags(out))
    }
    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
    fn need(&self, name: &str) -> anyhow::Result<&str> {
        self.get(name).ok_or_else(|| anyhow!("--{name} is required"))
    }
}

fn hex32(s: &str) -> anyhow::Result<[u8; 32]> {
    let s = s.trim();
    ensure!(s.len() == 64, "expected 64 hex characters");
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("not hex")?;
    }
    Ok(out)
}

fn keygen(flags: &Flags) -> anyhow::Result<()> {
    let id = flags.need("id")?;
    ensure!(envelope::valid_key_id(id), "a key id is 1 to 32 characters of a-z, 0-9 and -");
    let out = PathBuf::from(flags.need("out")?);
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| anyhow!("no random source: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&out).with_context(|| format!("{} (it must not exist yet)", out.display()))?;
    std::io::Write::write_all(&mut file, format!("{}\n", hash::hex(&seed)).as_bytes())?;
    let public = key.verifying_key().to_bytes();
    println!("Signing key written to {} — keep it offline and backed up; anyone holding it can approve updates.", out.display());
    println!("Public key ({id}): {}", hash::hex(&public));
    println!("For a release build, add to KEYS in apps/pdfcraft/src/updates/mod.rs:");
    println!("    TrustedKey {{ id: \"{id}\", key: {public:?} }},");
    println!("For a controlled test build only: PDF_OVERLAY_UPDATE_TEST_KEY={id}:{}", hash::hex(&public));
    Ok(())
}

/// The files under the zip's root folder, with sizes and hashes.
fn list_files(zip_path: &Path, root: &str) -> anyhow::Result<Vec<FileEntry>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(zip_path)?).context("not a readable zip")?;
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.to_string();
        let Some(rel) = name.strip_prefix(&format!("{root}/")) else { bail!("{name:?} is outside the folder {root:?}") };
        if rel.is_empty() || rel.ends_with('/') {
            continue;
        }
        ensure!(entry.size() <= manifest::MAX_UNPACKED, "{rel} is too large");
        let mut bytes = Vec::new();
        (&mut entry).take(manifest::MAX_UNPACKED).read_to_end(&mut bytes)?;
        files.push(FileEntry { path: rel.to_string(), size: bytes.len() as u64, sha256: hash::sha256(&bytes) });
    }
    Ok(files)
}

fn sign(flags: &Flags) -> anyhow::Result<()> {
    let zip_path = PathBuf::from(flags.need("zip")?);
    let file = zip_path.file_name().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("no file name"))?.to_string();
    let middle = file
        .strip_prefix(NAME_PREFIX)
        .and_then(|m| m.strip_suffix(NAME_SUFFIX))
        .ok_or_else(|| anyhow!("{file} isn't named {NAME_PREFIX}<version>-<commit>{NAME_SUFFIX} (overlay-windows.yml)"))?;
    let (workspace_version, sha7) = middle.rsplit_once('-').ok_or_else(|| anyhow!("{file} has no commit"))?;
    let root = file.trim_end_matches(".zip").to_string();
    let commit = flags.need("commit")?.trim().to_ascii_lowercase();
    ensure!(manifest::is_commit(&commit), "--commit must be the full 40-character commit");
    ensure!(commit.starts_with(sha7), "--commit {commit} isn't the commit {sha7} the artifact was built from");
    let run_id: u64 = flags.need("run-id")?.parse().context("--run-id")?;
    let seq: u64 = flags.need("seq")?.parse().context("--seq")?;
    ensure!(seq > 0, "--seq starts at 1");
    let days: i64 = flags.get("expires-days").unwrap_or("180").parse().context("--expires-days")?;
    ensure!((1..=730).contains(&days), "--expires-days must be 1 to 730");
    let key_id = flags.need("key-id")?;
    let key = SigningKey::from_bytes(&hex32(&std::fs::read_to_string(flags.need("key")?).context("--key")?)?);
    let out = PathBuf::from(flags.need("out")?);

    let bytes = std::fs::read(&zip_path).with_context(|| zip_path.display().to_string())?;
    let sha256 = hash::sha256(&bytes);
    if let Some(sum) = flags.get("sha256") {
        let text = std::fs::read_to_string(sum).context("--sha256")?;
        let expected = text.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
        ensure!(expected == sha256, "the zip's SHA-256 {sha256} doesn't match {sum} ({expected})");
    }
    let files = list_files(&zip_path, &root)?;
    let distribution_version = format!("{workspace_version}-ov.{seq}");
    let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
    let notes = |flag: &str| -> anyhow::Result<Option<String>> {
        flags.get(flag).map(|p| std::fs::read_to_string(p).map(|s| s.trim().to_string()).with_context(|| format!("--{flag}"))).transpose()
    };
    let m = Manifest {
        product: PRODUCT.into(),
        platform: PLATFORM.into(),
        package_kind: PACKAGE_KIND.into(),
        channel: CHANNEL.into(),
        seq,
        distribution_version: distribution_version.clone(),
        workspace_version: workspace_version.into(),
        commit: commit.clone(),
        source_repository: SOURCE_REPOSITORY.into(),
        build: BuildIdentity { workflow: "overlay-windows.yml".into(), run_id, artifact: root.clone() },
        approved_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        expires_at: (now + chrono::Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        min_updater_schema: 1,
        package: Package {
            url: format!("https://{HOST}/api/packages/{OWNER}/generic/{PACKAGE}/{distribution_version}/{file}"),
            file: file.clone(),
            size: bytes.len() as u64,
            sha256: sha256.clone(),
            root: root.clone(),
            unpacked_size: files.iter().map(|f| f.size).sum(),
            files,
        },
        notes: Notes { en: notes("notes-en")?, es: notes("notes-es")? },
    };
    let manifest_bytes = m.to_bytes()?;
    // The same checks the app makes: the manifest's form, the package against it, the signature,
    // and that the result is an update for a copy without it.
    let staging = std::env::temp_dir().join(format!("overlay-publish-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let extracted = archive::extract(&zip_path, &m, &staging.join("new"));
    let _ = std::fs::remove_dir_all(&staging);
    extracted.map_err(|e| anyhow!("{e}"))?;
    let sealed = envelope::seal(&manifest_bytes, key_id, &key)?;
    let trusted = [TrustedKey { id: leak(key_id), key: key.verifying_key().to_bytes() }];
    let trust = Trust { keys: &trusted, host: HOST, package_prefix: &format!("/api/packages/{OWNER}/generic/{PACKAGE}/") };
    let (_, check) = eligibility::evaluate(sealed.as_bytes(), &trust, &Installed { commit: "", installed_seq: 0 }, CheckContext::default(), now)?;
    ensure!(matches!(check, Check::Available(_)), "the signed envelope isn't offered as an update ({check:?})");
    if let Some(previous) = flags.get("previous") {
        let prev = std::fs::read(previous).context("--previous")?;
        let prev = Manifest::parse(&envelope::open(&prev, &trusted)?)?;
        ensure!(seq > prev.seq, "--seq {seq} must be greater than the published sequence {}", prev.seq);
    }

    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("channel.json"), &sealed)?;
    std::fs::write(out.join("manifest.json"), &manifest_bytes)?;
    std::fs::copy(&zip_path, out.join(&file))?;
    println!("Approved for automatic updates (envelope schema {MANIFEST_SCHEMA}):");
    println!("  {PRODUCT} {distribution_version} (sequence {seq}) for {PLATFORM}, channel {CHANNEL}");
    println!("  commit {commit}, run {run_id}, artifact {root}");
    println!("  package {file}: {} bytes, SHA-256 {sha256}, {} files", bytes.len(), m.package.files.len());
    println!("  valid until {}", m.expires_at);
    println!("Written to {}: channel.json (signed), manifest.json (for review), {file}.", out.display());
    println!();
    println!("To publish, from a machine that reaches {HOST}, with your own Forgejo token in FORGEJO_TOKEN,");
    println!("in this order (the package first, the stable pointer last):");
    let base = format!("https://{HOST}/api/packages/{OWNER}/generic");
    let dir = out.display();
    println!("  curl --fail -u {OWNER}:\"$FORGEJO_TOKEN\" --upload-file \"{dir}/{file}\" {base}/{PACKAGE}/{distribution_version}/{file}");
    println!("  curl --fail -u {OWNER}:\"$FORGEJO_TOKEN\" --upload-file \"{dir}/channel.json\" {base}/{CHANNEL_PACKAGE}/{seq}/channel.json");
    println!("  curl --fail -u {OWNER}:\"$FORGEJO_TOKEN\" -X DELETE {base}/{CHANNEL_PACKAGE}/stable   # absent the first time");
    println!("  curl --fail -u {OWNER}:\"$FORGEJO_TOKEN\" --upload-file \"{dir}/channel.json\" {base}/{CHANNEL_PACKAGE}/stable/channel.json");
    Ok(())
}

/// A key id for the lifetime of this short-lived command.
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

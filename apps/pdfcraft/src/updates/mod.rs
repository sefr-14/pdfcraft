// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Automatic updates of the Windows portable build (UPD.1; the design is `docs/updates.md`).
//!
//! - [`check`] fetches the owner-signed channel envelope from this fork's Forgejo package registry
//!   and decides whether it offers an update for this copy (`pdfcraft_overlay_update`).
//! - [`Installer`] downloads, verifies and stages an offered update while the app runs, and after
//!   the window has closed ([`after_exit`]) starts the helper: a copy of this very executable in
//!   the transaction folder, run with `--overlay-update-apply`, which replaces the program files,
//!   starts the new build, and commits or rolls back.
//! - [`startup`] runs before anything else on every start: it confirms a new build to the waiting
//!   helper, finishes or hands interrupted transactions to the helper (a running `pdfcraft.exe`
//!   never replaces or restores installed files), and reports the last result.
//!
//! No credentials are compiled in or sent. Updates are trusted only through the Ed25519 public
//! keys in [`KEYS`]; until the owner adds one, every update is refused as unverifiable.

mod net;
mod process;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use chrono::{DateTime, Utc};
use pdfcraft_overlay_update::eligibility::{self, Installed, Trust};
use pdfcraft_overlay_update::envelope::TrustedKey;
use pdfcraft_overlay_update::helper::{self, Env, Outcome, Processes, Timing};
use pdfcraft_overlay_update::journal::{Journal, State};
use pdfcraft_overlay_update::ops::RealOps;
use pdfcraft_overlay_update::transaction::{self, Layout, Lock, StartupAction};
use pdfcraft_overlay_update::{Check, CheckContext, archive, paths};
use pdfcraft_ui_egui::updates::{Offer, Prepared, Progress, UpdateInstaller};

/// The update host: this fork's Forgejo, behind its existing Cloudflare Zero Trust rules.
const HOST: &str = "git.ferrive.me";
/// Where approved packages live (package `pdf-overlay-windows-x64`, one version per release).
const PACKAGE_PREFIX: &str = "/api/packages/sefr/generic/pdf-overlay-windows-x64/";
/// The stable channel's signed envelope.
const CHANNEL_PATH: &str = "/api/packages/sefr/generic/pdf-overlay-channel/stable/channel.json";

/// The owner's update-signing public keys (primary, then backup). None has been created yet
/// (UPD.1 owner decision 2), so this build refuses every update as unverifiable.
const KEYS: &[TrustedKey] = &[];

/// A controlled test build may name its own key and host at compile time
/// (`PDF_OVERLAY_UPDATE_TEST_KEY=<id>:<64 hex>`, `PDF_OVERLAY_UPDATE_TEST_HOST=<host[:port]>`).
/// Release builds never set them.
const TEST_KEY: Option<&str> = option_env!("PDF_OVERLAY_UPDATE_TEST_KEY");
const TEST_HOST: Option<&str> = option_env!("PDF_OVERLAY_UPDATE_TEST_HOST");

/// The commit this build was made from (`build.rs`), when it is a build of this fork.
const COMMIT: Option<&str> = option_env!("PDF_OVERLAY_COMMIT");

/// The trusted keys: [`KEYS`], plus a test build's key.
fn keys() -> &'static [TrustedKey] {
    static KEYS_FOUND: OnceLock<Vec<TrustedKey>> = OnceLock::new();
    KEYS_FOUND.get_or_init(|| {
        let mut all = KEYS.to_vec();
        if let Some(k) = TEST_KEY.and_then(parse_test_key) {
            all.push(k);
        }
        all
    })
}

/// `<id>:<64 hex>`.
fn parse_test_key(s: &'static str) -> Option<TrustedKey> {
    let (id, hex) = s.split_once(':')?;
    if !pdfcraft_overlay_update::envelope::valid_key_id(id) || hex.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(TrustedKey { id, key })
}

fn host() -> &'static str {
    TEST_HOST.unwrap_or(HOST)
}

fn trust() -> Trust<'static> {
    Trust { keys: keys(), host: host(), package_prefix: PACKAGE_PREFIX }
}

/// The folder holding the running executable.
fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

fn now() -> DateTime<Utc> {
    DateTime::<Utc>::from(std::time::SystemTime::now())
}

/// 64 random bits from the operating system's source (through `RandomState`'s keys).
fn random() -> u64 {
    use std::hash::BuildHasher;
    std::hash::RandomState::new().hash_one(std::process::id())
}

/// The sequence UPD.1 installed into this copy, from its receipt (0 for a manual install).
pub fn installed_seq() -> u64 {
    match (install_dir(), COMMIT) {
        (Some(dir), Some(commit)) => transaction::installed_seq(&dir, keys(), commit),
        _ => 0,
    }
}

/// Help ▸ Check for updates: the approved build, if it is an update for this copy.
pub fn check(context: CheckContext) -> Result<Check, String> {
    let url = format!("https://{}{CHANNEL_PATH}", host());
    let bytes = net::fetch_envelope(host(), &url)?;
    let installed = Installed { commit: COMMIT.unwrap_or_default(), installed_seq: installed_seq() };
    eligibility::evaluate(&bytes, &trust(), &installed, context, now()).map(|(_, c)| c).map_err(|e| e.to_string())
}

/// The update staged this session, until the helper takes it.
struct Staged {
    layout: Layout,
    lock: Lock,
}

/// Installs updates on Windows; see the module documentation.
pub struct Installer {
    install: Option<PathBuf>,
    staged: Mutex<Option<Staged>>,
    handed_off: Mutex<Option<Layout>>,
}

/// The one installer of this process (the app and [`after_exit`] share it).
pub fn installer() -> Arc<Installer> {
    static INSTALLER: OnceLock<Arc<Installer>> = OnceLock::new();
    INSTALLER.get_or_init(|| Arc::new(Installer { install: install_dir(), staged: Mutex::new(None), handed_off: Mutex::new(None) })).clone()
}

impl Installer {
    fn prepare_inner(&self, offer: &Offer, progress: &Progress) -> Result<Prepared, String> {
        if let Some(why) = self.unavailable() {
            return Err(why);
        }
        let install = self.install.clone().ok_or("PDF Overlay's folder is unknown")?;
        // The offer is checked again: what is installed must be what was verified.
        let installed = Installed { commit: COMMIT.unwrap_or_default(), installed_seq: installed_seq() };
        let (manifest, check) =
            eligibility::evaluate(offer.envelope.as_bytes(), &trust(), &installed, CheckContext::default(), now()).map_err(|e| e.to_string())?;
        if !matches!(check, Check::Available(ref o) if o.seq == offer.seq) {
            return Err("the offered update is no longer an update for this copy".into());
        }
        let procs = process::OsProcesses;
        let lock = Lock::acquire(&install, std::process::id(), &|pid| procs.alive(pid)).map_err(|e| e.to_string())?;
        transaction::cleanup(&install, now());
        let needed = manifest.package.size.saturating_add(manifest.package.unpacked_size.saturating_mul(2)).saturating_add(16 << 20);
        if let Some(free) = process::free_space(&install)
            && free < needed
        {
            return Err(format!("not enough free disk space ({} MB needed, {} MB free)", needed >> 20, free >> 20));
        }
        let nonce = format!("{:016x}{:016x}", random(), random().rotate_left(17));
        let txn_id = transaction::new_txn_id(now(), random());
        let mut ops = RealOps::default();
        let (layout, mut journal) =
            transaction::begin(&install, &txn_id, &manifest, &offer.envelope, COMMIT, &nonce, &mut ops).map_err(|e| e.to_string())?;
        let staged = (|| -> Result<(), String> {
            let part = layout.txn.join("package.zip.part");
            net::download(host(), &manifest.package, &part, progress)?;
            std::fs::rename(&part, layout.package()).map_err(|e| e.to_string())?;
            archive::extract(&layout.package(), &manifest, &layout.new_dir()).map_err(|e| e.to_string())?;
            transaction::plan(&layout, &mut journal, &manifest, &mut ops).map_err(|e| e.to_string())?;
            // The helper is this (known-good) executable, run from the transaction folder.
            let me = std::env::current_exe().map_err(|e| e.to_string())?;
            std::fs::copy(&me, layout.helper()).map_err(|e| format!("couldn't prepare the update helper: {e}"))?;
            transaction::record_helper(&layout, &mut journal, &mut ops).map_err(|e| e.to_string())
        })();
        if let Err(e) = staged {
            transaction::discard(&layout);
            return Err(e);
        }
        log::info!("update {} downloaded, verified and staged in {}", manifest.distribution_version, layout.txn.display());
        let prepared = Prepared { id: txn_id, distribution_version: manifest.distribution_version.clone() };
        *self.staged.lock().unwrap_or_else(PoisonError::into_inner) = Some(Staged { layout, lock });
        Ok(prepared)
    }
}

impl UpdateInstaller for Installer {
    fn unavailable(&self) -> Option<String> {
        if !cfg!(windows) {
            return Some("updates install only in the Windows portable build".into());
        }
        if COMMIT.is_none() {
            return Some("this is a development build without a build identity".into());
        }
        if keys().is_empty() {
            return Some("this build has no update signing key".into());
        }
        let Some(install) = &self.install else { return Some("PDF Overlay's folder is unknown".into()) };
        let probe = install.join(format!(".pdf-overlay-write-test-{}", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                None
            }
            Err(e) => Some(format!("PDF Overlay's folder can't be written ({e}); update it by hand or move it to a writable folder")),
        }
    }

    fn prepare(&self, offer: &Offer, progress: &Progress) -> Result<Prepared, String> {
        self.prepare_inner(offer, progress).inspect_err(|e| log::warn!("update not prepared: {e}"))
    }

    fn others_running(&self) -> usize {
        self.install.as_ref().map_or(0, |dir| process::OsProcesses.using(dir, &[std::process::id()]).len())
    }

    fn hand_off(&self, prepared: &Prepared) {
        let staged = self.staged.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(Staged { layout, lock }) = staged
            && layout.txn.file_name().and_then(|n| n.to_str()) == Some(prepared.id.as_str())
        {
            // The helper takes the lock over once this process has exited.
            lock.hand_over();
            *self.handed_off.lock().unwrap_or_else(PoisonError::into_inner) = Some(layout);
        }
    }

    fn abandon(&self, _: &Prepared) {
        if let Some(Staged { layout, lock }) = self.staged.lock().unwrap_or_else(PoisonError::into_inner).take() {
            transaction::discard(&layout);
            drop(lock);
        }
    }
}

/// After the window has closed: start the helper for an update handed over this session.
pub fn after_exit() {
    let layout = installer().handed_off.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some(layout) = layout else { return };
    let args: Vec<OsString> = vec!["--overlay-update-apply".into(), layout.txn.clone().into_os_string(), std::process::id().to_string().into()];
    match std::process::Command::new(layout.helper()).args(&args).current_dir(&layout.txn).spawn() {
        Ok(_) => log::info!("update helper started for {}", layout.txn.display()),
        Err(e) => {
            log::error!("couldn't start the update helper: {e}; nothing was installed");
            transaction::discard(&layout);
        }
    }
}

/// The update flags of this start, taken out of the arguments.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    /// `--overlay-update-verify <txn> <nonce>`: the helper started this new build.
    verify: Option<(PathBuf, String)>,
    /// `--overlay-update-result <txn>`: the helper restarted the app to report.
    result: Option<PathBuf>,
}

impl Flags {
    /// Remove the update flags from `args`.
    pub fn take(args: Vec<String>) -> (Flags, Vec<String>) {
        let mut flags = Flags::default();
        let mut rest = Vec::with_capacity(args.len());
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--overlay-update-verify" => {
                    if let (Some(txn), Some(nonce)) = (it.next(), it.next()) {
                        flags.verify = Some((PathBuf::from(txn), nonce));
                    }
                }
                "--overlay-update-result" => flags.result = it.next().map(PathBuf::from),
                _ => rest.push(a),
            }
        }
        (flags, rest)
    }
}

/// What this start does about updates, besides starting normally.
#[derive(Clone, Default)]
pub struct StartInfo {
    pub installed_seq: u64,
    /// Show the helper's report: (succeeded, message).
    pub result: Option<(bool, String)>,
    /// Something to tell the user (manual recovery).
    pub notice: Option<String>,
    /// Confirm to the waiting helper once a frame is drawn.
    pub verify: Option<(Layout, String)>,
    /// Commit an unverified install once a frame is drawn (this is its new build).
    pub commit_after_start: Option<Layout>,
}

pub enum Startup {
    Continue(Box<StartInfo>),
    /// The helper repairs an interrupted update and restarts PDF Overlay: exit now.
    Exit,
}

/// The result message for a finished transaction.
fn describe(layout: &Layout) -> Option<(bool, String)> {
    let j = Journal::load(&layout.txn).ok()?;
    let note = j.note.clone().unwrap_or_default();
    Some(match j.state {
        State::Committed | State::Verified => (true, format!("PDF Overlay {} is installed.", j.distribution_version)),
        State::RolledBack => (false, format!("The previous version was restored: {note}")),
        State::Prepared => (false, format!("Nothing was installed: {note}")),
        State::RecoveryRequired => {
            (false, format!("Automatic recovery couldn't finish ({note}). Follow {}.", layout.txn.join(transaction::RECOVER).display()))
        }
        State::BackingUp | State::Installing | State::InstalledUnverified | State::RollingBack => {
            (false, "The update is still being repaired; it finishes when every PDF Overlay window is closed.".to_string())
        }
    })
}

/// Before anything else on a normal start (see the module documentation).
pub fn startup(flags: &Flags, files: &[String]) -> Startup {
    let mut info = StartInfo { installed_seq: installed_seq(), ..StartInfo::default() };
    let Some(install) = install_dir() else { return Startup::Continue(Box::new(info)) };
    // Nothing was ever staged here: no folder is created beside the program.
    if !transaction::updates_root(&install).is_dir() {
        return Startup::Continue(Box::new(info));
    }
    if let Some(txn) = &flags.result {
        info.result = Layout::from_txn_dir(txn).filter(|l| l.install == install).and_then(|l| describe(&l));
    }
    if let Some((txn, nonce)) = &flags.verify {
        // Started by the helper as the new build: confirm (or refuse) and leave recovery alone.
        if let Some(layout) = Layout::from_txn_dir(txn).filter(|l| l.install == install) {
            let expected = Journal::load(&layout.txn).ok().map(|j| j.new_commit);
            if expected.is_some() && expected.as_deref() == COMMIT {
                info.verify = Some((layout, nonce.clone()));
            } else {
                confirm(&layout, nonce, false, Some("the new version isn't the build that was approved".into()));
                return Startup::Exit;
            }
        }
        return Startup::Continue(Box::new(info));
    }
    let procs = process::OsProcesses;
    let me = std::process::id();
    // A helper at work holds the lock: leave everything to it.
    let Ok(lock) = Lock::acquire(&install, me, &|pid| procs.alive(pid)) else { return Startup::Continue(Box::new(info)) };
    for layout in transaction::transactions(&install) {
        let Ok(mut j) = Journal::load(&layout.txn) else { continue };
        let helper_ok = transaction::helper_intact(&layout, &j);
        match transaction::startup_action(&j, COMMIT, helper_ok) {
            StartupAction::Nothing => {}
            StartupAction::Discard => transaction::discard(&layout),
            StartupAction::Commit => {
                if let Err(e) = transaction::commit(&layout, &mut j, now(), &mut RealOps::default()) {
                    log::warn!("couldn't finish the update record: {e}");
                }
            }
            StartupAction::VerifyHere => info.commit_after_start = Some(layout),
            StartupAction::Manual => {
                if j.state != State::RecoveryRequired {
                    j.note = Some("the update helper is missing or was changed".into());
                    let _ = j.set(State::RecoveryRequired, &layout.txn, &mut RealOps::default());
                }
                info.notice = Some(format!(
                    "An update of PDF Overlay was interrupted and couldn't be repaired automatically. Follow {}.",
                    layout.txn.join(transaction::RECOVER).display()
                ));
            }
            StartupAction::Delegate => {
                if flags.result.is_some() || !procs.using(&install, &[me]).is_empty() {
                    // Just restarted by the helper, or other windows are open: no loop, no
                    // restore while they run. A later start repairs it.
                    info.notice = Some("An interrupted update will be repaired the next time PDF Overlay starts with no other window open.".into());
                    continue;
                }
                let mut args: Vec<OsString> =
                    vec!["--overlay-update-recover".into(), layout.txn.clone().into_os_string(), me.to_string().into(), "--".into()];
                args.extend(files.iter().map(OsString::from));
                match std::process::Command::new(layout.helper()).args(&args).current_dir(&layout.txn).spawn() {
                    Ok(_) => {
                        log::info!("an interrupted update is being repaired by its helper; PDF Overlay restarts afterwards");
                        lock.hand_over();
                        return Startup::Exit;
                    }
                    Err(e) => {
                        info.notice = Some(format!(
                            "An interrupted update couldn't be repaired ({e}). Follow {}.",
                            layout.txn.join(transaction::RECOVER).display()
                        ))
                    }
                }
            }
        }
    }
    transaction::cleanup(&install, now());
    drop(lock);
    Startup::Continue(Box::new(info))
}

/// Tell the waiting helper whether this new build started.
pub fn confirm(layout: &Layout, nonce: &str, ok: bool, reason: Option<String>) {
    let v =
        transaction::Verification { nonce: nonce.to_string(), commit: COMMIT.unwrap_or_default().to_string(), pid: std::process::id(), ok, reason };
    if let Err(e) = transaction::write_verification(layout, &v, &mut RealOps::default()) {
        log::error!("couldn't confirm the update: {e}");
    }
}

/// This start is the new build of an install whose confirmation was missed: it has drawn a frame,
/// so the update is final.
pub fn commit_here(layout: &Layout) {
    let Ok(mut j) = Journal::load(&layout.txn) else { return };
    if j.state == State::InstalledUnverified && COMMIT == Some(j.new_commit.as_str()) {
        match transaction::commit(layout, &mut j, now(), &mut RealOps::default()) {
            Ok(()) => log::info!("update {} confirmed by this start", j.distribution_version),
            Err(e) => log::warn!("couldn't finish the update record: {e}"),
        }
    }
}

/// Helper modes, before anything else in `main`: `Some` when this process was the helper.
pub fn run_helper_if_asked() -> Option<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let me_exe = std::env::current_exe().ok()?;
    let own_txn = me_exe.parent().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let in_txn = me_exe.file_name().is_some_and(|n| n == transaction::HELPER);
    let mode = args.first().and_then(|a| a.to_str());
    let (recovering, txn, parent, files) = match mode {
        Some("--overlay-update-apply") => (false, args.get(1)?.into(), parse_pid(args.get(2)), Vec::new()),
        Some("--overlay-update-recover") => (true, args.get(1)?.into(), parse_pid(args.get(2)), args.iter().skip(4).cloned().collect()),
        // Double-clicked in its transaction folder: the manual recovery of RECOVER.txt.
        None if in_txn => (true, own_txn.clone()?, 0, Vec::new()),
        _ => return None,
    };
    let txn: PathBuf = txn;
    // The helper only ever works on its own transaction folder.
    let txn = std::fs::canonicalize(&txn).unwrap_or(txn);
    if !in_txn || own_txn.as_ref() != Some(&txn) {
        log::error!("update helper refused: {} isn't its own transaction folder", txn.display());
        return Some(());
    }
    let layout = Layout::from_txn_dir(&txn)?;
    let mut log_file = std::fs::OpenOptions::new().create(true).append(true).open(layout.txn.join("update.log")).ok();
    let mut write_log = |line: &str| {
        log::info!("update helper: {line}");
        if let Some(f) = log_file.as_mut() {
            use std::io::Write as _;
            let _ = writeln!(f, "{} {line}", now().to_rfc3339());
        }
    };
    write_log(if recovering { "recovering" } else { "installing" });
    let procs = process::OsProcesses;
    let me = std::process::id();
    // Wait for the app that started us before taking its lock over.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while parent != 0 && procs.alive(parent) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let outcome = match Lock::acquire(&layout.install, me, &|pid| procs.alive(pid)) {
        Err(e) => Outcome::NotStarted(e.to_string()),
        Ok(_lock) => {
            let mut launcher = process::OsLauncher;
            let mut ops = RealOps::default();
            let mut env =
                Env { procs: &procs, launcher: &mut launcher, ops: &mut ops, timing: Timing::default(), me, now: &now, log: &mut write_log };
            if recovering { helper::recover(&layout, parent, &mut env) } else { helper::apply(&layout, parent, &mut env) }
        }
    };
    write_log(&format!("outcome: {outcome:?}"));
    if outcome.relaunch() || recovering {
        let exe = layout.installed(paths::APP_EXE);
        let mut relaunch: Vec<OsString> = vec!["--overlay-update-result".into(), layout.txn.clone().into_os_string()];
        relaunch.extend(files);
        if let Err(e) = std::process::Command::new(&exe).args(&relaunch).current_dir(&layout.install).spawn() {
            write_log(&format!("couldn't restart PDF Overlay: {e}"));
        }
    }
    Some(())
}

fn parse_pid(arg: Option<&OsString>) -> u32 {
    arg.and_then(|a| a.to_str()).and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_flags_are_taken_out_of_the_arguments() {
        let args = ["a.pdf", "--overlay-update-verify", "T", "N", "--page", "2", "--overlay-update-result", "R"].map(String::from).to_vec();
        let (flags, rest) = Flags::take(args);
        assert_eq!(rest, ["a.pdf", "--page", "2"]);
        assert_eq!(flags.verify, Some((PathBuf::from("T"), "N".into())));
        assert_eq!(flags.result, Some(PathBuf::from("R")));
    }

    #[test]
    fn test_keys_must_be_well_formed() {
        assert!(parse_test_key("test-a:00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff").is_some());
        for bad in ["test-a", "Test:00", "test-a:zz112233445566778899aabbccddeeff00112233445566778899aabbccddeeff", "a:0011"] {
            assert!(parse_test_key(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn release_builds_trust_no_key_but_the_owner_s() {
        // No production key exists yet, and a release build sets no test key or host.
        assert!(KEYS.is_empty());
        if TEST_KEY.is_none() {
            assert!(keys().is_empty());
        }
        assert_eq!(HOST, "git.ferrive.me");
        assert!(PACKAGE_PREFIX.starts_with("/api/packages/sefr/generic/"));
    }

    /// The fork's own update source only: never upstream's releases, never upstream's updater,
    /// never a credential (UPD.1 security rules; supersedes OV8.1's guard).
    #[test]
    fn upstream_and_credentials_are_never_used() {
        let code = |src: &'static str| src.split("#[cfg(test)]").next().unwrap_or(src);
        for (file, src) in [
            ("updates/mod.rs", include_str!("mod.rs")),
            ("updates/net.rs", include_str!("net.rs")),
            ("updates/process.rs", include_str!("process.rs")),
            ("ui-egui updates.rs", include_str!("../../../../crates/ui-egui/src/platform/updates.rs")),
        ] {
            let code = code(src);
            assert!(!code.contains(concat!("story", "told")), "{file} names upstream");
            assert!(!code.contains("/releases"), "{file} asks for releases");
            for secret in [concat!("CF-Access", "-Client"), concat!("Author", "ization"), concat!("FORGEJO", "_TOKEN"), concat!("token ", "=")] {
                assert!(!code.contains(secret), "{file} handles a credential ({secret})");
            }
        }
        // SYNC.13 (D1): upstream's self-updater stays out of this fork.
        let updater = concat!("craft", "_update");
        for (file, src) in [("main.rs", include_str!("../main.rs")), ("Cargo.toml", include_str!("../../Cargo.toml"))] {
            assert!(!src.contains(updater) && !src.contains(concat!("craft", "-update")), "{file} uses upstream's updater");
            assert!(!src.contains(concat!("--finish", "-update")), "{file} has upstream's self-update helper");
        }
        for (file, src) in
            [("ui-egui Cargo.toml", include_str!("../../../../crates/ui-egui/Cargo.toml")), ("Cargo.toml", include_str!("../../../../Cargo.toml"))]
        {
            assert!(!src.contains(concat!("craft", "-update")), "{file} depends on upstream's updater");
        }
    }

    /// Live (`cargo test -p pdfcraft -- --ignored`): what the update server answers from here.
    #[test]
    #[ignore = "needs network access to the update server"]
    fn the_update_server_answers() {
        let r = check(CheckContext::default());
        println!("{r:?}");
    }
}

//! The update transaction (PRD §9–§11): backup, install, complete rollback, commit, the
//! installation receipt, startup classification and cleanup.
//!
//! Layout, all inside the installation folder (same volume, so a rename is one `MoveFileEx`):
//!
//! ```text
//! <install>/.pdf-overlay-update/lock
//! <install>/.pdf-overlay-update/<txn>/journal.json      the journal (crate::journal)
//!                                    package.zip       the verified download
//!                                    new/…             extracted, verified files
//!                                    backup/…          the files the update replaces
//!                                    restore-or-apply.exe   the helper (a copy of the old app)
//!                                    verify.json       the new build's confirmation
//!                                    RECOVER.txt       manual recovery steps
//! <install>/update-receipt.json                         the last committed update
//! ```
//!
//! Several files can't be replaced atomically as a group; the journal plus hash-checked,
//! idempotent rollback make every interruption recoverable instead. Rollback acts on what is on
//! disk: a file is restored only when it differs from its backup's hash, and a file the update
//! introduced is removed only when it still holds exactly the new content.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::envelope::TrustedKey;
use crate::journal::{Entry, FORMAT, Journal, State};
use crate::manifest::Manifest;
use crate::ops::Ops;
use crate::{Error, envelope, hash, journal, paths};

/// The receipt's file name in the installation folder.
pub const RECEIPT: &str = "update-receipt.json";
/// The helper copy's name in its transaction folder.
pub const HELPER: &str = "restore-or-apply.exe";
/// The new build's confirmation.
pub const VERIFY: &str = "verify.json";
/// Manual recovery steps.
pub const RECOVER: &str = "RECOVER.txt";
/// The verified download.
pub const PACKAGE: &str = "package.zip";

/// Where one transaction lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The installation folder (the folder of `pdfcraft.exe`).
    pub install: PathBuf,
    /// `<install>/.pdf-overlay-update/<txn>`.
    pub txn: PathBuf,
}

impl Layout {
    pub fn new(install: &Path, txn_id: &str) -> Layout {
        Layout { install: install.to_path_buf(), txn: updates_root(install).join(txn_id) }
    }

    /// The layout of an existing transaction folder (its parent's parent is the installation).
    pub fn from_txn_dir(txn: &Path) -> Option<Layout> {
        let root = txn.parent()?;
        if root.file_name()? != paths::UPDATE_DIR {
            return None;
        }
        journal::valid_txn_id(txn.file_name()?.to_str()?)
            .then(|| Layout { install: root.parent().map(Path::to_path_buf).unwrap_or_default(), txn: txn.to_path_buf() })
    }

    pub fn new_dir(&self) -> PathBuf {
        self.txn.join("new")
    }
    pub fn backup_dir(&self) -> PathBuf {
        self.txn.join("backup")
    }
    pub fn helper(&self) -> PathBuf {
        self.txn.join(HELPER)
    }
    pub fn verify_file(&self) -> PathBuf {
        self.txn.join(VERIFY)
    }
    pub fn package(&self) -> PathBuf {
        self.txn.join(PACKAGE)
    }
    /// `rel` (with `/` separators) under `base`.
    fn under(base: &Path, rel: &str) -> PathBuf {
        rel.split('/').fold(base.to_path_buf(), |p, c| p.join(c))
    }
    pub fn installed(&self, rel: &str) -> PathBuf {
        Layout::under(&self.install, rel)
    }
    pub fn staged(&self, rel: &str) -> PathBuf {
        Layout::under(&self.new_dir(), rel)
    }
    pub fn backed_up(&self, rel: &str) -> PathBuf {
        Layout::under(&self.backup_dir(), rel)
    }
}

/// `<install>/.pdf-overlay-update`.
pub fn updates_root(install: &Path) -> PathBuf {
    install.join(paths::UPDATE_DIR)
}

/// A new transaction id from the time and a random number.
pub fn new_txn_id(now: DateTime<Utc>, random: u64) -> String {
    format!("txn-{}-{random:016x}", now.format("%Y%m%d%H%M%S"))
}

/// Start a transaction for a verified manifest: create its folder and a `Prepared` journal with
/// no files yet ([`plan`] fills them in after extraction).
pub fn begin(
    install: &Path,
    txn_id: &str,
    manifest: &Manifest,
    envelope: &str,
    old_commit: Option<&str>,
    nonce: &str,
    ops: &mut dyn Ops,
) -> Result<(Layout, Journal), Error> {
    if !journal::valid_txn_id(txn_id) {
        return Err(Error::Io(format!("bad transaction id {txn_id:?}")));
    }
    let layout = Layout::new(install, txn_id);
    let root = updates_root(install);
    ops.create_dir_all(&root).map_err(|e| Error::io("couldn't create", &root, &e))?;
    // The transaction folder must be new: never reuse or write through something planted.
    std::fs::create_dir(&layout.txn).map_err(|e| Error::io("couldn't create", &layout.txn, &e))?;
    let j = Journal {
        format: FORMAT,
        txn_id: txn_id.to_string(),
        state: State::Prepared,
        nonce: nonce.to_string(),
        helper_sha256: None,
        old_commit: old_commit.map(str::to_string),
        new_commit: manifest.commit.clone(),
        new_seq: manifest.seq,
        distribution_version: manifest.distribution_version.clone(),
        envelope: envelope.to_string(),
        files: Vec::new(),
        created_dirs: Vec::new(),
        new_pid: None,
        note: None,
    };
    j.save(&layout.txn, ops)?;
    write_recover_text(&layout, ops);
    Ok((layout, j))
}

/// After extraction: record, for every replaceable file of the package, whether it exists now and
/// its hash. A folder or link where a file should be refuses the update.
pub fn plan(layout: &Layout, j: &mut Journal, manifest: &Manifest, ops: &mut dyn Ops) -> Result<(), Error> {
    let mut files = Vec::new();
    for f in manifest.package.files.iter().filter(|f| paths::is_replaceable(&f.path)) {
        let target = layout.installed(&f.path);
        let old = hash::file_opt(&target).map_err(|e| Error::io("can't replace", &target, &e))?;
        files.push(Entry {
            path: f.path.clone(),
            existed_before: old.is_some(),
            introduced: old.is_none(),
            old_sha256: old,
            backup_ok: false,
            new_sha256: f.sha256.clone(),
            installed: false,
        });
    }
    files.sort_by(|a, b| paths::install_rank(&a.path).cmp(&paths::install_rank(&b.path)).then(a.path.cmp(&b.path)));
    j.files = files;
    j.save(&layout.txn, ops)
}

/// Re-hash the staged files (between verification and use, something may have changed them).
pub fn verify_staged(layout: &Layout, j: &Journal) -> Result<(), Error> {
    for e in &j.files {
        let p = layout.staged(&e.path);
        let got = hash::file(&p).map_err(|err| Error::io("missing staged file", &p, &err))?;
        if got != e.new_sha256 {
            return Err(Error::Package(format!("staged {} changed since it was verified", e.path)));
        }
    }
    Ok(())
}

/// Record the helper copy's hash (the app does this after copying itself).
pub fn record_helper(layout: &Layout, j: &mut Journal, ops: &mut dyn Ops) -> Result<(), Error> {
    let h = hash::file(&layout.helper()).map_err(|e| Error::io("couldn't read", &layout.helper(), &e))?;
    j.helper_sha256 = Some(h);
    j.save(&layout.txn, ops)
}

/// Whether the helper copy is the one the app recorded.
pub fn helper_intact(layout: &Layout, j: &Journal) -> bool {
    match (&j.helper_sha256, hash::file(&layout.helper())) {
        (Some(want), Ok(got)) => *want == got,
        _ => false,
    }
}

/// Copy every file about to be replaced into `backup/`, flushed and re-hashed. Nothing installed
/// changes here.
pub fn backup(layout: &Layout, j: &mut Journal, ops: &mut dyn Ops) -> Result<(), Error> {
    j.set(State::BackingUp, &layout.txn, ops)?;
    for i in 0..j.files.len() {
        let Some(e) = j.files.get(i).cloned() else { continue };
        if !e.existed_before || e.backup_ok {
            continue;
        }
        let from = layout.installed(&e.path);
        let to = layout.backed_up(&e.path);
        if let Some(parent) = to.parent() {
            ops.create_dir_all(parent).map_err(|err| Error::io("couldn't create", parent, &err))?;
        }
        ops.copy(&from, &to).map_err(|err| Error::io("couldn't back up", &from, &err))?;
        let got = hash::file(&to).map_err(|err| Error::io("couldn't read", &to, &err))?;
        if Some(&got) != e.old_sha256.as_ref() {
            return Err(Error::Io(format!("{} changed while it was backed up", e.path)));
        }
        if let Some(entry) = j.files.get_mut(i) {
            entry.backup_ok = true;
        }
        j.save(&layout.txn, ops)?;
    }
    Ok(())
}

/// Move every staged file into place, the application last, journaling each one.
pub fn install(layout: &Layout, j: &mut Journal, ops: &mut dyn Ops) -> Result<(), Error> {
    if j.files.iter().any(|e| e.existed_before && !e.backup_ok) {
        return Err(Error::Io("not every file was backed up".into()));
    }
    j.set(State::Installing, &layout.txn, ops)?;
    for i in 0..j.files.len() {
        let Some(e) = j.files.get(i).cloned() else { continue };
        if e.installed {
            continue;
        }
        let target = layout.installed(&e.path);
        if let Some(parent) = e.path.rsplit_once('/').map(|(dir, _)| dir) {
            let mut sofar = String::new();
            for part in parent.split('/') {
                if !sofar.is_empty() {
                    sofar.push('/');
                }
                sofar.push_str(part);
                let dir = layout.installed(&sofar);
                if !dir.exists() {
                    if !journal::creatable_dir(&sofar) {
                        return Err(Error::Io(format!("won't create folder {sofar:?}")));
                    }
                    // Journal the folder before creating it, so a rollback knows to remove it.
                    if !j.created_dirs.contains(&sofar) {
                        j.created_dirs.push(sofar.clone());
                        j.save(&layout.txn, ops)?;
                    }
                    ops.create_dir_all(&dir).map_err(|err| Error::io("couldn't create", &dir, &err))?;
                }
            }
        }
        ops.rename(&layout.staged(&e.path), &target).map_err(|err| Error::io("couldn't replace", &target, &err))?;
        if let Some(entry) = j.files.get_mut(i) {
            entry.installed = true;
        }
        j.save(&layout.txn, ops)?;
    }
    j.set(State::InstalledUnverified, &layout.txn, ops)
}

/// What a rollback left behind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RollbackReport {
    /// Files that couldn't be restored or removed, with why.
    pub problems: Vec<String>,
}

/// Restore the previous installation: every pre-existing file back to its backed-up content,
/// every introduced file removed (only while it still holds the new content), created folders
/// removed when empty. Idempotent: running it again after an interruption converges. Must only
/// run when no process uses the installation (the helper's job).
pub fn rollback(layout: &Layout, j: &mut Journal, ops: &mut dyn Ops) -> RollbackReport {
    let mut report = RollbackReport::default();
    if let Err(e) = j.set(State::RollingBack, &layout.txn, ops) {
        report.problems.push(format!("journal: {e}"));
    }
    for e in j.files.clone() {
        let target = layout.installed(&e.path);
        let current = match hash::file_opt(&target) {
            Ok(h) => h,
            Err(err) => {
                report.problems.push(format!("{}: {err}", e.path));
                continue;
            }
        };
        if e.existed_before {
            if current.is_some() && current == e.old_sha256 {
                continue;
            }
            let backup = layout.backed_up(&e.path);
            let backup_good = e.backup_ok && hash::file(&backup).ok() == e.old_sha256;
            if !backup_good {
                report.problems.push(format!("{}: no intact backup", e.path));
                continue;
            }
            if let Err(err) = restore(&backup, &target, e.old_sha256.as_deref(), ops) {
                report.problems.push(format!("{}: {err}", e.path));
            }
        } else if let Some(h) = current {
            if h == e.new_sha256 {
                if let Err(err) = ops.remove_file(&target) {
                    report.problems.push(format!("{}: {err}", e.path));
                }
            } else {
                // Not what the update put there: someone else's file, left alone.
                report.problems.push(format!("{}: unexpected content left in place", e.path));
            }
        }
    }
    // Deepest first; a folder that still holds anything (not ours) stays.
    let mut dirs = j.created_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs {
        let dir = layout.installed(&d);
        if dir.is_dir() && std::fs::read_dir(&dir).map(|mut r| r.next().is_none()).unwrap_or(false) {
            let _ = ops.remove_dir(&dir);
        }
    }
    let state = if report.problems.is_empty() { State::RolledBack } else { State::RecoveryRequired };
    if !report.problems.is_empty() {
        j.note = Some(report.problems.join("; "));
    }
    if let Err(e) = j.set(state, &layout.txn, ops) {
        report.problems.push(format!("journal: {e}"));
    }
    report
}

/// Put `backup` back at `target` through a temporary file beside it, then check it.
fn restore(backup: &Path, target: &Path, want: Option<&str>, ops: &mut dyn Ops) -> Result<(), Error> {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = target.with_file_name(format!("{name}.pdf-overlay-restore"));
    if let Some(parent) = target.parent() {
        ops.create_dir_all(parent).map_err(|e| Error::io("couldn't create", parent, &e))?;
    }
    ops.copy(backup, &tmp).map_err(|e| Error::io("couldn't restore", target, &e))?;
    ops.rename(&tmp, target).map_err(|e| Error::io("couldn't restore", target, &e))?;
    let got = hash::file(target).map_err(|e| Error::io("couldn't read", target, &e))?;
    if Some(got.as_str()) != want {
        return Err(Error::Io(format!("{} doesn't match its backup after restoring", target.display())));
    }
    Ok(())
}

/// The new build confirmed that it started: write the receipt and finish. Program files aren't
/// touched; the staged leftovers are removed (the backup stays as the previous version).
pub fn commit(layout: &Layout, j: &mut Journal, now: DateTime<Utc>, ops: &mut dyn Ops) -> Result<(), Error> {
    j.set(State::Verified, &layout.txn, ops)?;
    let receipt = json!({ "format": 1, "envelope": j.envelope, "installed_at": now.to_rfc3339(), "txn_id": j.txn_id });
    let path = layout.install.join(RECEIPT);
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|e| Error::Io(e.to_string()))?;
    ops.write_atomic(&path, &bytes).map_err(|e| Error::io("couldn't write", &path, &e))?;
    j.note = None;
    j.set(State::Committed, &layout.txn, ops)?;
    let _ = std::fs::remove_dir_all(layout.new_dir());
    let _ = std::fs::remove_file(layout.package());
    Ok(())
}

/// The sequence of the update installed by UPD.1, if the receipt is genuine and describes the
/// running build; 0 otherwise (a manual install, or a receipt that doesn't match).
pub fn installed_seq(install: &Path, keys: &[TrustedKey], own_commit: &str) -> u64 {
    let path = install.join(RECEIPT);
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return 0 };
    if !meta.is_file() || meta.len() > 512 * 1024 {
        return 0;
    }
    let Ok(bytes) = std::fs::read(&path) else { return 0 };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return 0 };
    let Some(env) = field(&v, "envelope").as_str() else { return 0 };
    let Ok(manifest) = envelope::open(env.as_bytes(), keys) else { return 0 };
    let Ok(m) = Manifest::parse(&manifest) else { return 0 };
    if m.commit == own_commit { m.seq } else { 0 }
}

/// What a normal start does about a transaction it finds (PRD §11). The running app never
/// changes installed program files: `Delegate` means "start the helper and exit".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupAction {
    /// Nothing pending.
    Nothing,
    /// Prepared but never started: remove the staging folder.
    Discard,
    /// Files must be restored: launch the verified helper, then exit.
    Delegate,
    /// This is the new build after an unverified install: verify here, commit on success.
    VerifyHere,
    /// Verified but not committed: write the receipt.
    Commit,
    /// Automatic recovery isn't possible: show the manual steps.
    Manual,
}

/// Classify a transaction for a start of the build with commit `own_commit`.
pub fn startup_action(j: &Journal, own_commit: Option<&str>, helper_ok: bool) -> StartupAction {
    let delegate = if helper_ok { StartupAction::Delegate } else { StartupAction::Manual };
    match j.state {
        State::Committed | State::RolledBack => StartupAction::Nothing,
        State::Prepared => StartupAction::Discard,
        State::BackingUp | State::Installing | State::RollingBack => delegate,
        State::InstalledUnverified if own_commit == Some(j.new_commit.as_str()) => StartupAction::VerifyHere,
        State::InstalledUnverified => delegate,
        State::Verified => StartupAction::Commit,
        State::RecoveryRequired => StartupAction::Manual,
    }
}

/// The transaction folders under `install`, newest id last.
pub fn transactions(install: &Path) -> Vec<Layout> {
    let Ok(dir) = std::fs::read_dir(updates_root(install)) else { return Vec::new() };
    let mut found: Vec<Layout> = dir
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().filter(|n| journal::valid_txn_id(n)).map(|n| Layout::new(install, n)))
        .collect();
    found.sort_by(|a, b| a.txn.cmp(&b.txn));
    found
}

/// Remove finished transactions, keeping the newest committed one's backup (the previous
/// version) and, for a week, rolled-back ones (their logs). Only the updater's own folder is
/// touched. Call while holding the lock.
pub fn cleanup(install: &Path, now: DateTime<Utc>) {
    let all = transactions(install);
    let newest_committed = all.iter().rev().find(|l| Journal::load(&l.txn).is_ok_and(|j| j.state == State::Committed)).cloned();
    for l in &all {
        let Ok(j) = Journal::load(&l.txn) else { continue };
        let remove = match j.state {
            State::Committed => Some(l) != newest_committed.as_ref(),
            State::RolledBack => modified(&l.txn).is_none_or(|t| now.signed_duration_since(t).num_days() >= 7),
            _ => false,
        };
        if remove {
            let _ = std::fs::remove_dir_all(&l.txn);
        } else if j.state == State::Committed {
            let _ = std::fs::remove_dir_all(l.new_dir());
            let _ = std::fs::remove_file(l.package());
            let _ = std::fs::remove_file(l.helper());
        }
    }
}

fn modified(p: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(p.join(journal::FILE)).and_then(|m| m.modified()).ok().map(DateTime::<Utc>::from)
}

/// Remove a transaction that never started installing.
pub fn discard(layout: &Layout) {
    if let Ok(j) = Journal::load(&layout.txn)
        && j.state != State::Prepared
    {
        return;
    }
    let _ = std::fs::remove_dir_all(&layout.txn);
}

/// What the new build writes once it has started properly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    pub nonce: String,
    pub commit: String,
    pub pid: u32,
    pub ok: bool,
    pub reason: Option<String>,
}

pub fn write_verification(layout: &Layout, v: &Verification, ops: &mut dyn Ops) -> Result<(), Error> {
    let body = json!({ "nonce": v.nonce, "commit": v.commit, "pid": v.pid, "ok": v.ok, "reason": v.reason });
    let path = layout.verify_file();
    let bytes = serde_json::to_vec(&body).map_err(|e| Error::Io(e.to_string()))?;
    ops.write_atomic(&path, &bytes).map_err(|e| Error::io("couldn't write", &path, &e))
}

pub fn read_verification(layout: &Layout) -> Option<Verification> {
    let bytes = std::fs::read(layout.verify_file()).ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some(Verification {
        nonce: field(&v, "nonce").as_str()?.to_string(),
        commit: field(&v, "commit").as_str()?.to_string(),
        pid: u32::try_from(field(&v, "pid").as_u64()?).ok()?,
        ok: field(&v, "ok").as_bool()?,
        reason: field(&v, "reason").as_str().map(str::to_string),
    })
}

/// Manual recovery steps, in English and Spanish, beside the backup.
fn write_recover_text(layout: &Layout, ops: &mut dyn Ops) {
    let text = format!(
        "PDF Overlay update {txn}\r\n\r\n\
         If PDF Overlay doesn't start after an update:\r\n\
         1. Close every PDF Overlay window.\r\n\
         2. Double-click {helper} in this folder. It restores the previous version and starts it.\r\n\
         3. If that doesn't work: copy every file from the backup folder here into the PDF Overlay\r\n\
            folder (two levels up), replacing the files there, and delete the files that\r\n\
            journal.json lists with \"introduced\": true.\r\n\
         Your settings and documents (PdfCraftData, portable.txt) are never changed by an update.\r\n\r\n\
         Si PDF Overlay no se inicia después de una actualización:\r\n\
         1. Cierre todas las ventanas de PDF Overlay.\r\n\
         2. Haga doble clic en {helper} en esta carpeta. Restaura la versión anterior y la inicia.\r\n\
         3. Si no funciona: copie todos los archivos de la carpeta backup a la carpeta de\r\n\
            PDF Overlay (dos niveles más arriba), reemplazando los que haya, y elimine los archivos\r\n\
            que journal.json indica con \"introduced\": true.\r\n\
         Una actualización nunca modifica su configuración ni sus documentos (PdfCraftData, portable.txt).\r\n",
        txn = layout.txn.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        helper = HELPER,
    );
    let _ = ops.write_atomic(&layout.txn.join(RECOVER), text.as_bytes());
}

/// Only one update at a time per installation: `<install>/.pdf-overlay-update/lock` holds the
/// owner's process id. A lock whose process is gone is taken over.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
    held: bool,
}

impl Lock {
    pub fn acquire(install: &Path, pid: u32, alive: &dyn Fn(u32) -> bool) -> Result<Lock, Error> {
        let root = updates_root(install);
        std::fs::create_dir_all(&root).map_err(|e| Error::io("couldn't create", &root, &e))?;
        let path = root.join("lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    use std::io::Write as _;
                    f.write_all(pid.to_string().as_bytes()).map_err(|e| Error::io("couldn't write", &path, &e))?;
                    return Ok(Lock { path, held: true });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let owner = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok());
                    match owner {
                        Some(o) if o == pid => return Ok(Lock { path, held: true }),
                        Some(o) if alive(o) => return Err(Error::Busy("another update of this PDF Overlay is in progress".into())),
                        _ => {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                Err(e) => return Err(Error::io("couldn't create", &path, &e)),
            }
        }
        Err(Error::Busy("another update of this PDF Overlay is in progress".into()))
    }

    /// Hand the lock to a helper that will take it over once this process has exited.
    pub fn hand_over(mut self) {
        self.held = false;
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if self.held {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A JSON object's field, `null` when absent (never panics, unlike indexing).
fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

#[cfg(test)]
pub(crate) mod tests;

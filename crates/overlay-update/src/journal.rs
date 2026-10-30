//! The transaction journal (PRD §10): one JSON file per update, rewritten through a flushed
//! temporary file and a rename after every step, so an interrupted update can be finished or
//! undone. It is read back as untrusted input: every path in it must still be a replaceable
//! application file, so a damaged or planted journal can't make the helper touch anything else.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ops::Ops;
use crate::{Error, manifest, paths};

/// The journal's file name inside its transaction folder.
pub const FILE: &str = "journal.json";
/// The largest journal read.
const MAX_JOURNAL: u64 = 1024 * 1024;
/// The journal format this crate writes.
pub const FORMAT: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Downloaded, verified and extracted; no installed file touched.
    Prepared,
    /// Copying the files about to be replaced.
    BackingUp,
    /// Moving new files into place.
    Installing,
    /// Every file is in place; the new build hasn't confirmed that it starts.
    InstalledUnverified,
    /// The new build confirmed that it starts; the receipt isn't written yet.
    Verified,
    /// Done. `backup/` keeps the previous version.
    Committed,
    /// Restoring the previous files.
    RollingBack,
    /// The previous files are back.
    RolledBack,
    /// Automatic recovery couldn't finish: the user follows `RECOVER.txt`.
    RecoveryRequired,
}

impl State {
    /// No further work is pending.
    pub fn terminal(self) -> bool {
        matches!(self, State::Committed | State::RolledBack)
    }
}

/// One replaceable file of the update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative to the installation folder, `/` separators.
    pub path: String,
    /// A file was there before the update.
    pub existed_before: bool,
    /// Its hash then.
    pub old_sha256: Option<String>,
    /// `backup/<path>` was written, flushed and found equal to `old_sha256`.
    pub backup_ok: bool,
    /// The signed hash of the new file.
    pub new_sha256: String,
    /// The update creates this file (it didn't exist before).
    pub introduced: bool,
    /// The new file has been moved into place.
    pub installed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub format: u32,
    pub txn_id: String,
    pub state: State,
    /// Random, written by the app; the new build must echo it to confirm that it started.
    pub nonce: String,
    /// The hash of the helper copy, recorded by the app that made it.
    pub helper_sha256: Option<String>,
    /// The commit of the build that prepared the update (`None` for a development build).
    pub old_commit: Option<String>,
    pub new_commit: String,
    pub new_seq: u64,
    pub distribution_version: String,
    /// The accepted channel envelope, copied into the receipt at commit.
    pub envelope: String,
    pub files: Vec<Entry>,
    /// Folders the install created (removed again by a rollback when empty).
    #[serde(default)]
    pub created_dirs: Vec<String>,
    /// The new build's process, while it is being verified.
    #[serde(default)]
    pub new_pid: Option<u32>,
    /// The last failure, for the log and the result dialog.
    #[serde(default)]
    pub note: Option<String>,
}

/// A transaction id: `txn-` and up to 60 plain characters.
pub fn valid_txn_id(id: &str) -> bool {
    id.len() <= 64 && id.strip_prefix("txn-").is_some_and(|r| !r.is_empty() && r.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
}

/// A folder an install may create: the licence folders the package carries.
pub fn creatable_dir(path: &str) -> bool {
    matches!(path, "licenses" | "licenses/epaint_default_fonts")
}

impl Journal {
    /// Read and check the journal in `txn_dir`.
    pub fn load(txn_dir: &Path) -> Result<Journal, Error> {
        let path = txn_dir.join(FILE);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| Error::io("couldn't read", &path, &e))?;
        if !meta.is_file() || meta.len() > MAX_JOURNAL {
            return Err(Error::Io(format!("{} isn't a journal", path.display())));
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io("couldn't read", &path, &e))?;
        let j: Journal = serde_json::from_slice(&bytes).map_err(|e| Error::Io(format!("{} is damaged: {e}", path.display())))?;
        j.check()?;
        Ok(j)
    }

    fn check(&self) -> Result<(), Error> {
        let bad = |why: String| Err(Error::Io(format!("the update journal is damaged: {why}")));
        if self.format != FORMAT {
            return bad(format!("format {}", self.format));
        }
        if !valid_txn_id(&self.txn_id) {
            return bad("transaction id".into());
        }
        if !manifest::is_commit(&self.new_commit) || self.old_commit.as_deref().is_some_and(|c| !manifest::is_commit(c)) {
            return bad("commit".into());
        }
        if self.files.len() > manifest::MAX_FILES {
            return bad("too many files".into());
        }
        let mut seen = std::collections::HashSet::new();
        for e in &self.files {
            paths::check_relative(&e.path).map_err(|e| Error::Io(format!("the update journal is damaged: {e}")))?;
            if !paths::is_replaceable(&e.path) || !seen.insert(paths::fold(&e.path)) {
                return bad(format!("{:?} isn't a replaceable file", e.path));
            }
            if !manifest::is_sha256(&e.new_sha256) || e.old_sha256.as_deref().is_some_and(|h| !manifest::is_sha256(h)) {
                return bad(format!("hash of {:?}", e.path));
            }
            if e.existed_before == e.introduced || e.existed_before != e.old_sha256.is_some() {
                return bad(format!("state of {:?}", e.path));
            }
        }
        if self.created_dirs.iter().any(|d| !creatable_dir(d)) {
            return bad("created folder".into());
        }
        Ok(())
    }

    /// Write the journal into `txn_dir`.
    pub fn save(&self, txn_dir: &Path, ops: &mut dyn Ops) -> Result<(), Error> {
        let path = txn_dir.join(FILE);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| Error::Io(e.to_string()))?;
        ops.write_atomic(&path, &bytes).map_err(|e| Error::io("couldn't write", &path, &e))
    }

    /// Move to `state` and save.
    pub fn set(&mut self, state: State, txn_dir: &Path, ops: &mut dyn Ops) -> Result<(), Error> {
        self.state = state;
        self.save(txn_dir, ops)
    }
}

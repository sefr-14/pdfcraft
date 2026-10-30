//! The external helper's work (PRD §9–§11), independent of how processes are listed or started:
//! [`apply`] installs a prepared update after the app has quit, launches the new build and
//! commits or rolls back; [`recover`] finishes or undoes an interrupted transaction. Both only
//! touch installed files once no process runs from the installation folder, so an executable in
//! use is never replaced or restored.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::journal::{Journal, State};
use crate::ops::Ops;
use crate::paths;
use crate::transaction::{self, Layout};

/// The processes on this computer, as far as the helper needs them.
pub trait Processes {
    /// Whether process `pid` still runs.
    fn alive(&self, pid: u32) -> bool;
    /// Processes, other than `except`, whose executable is directly in `install` (the app, the
    /// command-line tool). The helper runs from a subfolder and isn't one of them.
    fn using(&self, install: &Path, except: &[u32]) -> Vec<u32>;
}

/// A process the helper started.
pub trait Launched {
    fn pid(&self) -> u32;
    /// `None` while it runs, else whether it exited successfully.
    fn exited(&mut self) -> Option<bool>;
}

/// Starts processes.
pub trait Launcher {
    fn launch(&mut self, exe: &Path, args: &[OsString]) -> io::Result<Box<dyn Launched>>;
}

/// How long the helper waits for things.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// For the app (and every other process using the installation) to exit.
    pub wait_exit: Duration,
    /// For the new build to confirm that it started.
    pub verify: Duration,
    /// How long the new build is watched after confirming.
    pub observe: Duration,
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            wait_exit: Duration::from_secs(60),
            verify: Duration::from_secs(120),
            observe: Duration::from_secs(10),
            poll: Duration::from_millis(250),
        }
    }
}

/// How an update or a recovery ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The new build runs and confirmed that it started; the update is final.
    Committed,
    /// Nothing installed changed.
    NotStarted(String),
    /// The previous version is back.
    RolledBack(String),
    /// Automatic recovery couldn't finish; `RECOVER.txt` has the manual steps.
    RecoveryRequired(String),
    /// The new build still runs without confirming; the next start of PDF Overlay decides.
    Unverified,
}

impl Outcome {
    /// Whether the helper should start the (previous or restored) application to report.
    pub fn relaunch(&self) -> bool {
        matches!(self, Outcome::NotStarted(_) | Outcome::RolledBack(_) | Outcome::RecoveryRequired(_))
    }
}

/// What the helper works with.
pub struct Env<'a> {
    pub procs: &'a dyn Processes,
    pub launcher: &'a mut dyn Launcher,
    pub ops: &'a mut dyn Ops,
    pub timing: Timing,
    /// The helper's own process id.
    pub me: u32,
    pub now: &'a dyn Fn() -> DateTime<Utc>,
    pub log: &'a mut dyn FnMut(&str),
}

/// Wait until none of `pids` runs and nothing else uses the installation.
pub fn wait_until_free(install: &Path, pids: &[u32], env: &mut Env<'_>) -> bool {
    let deadline = Instant::now() + env.timing.wait_exit;
    loop {
        let waiting: Vec<u32> = pids.iter().copied().filter(|&p| p != 0 && p != env.me && env.procs.alive(p)).collect();
        let others = env.procs.using(install, &[env.me]);
        if waiting.is_empty() && others.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            (env.log)(&format!("still running: {waiting:?}, using the installation: {others:?}"));
            return false;
        }
        std::thread::sleep(env.timing.poll);
    }
}

const STILL_RUNNING: &str = "PDF Overlay (or its command-line tool) is still running, so the update wasn't installed";

/// Install the prepared update in `layout` once `parent` (the app that prepared it) has exited.
pub fn apply(layout: &Layout, parent: u32, env: &mut Env<'_>) -> Outcome {
    let mut j = match Journal::load(&layout.txn) {
        Ok(j) => j,
        Err(e) => return Outcome::NotStarted(e.to_string()),
    };
    if j.state != State::Prepared {
        return Outcome::NotStarted(format!("the update is {:?}, not ready to install", j.state));
    }
    if !wait_until_free(&layout.install, &[parent], env) {
        return not_started(layout, &mut j, STILL_RUNNING.into(), env);
    }
    if let Err(e) = transaction::verify_staged(layout, &j) {
        return not_started(layout, &mut j, e.to_string(), env);
    }
    if let Err(e) = transaction::backup(layout, &mut j, env.ops) {
        return not_started(layout, &mut j, e.to_string(), env);
    }
    (env.log)("backup complete; installing");
    if let Err(e) = transaction::install(layout, &mut j, env.ops) {
        (env.log)(&format!("install failed: {e}"));
        return roll_back(layout, &mut j, &[], format!("a file couldn't be replaced ({e})"), env);
    }
    (env.log)("installed; starting the new version");
    let exe = layout.installed(paths::APP_EXE);
    let args: Vec<OsString> = vec!["--overlay-update-verify".into(), layout.txn.clone().into_os_string(), j.nonce.clone().into()];
    let mut child = match env.launcher.launch(&exe, &args) {
        Ok(c) => c,
        Err(e) => return roll_back(layout, &mut j, &[], format!("the new version couldn't be started ({e})"), env),
    };
    j.new_pid = Some(child.pid());
    let _ = j.save(&layout.txn, env.ops);
    let deadline = Instant::now() + env.timing.verify;
    let verdict = loop {
        if let Some(v) = transaction::read_verification(layout) {
            break Some(v);
        }
        if child.exited().is_some() {
            break transaction::read_verification(layout);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(env.timing.poll);
    };
    let pid = child.pid();
    match verdict {
        Some(v) if v.ok && v.nonce == j.nonce && v.commit == j.new_commit => {
            let until = Instant::now() + env.timing.observe;
            while Instant::now() < until {
                match child.exited() {
                    Some(false) => return roll_back(layout, &mut j, &[pid], "the new version stopped unexpectedly after starting".into(), env),
                    Some(true) => break,
                    None => std::thread::sleep(env.timing.poll),
                }
            }
            match transaction::commit(layout, &mut j, (env.now)(), env.ops) {
                Ok(()) => (env.log)("update committed"),
                // Verified: the next start writes the receipt.
                Err(e) => (env.log)(&format!("verified, but the receipt couldn't be written yet: {e}")),
            }
            Outcome::Committed
        }
        Some(v) => {
            let why = if v.nonce != j.nonce || v.commit != j.new_commit {
                "the new version didn't identify itself correctly".to_string()
            } else {
                v.reason.unwrap_or_else(|| "the new version reported a problem".into())
            };
            roll_back(layout, &mut j, &[pid], why, env)
        }
        None if child.exited().is_some() => roll_back(layout, &mut j, &[pid], "the new version closed before confirming that it started".into(), env),
        None => {
            (env.log)("the new version runs but hasn't confirmed; leaving the decision to its next start");
            j.note = Some("the new version didn't confirm in time".into());
            let _ = j.save(&layout.txn, env.ops);
            Outcome::Unverified
        }
    }
}

fn not_started(layout: &Layout, j: &mut Journal, why: String, env: &mut Env<'_>) -> Outcome {
    (env.log)(&format!("not installed: {why}"));
    j.note = Some(why.clone());
    // Nothing installed changed: the staging can go (startup discards Prepared transactions).
    let _ = j.set(State::Prepared, &layout.txn, env.ops);
    Outcome::NotStarted(why)
}

/// Roll back once `pids` have exited and nothing uses the installation; never while they run.
fn roll_back(layout: &Layout, j: &mut Journal, pids: &[u32], why: String, env: &mut Env<'_>) -> Outcome {
    if !wait_until_free(&layout.install, pids, env) {
        j.note = Some(format!("{why}; rollback waits until PDF Overlay is closed"));
        let _ = j.save(&layout.txn, env.ops);
        return Outcome::Unverified;
    }
    j.note = Some(why.clone());
    let report = transaction::rollback(layout, j, env.ops);
    if report.problems.is_empty() {
        (env.log)(&format!("rolled back: {why}"));
        Outcome::RolledBack(why)
    } else {
        let problems = report.problems.join("; ");
        (env.log)(&format!("rollback incomplete: {problems}"));
        Outcome::RecoveryRequired(problems)
    }
}

/// Finish or undo the interrupted transaction in `layout` once `parent` (the start that found
/// it) and every process using the installation have exited.
pub fn recover(layout: &Layout, parent: u32, env: &mut Env<'_>) -> Outcome {
    let mut j = match Journal::load(&layout.txn) {
        Ok(j) => j,
        Err(e) => return Outcome::RecoveryRequired(e.to_string()),
    };
    match j.state {
        State::Prepared => {
            transaction::discard(layout);
            Outcome::NotStarted("the prepared update was discarded".into())
        }
        State::Committed | State::RolledBack => Outcome::NotStarted("nothing to recover".into()),
        State::Verified => match transaction::commit(layout, &mut j, (env.now)(), env.ops) {
            Ok(()) => Outcome::Committed,
            Err(e) => Outcome::RecoveryRequired(e.to_string()),
        },
        State::BackingUp | State::Installing | State::InstalledUnverified | State::RollingBack | State::RecoveryRequired => {
            let pids: Vec<u32> = std::iter::once(parent).chain(j.new_pid).collect();
            if !wait_until_free(&layout.install, &pids, env) {
                return Outcome::NotStarted(STILL_RUNNING.replace("the update wasn't installed", "the interrupted update wasn't repaired yet"));
            }
            let why = j.note.clone().unwrap_or_else(|| "the update was interrupted".into());
            let report = transaction::rollback(layout, &mut j, env.ops);
            if report.problems.is_empty() {
                (env.log)("recovered: previous version restored");
                Outcome::RolledBack(why)
            } else {
                Outcome::RecoveryRequired(report.problems.join("; "))
            }
        }
    }
}

#[cfg(test)]
mod tests;

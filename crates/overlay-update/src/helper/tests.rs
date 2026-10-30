use super::*;
use crate::journal::Journal;
use crate::ops::{FaultyOps, Op};
use crate::transaction::tests::{OLD_COMMIT, assert_protected_untouched, installation, prepared, snapshot};
use crate::transaction::{RECEIPT, Verification};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

const PARENT: u32 = 10;
const CHILD: u32 = 77;
const OTHER: u32 = 999;

/// The fake computer: how long the old app and other users keep running, and the new build.
#[derive(Default)]
struct World {
    parent_polls: Cell<u32>,
    others_polls: Cell<u32>,
    child_alive: Cell<bool>,
    /// Polls until the child exits by itself (`None`: it keeps running).
    child_exit_after: Cell<Option<u32>>,
    child_exit_ok: Cell<bool>,
}

impl World {
    /// Whether the new build still runs, counting down to its own exit.
    fn child_running(&self) -> bool {
        if !self.child_alive.get() {
            return false;
        }
        match self.child_exit_after.get() {
            Some(0) => {
                self.child_alive.set(false);
                false
            }
            Some(n) => {
                self.child_exit_after.set(Some(n - 1));
                true
            }
            None => true,
        }
    }

    fn busy(&self) -> bool {
        self.parent_polls.get() > 0 || self.others_polls.get() > 0 || self.child_alive.get()
    }
}

struct Procs(Rc<World>);

impl Processes for Procs {
    fn alive(&self, pid: u32) -> bool {
        let w = &self.0;
        match pid {
            PARENT if w.parent_polls.get() > 0 => {
                w.parent_polls.set(w.parent_polls.get() - 1);
                true
            }
            CHILD => w.child_running(),
            _ => false,
        }
    }
    fn using(&self, _install: &Path, except: &[u32]) -> Vec<u32> {
        let w = &self.0;
        let mut v = Vec::new();
        if w.others_polls.get() > 0 {
            w.others_polls.set(w.others_polls.get() - 1);
            v.push(OTHER);
        }
        if !except.contains(&CHILD) && w.child_running() {
            v.push(CHILD);
        }
        v
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NewBuild {
    Confirms,
    ConfirmsThenCrashes,
    ExitsSilently,
    ReportsFailureAndExits,
    ReportsFailureAndStays,
    WrongNonce,
    Hangs,
    WontStart,
}

struct Child(Rc<World>);

impl Launched for Child {
    fn pid(&self) -> u32 {
        CHILD
    }
    fn exited(&mut self) -> Option<bool> {
        let w = &self.0;
        if w.child_running() { None } else { Some(w.child_exit_ok.get()) }
    }
}

struct Launch {
    world: Rc<World>,
    build: NewBuild,
    launched: Vec<(PathBuf, Vec<OsString>)>,
}

impl Launcher for Launch {
    fn launch(&mut self, exe: &Path, args: &[OsString]) -> io::Result<Box<dyn Launched>> {
        self.launched.push((exe.to_path_buf(), args.to_vec()));
        if self.build == NewBuild::WontStart {
            return Err(io::Error::other("not a valid application"));
        }
        let txn = PathBuf::from(&args[1]);
        let layout = Layout::from_txn_dir(&txn).unwrap();
        let j = Journal::load(&txn).unwrap();
        let mut v = Verification { nonce: j.nonce.clone(), commit: j.new_commit.clone(), pid: CHILD, ok: true, reason: None };
        let w = &self.world;
        w.child_alive.set(true);
        w.child_exit_ok.set(true);
        w.child_exit_after.set(None);
        match self.build {
            NewBuild::Confirms => {}
            NewBuild::ConfirmsThenCrashes => {
                w.child_exit_after.set(Some(2));
                w.child_exit_ok.set(false);
            }
            NewBuild::ExitsSilently => {
                w.child_exit_after.set(Some(1));
                w.child_exit_ok.set(false);
                return Ok(Box::new(Child(Rc::clone(w))));
            }
            NewBuild::ReportsFailureAndExits => {
                v.ok = false;
                v.reason = Some("settings couldn't be read".into());
                w.child_exit_after.set(Some(0));
            }
            NewBuild::ReportsFailureAndStays => {
                v.ok = false;
                v.reason = Some("settings couldn't be read".into());
            }
            NewBuild::WrongNonce => v.nonce = "guessed".into(),
            NewBuild::Hangs => return Ok(Box::new(Child(Rc::clone(w)))),
            NewBuild::WontStart => {}
        }
        crate::transaction::write_verification(&layout, &v, &mut FaultyOps::new()).unwrap();
        Ok(Box::new(Child(Rc::clone(w))))
    }
}

/// Records operations and fails the test if one changes an installed file while a process
/// still uses the installation: the rule the whole helper exists for.
struct Guarded {
    inner: FaultyOps,
    world: Rc<World>,
    install: PathBuf,
}

impl Guarded {
    fn check(&self, targets: &[&Path]) {
        for t in targets {
            let Ok(rel) = t.strip_prefix(&self.install) else { continue };
            let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            if first == crate::paths::UPDATE_DIR || first == RECEIPT {
                continue;
            }
            assert!(!self.world.busy(), "{} changed while PDF Overlay was running", t.display());
        }
    }
}

impl Ops for Guarded {
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.check(&[from, to]);
        self.inner.rename(from, to)
    }
    fn copy(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.check(&[to]);
        self.inner.copy(from, to)
    }
    fn remove_file(&mut self, p: &Path) -> io::Result<()> {
        self.check(&[p]);
        self.inner.remove_file(p)
    }
    fn create_dir_all(&mut self, p: &Path) -> io::Result<()> {
        self.check(&[p]);
        self.inner.create_dir_all(p)
    }
    fn remove_dir(&mut self, p: &Path) -> io::Result<()> {
        self.check(&[p]);
        self.inner.remove_dir(p)
    }
    fn write_atomic(&mut self, p: &Path, b: &[u8]) -> io::Result<()> {
        self.check(&[p]);
        self.inner.write_atomic(p, b)
    }
}

struct Run {
    outcome: Outcome,
    log: Vec<Op>,
    launched: Vec<(PathBuf, Vec<OsString>)>,
}

fn timing() -> Timing {
    Timing {
        wait_exit: Duration::from_millis(300),
        verify: Duration::from_millis(300),
        observe: Duration::from_millis(30),
        poll: Duration::from_millis(1),
    }
}

fn now() -> DateTime<Utc> {
    crate::manifest::time("2026-11-10T00:00:00Z").unwrap()
}

fn run(install: &Path, layout: &Layout, world: Rc<World>, build: NewBuild, recovering: bool) -> Run {
    let procs = Procs(Rc::clone(&world));
    let mut launcher = Launch { world: Rc::clone(&world), build, launched: Vec::new() };
    let mut ops = Guarded { inner: FaultyOps::new(), world, install: install.to_path_buf() };
    let mut lines = Vec::new();
    let mut log = |l: &str| lines.push(l.to_string());
    let mut env = Env { procs: &procs, launcher: &mut launcher, ops: &mut ops, timing: timing(), me: 1, now: &now, log: &mut log };
    let outcome = if recovering { recover(layout, PARENT, &mut env) } else { apply(layout, PARENT, &mut env) };
    Run { outcome, log: ops.inner.log, launched: launcher.launched }
}

fn setup() -> (PathBuf, Layout, std::collections::BTreeMap<String, Vec<u8>>) {
    let install = installation();
    let before = snapshot(&install);
    let (layout, _, _) = prepared(&install, &mut FaultyOps::new());
    (install, layout, before)
}

fn state(layout: &Layout) -> State {
    Journal::load(&layout.txn).unwrap().state
}

#[test]
fn an_update_installs_after_the_app_exits_and_commits_once_confirmed() {
    let (install, layout, before) = setup();
    let world = Rc::new(World { parent_polls: Cell::new(5), ..World::default() });
    let r = run(&install, &layout, world, NewBuild::Confirms, false);
    assert_eq!(r.outcome, Outcome::Committed);
    assert_eq!(state(&layout), State::Committed);
    assert_eq!(std::fs::read(install.join("pdfcraft.exe")).unwrap(), b"new app");
    assert!(install.join(RECEIPT).is_file());
    let (exe, args) = &r.launched[0];
    assert_eq!(exe, &install.join("pdfcraft.exe"));
    assert_eq!(args[0], OsString::from("--overlay-update-verify"));
    for p in ["portable.txt", "PdfCraftData/app.ron", "my drawing.pdf"] {
        assert_eq!(snapshot(&install).get(p), before.get(p));
    }
    assert_protected_untouched(&install, &r.log);
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn failures_after_installing_restore_the_previous_version() {
    for build in [NewBuild::ExitsSilently, NewBuild::ReportsFailureAndExits, NewBuild::WrongNonce, NewBuild::ConfirmsThenCrashes, NewBuild::WontStart]
    {
        let (install, layout, before) = setup();
        let world = Rc::new(World::default());
        let r = run(&install, &layout, Rc::clone(&world), build, false);
        // WrongNonce: the build runs; rollback waits for it, so make it exit.
        if r.outcome == Outcome::Unverified {
            world.child_alive.set(false);
            let r2 = run(&install, &layout, world, build, true);
            assert!(matches!(r2.outcome, Outcome::RolledBack(_)), "{:?}", r2.outcome);
        } else {
            assert!(matches!(r.outcome, Outcome::RolledBack(_)), "{:?}", r.outcome);
        }
        assert_eq!(snapshot(&install), before);
        assert_eq!(state(&layout), State::RolledBack);
        let _ = std::fs::remove_dir_all(&install);
    }
}

#[test]
fn nothing_is_rolled_back_while_the_new_version_runs() {
    let (install, layout, before) = setup();
    let world = Rc::new(World::default());
    let r = run(&install, &layout, Rc::clone(&world), NewBuild::Hangs, false);
    assert_eq!(r.outcome, Outcome::Unverified);
    assert_eq!(state(&layout), State::InstalledUnverified);
    assert_eq!(std::fs::read(install.join("pdfcraft.exe")).unwrap(), b"new app");
    // A failure reported by a build that stays open waits too.
    let (install2, layout2, _) = setup();
    let world2 = Rc::new(World::default());
    let r = run(&install2, &layout2, Rc::clone(&world2), NewBuild::ReportsFailureAndStays, false);
    assert_eq!(r.outcome, Outcome::Unverified);
    assert_eq!(std::fs::read(install2.join("pdfcraft.exe")).unwrap(), b"new app");
    // Once it has exited, recovery restores.
    world.child_alive.set(false);
    let r = run(&install, &layout, world, NewBuild::Hangs, true);
    assert!(matches!(r.outcome, Outcome::RolledBack(_)), "{:?}", r.outcome);
    assert_eq!(snapshot(&install), before);
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&install2);
}

#[test]
fn nothing_changes_while_the_old_app_or_another_instance_keeps_running() {
    let (install, layout, before) = setup();
    let world = Rc::new(World { parent_polls: Cell::new(u32::MAX), ..World::default() });
    let r = run(&install, &layout, world, NewBuild::Confirms, false);
    assert!(matches!(r.outcome, Outcome::NotStarted(_)), "{:?}", r.outcome);
    assert!(r.outcome.relaunch());
    assert_eq!(snapshot(&install), before);
    assert_eq!(state(&layout), State::Prepared);
    assert!(r.launched.is_empty());
    let world = Rc::new(World { others_polls: Cell::new(u32::MAX), ..World::default() });
    let r = run(&install, &layout, world, NewBuild::Confirms, false);
    assert!(matches!(r.outcome, Outcome::NotStarted(_)), "{:?}", r.outcome);
    assert_eq!(snapshot(&install), before);
    let _ = std::fs::remove_dir_all(&install);
}

/// An installation interrupted at any state, then started: the helper restores only once every
/// process has exited, and a second recovery changes nothing.
#[test]
fn recovery_from_every_interrupted_state_waits_for_every_process() {
    for interrupted in [State::BackingUp, State::Installing, State::InstalledUnverified, State::RollingBack] {
        let (install, layout, before) = setup();
        let mut j = Journal::load(&layout.txn).unwrap();
        let mut ops = FaultyOps::new();
        crate::transaction::backup(&layout, &mut j, &mut ops).unwrap();
        if interrupted != State::BackingUp {
            crate::transaction::install(&layout, &mut j, &mut ops).unwrap();
        }
        j.state = interrupted;
        j.save(&layout.txn, &mut ops).unwrap();
        // The start that found it is still exiting and another window is open for a while.
        let world = Rc::new(World { parent_polls: Cell::new(20), others_polls: Cell::new(20), ..World::default() });
        let r = run(&install, &layout, world, NewBuild::Confirms, true);
        assert!(matches!(r.outcome, Outcome::RolledBack(_)), "{interrupted:?}: {:?}", r.outcome);
        assert_eq!(snapshot(&install), before, "{interrupted:?}");
        assert!(r.launched.is_empty(), "recovery starts nothing itself");
        let again = run(&install, &layout, Rc::new(World::default()), NewBuild::Confirms, true);
        assert_eq!(again.outcome, Outcome::NotStarted("nothing to recover".into()));
        assert_eq!(snapshot(&install), before);
        let _ = std::fs::remove_dir_all(&install);
    }
}

#[test]
fn recovery_gives_up_without_touching_anything_while_processes_stay() {
    let (install, layout, _) = setup();
    let mut j = Journal::load(&layout.txn).unwrap();
    let mut ops = FaultyOps::new();
    crate::transaction::backup(&layout, &mut j, &mut ops).unwrap();
    crate::transaction::install(&layout, &mut j, &mut ops).unwrap();
    let installed = snapshot(&install);
    let world = Rc::new(World { others_polls: Cell::new(u32::MAX), ..World::default() });
    let r = run(&install, &layout, world, NewBuild::Confirms, true);
    assert!(matches!(r.outcome, Outcome::NotStarted(_)), "{:?}", r.outcome);
    assert_eq!(snapshot(&install), installed);
    assert_eq!(state(&layout), State::InstalledUnverified, "left for a later start");
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn a_verified_transaction_is_committed_by_recovery() {
    let (install, layout, _) = setup();
    let mut j = Journal::load(&layout.txn).unwrap();
    j.state = State::Verified;
    j.save(&layout.txn, &mut FaultyOps::new()).unwrap();
    let r = run(&install, &layout, Rc::new(World::default()), NewBuild::Confirms, true);
    assert_eq!(r.outcome, Outcome::Committed);
    assert!(install.join(RECEIPT).is_file());
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn a_prepared_transaction_found_at_startup_is_discarded() {
    let (install, layout, before) = setup();
    let r = run(&install, &layout, Rc::new(World::default()), NewBuild::Confirms, true);
    assert!(matches!(r.outcome, Outcome::NotStarted(_)));
    assert!(!layout.txn.exists());
    assert_eq!(snapshot(&install), before);
    let _ = OLD_COMMIT;
    let _ = std::fs::remove_dir_all(&install);
}

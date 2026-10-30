//! An installation interrupted mid-update, then started (PRD §11, UPD.1): the start hands the
//! repair to the helper and exits; the helper restores the previous files only once that process
//! has exited, never while it runs. Real processes: this test binary re-runs itself as the
//! installed "app" (`OVERLAY_UPDATE_FAKE_APP`), copied into the installation folder. On Windows
//! the operating system also refuses to replace a running executable, so a violation would fail
//! there twice over.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pdfcraft_overlay_update::helper::{self, Env, Launched, Launcher, Outcome, Processes, Timing};
use pdfcraft_overlay_update::journal::{Journal, State};
use pdfcraft_overlay_update::manifest::{BuildIdentity, FileEntry, Manifest, Notes, Package};
use pdfcraft_overlay_update::ops::{Ops, RealOps};
use pdfcraft_overlay_update::{archive, hash, transaction};

const FAKE_APP: &str = "OVERLAY_UPDATE_FAKE_APP";

fn main() {
    // Re-run as the installed app: stay up for the given milliseconds, then exit.
    if let Ok(ms) = std::env::var(FAKE_APP) {
        std::thread::sleep(Duration::from_millis(ms.parse().unwrap_or(300)));
        return;
    }
    let tests: [(&str, fn()); 2] = [
        ("an_interrupted_install_is_restored_only_after_the_started_app_exits", interrupted_install_then_start),
        ("a_still_running_app_blocks_recovery_without_any_change", running_app_blocks_recovery),
    ];
    for (name, test) in tests {
        print!("test {name} ... ");
        let _ = std::io::stdout().flush();
        test();
        println!("ok");
    }
}

/// Children started by this test, by pid.
#[derive(Clone, Default)]
struct Children(Arc<Mutex<BTreeMap<u32, std::process::Child>>>);

impl Children {
    fn start(&self, exe: &Path, ms: u64) -> u32 {
        let child = std::process::Command::new(exe).env(FAKE_APP, ms.to_string()).spawn().unwrap();
        let pid = child.id();
        self.0.lock().unwrap().insert(pid, child);
        pid
    }
    fn running(&self, pid: u32) -> bool {
        self.0.lock().unwrap().get_mut(&pid).is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
    fn any_running(&self) -> bool {
        let pids: Vec<u32> = self.0.lock().unwrap().keys().copied().collect();
        pids.into_iter().any(|p| self.running(p))
    }
}

impl Processes for Children {
    fn alive(&self, pid: u32) -> bool {
        self.running(pid)
    }
    fn using(&self, _: &Path, except: &[u32]) -> Vec<u32> {
        let pids: Vec<u32> = self.0.lock().unwrap().keys().copied().collect();
        pids.into_iter().filter(|p| !except.contains(p) && self.running(*p)).collect()
    }
}

struct NoLaunch;
impl Launcher for NoLaunch {
    fn launch(&mut self, _: &Path, _: &[OsString]) -> std::io::Result<Box<dyn Launched>> {
        panic!("recovery starts nothing");
    }
}

/// Real operations that fail the test if they change an installed file while a child runs.
struct Guarded {
    real: RealOps,
    children: Children,
    install: PathBuf,
}

impl Guarded {
    fn check(&self, p: &Path) {
        let Ok(rel) = p.strip_prefix(&self.install) else { return };
        if rel.starts_with(".pdf-overlay-update") {
            return;
        }
        assert!(!self.children.any_running(), "{} changed while the installed app was running", p.display());
    }
}

impl Ops for Guarded {
    fn rename(&mut self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.check(a);
        self.check(b);
        self.real.rename(a, b)
    }
    fn copy(&mut self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.check(b);
        self.real.copy(a, b)
    }
    fn remove_file(&mut self, p: &Path) -> std::io::Result<()> {
        self.check(p);
        self.real.remove_file(p)
    }
    fn create_dir_all(&mut self, p: &Path) -> std::io::Result<()> {
        self.check(p);
        self.real.create_dir_all(p)
    }
    fn remove_dir(&mut self, p: &Path) -> std::io::Result<()> {
        self.check(p);
        self.real.remove_dir(p)
    }
    fn write_atomic(&mut self, p: &Path, b: &[u8]) -> std::io::Result<()> {
        self.check(p);
        self.real.write_atomic(p, b)
    }
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("overlay-update-processes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
            if rel.starts_with(".pdf-overlay-update") || rel == transaction::RECEIPT {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

const ROOT: &str = "pdf-overlay-test-package";

/// An installation whose `pdfcraft.exe` is this test binary (the "old app"), with an update
/// interrupted while installing: `NOTICE` and an introduced licence already replaced.
fn interrupted(tag: &str) -> (PathBuf, transaction::Layout, BTreeMap<String, Vec<u8>>) {
    let install = temp(tag);
    std::fs::copy(std::env::current_exe().unwrap(), install.join("pdfcraft.exe")).unwrap();
    std::fs::write(install.join("NOTICE"), b"old notice").unwrap();
    std::fs::write(install.join("portable.txt"), b"marker").unwrap();
    std::fs::create_dir_all(install.join("PdfCraftData")).unwrap();
    std::fs::write(install.join("PdfCraftData").join("app.ron"), b"(settings)").unwrap();
    let before = snapshot(&install);
    let files: [(&str, &[u8]); 3] = [("pdfcraft.exe", b"new app"), ("NOTICE", b"new notice"), ("licenses/README.txt", b"introduced")];
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (p, b) in files {
        w.start_file(format!("{ROOT}/{p}"), zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b).unwrap();
    }
    let zip = w.finish().unwrap().into_inner();
    let manifest = Manifest {
        product: "pdf-overlay".into(),
        platform: "windows-x86_64".into(),
        package_kind: "portable-zip".into(),
        channel: "stable".into(),
        seq: 2,
        distribution_version: "0.15.0-ov.2".into(),
        workspace_version: "0.15.0".into(),
        commit: "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d".into(),
        source_repository: "sefr-14/pdfcraft".into(),
        build: BuildIdentity { workflow: "overlay-windows.yml".into(), run_id: 1, artifact: "a".into() },
        approved_at: "2026-11-02T10:00:00Z".into(),
        expires_at: "2027-05-01T00:00:00Z".into(),
        min_updater_schema: 1,
        package: Package {
            url: "https://example.invalid/p.zip".into(),
            file: "p.zip".into(),
            size: zip.len() as u64,
            sha256: hash::sha256(&zip),
            root: ROOT.into(),
            unpacked_size: files.iter().map(|(_, b)| b.len() as u64).sum(),
            files: files.iter().map(|(p, b)| FileEntry { path: (*p).into(), size: b.len() as u64, sha256: hash::sha256(b) }).collect(),
        },
        notes: Notes::default(),
    };
    let mut ops = RealOps { retries: 0, delay: Duration::ZERO };
    let id = transaction::new_txn_id(chrono::DateTime::from(std::time::SystemTime::now()), 7);
    let (layout, mut j) = transaction::begin(&install, &id, &manifest, "{}", None, "n", &mut ops).unwrap();
    std::fs::write(layout.package(), &zip).unwrap();
    archive::extract(&layout.package(), &manifest, &layout.new_dir()).unwrap();
    transaction::plan(&layout, &mut j, &manifest, &mut ops).unwrap();
    transaction::backup(&layout, &mut j, &mut ops).unwrap();
    // Interrupted: two files moved in, the journal says Installing, the application not yet.
    j.state = State::Installing;
    for e in j.files.iter_mut().filter(|e| e.path != "pdfcraft.exe") {
        std::fs::create_dir_all(layout.installed(&e.path).parent().unwrap()).unwrap();
        std::fs::rename(layout.staged(&e.path), layout.installed(&e.path)).unwrap();
        e.installed = true;
    }
    j.created_dirs.push("licenses".into());
    j.save(&layout.txn, &mut ops).unwrap();
    (install, layout, before)
}

fn env_run(layout: &transaction::Layout, children: &Children, parent: u32, wait: Duration) -> Outcome {
    let mut ops = Guarded { real: RealOps { retries: 0, delay: Duration::ZERO }, children: children.clone(), install: layout.install.clone() };
    let mut launcher = NoLaunch;
    let mut log = |l: &str| eprintln!("helper: {l}");
    let now = || chrono::DateTime::from(std::time::SystemTime::now());
    let mut env = Env {
        procs: children,
        launcher: &mut launcher,
        ops: &mut ops,
        timing: Timing { wait_exit: wait, verify: Duration::from_secs(1), observe: Duration::ZERO, poll: Duration::from_millis(10) },
        me: std::process::id(),
        now: &now,
        log: &mut log,
    };
    helper::recover(layout, parent, &mut env)
}

fn interrupted_install_then_start() {
    let (install, layout, before) = interrupted("restart");
    // The user starts the installed app; it finds the interrupted update, hands it to the
    // helper and exits (here: it simply runs for a moment).
    let children = Children::default();
    let app = children.start(&install.join("pdfcraft.exe"), 400);
    let started = std::time::Instant::now();
    let outcome = env_run(&layout, &children, app, Duration::from_secs(20));
    assert!(matches!(outcome, Outcome::RolledBack(_)), "{outcome:?}");
    assert!(started.elapsed() >= Duration::from_millis(300), "the helper waited for the app");
    assert_eq!(snapshot(&install), before, "every file is back, the introduced licence is gone");
    assert_eq!(Journal::load(&layout.txn).unwrap().state, State::RolledBack);
    // Running recovery again changes nothing.
    assert!(matches!(env_run(&layout, &children, 0, Duration::from_secs(1)), Outcome::NotStarted(_)));
    assert_eq!(snapshot(&install), before);
    let _ = std::fs::remove_dir_all(&install);
}

fn running_app_blocks_recovery() {
    let (install, layout, _) = interrupted("blocked");
    let mid = snapshot(&install);
    let children = Children::default();
    let app = children.start(&install.join("pdfcraft.exe"), 3000);
    let outcome = env_run(&layout, &children, app, Duration::from_millis(300));
    assert!(matches!(outcome, Outcome::NotStarted(_)), "{outcome:?}");
    assert_eq!(snapshot(&install), mid, "nothing changed while it ran");
    assert_eq!(Journal::load(&layout.txn).unwrap().state, State::Installing, "left for a later start");
    while children.running(app) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&install);
}

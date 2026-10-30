use super::*;
use crate::archive::tests::{package, temp};
use crate::ops::{FaultyOps, Op, RealOps};
use std::collections::BTreeMap;

pub(crate) const ROOT: &str = "pdfcraft-overlay-0.15.0-1a2b3c4-windows-x64-portable";
pub(crate) const OLD_COMMIT: &str = "0000000000000000000000000000000000000000";

/// Every file under `dir` except the updater's own folder, by relative path.
pub(crate) fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if rel == paths::UPDATE_DIR || rel == RECEIPT {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                walk(base, &p, out);
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// An installation with old program files, a portable marker and user data.
pub(crate) fn installation() -> PathBuf {
    let d = temp("install");
    for (p, b) in [
        ("pdfcraft.exe", &b"old app"[..]),
        ("pdfcraft-cli.exe", b"old cli"),
        ("NOTICE", b"old notice"),
        ("portable.txt", b"my own marker"),
        ("PdfCraftData/app.ron", b"(settings)"),
        ("PdfCraftData/Digital IDs/me.p12", b"secret"),
        ("my drawing.pdf", b"%PDF"),
    ] {
        let path = d.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b).unwrap();
    }
    d
}

pub(crate) const NEW_FILES: &[(&str, &[u8])] = &[
    ("pdfcraft.exe", b"new app"),
    ("pdfcraft-cli.exe", b"new cli"),
    ("NOTICE", b"new notice"),
    ("portable.txt", b"package marker"),
    ("licenses/epaint_default_fonts/OFL.txt", b"introduced licence"),
];

/// A prepared transaction for [`NEW_FILES`] in `install`.
pub(crate) fn prepared(install: &Path, ops: &mut dyn Ops) -> (Layout, Journal, Manifest) {
    let entries: Vec<(String, &[u8])> = NEW_FILES.iter().map(|(p, b)| (format!("{ROOT}/{p}"), *b)).collect();
    let entries: Vec<(&str, &[u8])> = entries.iter().map(|(p, b)| (p.as_str(), *b)).collect();
    let (zip, m) = package(ROOT, &entries, NEW_FILES);
    let id = new_txn_id(crate::manifest::time("2026-11-10T00:00:00Z").unwrap(), 42);
    let (layout, mut j) = begin(install, &id, &m, "{\"envelope\":\"test\"}", Some(OLD_COMMIT), "nonce-1", ops).unwrap();
    std::fs::write(layout.package(), &zip).unwrap();
    crate::archive::extract(&layout.package(), &m, &layout.new_dir()).unwrap();
    plan(&layout, &mut j, &m, ops).unwrap();
    std::fs::write(layout.helper(), b"helper exe").unwrap();
    record_helper(&layout, &mut j, ops).unwrap();
    (layout, j, m)
}

fn real() -> RealOps {
    RealOps { retries: 0, delay: std::time::Duration::ZERO }
}

fn now() -> DateTime<Utc> {
    crate::manifest::time("2026-11-10T00:00:00Z").unwrap()
}

#[test]
fn a_complete_update_replaces_only_application_files() {
    let install = installation();
    let before = snapshot(&install);
    let mut ops = FaultyOps::new();
    let (layout, mut j, _) = prepared(&install, &mut ops);
    backup(&layout, &mut j, &mut ops).unwrap();
    install_files(&layout, &mut j, &mut ops);
    let after = snapshot(&install);
    assert_eq!(after.get("pdfcraft.exe").map(Vec::as_slice), Some(&b"new app"[..]));
    assert_eq!(after.get("licenses/epaint_default_fonts/OFL.txt").map(Vec::as_slice), Some(&b"introduced licence"[..]));
    // Protected: the marker keeps its own contents, user data is untouched.
    for p in ["portable.txt", "PdfCraftData/app.ron", "PdfCraftData/Digital IDs/me.p12", "my drawing.pdf"] {
        assert_eq!(after.get(p), before.get(p), "{p}");
    }
    assert_protected_untouched(&install, &ops.log);
    commit(&layout, &mut j, now(), &mut ops).unwrap();
    assert_eq!(Journal::load(&layout.txn).unwrap().state, State::Committed);
    assert!(install.join(RECEIPT).is_file());
    assert!(!layout.new_dir().exists());
    assert!(layout.backed_up("pdfcraft.exe").is_file(), "the previous version is kept");
    let _ = std::fs::remove_dir_all(&install);
}

fn install_files(layout: &Layout, j: &mut Journal, ops: &mut dyn Ops) {
    install(layout, j, ops).unwrap();
    assert_eq!(j.state, State::InstalledUnverified);
}

/// No recorded operation changed a protected path.
pub(crate) fn assert_protected_untouched(install: &Path, log: &[Op]) {
    let protected = [install.join("portable.txt"), install.join("PdfCraft.portable"), install.join("PdfCraftData"), install.join("my drawing.pdf")];
    for op in log {
        for t in op.targets() {
            assert!(!protected.iter().any(|p| t.starts_with(p)), "{op:?} touches a protected path");
        }
    }
}

#[test]
fn rollback_restores_the_exact_previous_installation_including_introduced_files() {
    let install = installation();
    let before = snapshot(&install);
    let mut ops = FaultyOps::new();
    let (layout, mut j, _) = prepared(&install, &mut ops);
    backup(&layout, &mut j, &mut ops).unwrap();
    install_files(&layout, &mut j, &mut ops);
    assert_ne!(snapshot(&install), before);
    let report = rollback(&layout, &mut j, &mut ops);
    assert_eq!(report.problems, Vec::<String>::new());
    assert_eq!(snapshot(&install), before, "introduced files and folders are gone, old files are back");
    assert_eq!(Journal::load(&layout.txn).unwrap().state, State::RolledBack);
    // Again: nothing changes.
    let mut again = FaultyOps::new();
    let mut j = Journal::load(&layout.txn).unwrap();
    assert!(rollback(&layout, &mut j, &mut again).problems.is_empty());
    assert_eq!(snapshot(&install), before);
    assert!(again.log.iter().all(|op| matches!(op, Op::WriteAtomic(_))), "a second rollback only rewrites the journal: {:?}", again.log);
    assert_protected_untouched(&install, &ops.log);
    let _ = std::fs::remove_dir_all(&install);
}

/// A crash (every operation failing from number `k` on) at each point of backup and install,
/// then recovery from the journal on disk: the installation is exactly the old one again.
#[test]
fn a_crash_at_any_operation_is_recovered_by_rollback() {
    let mut k = 0;
    loop {
        let install = installation();
        let before = snapshot(&install);
        let mut setup = FaultyOps::new();
        let (layout, _, _) = prepared(&install, &mut setup);
        let mut j = Journal::load(&layout.txn).unwrap();
        let mut crashing = FaultyOps::failing_at(k, true);
        let finished = backup(&layout, &mut j, &mut crashing).and_then(|()| super::install(&layout, &mut j, &mut crashing)).is_ok();
        // After the "power loss": only the journal on disk is known.
        let mut j = Journal::load(&layout.txn).unwrap();
        let report = rollback(&layout, &mut j, &mut real());
        assert_eq!(report.problems, Vec::<String>::new(), "crash at operation {k}");
        assert_eq!(snapshot(&install), before, "crash at operation {k}");
        assert_protected_untouched(&install, &crashing.log);
        let _ = std::fs::remove_dir_all(&install);
        if finished {
            break;
        }
        k += 1;
    }
    assert!(k > 10, "the injection reached every operation ({k})");
}

/// A crash during the rollback itself is finished by running it again.
#[test]
fn an_interrupted_rollback_converges_when_run_again() {
    let mut k = 0;
    loop {
        let install = installation();
        let before = snapshot(&install);
        let mut ops = FaultyOps::new();
        let (layout, mut j, _) = prepared(&install, &mut ops);
        backup(&layout, &mut j, &mut ops).unwrap();
        super::install(&layout, &mut j, &mut ops).unwrap();
        let mut crashing = FaultyOps::failing_at(k, true);
        let first = rollback(&layout, &mut j, &mut crashing);
        let mut j = Journal::load(&layout.txn).unwrap();
        let second = rollback(&layout, &mut j, &mut real());
        assert_eq!(second.problems, Vec::<String>::new(), "crash at rollback operation {k}");
        assert_eq!(snapshot(&install), before, "crash at rollback operation {k}");
        let _ = std::fs::remove_dir_all(&install);
        if first.problems.is_empty() && crashing.log.len() <= k {
            break;
        }
        k += 1;
    }
}

#[test]
fn a_foreign_file_at_an_introduced_path_is_left_alone() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let (layout, mut j, _) = prepared(&install, &mut ops);
    backup(&layout, &mut j, &mut ops).unwrap();
    super::install(&layout, &mut j, &mut ops).unwrap();
    let licence = install.join("licenses/epaint_default_fonts/OFL.txt");
    std::fs::write(&licence, b"someone else's").unwrap();
    let report = rollback(&layout, &mut j, &mut ops);
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert_eq!(std::fs::read(&licence).unwrap(), b"someone else's");
    assert_eq!(std::fs::read(install.join("pdfcraft.exe")).unwrap(), b"old app");
    assert_eq!(Journal::load(&layout.txn).unwrap().state, State::RecoveryRequired);
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn a_damaged_backup_is_never_restored() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let (layout, mut j, _) = prepared(&install, &mut ops);
    backup(&layout, &mut j, &mut ops).unwrap();
    super::install(&layout, &mut j, &mut ops).unwrap();
    std::fs::write(layout.backed_up("NOTICE"), b"tampered").unwrap();
    let report = rollback(&layout, &mut j, &mut ops);
    assert!(report.problems.iter().any(|p| p.contains("NOTICE")), "{:?}", report.problems);
    assert_eq!(std::fs::read(install.join("NOTICE")).unwrap(), b"new notice", "left as installed, not replaced with bad bytes");
    assert_eq!(std::fs::read(install.join("pdfcraft.exe")).unwrap(), b"old app", "the others are restored");
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn staged_files_changed_after_verification_are_refused() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let (layout, j, _) = prepared(&install, &mut ops);
    std::fs::write(layout.staged("pdfcraft.exe"), b"swapped").unwrap();
    assert!(verify_staged(&layout, &j).is_err());
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn the_absent_portable_marker_stays_absent() {
    let install = installation();
    std::fs::remove_file(install.join("portable.txt")).unwrap();
    let mut ops = FaultyOps::new();
    let (layout, mut j, _) = prepared(&install, &mut ops);
    assert!(j.files.iter().all(|e| e.path != "portable.txt"));
    backup(&layout, &mut j, &mut ops).unwrap();
    super::install(&layout, &mut j, &mut ops).unwrap();
    assert!(!install.join("portable.txt").exists(), "an update never switches portable mode on");
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn the_application_is_replaced_last() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let (layout, mut j, _) = prepared(&install, &mut ops);
    backup(&layout, &mut j, &mut ops).unwrap();
    let mut log = FaultyOps::new();
    super::install(&layout, &mut j, &mut log).unwrap();
    let renames: Vec<&PathBuf> = log.log.iter().filter_map(|op| if let Op::Rename(_, to) = op { Some(to) } else { None }).collect();
    assert_eq!(renames.last().copied(), Some(&install.join("pdfcraft.exe")));
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn startup_decisions_follow_the_journal_state() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let (_, mut j, m) = prepared(&install, &mut ops);
    let new = m.commit.clone();
    let cases = [
        (State::Prepared, Some(OLD_COMMIT), true, StartupAction::Discard),
        (State::BackingUp, Some(OLD_COMMIT), true, StartupAction::Delegate),
        (State::Installing, Some(new.as_str()), true, StartupAction::Delegate),
        (State::Installing, Some(OLD_COMMIT), false, StartupAction::Manual),
        (State::InstalledUnverified, Some(new.as_str()), true, StartupAction::VerifyHere),
        (State::InstalledUnverified, Some(OLD_COMMIT), true, StartupAction::Delegate),
        (State::InstalledUnverified, None, true, StartupAction::Delegate),
        (State::Verified, Some(new.as_str()), true, StartupAction::Commit),
        (State::Committed, Some(new.as_str()), true, StartupAction::Nothing),
        (State::RollingBack, Some(OLD_COMMIT), true, StartupAction::Delegate),
        (State::RolledBack, Some(OLD_COMMIT), true, StartupAction::Nothing),
        (State::RecoveryRequired, Some(OLD_COMMIT), true, StartupAction::Manual),
    ];
    for (state, own, helper_ok, expected) in cases {
        j.state = state;
        assert_eq!(startup_action(&j, own, helper_ok), expected, "{state:?} {own:?} {helper_ok}");
    }
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn a_tampered_helper_or_journal_is_detected() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let (layout, j, _) = prepared(&install, &mut ops);
    assert!(helper_intact(&layout, &j));
    std::fs::write(layout.helper(), b"planted").unwrap();
    assert!(!helper_intact(&layout, &j));
    // A journal naming a protected file is refused, so a rollback can't be steered at it.
    let text = std::fs::read_to_string(layout.txn.join(journal::FILE)).unwrap().replacen("\"NOTICE\"", "\"PdfCraftData/app.ron\"", 1);
    std::fs::write(layout.txn.join(journal::FILE), text).unwrap();
    assert!(Journal::load(&layout.txn).is_err());
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn the_receipt_names_the_installed_sequence_only_when_genuine() {
    use ed25519_dalek::SigningKey;
    let install = temp("receipt");
    let sk = SigningKey::from_bytes(&[5; 32]);
    let keys = [TrustedKey { id: "test-a", key: sk.verifying_key().to_bytes() }];
    let m = crate::manifest::tests::sample();
    let env = envelope::seal(&m.to_bytes().unwrap(), "test-a", &sk).unwrap();
    std::fs::write(install.join(RECEIPT), json!({"format": 1, "envelope": env}).to_string()).unwrap();
    assert_eq!(installed_seq(&install, &keys, &m.commit), 7);
    assert_eq!(installed_seq(&install, &keys, OLD_COMMIT), 0, "another build than the receipt's");
    let other = [TrustedKey { id: "test-a", key: SigningKey::from_bytes(&[6; 32]).verifying_key().to_bytes() }];
    assert_eq!(installed_seq(&install, &other, &m.commit), 0, "not signed by a trusted key");
    std::fs::write(install.join(RECEIPT), "garbage").unwrap();
    assert_eq!(installed_seq(&install, &keys, &m.commit), 0);
    std::fs::remove_file(install.join(RECEIPT)).unwrap();
    assert_eq!(installed_seq(&install, &keys, &m.commit), 0);
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn one_update_at_a_time() {
    let install = temp("lock");
    let a = Lock::acquire(&install, 100, &|_| true).unwrap();
    assert!(matches!(Lock::acquire(&install, 200, &|_| true), Err(Error::Busy(_))));
    // The same process may take it again; a dead owner's lock is taken over.
    drop(Lock::acquire(&install, 100, &|_| true).unwrap());
    let a2 = Lock::acquire(&install, 100, &|_| true).unwrap();
    a2.hand_over();
    let b = Lock::acquire(&install, 200, &|pid| pid != 100).unwrap();
    drop(b);
    drop(a);
    assert!(!updates_root(&install).join("lock").exists());
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn cleanup_keeps_the_newest_committed_backup() {
    let install = installation();
    let mut ops = FaultyOps::new();
    let mut layouts = Vec::new();
    for i in 0..2u64 {
        let entries: Vec<(String, &[u8])> = NEW_FILES.iter().map(|(p, b)| (format!("{ROOT}/{p}"), *b)).collect();
        let entries: Vec<(&str, &[u8])> = entries.iter().map(|(p, b)| (p.as_str(), *b)).collect();
        let (_, m) = package(ROOT, &entries, NEW_FILES);
        let (layout, mut j) = begin(&install, &new_txn_id(now(), i), &m, "{}", None, "n", &mut ops).unwrap();
        j.state = State::Committed;
        j.save(&layout.txn, &mut ops).unwrap();
        std::fs::create_dir_all(layout.backup_dir()).unwrap();
        layouts.push(layout);
    }
    cleanup(&install, now());
    assert!(!layouts[0].txn.exists());
    assert!(layouts[1].backup_dir().exists());
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn transaction_folders_are_found_only_under_the_update_folder() {
    let install = temp("layout");
    let l = Layout::new(&install, "txn-1");
    assert_eq!(Layout::from_txn_dir(&l.txn), Some(l.clone()));
    assert_eq!(Layout::from_txn_dir(&install.join("other").join("txn-1")), None);
    assert_eq!(Layout::from_txn_dir(&updates_root(&install).join("not-a-txn")), None);
    let _ = std::fs::remove_dir_all(&install);
}

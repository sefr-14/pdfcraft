// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Help ▸ Check for updates (UPD.1): approved updates, "Remind me later", the automatic check
//! when a reminder comes due, and Update and Restart through the usual quit questions, with
//! stand-in sources and installers (no network, nothing installed).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::updates::{Check, CheckContext, Interval, Offer, Prepared, Progress, UpdateInstaller, UpdateSource};

fn offer(seq: u64) -> Offer {
    Offer {
        seq,
        distribution_version: format!("0.15.0-ov.{seq}"),
        commit: "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d".into(),
        approved_at: chrono::DateTime::parse_from_rfc3339("2026-11-02T10:00:00Z").unwrap().with_timezone(&chrono::Utc),
        size: 1000,
        notes_en: Some("Faster overlays.".into()),
        notes_es: Some("Superposiciones más rápidas.".into()),
        envelope: "{}".into(),
    }
}

/// A stand-in installer that records what the app asked of it.
#[derive(Default)]
struct FakeInstaller {
    unavailable: Option<String>,
    others: usize,
    fail: Option<String>,
    prepared: AtomicUsize,
    handed_off: Mutex<Option<Prepared>>,
    abandoned: AtomicBool,
}

impl UpdateInstaller for FakeInstaller {
    fn unavailable(&self) -> Option<String> {
        self.unavailable.clone()
    }
    fn prepare(&self, offer: &Offer, progress: &Progress) -> Result<Prepared, String> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        progress.done.store(offer.size, Ordering::Relaxed);
        match &self.fail {
            Some(e) => Err(e.clone()),
            None => Ok(Prepared { id: "txn-1".into(), distribution_version: offer.distribution_version.clone() }),
        }
    }
    fn others_running(&self) -> usize {
        self.others
    }
    fn hand_off(&self, prepared: &Prepared) {
        *self.handed_off.lock().unwrap() = Some(prepared.clone());
    }
    fn abandon(&self, _: &Prepared) {
        self.abandoned.store(true, Ordering::SeqCst);
    }
}

struct Setup {
    source: UpdateSource,
    installer: Option<Arc<FakeInstaller>>,
    settings: Option<String>,
    delay: Option<Duration>,
    document: bool,
}

impl Setup {
    fn new(answer: Result<Check, &str>) -> Setup {
        let answer = answer.map_err(str::to_string);
        Setup {
            source: Arc::new(move |_| answer.clone()),
            installer: Some(Arc::new(FakeInstaller::default())),
            settings: None,
            delay: None,
            document: false,
        }
    }
    fn build(self) -> Harness<'static, PdfCraftApp> {
        let Setup { source, installer, settings, delay, document } = self;
        Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            if let Some(s) = &settings {
                app.restore(s);
            }
            app.update_source = Some(source.clone());
            if let Some(i) = &installer {
                app.update_installer = Some(i.clone() as Arc<dyn UpdateInstaller>);
            }
            if let Some(d) = delay {
                app.set_automatic_update_delay(d);
            }
            if document {
                app.open_bytes("a.pdf", None, fixture()).unwrap();
            }
            app
        })
    }
}

/// A one-page document.
fn fixture() -> Vec<u8> {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>", "<< /Type /Page /Parent 2 0 R >>"];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Run frames until nothing is checking or downloading any more.
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        h.run_steps(2);
        if h.query_by_label_contains("Checking for updates").is_none() && h.query_by_label_contains("Downloading and verifying").is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the update worker");
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn checked(setup: Setup) -> Harness<'static, PdfCraftApp> {
    let mut h = setup.build();
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from(std::time::SystemTime::now())
}

#[test]
fn an_approved_update_is_offered_with_the_approved_choices() {
    let h = checked(Setup::new(Ok(Check::Available(offer(7)))));
    h.get_by_label("PDF Overlay — Update available");
    h.get_by_label("A newer approved Windows x64 build of PDF Overlay is available.");
    h.get_by_label_contains("then restart to install it. Documents and preferences will be preserved.");
    h.get_by_label("Version 0.15.0-ov.7, approved 11/2/2026");
    h.get_by_label("Faster overlays.");
    h.get_by_label("Remind me in:");
    for choice in ["1 day", "1 week", "1 month"] {
        h.get_by_label(choice);
    }
    h.get_by_label("Remind me later");
    h.get_by_label("Update and Restart");
    // Nothing was downloaded just by checking.
    assert!(h.state().update_reminder().postponed_seq.is_none());
}

#[test]
fn up_to_date_and_failures_say_so() {
    let h = checked(Setup::new(Ok(Check::UpToDate { distribution_version: "0.15.0-ov.7".into(), seen_seq: 7 })));
    h.get_by_label("PDF Overlay is up to date.");
    h.get_by_label("Latest approved version: 0.15.0-ov.7");
    assert!(h.query_by_label("Update and Restart").is_none());
    assert_eq!(h.state().update_reminder().highest_seen_seq, 7, "the sequence seen is remembered against replays");

    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.source = Arc::new(move |_| match counted.fetch_add(1, Ordering::SeqCst) {
        0 => Err("The update server asked for sign-in or refused access".into()),
        _ => Ok(Check::Available(offer(7))),
    });
    let mut h = checked(s);
    h.get_by_label("Couldn't check for updates: The update server asked for sign-in or refused access");
    assert!(h.query_by_label("Update and Restart").is_none());
    h.get_by_label("Try again").click();
    settle(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    h.get_by_label("Update and Restart");
}

#[test]
fn remind_me_later_saves_each_interval_and_survives_a_restart() {
    for (label, interval, days) in [("1 day", Interval::Day, 1), ("1 week", Interval::Week, 7), ("1 month", Interval::Month, 28)] {
        let mut h = checked(Setup::new(Ok(Check::Available(offer(7)))));
        h.get_by_label(label).click();
        h.run_steps(2);
        h.get_by_label("Remind me later").click();
        h.run_steps(3);
        assert!(h.query_by_label("PDF Overlay — Update available").is_none(), "the dialog closes");
        let saved = h.state().persist();
        // A new session reads the same reminder back.
        let mut restarted = PdfCraftApp::new();
        restarted.restore(&saved);
        let r = restarted.update_reminder();
        assert_eq!((r.interval, r.postponed_seq), (interval, Some(7)), "{label}");
        let until = r.postponed_until.unwrap();
        assert!(until >= now() + chrono::Duration::days(days) - chrono::Duration::minutes(1), "{label}: {until}");
        assert!(until <= now() + chrono::Duration::days(31) + chrono::Duration::minutes(1), "{label}: {until}");
    }
}

#[test]
fn one_week_is_the_default_and_dismissing_postpones() {
    let mut h = checked(Setup::new(Ok(Check::Available(offer(7)))));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("PDF Overlay — Update available").is_none());
    let r = h.state().update_reminder();
    assert_eq!((r.interval, r.postponed_seq), (Interval::Week, Some(7)));
}

/// The settings of a postponed update: due (in the past) or not yet.
fn postponed(seq: u64, due: bool) -> String {
    let until = if due { now() - chrono::Duration::hours(1) } else { now() + chrono::Duration::days(3) };
    serde_json::json!({ "updates": { "remind_interval": "week", "postponed_seq": seq, "postponed_until": until.to_rfc3339(), "highest_seen_seq": seq } }).to_string()
}

fn counting(answer: Check) -> (UpdateSource, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let source: UpdateSource = Arc::new(move |_: CheckContext| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(answer.clone())
    });
    (source, calls)
}

fn run_a_while(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..20 {
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(h);
}

#[test]
fn a_due_reminder_checks_once_and_shows_the_update() {
    let (source, calls) = counting(Check::Available(offer(7)));
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.source = source;
    s.settings = Some(postponed(7, true));
    s.delay = Some(Duration::ZERO);
    let mut h = s.build();
    run_a_while(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    h.get_by_label("PDF Overlay — Update available");
    // Postponed again: no second prompt this session.
    h.get_by_label("Remind me later").click();
    run_a_while(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(h.query_by_label("PDF Overlay — Update available").is_none());
}

#[test]
fn nothing_is_checked_at_start_unless_a_reminder_is_due() {
    for settings in [None, Some(postponed(7, false)), Some(r#"{"check_updates_at_start": true}"#.to_string())] {
        let (source, calls) = counting(Check::Available(offer(8)));
        let mut s = Setup::new(Ok(Check::Available(offer(8))));
        s.source = source;
        s.settings = settings.clone();
        s.delay = Some(Duration::ZERO);
        let mut h = s.build();
        run_a_while(&mut h);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "{settings:?}");
        assert!(h.query_by_label("PDF Overlay — Update available").is_none());
    }
}

#[test]
fn a_failed_automatic_check_stays_quiet() {
    let mut s = Setup::new(Err("couldn't reach the update server"));
    s.settings = Some(postponed(7, true));
    s.delay = Some(Duration::ZERO);
    let mut h = s.build();
    run_a_while(&mut h);
    assert!(h.query_by_label_contains("Couldn't check for updates").is_none());
}

#[test]
fn a_manual_check_shows_a_postponed_update_and_a_newer_one() {
    for seq in [7, 8] {
        let mut s = Setup::new(Ok(Check::Available(offer(seq))));
        s.settings = Some(postponed(7, false));
        let h = checked(s);
        h.get_by_label("PDF Overlay — Update available");
        h.get_by_label(&format!("Version 0.15.0-ov.{seq}, approved 11/2/2026"));
    }
}

#[test]
fn an_automatic_check_that_finds_an_older_postponed_update_stays_quiet() {
    // Due, but what's offered is no newer than what was postponed: may prompt (it's due).
    // Not due: no automatic check at all (covered above). A newer one while due prompts too.
    let mut s = Setup::new(Ok(Check::UpToDate { distribution_version: "0.15.0-ov.7".into(), seen_seq: 7 }));
    s.settings = Some(postponed(7, true));
    s.delay = Some(Duration::ZERO);
    let mut h = s.build();
    run_a_while(&mut h);
    assert!(h.query_by_label("PDF Overlay is up to date.").is_none(), "an automatic check never shows 'up to date'");
}

#[test]
fn update_and_restart_downloads_then_quits_and_hands_off() {
    let installer = Arc::new(FakeInstaller::default());
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.installer = Some(installer.clone());
    let mut h = checked(s);
    h.get_by_label("Update and Restart").click();
    settle(&mut h);
    assert_eq!(installer.prepared.load(Ordering::SeqCst), 1);
    // No unsaved documents: the app quits at once and the staged update is handed over.
    assert_eq!(installer.handed_off.lock().unwrap().as_ref().map(|p| p.id.as_str()), Some("txn-1"));
    assert!(!installer.abandoned.load(Ordering::SeqCst));
}

#[test]
fn unsaved_documents_are_asked_about_and_cancel_installs_nothing() {
    let installer = Arc::new(FakeInstaller::default());
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.installer = Some(installer.clone());
    s.document = true;
    let mut h = s.build();
    h.run_steps(3);
    assert!(h.state_mut().execute("page.rotate"));
    h.run_steps(3);
    assert!(h.state().first_dirty().is_some());
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label("Update and Restart").click();
    settle(&mut h);
    h.get_by_label_contains("Save changes to");
    assert!(installer.handed_off.lock().unwrap().is_none(), "nothing is handed over while a question is open");
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert!(installer.handed_off.lock().unwrap().is_none());
    assert!(installer.abandoned.load(Ordering::SeqCst));
    h.get_by_label("Couldn't update: The update was cancelled. Nothing was installed.");
    assert!(h.state().first_dirty().is_some(), "the document and its changes are kept");
}

#[test]
fn don_t_save_lets_the_update_proceed() {
    let installer = Arc::new(FakeInstaller::default());
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.installer = Some(installer.clone());
    s.document = true;
    let mut h = s.build();
    h.run_steps(3);
    assert!(h.state_mut().execute("page.rotate"));
    h.run_steps(3);
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label("Update and Restart").click();
    settle(&mut h);
    h.get_by_label("Don't save").click();
    h.run_steps(4);
    assert!(installer.handed_off.lock().unwrap().is_some());
}

#[test]
fn other_instances_block_the_restart_until_closed() {
    let installer = Arc::new(FakeInstaller { others: 2, ..FakeInstaller::default() });
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.installer = Some(installer.clone());
    let mut h = checked(s);
    h.get_by_label("Update and Restart").click();
    settle(&mut h);
    h.get_by_label("The update is ready, but PDF Overlay can't restart yet:");
    h.get_by_label_contains("running from this folder (2)");
    assert!(installer.handed_off.lock().unwrap().is_none());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(installer.abandoned.load(Ordering::SeqCst));
}

#[test]
fn a_failed_download_installs_nothing() {
    let installer = Arc::new(FakeInstaller {
        fail: Some("the downloaded package failed verification (SHA-256 doesn't match the signed value)".into()),
        ..FakeInstaller::default()
    });
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.installer = Some(installer.clone());
    let mut h = checked(s);
    h.get_by_label("Update and Restart").click();
    settle(&mut h);
    h.get_by_label("Couldn't update: the downloaded package failed verification (SHA-256 doesn't match the signed value)");
    h.get_by_label("Nothing was installed.");
    assert!(installer.handed_off.lock().unwrap().is_none());
}

#[test]
fn without_an_installer_the_offer_says_why() {
    let mut s = Setup::new(Ok(Check::Available(offer(7))));
    s.installer = Some(Arc::new(FakeInstaller { unavailable: Some("this is a development build".into()), ..FakeInstaller::default() }));
    let h = checked(s);
    h.get_by_label("Update and Restart isn't available: this is a development build");
    assert!(h.query_by_label("Update and Restart").is_none());
    h.get_by_label("Remind me later");
}

#[test]
fn without_a_source_the_command_says_updates_are_unavailable() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.state_mut().execute("help.check_updates");
    h.run_steps(2);
    h.get_by_label("Couldn't check for updates: updates aren't available in this version of PDF Overlay");
    assert_eq!(h.state().last_opened_url, None, "no page is opened instead");
}

#[test]
fn the_helper_s_result_is_shown_after_a_restart() {
    let mut h = Setup::new(Ok(Check::Available(offer(7)))).build();
    h.state_mut().show_update_result(false, "The previous version was restored: the new version closed before confirming that it started.");
    h.run_steps(2);
    h.get_by_label("The update couldn't be completed.");
    h.get_by_label_contains("The previous version was restored");
}

#[test]
fn an_installed_update_clears_its_postponement() {
    let mut app = PdfCraftApp::new();
    app.set_installed_update(7);
    app.restore(&postponed(7, false));
    assert_eq!(app.update_reminder().postponed_seq, None);
    let mut app = PdfCraftApp::new();
    app.set_installed_update(6);
    app.restore(&postponed(7, false));
    assert_eq!(app.update_reminder().postponed_seq, Some(7));
}

#[test]
fn spanish_has_the_approved_wording() {
    let mut h = checked(Setup::new(Ok(Check::Available(offer(7)))));
    h.state_mut().set_option("language", "es").unwrap();
    h.run_steps(3);
    h.get_by_label("PDF Overlay — Actualización disponible");
    h.get_by_label("Recordármelo más tarde");
    h.get_by_label("Actualizar y reiniciar");
    h.get_by_label("1 semana");
    h.get_by_label("Superposiciones más rápidas.");
}

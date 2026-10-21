//! Help ▸ Check for updates (OV8): the latest available Windows build of this fork, with
//! stand-in sources (no network).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::updates::{Build, Builds, RUN_PAGE, UpdateSource, Utc, WORKFLOW_PAGE};

fn build(version: Option<&str>) -> Build {
    Build {
        version: version.map(Into::into),
        artifact: "pdfcraft-overlay-0.10.1-6740201-windows-x64-portable".into(),
        built: Utc::parse("2026-10-21T12:17:14Z").unwrap(),
        run_id: 39595600341,
    }
}

fn harness_with(source: UpdateSource) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.update_source = Some(source.clone());
        app
    })
}

fn harness(answer: Result<Builds, &str>) -> Harness<'static, PdfCraftApp> {
    let answer = answer.map_err(str::to_string);
    harness_with(Arc::new(move || answer.clone()))
}

/// Run frames until the background check has reported. By what the dialog shows, with the
/// same 30-second deadline as `support::wait_until`, not for a fixed second: under load the
/// check's thread can take longer (#1277).
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.run_steps(2);
        if h.query_by_label_contains("Looking for the latest Windows build").is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the update check");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn checked(answer: Result<Builds, &str>) -> Harness<'static, PdfCraftApp> {
    let mut h = harness(answer);
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    no_comparison(&h);
    h
}

/// The dialog never claims more than it knows: several builds share a version number.
fn no_comparison(h: &Harness<'static, PdfCraftApp>) {
    for claim in ["up to date", "is available", "newer", "out of date", "Download"] {
        assert!(h.query_by_label_contains(claim).is_none(), "the dialog says {claim:?}");
    }
}

#[test]
fn the_latest_available_build_is_shown_with_its_run_page() {
    let mut h = checked(Ok(Builds::Found(build(Some("0.10.1")))));
    h.get_by_label("Latest available Windows build");
    h.get_by_label("Version: 0.10.1");
    // Preferences ▸ Date format's default (m/d/yyyy), and the time in UTC.
    h.get_by_label("Built: 10/21/2026 12:17 UTC");
    h.get_by_label("Artifact: pdfcraft-overlay-0.10.1-6740201-windows-x64-portable");
    h.get_by_label(concat!("This copy is version ", env!("CARGO_PKG_VERSION"), "."));
    h.get_by_label_contains("GitHub may ask you to sign in");
    h.get_by_label("View build on GitHub").click();
    h.run_steps(3);
    assert_eq!(h.state().last_opened_url.as_deref(), Some("https://github.com/sefr-14/pdfcraft/actions/runs/39595600341"));
    assert!(h.state().last_opened_url.as_deref().is_some_and(|u| u.starts_with(RUN_PAGE)));
    assert!(h.query_by_label("Latest available Windows build").is_none(), "the dialog closes");
}

#[test]
fn a_build_without_a_readable_version_is_shown_by_name_and_date() {
    let h = checked(Ok(Builds::Found(build(None))));
    h.get_by_label("Latest available Windows build");
    assert!(h.query_by_label_contains("Version:").is_none(), "no version is made up");
    h.get_by_label("Built: 10/21/2026 12:17 UTC");
    h.get_by_label("Artifact: pdfcraft-overlay-0.10.1-6740201-windows-x64-portable");
}

#[test]
fn no_build_or_expired_builds_say_so_and_link_the_workflow() {
    let mut h = checked(Ok(Builds::NoneFound { checked: 5, total: 12 }));
    h.get_by_label("No downloadable Windows build was found in the most recent successful builds (5 checked).");
    h.get_by_label("Older builds weren't checked.");
    h.get_by_label("View builds on GitHub").click();
    h.run_steps(3);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(WORKFLOW_PAGE));

    let h = checked(Ok(Builds::NoneFound { checked: 0, total: 0 }));
    h.get_by_label("No successful Windows build was found.");
    assert!(h.query_by_label("Older builds weren't checked.").is_none());

    let h = checked(Ok(Builds::Expired { checked: 2, total: 2 }));
    h.get_by_label("The Windows builds checked have expired and can't be downloaded anymore (2 checked).");
    assert!(h.query_by_label("Older builds weren't checked.").is_none());
    h.get_by_label("View builds on GitHub");
    assert!(h.query_by_label("View build on GitHub").is_none(), "an expired build is never offered");
}

#[test]
fn a_failed_check_says_why_and_can_be_tried_again() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut h = harness_with(Arc::new(move || match counted.fetch_add(1, Ordering::SeqCst) {
        0 => Err("GitHub took too long to answer".into()),
        _ => Ok(Builds::Found(build(Some("0.10.1")))),
    }));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    no_comparison(&h);
    h.get_by_label("Couldn't check for updates: GitHub took too long to answer");
    h.get_by_label(&format!("Windows builds are listed at {WORKFLOW_PAGE}."));
    assert!(h.query_by_label_contains("No downloadable").is_none(), "a failure isn't reported as no build");
    h.get_by_label("Try again").click();
    settle(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    h.get_by_label("Latest available Windows build");
}

#[test]
fn nothing_is_asked_until_the_user_checks() {
    // No check at start (the owner's decision): the source is only called from the command.
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Settings from a build that had the startup option are ignored.
        app.restore(r#"{"check_updates_at_start": true}"#);
        let counted = counted.clone();
        app.update_source = Some(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Builds::Found(build(Some("0.10.1"))))
        }));
        app
    });
    settle(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(h.query_by_label("Latest available Windows build").is_none());
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    h.get_by_label("Latest available Windows build");
}

#[test]
fn without_a_source_the_command_opens_the_workflow_page() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.state_mut().execute("help.check_updates");
    h.run_steps(2);
    assert_eq!(h.state().last_opened_url.as_deref(), Some(WORKFLOW_PAGE));
}

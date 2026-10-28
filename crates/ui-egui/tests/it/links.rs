// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Links and identity (LIC.1): this fork has no Discord button and no community card, its Help menu
//! and About dialog open its own issue tracker and upstream's repository, and it calls itself
//! PDF Overlay. The About dialog credits upstream in plain text and says what the fork adds.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::links;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn the_top_bar_has_no_discord_button() {
    let h = harness(|_| {});
    h.get_by_label("Keyboard shortcuts");
    assert!(h.query_by_label("Discord").is_none(), "Discord button in the top bar");
    assert!(pdfcraft_engine::commands::command("help.discord").is_none(), "help.discord is still a command");
}

#[test]
fn home_screen_names_the_fork_and_has_no_community_card() {
    let h = harness(|_| {});
    h.get_by_label("Welcome to PDF Overlay");
    assert!(h.query_by_label("Join the ArtCraft community").is_none(), "community card on the home screen");
    for l in links::LINKS {
        assert!(h.query_by_label(l.label).is_none(), "{} on the home screen", l.label);
    }
    assert!(h.query_by_label("Join our Discord").is_none(), "Discord pill on the home screen");
}

#[test]
fn about_dialog_names_the_fork_credits_upstream_and_shows_the_links() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    for (label, url) in [("Report a problem", links::ISSUES), ("Upstream PdfCraft on GitHub", links::GITHUB)] {
        // With a document open, as people usually reach About.
        let mut h = harness(move |app| {
            app.open_bytes("one.pdf", None, pdf.to_vec()).unwrap();
            app.dialog = Some(Dialog::About);
        });
        h.get_by_label("PDF Overlay");
        // This fork leaves the ArtCraft marks out (their licence requires it for published
        // modified versions), credits upstream in plain text and says what the fork adds.
        assert_eq!(h.query_all_by_label("ArtCraft").count(), 0, "no ArtCraft mark or wordmark (alt text)");
        h.get_by_label("Based on PdfCraft by the ArtCraft team.");
        h.get_by_label("An unofficial fork that adds overlay comparison and drawing-review tools.");
        for gone in ["Join our Discord", "Join the ArtCraft Discord", "ArtCraft website", "PdfCraft web page"] {
            assert!(h.query_by_label(gone).is_none(), "{gone} in the About dialog");
        }
        h.get_by_label(label).click();
        h.run_steps(2);
        assert_eq!(h.state().last_opened_url.as_deref(), Some(url), "{label}");
    }
    assert_eq!(links::ISSUES, "https://github.com/sefr-14/pdfcraft/issues");
    assert_eq!(links::GITHUB, "https://github.com/storytold/pdfcraft");
}

#[test]
fn help_commands_open_each_link() {
    for l in links::LINKS {
        let mut h = harness(|_| {});
        assert!(h.state_mut().execute(l.command), "{}", l.command);
        assert_eq!(h.state().last_opened_url.as_deref(), Some(l.url));
        assert_eq!(pdfcraft_engine::commands::command(l.command).unwrap().menu, Some("Help"));
    }
}

/// Runs frames until the node labelled `label` exists and has stopped moving, then returns it.
///
/// The About dialog is a modal centred on its own size, and switching tabs changes that size, so
/// for a few frames afterwards the modal grows and recentres and every control in it moves. A
/// fixed number of frames is a guess at how long that takes: it was too few after the 0.5.0
/// contributors refresh (78 names instead of 5) on some machines, the click on Table landed where
/// the button had been, and the table never opened (#671).
fn settled<'a>(h: &'a mut Harness<'static, PdfCraftApp>, label: &'a str) -> egui_kittest::Node<'a> {
    let mut last = None;
    for _ in 0..60 {
        h.run_steps(1);
        let now = h.query_by_label(label).map(|n| n.rect());
        if now.is_some() && now == last {
            return h.get_by_label(label);
        }
        last = now;
    }
    panic!("{label:?} never appeared or never stopped moving");
}

#[test]
fn about_dialog_has_contributors_and_models_tabs() {
    let mut h = harness(|app| app.dialog = Some(Dialog::About));
    settled(&mut h, "Contributors").click();
    // The owner is always in the compiled-in credits (contributors/contributors.json), shown by username.
    settled(&mut h, "@echelon");
    settled(&mut h, "Table").click();
    settled(&mut h, "PRs");
    settled(&mut h, "Display name").click();
    settled(&mut h, "Brandon Thomas");
    settled(&mut h, "Models").click();
    h.run_steps(4);
    assert!(h.query_all_by_label("Anthropic").count() >= 1);
}

/// LIC.1: the ArtCraft brand licence requires a published modified version to remove the marks.
/// `cargo xtask assets` still accepts brand logos under `docs/brand/` (upstream's rule), so an
/// upstream sync could bring them back unnoticed; only their licence file may stay.
#[test]
fn the_artcraft_logo_files_stay_removed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/brand");
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["LICENSE-brand.txt"], "files in {}", dir.display());
    let manifest = std::fs::read_to_string(dir.join("../../ATTRIBUTION.toml")).unwrap();
    assert!(!manifest.contains("licence = \"LicenseRef-Storyteller-Trademark\""), "a brand asset is listed in ATTRIBUTION.toml");
}

/// BRAND.1: with no document open the window is called PDF Overlay. The title is rebuilt every
/// frame, which used to put upstream's name back over the one set at startup.
#[test]
fn the_window_is_called_pdf_overlay() {
    let h = harness(|_| {});
    assert_eq!(h.state().window_title, "PDF Overlay");
}

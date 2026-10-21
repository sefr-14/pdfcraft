//! View ▸ Thin lines (OV9): app-wide, off on first start, remembered with the settings, and
//! followed by every view — duplicate tabs and tiles included. These run real frames.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::DocId;

use crate::{PdfCraftApp, ViewLayout};

fn open(app: &mut PdfCraftApp, name: &str) -> DocId {
    let bytes = app.session.create_blank(612.0, 792.0, 2).unwrap();
    app.open_bytes(name, None, bytes.as_ref().clone()).unwrap();
    app.views.last().unwrap().id
}

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(vec2(2600.0, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

/// Run frames until every view shows its current page sharp (rendered with the current setting).
fn all_sharp(h: &mut Harness<'static, PdfCraftApp>) -> bool {
    for _ in 0..600 {
        h.run_steps(1);
        if h.state().views.iter().all(|v| v.current_page_sharp()) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
fn thin_lines_reach_every_view_of_every_document_and_change_no_file() {
    let mut h = harness(|app| {
        open(app, "plan.pdf");
        app.duplicate_view(0);
        open(app, "detail.pdf");
        app.set_view_layout(ViewLayout::Tiles);
    });
    assert_eq!(h.state().views.len(), 3);
    assert!(!h.state().thin_lines, "off on first start");
    assert!(all_sharp(&mut h), "every tile renders");

    assert!(h.state_mut().execute("view.thin_lines"));
    let app = h.state();
    assert!(app.thin_lines);
    for v in &app.views {
        assert_eq!(app.session.thin_lines(v.id), Some(true));
        // Both tabs of plan.pdf drop their rasters, not just the first one.
        assert!(!v.current_page_sharp(), "a view kept its old page image");
        assert!(!app.session.get(v.id).unwrap().dirty, "a view setting doesn't modify the document");
    }
    assert!(all_sharp(&mut h), "every tile renders again, thin");
    assert!(h.state_mut().execute("view.thin_lines"));
    assert!(!h.state().thin_lines);
    assert!(h.state().views.iter().all(|v| h.state().session.thin_lines(v.id) == Some(false)));
}

#[test]
fn thin_lines_are_remembered_and_apply_to_documents_opened_later() {
    let mut h = harness(|_| {});
    let setting = |json: &str| serde_json::from_str::<serde_json::Value>(json).unwrap()["thin_lines"].clone();
    assert!(!h.state().thin_lines);
    assert_eq!(setting(&h.state().persist()), serde_json::json!(false));
    h.state_mut().set_thin_lines(true);
    let saved = h.state().persist();
    assert_eq!(setting(&saved), serde_json::json!(true));

    // A restart: the settings come back before any document opens.
    let mut h = harness(move |app| {
        app.restore(&saved);
        open(app, "plan.pdf");
    });
    assert!(h.state().thin_lines);
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.thin_lines(id), Some(true));
    // Settings are untrusted: anything but a boolean keeps Off.
    let mut fresh = PdfCraftApp::new();
    fresh.restore(r#"{"thin_lines": "yes"}"#);
    assert!(!fresh.thin_lines);
    h.run_steps(2);
}

#[test]
fn the_page_display_checkbox_toggles_thin_lines() {
    let mut h = harness(|app| {
        open(app, "plan.pdf");
    });
    let id = h.state().views[0].id;
    // The rail's Page display menu: View ▸ Page display draws the same section
    // (`chrome::page_display_menu`), so this covers both.
    for on in [true, false] {
        h.get_by_label_contains("Page display:").click();
        h.run_steps(3);
        h.get_by_label("Thin lines").click();
        h.run_steps(4);
        assert_eq!(h.state().thin_lines, on);
        assert_eq!(h.state().session.thin_lines(id), Some(on));
    }
}

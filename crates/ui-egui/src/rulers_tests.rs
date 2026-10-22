//! Rulers (this fork, OV11) in real frames: shown per PDF from View ▸ Page display, the palette
//! and ⌘R / Ctrl+R; taking their strips off the page view (each tile its own); the unit from the
//! ruler's menu; remembered per PDF across a restart; and never a change to the PDF.

use egui::{Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::DocId;

use crate::rulers::STRIP;
use crate::units::Unit;
use crate::{PdfCraftApp, ViewLayout};

/// A two-page PDF saved as a real file (named `name`, unique to this test run), and its path.
fn file(name: &str) -> (String, Vec<u8>) {
    let bytes = pdfcraft_engine::Session::new().create_blank(612.0, 792.0, 2).unwrap();
    let path = std::env::temp_dir().join(format!("pdfcraft-rulers-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes.as_ref()).unwrap();
    (path.to_string_lossy().into_owned(), bytes.as_ref().clone())
}

fn harness(width: f32, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(vec2(width, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

fn open(app: &mut PdfCraftApp, (path, bytes): &(String, Vec<u8>)) {
    let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
    app.open_bytes(&name, Some(path.clone()), bytes.clone()).unwrap();
}

fn prefs(h: &Harness<'static, PdfCraftApp>, view: usize) -> crate::doc_prefs::ViewPrefs {
    let app = h.state();
    app.doc_prefs.state(app.views[view].id).expect("followed").prefs
}

fn viewport(h: &Harness<'static, PdfCraftApp>, view: usize) -> egui::Rect {
    h.state().views[view].viewport_rect()
}

/// The PDF is as it was opened: nothing to save, no undo step, the same bytes.
fn untouched(h: &mut Harness<'static, PdfCraftApp>, id: DocId, bytes: &[u8]) {
    let doc = h.state().session.get(id).unwrap();
    assert!(!doc.dirty, "a ruler setting changed the document");
    assert_eq!(doc.can_undo(), None, "a ruler setting added an undo step");
    assert_eq!(h.state_mut().session.save_bytes(id).unwrap().as_slice(), bytes, "a ruler setting changed the PDF's bytes");
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, at: Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
}

#[test]
fn rulers_show_per_pdf_take_their_strips_and_dont_change_the_pdf() {
    let (a, b) = (file("a.pdf"), file("b.pdf"));
    let (a_bytes, b_bytes) = (a.1.clone(), b.1.clone());
    let mut h = harness(1600.0, move |app| {
        open(app, &a);
        open(app, &b);
        app.active = Some(0);
    });
    let (ida, idb) = (h.state().views[0].id, h.state().views[1].id);
    assert_eq!(prefs(&h, 0), crate::doc_prefs::ViewPrefs::new(h.state().units), "a new PDF: rulers hidden, guides shown");
    let before = viewport(&h, 0);

    // View ▸ Page display (the rail's menu draws the same section) ▸ Rulers.
    h.get_by_label_contains("Page display:").click();
    h.run_steps(3);
    h.get_by_label("Rulers").click();
    h.run_steps(4);
    assert!(prefs(&h, 0).rulers);
    let after = viewport(&h, 0);
    assert_eq!(after.min, before.min + vec2(STRIP, STRIP), "the strips come off the top and left");
    assert_eq!(after.max, before.max);

    // The unit, from the ruler's own menu.
    right_click(&mut h, Pos2::new(after.left() + 120.0, after.top() - STRIP / 2.0));
    h.get_by_label("Centimeters (cm)").click();
    h.run_steps(3);
    assert_eq!(prefs(&h, 0).unit, Unit::Centimetres);
    // The guides' checkbox there too.
    right_click(&mut h, Pos2::new(after.left() - STRIP / 2.0, after.top() + 120.0));
    h.get_by_label("Guides").click();
    h.run_steps(3);
    assert!(!prefs(&h, 0).guides_shown);
    untouched(&mut h, ida, &a_bytes);

    // The other PDF keeps its own: no rulers.
    h.state_mut().active = Some(1);
    h.run_steps(4);
    assert_eq!(prefs(&h, 1), crate::doc_prefs::ViewPrefs::new(h.state().units));
    assert_eq!(viewport(&h, 1), before, "no strips for the other PDF");
    // ⌘R / Ctrl+R and the palette's command.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::R);
    h.run_steps(3);
    assert!(prefs(&h, 1).rulers, "⌘R / Ctrl+R");
    assert!(h.state_mut().execute("view.rulers"));
    h.run_steps(2);
    assert!(!prefs(&h, 1).rulers);
    assert!(h.state_mut().execute("view.guides"));
    assert!(!prefs(&h, 1).guides_shown);
    untouched(&mut h, idb, &b_bytes);
    assert!(prefs(&h, 0).rulers && !prefs(&h, 0).guides_shown, "and the first keeps its own");
}

#[test]
fn rulers_and_their_unit_are_remembered_per_pdf_across_a_restart() {
    let (a, b) = (file("restart-a.pdf"), file("restart-b.pdf"));
    let (a2, b2) = (a.clone(), b.clone());
    let mut h = harness(1600.0, move |app| {
        open(app, &a);
        open(app, &b);
    });
    h.state_mut().active = Some(0);
    h.state_mut().set_option("rulers", "on").unwrap();
    h.state_mut().set_option("ruler_unit", "mm").unwrap();
    h.state_mut().active = Some(1);
    h.state_mut().set_option("guides", "off").unwrap();
    assert!(h.state_mut().set_option("ruler_unit", "furlong").is_err());
    assert!(h.state_mut().set_option("rulers", "maybe").is_err());
    h.run_steps(2);
    assert!(h.state().doc_prefs.changed(), "due a write of the settings file");
    let saved = h.state().persist();

    // Restart, opening the files in the other order.
    let h = harness(1600.0, move |app| {
        app.restore(&saved);
        open(app, &b2);
        open(app, &a2);
    });
    assert_eq!(prefs(&h, 1), crate::doc_prefs::ViewPrefs { rulers: true, guides_shown: true, unit: Unit::Millimetres });
    assert_eq!(prefs(&h, 0), crate::doc_prefs::ViewPrefs { rulers: false, guides_shown: false, unit: h.state().units });
}

#[test]
fn each_tile_has_its_documents_rulers_and_duplicate_tabs_share_them() {
    let (a, b) = (file("tile-a.pdf"), file("tile-b.pdf"));
    let mut h = harness(2600.0, move |app| {
        open(app, &a);
        app.duplicate_view(0);
        open(app, &b);
        app.set_view_layout(ViewLayout::Tiles);
        app.active = Some(0);
    });
    assert_eq!(h.state().views.len(), 3);
    let before: Vec<egui::Rect> = (0..3).map(|i| viewport(&h, i)).collect();
    h.state_mut().set_option("rulers", "on").unwrap();
    h.run_steps(4);
    assert!(prefs(&h, 0).rulers && prefs(&h, 1).rulers, "both tabs of the PDF");
    for (i, was) in before.iter().enumerate().take(2) {
        assert_eq!(viewport(&h, i).min, was.min + vec2(STRIP, STRIP), "tile {i}");
    }
    assert_eq!(viewport(&h, 2), before[2], "the other PDF's tile has none");
    // Each tile's page view keeps its own zoom.
    h.state_mut().views[1].set_zoom(2.0);
    h.run_steps(3);
    assert!((h.state().views[0].zoom - h.state().views[1].zoom).abs() > 0.1);
}

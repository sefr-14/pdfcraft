//! Rulers and guides (this fork, OV11) in real frames: rulers shown per PDF from View ▸ Page
//! display, the palette and ⌘R / Ctrl+R, taking their strips off the page view (each tile its
//! own), with the unit from the ruler's menu; guides pulled out of them, moved, selected and
//! deleted with the Hand tool, cleared, and shared by every tab of the PDF; all of it remembered
//! per PDF across a restart, and never a change to the PDF.

use egui::{Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::DocId;

use crate::rulers::STRIP;
use crate::units::Unit;
use crate::{PdfCraftApp, QuickTool, ViewLayout};

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

// Guides (this fork, OV11): pulled out of the rulers or made by double-clicking them; moved,
// selected and deleted with the Hand tool; cleared; shared by every tab of the PDF; remembered.

use crate::doc_prefs::{Axis, GuideRef};
use crate::guides;

/// Lines on page 1: (50, 50)–(350, 50), (50, 50)–(50, 250) and (200, 150)–(260, 150).
const LINES: &str = "0 G 2 w 50 50 m 350 50 l S 50 50 m 50 250 l S 200 150 m 260 150 l S";

/// Three pages, written to a file: a 400 × 300 page with `LINES` and a square comment; a page
/// turned a quarter with an offset crop box; a plain 400 × 300 page. With an /ID.
fn plans(name: &str) -> (String, Vec<u8>) {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents 6 0 R /Annots [7 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 500] /CropBox [100 50 500 350] /Rotate 90 /Contents 6 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents 6 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{LINES}\nendstream", LINES.len()),
        "<< /Type /Annot /Subtype /Square /Rect [300 200 350 250] /C [1 0 0] >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R /ID [<0a11ce> <0a11ce>] >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
    );
    let path = std::env::temp_dir().join(format!("pdfcraft-guides-{}-{name}", std::process::id()));
    std::fs::write(&path, &out).unwrap();
    (path.to_string_lossy().into_owned(), out)
}

/// Screen position of user-space point (`x`, `y`) on `page` of view `view`.
fn on_page(h: &Harness<'static, PdfCraftApp>, view: usize, page: usize, x: f64, y: f64) -> Pos2 {
    let app = h.state();
    let v = &app.views[view];
    let xf = v.page_xform(page).expect("page on screen");
    let p = guides::to_screen(&app.session.get(v.id).unwrap().info.pages[page], &xf, x, y);
    assert!(v.viewport_rect().contains(p), "({x}, {y}) on page {page} is at {p:?}, outside {:?}", v.viewport_rect());
    p
}

fn button(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
}

/// Press at `from`, move to `to` over `frames` frames, then (unless `hold`) release there.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, frames: usize, hold: bool) {
    h.hover_at(from);
    h.run_steps(1);
    button(h, from, true);
    h.run_steps(1);
    for k in 1..=frames {
        h.hover_at(from + (to - from) * (k as f32 / frames as f32));
        h.run_steps(1);
    }
    if !hold {
        button(h, to, false);
        h.run_steps(3);
    }
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    button(h, p, true);
    button(h, p, false);
    h.run_steps(3);
}

fn guides_of(h: &Harness<'static, PdfCraftApp>, view: usize, page: usize) -> crate::doc_prefs::PageGuides {
    let app = h.state();
    app.doc_prefs.state(app.views[view].id).and_then(|s| s.page(page)).cloned().unwrap_or_default()
}

/// The top ruler's strip at screen x `x` of view `view`; the left one's at screen y `y`.
fn top_ruler(h: &Harness<'static, PdfCraftApp>, view: usize, x: f32) -> Pos2 {
    Pos2::new(x, viewport(h, view).top() - STRIP / 2.0)
}

fn left_ruler(h: &Harness<'static, PdfCraftApp>, view: usize, y: f32) -> Pos2 {
    Pos2::new(viewport(h, view).left() - STRIP / 2.0, y)
}

/// Within half a screen point at the view's zoom.
fn near(got: f64, want: f64, h: &Harness<'static, PdfCraftApp>, view: usize) -> bool {
    (got - want).abs() <= 0.6 / f64::from(h.state().views[view].zoom).max(0.01)
}

#[test]
fn guides_pulled_out_of_the_rulers_land_on_the_page_and_come_back_after_a_restart() {
    let file = plans("restart.pdf");
    let (again, bytes) = (file.clone(), file.1.clone());
    let mut h = harness(1600.0, move |app| open(app, &file));
    let id = h.state().views[0].id;
    h.state_mut().set_option("rulers", "on").unwrap();
    h.run_steps(4);

    // Down from the top ruler onto the line at y = 150: a horizontal guide there.
    let target = on_page(&h, 0, 0, 230.0, 150.0);
    let p = top_ruler(&h, 0, target.x);
    drag(&mut h, p, target, 6, false);
    let g = guides_of(&h, 0, 0);
    assert_eq!(g.x, Vec::<f64>::new());
    assert!(g.y.len() == 1 && near(g.y[0], 150.0, &h, 0), "{g:?}");
    // Right from the left ruler onto the line at x = 50: a vertical guide.
    let target = on_page(&h, 0, 0, 50.0, 200.0);
    let p = left_ruler(&h, 0, target.y);
    drag(&mut h, p, target, 6, false);
    let g = guides_of(&h, 0, 0);
    assert!(g.x.len() == 1 && near(g.x[0], 50.0, &h, 0), "{g:?}");

    // Released off every page: nothing.
    let vp = viewport(&h, 0);
    let off = Pos2::new(vp.left() + 3.0, vp.center().y);
    assert!(h.state().views[0].visible_page_rects().iter().all(|(_, r)| !r.contains(off)), "a point off the pages");
    let p = top_ruler(&h, 0, off.x + 40.0);
    drag(&mut h, p, off, 6, false);
    // Escape while pulling: nothing either, and the press is taken (rulers stay on).
    let target = on_page(&h, 0, 0, 100.0, 220.0);
    let p = top_ruler(&h, 0, target.x);
    drag(&mut h, p, target, 6, true);
    assert!(h.state().views[0].guides.pulling.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    button(&mut h, target, false);
    h.run_steps(3);
    let g = guides_of(&h, 0, 0);
    assert_eq!(g.x.len() + g.y.len(), 2, "no guide from the cancelled pull or the release off the pages: {g:?}");
    assert!(prefs(&h, 0).rulers);

    // Double-click the top ruler above x = 300: a vertical guide at that x on the current page.
    let p = top_ruler(&h, 0, on_page(&h, 0, 0, 300.0, 150.0).x);
    h.hover_at(p);
    h.run_steps(1);
    // Both clicks in one frame: queued events each get a frame of their own, a quarter second
    // apart, past egui's double-click time.
    let press = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.input_mut().events.extend([press(true), press(false), press(true), press(false)]);
    h.step();
    h.run_steps(2);
    let g = guides_of(&h, 0, 0);
    assert!(g.x.len() == 2 && g.x.iter().any(|x| near(*x, 300.0, &h, 0)), "{g:?}");

    // On the page turned a quarter (crop box [100 50 500 350]), a guide horizontal on screen
    // fixes x: through the middle of the page, x = 300.
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(6);
    let target = on_page(&h, 0, 1, 300.0, 200.0);
    let p = top_ruler(&h, 0, target.x);
    drag(&mut h, p, target, 6, false);
    let g = guides_of(&h, 0, 1);
    assert!(g.y.is_empty() && g.x.len() == 1 && near(g.x[0], 300.0, &h, 0), "{g:?}");
    untouched(&mut h, id, &bytes);
    let before = [guides_of(&h, 0, 0), guides_of(&h, 0, 1)];
    let saved = h.state().persist();

    // Restart at another zoom: the same guides, on the same drawing lines.
    let mut h = harness(1600.0, move |app| {
        app.restore(&saved);
        open(app, &again);
    });
    assert_eq!([guides_of(&h, 0, 0), guides_of(&h, 0, 1)], before);
    assert!(prefs(&h, 0).rulers);
    h.state_mut().views[0].set_zoom(2.3);
    h.run_steps(4);
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(6);
    let app = h.state();
    let v = &app.views[0];
    let page = &app.session.get(v.id).unwrap().info.pages[0];
    let xf = v.page_xform(0).unwrap();
    let seg = guides::segment(page, &xf, Axis::Y, before[0].y[0]);
    let line = guides::to_screen(page, &xf, 230.0, 150.0);
    assert!((seg[0].y - line.y).abs() < 1.5 && (seg[1].y - line.y).abs() < 1.5, "{seg:?} against the line at {line:?}");
}

#[test]
fn the_hand_tool_moves_selects_and_deletes_guides_and_escape_puts_one_back() {
    let file = plans("hand.pdf");
    let bytes = file.1.clone();
    let mut h = harness(1600.0, move |app| open(app, &file));
    let id = h.state().views[0].id;
    // Rulers stay hidden: guides are still shown, moved, selected and deleted.
    let (y, x) = {
        let p = &mut h.state_mut().doc_prefs;
        (p.add_guide(id, 0, Axis::Y, 150.0).unwrap(), p.add_guide(id, 0, Axis::X, 120.0).unwrap())
    };
    h.state_mut().quick_tool = QuickTool::Hand;
    h.run_steps(4);
    let page_at = h.state().views[0].page_screen_rect(0);

    // Drag the horizontal guide from y = 150 to y = 100: it moves, the page doesn't pan.
    let from = on_page(&h, 0, 0, 250.0, 150.0);
    let to = on_page(&h, 0, 0, 250.0, 100.0);
    drag(&mut h, from, to, 6, false);
    let g = guides_of(&h, 0, 0);
    assert!(g.y.len() == 1 && near(g.y[0], 100.0, &h, 0), "{g:?}");
    assert_eq!(h.state().views[0].page_screen_rect(0), page_at, "the press took the guide, not the page");
    let moved = GuideRef { at: g.y[0], ..y };
    assert_eq!(h.state().views[0].guides.selected, Some(moved));

    // Escape mid-drag: the guide goes back to where the drag began, and the page stays.
    let from = on_page(&h, 0, 0, 250.0, 100.0);
    let p = on_page(&h, 0, 0, 250.0, 200.0);
    drag(&mut h, from, p, 6, true);
    assert!(!near(guides_of(&h, 0, 0).y[0], 100.0, &h, 0), "moving");
    h.key_press(egui::Key::Escape);
    h.run_steps(1);
    h.hover_at(on_page(&h, 0, 0, 250.0, 60.0));
    h.run_steps(1);
    let p = on_page(&h, 0, 0, 250.0, 60.0);
    button(&mut h, p, false);
    h.run_steps(3);
    assert_eq!(guides_of(&h, 0, 0).y, vec![moved.at], "back where it was");
    assert_eq!(h.state().views[0].page_screen_rect(0), page_at);
    // The next Escape lets go of the selection, and only that.
    assert!(h.state().views[0].guides.selected.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().views[0].guides.selected, None);
    assert_eq!(h.state().quick_tool, QuickTool::Hand);

    // Click the vertical guide, then Delete: it goes, and the comment selected meanwhile stays.
    let p = on_page(&h, 0, 0, 120.0, 220.0);
    click(&mut h, p);
    assert_eq!(h.state().views[0].guides.selected, Some(x));
    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(guides_of(&h, 0, 0).x, Vec::<f64>::new());
    assert_eq!(h.state().views[0].guides.selected, None);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    untouched(&mut h, id, &bytes);

    // A click beside a guide lets go of the selection.
    let p = on_page(&h, 0, 0, 250.0, moved.at);
    click(&mut h, p);
    assert!(h.state().views[0].guides.selected.is_some());
    let p = on_page(&h, 0, 0, 250.0, moved.at + 40.0);
    click(&mut h, p);
    assert_eq!(h.state().views[0].guides.selected, None);

    // Hidden guides take no input: the press pans instead, and Delete deletes nothing.
    assert!(h.state_mut().execute("view.guides"));
    h.run_steps(2);
    let from = on_page(&h, 0, 0, 250.0, moved.at);
    drag(&mut h, from, from + vec2(0.0, -80.0), 6, false);
    assert_eq!(guides_of(&h, 0, 0).y, vec![moved.at], "kept, not moved");
    assert_eq!(h.state().views[0].guides.selected, None);
    untouched(&mut h, id, &bytes);
}

#[test]
fn clearing_guides_and_two_tabs_of_one_pdf() {
    let file = plans("tabs.pdf");
    let bytes = file.1.clone();
    let mut h = harness(1600.0, move |app| {
        open(app, &file);
        app.duplicate_view(0);
    });
    assert_eq!(h.state().views.len(), 2);
    let id = h.state().views[0].id;
    h.state_mut().active = Some(1);
    h.state_mut().set_option("rulers", "on").unwrap();
    h.run_steps(4);
    // Made in the second tab, the guide is the first tab's too.
    let target = on_page(&h, 1, 0, 230.0, 150.0);
    let p = top_ruler(&h, 1, target.x);
    drag(&mut h, p, target, 6, false);
    assert_eq!(guides_of(&h, 0, 0), guides_of(&h, 1, 0));
    assert_eq!(guides_of(&h, 0, 0).y.len(), 1);
    // Moved in the first tab (Hand tool), it moves in the second; each tab keeps its own zoom.
    h.state_mut().views[1].set_zoom(1.7);
    h.state_mut().active = Some(0);
    h.state_mut().quick_tool = QuickTool::Hand;
    h.run_steps(4);
    let at = guides_of(&h, 0, 0).y[0];
    let p = on_page(&h, 0, 0, 250.0, at);
    let q = on_page(&h, 0, 0, 250.0, 90.0);
    drag(&mut h, p, q, 6, false);
    assert!(near(guides_of(&h, 1, 0).y[0], 90.0, &h, 0), "{:?}", guides_of(&h, 1, 0));
    assert!((h.state().views[0].zoom - h.state().views[1].zoom).abs() > 0.1);

    // Clear Guides on Page from the ruler's menu: this page's only.
    {
        let p = &mut h.state_mut().doc_prefs;
        p.add_guide(id, 0, Axis::X, 75.0).unwrap();
        p.add_guide(id, 2, Axis::Y, 33.0).unwrap();
    }
    h.run_steps(2);
    let p = top_ruler(&h, 0, viewport(&h, 0).center().x);
    right_click(&mut h, p);
    h.get_by_label("Clear Guides on Page").click();
    h.run_steps(3);
    assert_eq!(guides_of(&h, 0, 0), Default::default());
    assert_eq!(guides_of(&h, 1, 2).y, vec![33.0], "another page keeps its guides");
    // Clear All Guides from the palette's command.
    assert!(h.state_mut().execute("view.clear_guides"));
    h.run_steps(2);
    assert_eq!(guides_of(&h, 1, 2), Default::default());
    untouched(&mut h, id, &bytes);
    let saved = h.state().persist();
    let stored: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let entry =
        stored["document_views"]["docs"].as_array().unwrap().iter().find(|d| d["path"].as_str().is_some_and(|p| p.ends_with("tabs.pdf"))).cloned();
    assert_eq!(entry.map(|e| e["guides"].clone()), Some(serde_json::json!([])), "cleared in the settings too");
}

/// Guides are drawn whether or not the page's border is: upstream's page gaps can be turned off
/// (SYNC.10b), and the guides came after the border.
#[test]
fn guides_are_drawn_with_page_gaps_on_and_off() {
    let file = plans("gaps.pdf");
    let mut h = harness(1600.0, move |app| open(app, &file));
    let id = h.state().views[0].id;
    h.state_mut().doc_prefs.add_guide(id, 0, Axis::Y, 150.0).unwrap();
    for gaps in [true, false] {
        h.state_mut().views[0].show_page_gaps = gaps;
        h.run_steps(3);
        let drawn = h.output().shapes.iter().any(|c| matches!(&c.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == guides::GUIDE));
        assert!(drawn, "page gaps {gaps}: the guide is drawn");
    }
}

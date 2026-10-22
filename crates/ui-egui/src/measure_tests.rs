//! Measuring (this fork, OV10) with real pointer events in real frames: what a click keeps, what
//! it says when it keeps nothing, and what it snaps to, in a tab, a duplicate tab and tiles.

use std::collections::HashSet;
use std::sync::Arc;

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{DocId, Session, measure::snap::SnapKind};

use crate::measure_ui::{Snapper, Tool};
use crate::{PdfCraftApp, QuickTool, ViewLayout};

/// Three lines: (50, 50)–(350, 50), (50, 50)–(50, 250) and (200, 150)–(260, 150).
const LINES: &str = "0 G 2 w 50 50 m 350 50 l S 50 50 m 50 250 l S 200 150 m 260 150 l S";

/// A 400 × 300 pt page drawing `content`, with `catalog` and `page` keys and `objects` added.
fn pdf(catalog: &str, page: &str, objects: &str, content: &str) -> Vec<u8> {
    format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R {catalog} >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents 4 0 R {page} >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n{objects}\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    )
    .into_bytes()
}

fn plain() -> Vec<u8> {
    pdf("", "", "", LINES)
}

/// `plain()` with a page viewport whose `/Measure` is `measure`.
fn with_scale(measure: &str) -> Vec<u8> {
    pdf("", &format!("/VP [<< /Type /Viewport /BBox [0 0 400 300] /Measure << /Type /Measure {measure} >> >>]"), "", LINES)
}

fn harness_with(width: f32, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(width, 1000.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

fn harness(bytes: Vec<u8>) -> (Harness<'static, PdfCraftApp>, DocId) {
    let h = harness_with(1600.0, move |app| {
        app.open_bytes("plan.pdf", None, bytes).unwrap();
    });
    let id = h.state().views[0].id;
    (h, id)
}

/// Pick a tool as the Measure panel's buttons do (which work whatever the command's state).
fn tool(h: &mut Harness<'static, PdfCraftApp>, t: Tool) {
    let app = h.state_mut();
    app.left = crate::LeftPanel::Tool("measure");
    app.left_open = true;
    app.quick_tool = QuickTool::Measure(t);
    h.run_steps(3);
}

/// Screen position of user-space point (`x`, `y`) on page 1 of view `view`.
fn at(h: &Harness<'static, PdfCraftApp>, view: usize, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    let v = &app.views[view];
    let r = v.page_screen_rect(0).expect("page on screen");
    let info = &app.session.get(v.id).unwrap().info.pages[0];
    let [vx, vy] = info.user_to_view(x, y);
    let p = pos2(r.left() + vx / info.width * r.width(), r.top() + vy / info.height * r.height());
    // A click outside the document area tests nothing (an earlier rotated-page probe did that).
    assert!(v.viewport_rect().contains(p), "({x}, {y}) is at {p:?}, outside the document area {:?}", v.viewport_rect());
    p
}

fn button(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
}

/// Hover `p` until the page's lines have been read for snapping.
fn settle(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    for _ in 0..500 {
        h.run_steps(1);
        if h.state_mut().views.iter_mut().all(|v| !v.measure.snapping_loading()) {
            // A frame that uses the lines (the worker may have finished after the last one).
            h.run_steps(2);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("snapping never finished loading");
}

/// Press at `from`, hold for `frames` frames (a quarter second each), release at `to`.
fn press_release(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, frames: usize) {
    settle(h, from);
    button(h, from, true);
    for k in 1..=frames {
        h.hover_at(from + (to - from) * (k as f32 / frames as f32));
        h.run_steps(1);
    }
    button(h, to, false);
    h.run_steps(3);
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    press_release(h, p, p, 1);
}

fn click_at(h: &mut Harness<'static, PdfCraftApp>, view: usize, x: f32, y: f32) {
    let p = at(h, view, x, y);
    click(h, p);
}

fn settle_at(h: &mut Harness<'static, PdfCraftApp>, view: usize, x: f32, y: f32) {
    let p = at(h, view, x, y);
    settle(h, p);
}

fn listing(h: &Harness<'static, PdfCraftApp>, id: DocId) -> pdfcraft_engine::measure::Listing {
    h.state().session.get(id).unwrap().measurements().unwrap()
}

fn kept(h: &Harness<'static, PdfCraftApp>, id: DocId) -> usize {
    listing(h, id).measurements.len()
}

/// The drawing scale's page side in points: it is kept in its page unit (upstream's ratio, "1 mm =
/// 50 mm"), so a calibration's two clicks read back as the distance between them.
fn drawing_pt(h: &Harness<'static, PdfCraftApp>) -> f64 {
    let app = h.state();
    let m = &app.views[0].measure;
    m.drawing_points * crate::measure_ui::page_unit_of(m.page_unit, &m.unit, app.units).points()
}

fn says(h: &Harness<'static, PdfCraftApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

fn close(a: [f64; 2], b: [f64; 2], within: f64) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= within
}

#[test]
fn the_second_click_keeps_a_distance_that_undo_delete_and_save_handle() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Distance);
    let (a, b) = (at(&h, 0, 100.0, 100.0), at(&h, 0, 300.0, 200.0));
    click(&mut h, a);
    assert_eq!((kept(&h, id), h.state().views[0].measure.points.len()), (0, 1));
    let read = h.state().views[0].measure.lines_read;
    assert_eq!(read, 1);
    click(&mut h, b);
    let saved = listing(&h, id);
    let [m] = saved.measurements.as_slice() else { panic!("{saved:?}") };
    let points = &m.points;
    assert!(close(points[0], [100.0, 100.0], 1.0) && close(points[1], [300.0, 200.0], 1.0), "{points:?}");
    assert!(m.reading.value > 0.0);
    let app = h.state();
    assert!(app.session.get(id).unwrap().dirty, "the measurement is an edit");
    assert!(app.views[0].measure.points.is_empty() && app.views[0].measure.notice.is_none());
    // The measurement doesn't change the drawing, so its lines aren't read again (and the next
    // click isn't held up while they would be).
    assert_eq!(app.views[0].measure.lines_read, read);

    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    assert_eq!(kept(&h, id), 0);
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(2);
    assert_eq!(kept(&h, id), 1);

    // Save, then reopen: the same measurement, value and caption.
    let bytes = h.state_mut().session.save_bytes(id).unwrap();
    let mut session = Session::new();
    let reopened = session.open("plan.pdf", None, Arc::new(bytes.as_ref().clone()), None).unwrap();
    let again = session.get(reopened).unwrap().measurements().unwrap();
    assert_eq!(again.measurements, saved.measurements);

    // Select it in the panel's list, then Delete.
    h.get_by_label(&format!("Page 1: {} ", m.reading.label)).click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    let away = at(&h, 0, 380.0, 280.0);
    h.hover_at(away);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(kept(&h, id), 0, "Delete removes a selected measurement");
}

#[test]
fn a_slow_or_drifting_second_click_still_finishes_the_distance() {
    for (name, frames, drift) in [("held for a second", 4, 0.0), ("moved 8 px while pressed", 2, 8.0)] {
        let (mut h, id) = harness(plain());
        tool(&mut h, Tool::Distance);
        click_at(&mut h, 0, 100.0, 100.0);
        let b = at(&h, 0, 300.0, 200.0);
        press_release(&mut h, b, b + egui::vec2(drift, 0.0), frames);
        assert_eq!(kept(&h, id), 1, "{name}");
        // The point is where the button came up.
        let end = listing(&h, id).measurements[0].points[1];
        let pixels = f64::from(h.state().views[0].page_screen_rect(0).unwrap().width()) / 400.0;
        assert!(close(end, [300.0 + f64::from(drift) / pixels, 200.0], 1.0), "{name}: {end:?}");
    }
}

#[test]
fn a_second_click_on_the_first_point_says_to_click_another() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Distance);
    // Both clicks snap onto the end of the bottom line.
    click_at(&mut h, 0, 351.0, 51.0);
    click_at(&mut h, 0, 349.0, 49.5);
    assert_eq!((kept(&h, id), h.state().views[0].measure.points.len()), (0, 1));
    assert!(says(&h, "Click a different point"));
    click_at(&mut h, 0, 300.0, 200.0);
    assert_eq!(kept(&h, id), 1);
    assert!(!says(&h, "Click a different point"));
}

#[test]
fn a_drawing_scale_that_cant_be_read_is_explained_and_feet_and_inches_are_read() {
    let fraction =
        "/Subtype /RL /R (1 in = 1 in) /X [<< /U (in) /C 0.0138889 /D 16 /F /F >>] /D [<< /U (in) /C 1 /D 16 /F /F >>] /A [<< /U (sq in) /C 1 >>]";
    for (name, measure) in [("fractional inches", fraction), ("geospatial", "/Subtype /GEO")] {
        let (mut h, id) = harness(with_scale(measure));
        tool(&mut h, Tool::Distance);
        click_at(&mut h, 0, 100.0, 100.0);
        assert_eq!((kept(&h, id), h.state().views[0].measure.points.len()), (0, 0), "{name}");
        assert!(says(&h, "drawing scale PdfCraft can't read"), "{name}: nothing said");
        // Calibrating sets a scale: it isn't stopped by the one it replaces.
        tool(&mut h, Tool::Calibrate);
        click_at(&mut h, 0, 100.0, 100.0);
        click_at(&mut h, 0, 300.0, 100.0);
        assert!((drawing_pt(&h) - 200.0).abs() < 1.0, "{name}");
    }
    // An Acrobat-style architectural scale, 1 in = 4 ft, in feet and inches.
    let (mut h, id) = harness(with_scale(
        "/Subtype /RL /R (1 in = 4 ft) /X [<< /U (ft) /C 0.0555556 /D 1 /F /D >> << /U (in) /C 12 /D 8 /F /F >>] \
         /D [<< /U (ft) /C 1 /D 1 /F /D >> << /U (in) /C 12 /D 8 /F /F >>] /A [<< /U (sq ft) /C 1 /D 100 >>]",
    ));
    tool(&mut h, Tool::Distance);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 200.0);
    let label = listing(&h, id).measurements.first().map(|m| m.reading.label.clone());
    // 223.6 pt is 12.42 ft.
    assert!(label.as_deref().is_some_and(|l| l.starts_with("12'-") && l.ends_with('"')), "{label:?}");
}

#[test]
fn a_pdf_that_forbids_comments_measures_but_says_nothing_is_kept() {
    let restricted = {
        let mut s = Session::new();
        let id = s.open("plan.pdf", None, Arc::new(plain()), None).unwrap();
        let p = pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            changes: pdfcraft_engine::Changes::None,
            ..pdfcraft_engine::Protection::default()
        };
        s.apply(id, pdfcraft_engine::Edit::Protect(p)).unwrap();
        s.save_bytes(id).unwrap().as_ref().clone()
    };
    let (mut h, id) = harness(restricted);
    assert!(!h.state().session.get(id).unwrap().allows_annotation());
    tool(&mut h, Tool::Distance);
    settle_at(&mut h, 0, 100.0, 100.0);
    assert!(says(&h, "doesn't allow comments"), "said before the first click");
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 200.0);
    let app = h.state();
    assert_eq!(kept(&h, id), 0);
    assert!(!app.session.get(id).unwrap().dirty);
    let reading = app.views[0].measure.reading.as_ref().expect("the reading stays");
    assert!(reading.value > 0.0);
    assert!(says(&h, "doesn't allow comments"));
}

#[test]
fn the_snap_target_shows_before_the_first_point_with_every_tool() {
    for t in [Tool::Distance, Tool::Perimeter, Tool::Area, Tool::Calibrate] {
        let (mut h, id) = harness(plain());
        tool(&mut h, t);
        settle_at(&mut h, 0, 351.0, 51.0);
        let m = &h.state().views[0].measure;
        assert!(m.points.is_empty());
        assert_eq!(m.snapped, Some(([350.0, 50.0], SnapKind::Endpoint)), "{t:?}");
        assert!(h.query_by_label("Endpoint").is_some(), "{t:?}: the target is named before the first point");
        click_at(&mut h, 0, 351.0, 51.0);
        assert_eq!(h.state().views[0].measure.points, vec![[350.0, 50.0]], "{t:?}: the first point snaps");
        click_at(&mut h, 0, 51.5, 249.0);
        match t {
            Tool::Distance => assert_eq!(listing(&h, id).measurements[0].points, vec![[350.0, 50.0], [50.0, 250.0]]),
            Tool::Calibrate => assert!((drawing_pt(&h) - 300f64.hypot(200.0)).abs() < 1e-9),
            _ => assert_eq!(h.state().views[0].measure.points, vec![[350.0, 50.0], [50.0, 250.0]], "{t:?}"),
        }
    }
}

#[test]
fn no_point_is_placed_while_the_drawing_is_read_for_snapping() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Distance);
    let hold = |h: &mut Harness<'static, PdfCraftApp>| {
        let app = h.state_mut();
        let doc = app.session.get(id).unwrap();
        app.views[0].measure.hold_snapping(doc, 0)
    };
    let tx = hold(&mut h);
    let p = at(&h, 0, 351.0, 51.0);
    h.hover_at(p);
    h.run_steps(2);
    assert!(h.state_mut().views[0].measure.snapping_loading());
    assert!(says(&h, "Reading the drawing for snapping"));
    button(&mut h, p, true);
    h.run_steps(1);
    button(&mut h, p, false);
    h.run_steps(3);
    // The release waited (half a second) for lines that never came: no point, and it says why.
    assert!(h.state().views[0].measure.points.is_empty(), "no unsnapped point while loading");
    assert_eq!(h.state().views[0].measure.click_waits, 1);
    assert!(says(&h, "Reading the drawing for snapping"));
    // The lines arrive: the same click snaps.
    let bytes = h.state().session.get(id).unwrap().bytes.clone();
    tx.send(Snapper::read(bytes, None, 0, &HashSet::new())).unwrap();
    click(&mut h, p);
    assert_eq!(h.state().views[0].measure.points, vec![[350.0, 50.0]]);
    assert!(!says(&h, "Reading the drawing for snapping"));
    assert_eq!(kept(&h, id), 0);

    // Snap to drawing off is the way around a slow sheet: the click lands where it is.
    h.state_mut().views[0].measure.cancel();
    let _held = hold(&mut h);
    h.state_mut().views[0].measure.snap_enabled = false;
    button(&mut h, p, true);
    h.run_steps(1);
    button(&mut h, p, false);
    h.run_steps(3);
    let points = h.state().views[0].measure.points.clone();
    assert!(points.len() == 1 && close(points[0], [351.0, 51.0], 1.0) && points[0] != [350.0, 50.0], "{points:?}");
    assert_eq!(h.state().views[0].measure.click_waits, 1, "with snapping off nothing waits");
}

/// Windows build #22: the control channel's click (one hover frame, then press and release in
/// the next) came before the worker had read the page's lines, and was lost, because frames run
/// back to back. A worker 100 ms behind stands in for the slow runner, deterministically.
#[test]
fn a_click_while_the_lines_load_waits_for_them() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Distance);
    let tx = {
        let app = h.state_mut();
        let doc = app.session.get(id).unwrap();
        app.views[0].measure.hold_snapping(doc, 0)
    };
    let bytes = h.state().session.get(id).unwrap().bytes.clone();
    let slow = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = tx.send(Snapper::read(bytes, None, 0, &HashSet::new()));
    });
    let p = at(&h, 0, 351.0, 51.0);
    h.hover_at(p);
    h.run_steps(1);
    // The press alone places nothing, so it doesn't wait.
    button(&mut h, p, true);
    h.run_steps(1);
    assert_eq!((h.state().views[0].measure.click_waits, h.state().views[0].measure.points.len()), (0, 0));
    // The release does: it waits for the lines once and lands snapped, in its own frame.
    button(&mut h, p, false);
    h.run_steps(1);
    let m = &h.state().views[0].measure;
    assert_eq!(m.points, vec![[350.0, 50.0]], "the click is placed, snapped, in its own frame");
    assert_eq!(m.click_waits, 1);
    h.run_steps(3);
    assert_eq!(h.state().views[0].measure.click_waits, 1, "nothing else waits once the lines are in");
    slow.join().unwrap();
}

#[test]
fn hidden_layers_and_dense_sheets_snap_as_they_are_shown() {
    // A line in a layer, (100, 220)–(150, 220).
    let layered = pdf(
        "/OCProperties << /OCGs [5 0 R] /D << /ON [5 0 R] >> >>",
        "/Resources << /Properties << /L1 5 0 R >> >>",
        "5 0 obj << /Type /OCG /Name (Grid) >> endobj\n",
        &format!("{LINES}\n/OC /L1 BDC 100 220 m 150 220 l S EMC"),
    );
    let (mut h, id) = harness(layered);
    tool(&mut h, Tool::Distance);
    let end = at(&h, 0, 150.5, 220.5);
    settle(&mut h, end);
    assert_eq!(h.state().views[0].measure.snapped, Some(([150.0, 220.0], SnapKind::Endpoint)));
    assert!(h.state_mut().session.set_layer_visible(id, 0, false));
    settle(&mut h, end);
    assert_eq!(h.state().views[0].measure.snapped, None, "a hidden layer's line isn't snapped to");

    // 25,000 short segments first, then the line to snap to: past the 20,000 segments the
    // measuring snapper reads.
    let mut dense = String::from("0 G 0.1 w\n");
    for i in 0..25_000u32 {
        let (x, y) = (300.0 + f64::from(i % 160) * 0.5, 200.0 + f64::from(i / 160) * 0.5);
        dense.push_str(&format!("{x} {y} m {} {y} l\n", x + 0.2));
    }
    dense.push_str("S 2 w 100 100 m 160 100 l S");
    let (mut h, _) = harness(pdf("", "", "", &dense));
    tool(&mut h, Tool::Distance);
    settle_at(&mut h, 0, 160.5, 100.5);
    assert_eq!(h.state().views[0].measure.snapped, Some(([160.0, 100.0], SnapKind::Endpoint)));
}

#[test]
fn a_duplicate_tab_and_a_tile_keep_their_measurements() {
    let mut h = harness_with(1600.0, |app| {
        app.open_bytes("plan.pdf", None, plain()).unwrap();
        app.duplicate_view(0);
    });
    assert_eq!(h.state().views.len(), 2);
    let view = h.state().active.unwrap();
    let id = h.state().views[view].id;
    tool(&mut h, Tool::Distance);
    click_at(&mut h, view, 100.0, 100.0);
    click_at(&mut h, view, 300.0, 200.0);
    assert_eq!(kept(&h, id), 1, "the duplicate tab keeps it");

    let mut h = harness_with(2600.0, |app| {
        app.open_bytes("plan.pdf", None, plain()).unwrap();
        app.open_bytes("detail.pdf", None, plain()).unwrap();
        app.set_view_layout(ViewLayout::Tiles);
        app.active = Some(0);
    });
    let (first, second) = (h.state().views[0].id, h.state().views[1].id);
    tool(&mut h, Tool::Distance);
    // A press in the tile that isn't active only picks it (tile_view.rs, D1).
    click_at(&mut h, 1, 100.0, 100.0);
    assert_eq!((h.state().active, h.state().views[1].measure.points.len()), (Some(1), 0));
    click_at(&mut h, 1, 100.0, 100.0);
    click_at(&mut h, 1, 300.0, 200.0);
    h.run_steps(3);
    assert_eq!((kept(&h, first), kept(&h, second)), (0, 1), "a tile keeps its own measurement");
}

/// The known-distance field for feet and inches (showing what was typed or converted).
fn feet_field<'a>(h: &'a Harness<'static, PdfCraftApp>, shown: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by(|n| n.value().as_deref() == Some(shown) && n.role() == egui::accesskit::Role::TextInput)
        .next()
        .unwrap_or_else(|| panic!("no feet-and-inches field showing {shown:?}"))
}

fn type_feet(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    let shown = h.state().views[0].measure.known_text.clone();
    feet_field(h, &shown).click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    feet_field(h, &shown).type_text(text);
    h.run_steps(2);
    assert_eq!(h.state().views[0].measure.known_text, text);
}

fn pick_unit(h: &mut Harness<'static, PdfCraftApp>, from: &str, to: &str) {
    h.get_by_value(from).click();
    h.run_steps(2);
    h.get_by_label(to).click();
    h.run_steps(2);
}

#[test]
fn calibrating_in_feet_and_inches_from_the_panel() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Calibrate);
    // Both ends of the bottom line, snapped: 300 pt apart.
    click_at(&mut h, 0, 50.5, 50.5);
    click_at(&mut h, 0, 349.5, 50.5);
    assert!((drawing_pt(&h) - 300.0).abs() < 1e-9);
    assert!(says(&h, "Type the real distance between the two points"));
    assert!(
        h.query_all_by(|n| n.is_focused() && n.role() == egui::accesskit::Role::SpinButton).next().is_some(),
        "the known distance takes the keyboard"
    );
    assert!(!says(&h, "All pages"), "a scale is set per page or region");

    // Inches to feet and inches: the 1 in the page started with is converted, not reset.
    pick_unit(&mut h, "Inches (in)", "Feet and inches (ft-in)");
    let m = &h.state().views[0].measure;
    assert!(m.feet_inches && m.unit == "ft" && m.known_text == "0'-1\"", "{} {}", m.unit, m.known_text);
    assert!(says(&h, "Nearest 1/8 inch"));
    type_feet(&mut h, "100'-0\"");
    assert!((h.state().views[0].measure.real_distance - 100.0).abs() < 1e-12);
    let read = h.state().views[0].measure.lines_read;
    h.get_by_label("Apply scale").click();
    h.run_steps(3);
    let scale = h.state().session.get(id).unwrap().measurement_scale(0, [100.0, 100.0]).unwrap();
    assert_eq!((scale.unit.as_str(), scale.feet_inches), ("ft", Some(8)));
    assert!(!says(&h, "Type the real distance between the two points"));

    // 200 pt at 300 pt = 100 ft is 66'-8".
    tool(&mut h, Tool::Distance);
    click_at(&mut h, 0, 50.5, 50.5);
    click_at(&mut h, 0, 50.5, 249.5);
    let listed = listing(&h, id);
    assert_eq!(listed.measurements[0].reading.label, "66'-8\"");
    // The new scale is used at once, and the page's lines weren't read again for it.
    assert_eq!(h.state().views[0].measure.lines_read, read);
    let bytes = h.state_mut().session.save_bytes(id).unwrap();
    let mut session = Session::new();
    let reopened = session.open("plan.pdf", None, Arc::new(bytes.as_ref().clone()), None).unwrap();
    let again = session.get(reopened).unwrap().measurements().unwrap();
    assert_eq!((again.measurements[0].reading.label.as_str(), again.unsupported.len()), ("66'-8\"", 0));

    // What isn't a length is said and not applied.
    type_feet(&mut h, "twelve feet");
    assert!(says(&h, "Type a length such as"));
    let edits = h.state().session.get(id).unwrap().edit_generation();
    h.get_by_label("Apply scale").click();
    h.run_steps(3);
    assert_eq!(h.state().session.get(id).unwrap().edit_generation(), edits, "no scale from the last length that could be read");
    type_feet(&mut h, "12' 6\"");
    assert!((h.state().views[0].measure.real_distance - 12.5).abs() < 1e-12);

    // Feet and inches to metres keeps the scale: 12.5 ft is 3.81 m.
    pick_unit(&mut h, "Feet and inches (ft-in)", "Meters (m)");
    let m = &h.state().views[0].measure;
    assert!(!m.feet_inches && m.unit == "m" && (m.real_distance - 3.81).abs() < 1e-9, "{} {}", m.unit, m.real_distance);
    assert!(!says(&h, "Nearest 1/8 inch"), "decimals again");
    // Custom keeps the label as it is, to type another.
    pick_unit(&mut h, "Meters (m)", "Custom");
    let m = &h.state().views[0].measure;
    assert!(m.custom_unit && m.unit == "m");
    assert!(h.query_all_by_value("m").next().is_some(), "the unit is typed in");
}

/// H3 of the "nothing was kept" report: page dictionaries a little out of the ordinary keep the
/// measurement too.
#[test]
fn unusual_annotation_arrays_and_rotated_pages_keep_the_measurement() {
    for (name, page, objects) in [
        ("an annotation written inline in /Annots", "/Annots [<< /Type /Annot /Subtype /Square /Rect [10 10 20 20] >>]", ""),
        ("an indirect /Annots array", "/Annots 5 0 R", "5 0 obj [] endobj\n"),
        ("/Rotate 90", "/Rotate 90", ""),
        ("/Rotate 270 and a crop box", "/Rotate 270 /CropBox [20 10 380 290]", ""),
    ] {
        let (mut h, id) = harness(pdf("", page, objects, LINES));
        // A turned page stands taller than the window at the default zoom: show all of it.
        h.state_mut().set_option("zoom", "100").unwrap();
        h.run_steps(3);
        tool(&mut h, Tool::Distance);
        click_at(&mut h, 0, 100.0, 100.0);
        click_at(&mut h, 0, 300.0, 200.0);
        let listed = listing(&h, id);
        assert_eq!(listed.measurements.len(), 1, "{name}: {listed:?}");
        let points = &listed.measurements[0].points;
        // Points are listed in displayed coordinates; their distance doesn't depend on the turn.
        let shown = (points[1][0] - points[0][0]).hypot(points[1][1] - points[0][1]);
        assert!((shown - 200f64.hypot(100.0)).abs() < 1.5, "{name}: {points:?}");
    }
}

// ── Escape: one step back per press (this fork, OV11) ───────────────────────────────────────

fn escape(h: &mut Harness<'static, PdfCraftApp>) {
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
}

/// Nothing was edited: no change to save and no undo step.
fn untouched(h: &Harness<'static, PdfCraftApp>, id: DocId) {
    let doc = h.state().session.get(id).unwrap();
    assert!(!doc.dirty, "Escape changed the document");
    assert_eq!(doc.can_undo(), None, "Escape added an undo step");
}

#[test]
fn escape_cancels_the_points_in_progress_then_leaves_the_tool() {
    for (t, points) in [(Tool::Distance, 1), (Tool::Perimeter, 3), (Tool::Area, 3)] {
        let (mut h, id) = harness(plain());
        tool(&mut h, t);
        for (x, y) in [(100.0, 100.0), (300.0, 200.0), (300.0, 100.0)].into_iter().take(points) {
            click_at(&mut h, 0, x, y);
        }
        assert_eq!(h.state().views[0].measure.points.len(), points, "{t:?}");
        escape(&mut h);
        let app = h.state();
        assert!(app.views[0].measure.points.is_empty() && app.views[0].measure.reading.is_none(), "{t:?}: the first press cancels");
        assert_eq!(app.quick_tool, QuickTool::Measure(t), "{t:?}: the first press keeps the tool");
        assert_eq!(kept(&h, id), 0, "{t:?}: nothing is kept");
        untouched(&h, id);
        escape(&mut h);
        assert_eq!(h.state().quick_tool, QuickTool::Select, "{t:?}: the second press leaves the idle tool");
        untouched(&h, id);
    }
}

#[test]
fn escape_after_a_measurement_leaves_the_tool_and_keeps_the_measurement() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Distance);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 200.0);
    assert_eq!(kept(&h, id), 1);
    escape(&mut h);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(kept(&h, id), 1, "the measurement stays");
}

#[test]
fn escape_cancels_a_pending_calibration_and_puts_the_scale_fields_back() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Calibrate);
    let before = {
        let m = &h.state().views[0].measure;
        (m.drawing_points, m.real_distance, m.unit.clone(), m.known_text.clone(), m.page_unit)
    };
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 100.0);
    let m = &h.state().views[0].measure;
    assert!(m.calibrating() && m.points.is_empty(), "two clicks ask for the known distance");
    assert!((drawing_pt(&h) - 200.0).abs() < 1.0, "{}", m.drawing_points);
    // The known-distance field takes the keyboard; Escape there is the calibration's own step.
    assert!(h.ctx.memory(|mem| mem.focused()).is_some(), "the known-distance field has the keyboard");
    escape(&mut h);
    let m = &h.state().views[0].measure;
    assert!(!m.calibrating(), "the first press cancels the calibration");
    assert_eq!((m.drawing_points, m.real_distance, m.unit.clone(), m.known_text.clone(), m.page_unit), before, "the fields are back");
    assert_eq!(h.state().quick_tool, QuickTool::Measure(Tool::Calibrate), "the tool stays for the next press");
    untouched(&h, id);
    escape(&mut h);
    assert_eq!(h.state().quick_tool, QuickTool::Select);

    // Without the field focused (clicked away to the page's margin), the same single step.
    tool(&mut h, Tool::Calibrate);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 100.0);
    h.ctx.memory_mut(|mem| mem.stop_text_input());
    h.run_steps(2);
    escape(&mut h);
    assert!(!h.state().views[0].measure.calibrating());
    assert_eq!(h.state().quick_tool, QuickTool::Measure(Tool::Calibrate));
    untouched(&h, id);
}

#[test]
fn a_pending_calibration_is_not_taken_for_points_in_progress() {
    // A first point of a new pair while the known distance is still asked for: the points go
    // first, the calibration with the next press, the tool with the one after.
    let (mut h, _) = harness(plain());
    tool(&mut h, Tool::Calibrate);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 100.0);
    h.ctx.memory_mut(|mem| mem.stop_text_input());
    click_at(&mut h, 0, 100.0, 200.0);
    let m = &h.state().views[0].measure;
    assert!(m.calibrating() && m.points.len() == 1);
    escape(&mut h);
    let m = &h.state().views[0].measure;
    assert!(m.calibrating() && m.points.is_empty(), "the points first");
    escape(&mut h);
    assert!(!h.state().views[0].measure.calibrating(), "then the calibration");
    assert_eq!(h.state().quick_tool, QuickTool::Measure(Tool::Calibrate));
    escape(&mut h);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn escape_in_a_measure_panel_field_only_leaves_the_field() {
    let (mut h, id) = harness(plain());
    tool(&mut h, Tool::Perimeter);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 200.0);
    let label = egui::Id::new("measure-label");
    h.ctx.memory_mut(|mem| mem.request_focus(label));
    h.run_steps(2);
    assert_eq!(h.ctx.memory(|mem| mem.focused()), Some(label));
    escape(&mut h);
    assert_eq!(h.state().views[0].measure.points.len(), 2, "the perimeter in progress stays");
    assert_eq!(h.state().quick_tool, QuickTool::Measure(Tool::Perimeter));
    assert_eq!(h.ctx.memory(|mem| mem.focused()), None, "the field let go of the keyboard");
    escape(&mut h);
    assert!(h.state().views[0].measure.points.is_empty(), "the next press cancels it");
    untouched(&h, id);
}

#[test]
fn one_escape_never_also_closes_the_find_bar_or_full_screen() {
    let (mut h, id) = harness(plain());
    let steps = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        (app.views[0].measure.points.len(), app.views[0].find.is_some(), app.quick_tool, app.full_screen)
    };
    // Measuring with the find bar open: the points, then the find bar, then the tool.
    tool(&mut h, Tool::Distance);
    h.state_mut().views[0].find = Some(Default::default());
    h.run_steps(3);
    click_at(&mut h, 0, 100.0, 100.0);
    assert_eq!(steps(&h), (1, true, QuickTool::Measure(Tool::Distance), false));
    escape(&mut h);
    assert_eq!(steps(&h), (0, true, QuickTool::Measure(Tool::Distance), false), "the points only");
    escape(&mut h);
    assert_eq!(steps(&h), (0, false, QuickTool::Measure(Tool::Distance), false), "then the find bar");
    escape(&mut h);
    assert_eq!(steps(&h), (0, false, QuickTool::Select, false), "then the tool");

    // Full screen hides the tools and selects Select there, giving the tool back on leaving
    // (upstream, review on #2070), so nothing is measured in it. With the find bar open, one
    // press closes the find bar and the next leaves full screen, never both at once.
    tool(&mut h, Tool::Distance);
    h.state_mut().views[0].find = Some(Default::default());
    let ctx = h.ctx.clone();
    h.state_mut().set_full_screen(&ctx, true);
    h.run_steps(3);
    assert_eq!(steps(&h), (0, true, QuickTool::Select, true));
    escape(&mut h);
    assert_eq!(steps(&h), (0, false, QuickTool::Select, true), "the find bar only");
    escape(&mut h);
    assert_eq!(steps(&h), (0, false, QuickTool::Measure(Tool::Distance), false), "then full screen, the tool back");
    untouched(&h, id);
}

#[test]
fn escape_for_a_dialog_the_palette_or_the_wizard_keeps_the_points() {
    for over in ["dialog", "palette", "wizard"] {
        let (mut h, id) = harness(plain());
        tool(&mut h, Tool::Distance);
        click_at(&mut h, 0, 100.0, 100.0);
        match over {
            "dialog" => h.state_mut().dialog = Some(crate::Dialog::Preferences),
            "palette" => h.state_mut().palette_open = true,
            _ => h.state_mut().overlay_open(),
        }
        h.run_steps(4);
        if over == "wizard" {
            // The pointer over the wizard's window, which covers the page.
            let r = h.ctx.memory(|mem| mem.area_rect(crate::overlay_ui::wizard_id())).expect("the wizard is shown");
            h.hover_at(r.center());
            h.run_steps(2);
        }
        escape(&mut h);
        let app = h.state();
        assert_eq!(app.views[0].measure.points.len(), 1, "{over}: Escape there leaves the measurement alone");
        assert_eq!(app.quick_tool, QuickTool::Measure(Tool::Distance), "{over}");
        match over {
            "dialog" => assert!(app.dialog.is_none(), "the dialog closes"),
            "palette" => assert!(!app.palette_open, "the palette closes"),
            _ => {}
        }
        untouched(&h, id);
    }
}

#[test]
fn the_wizard_over_the_page_doesnt_drive_the_measure_tool() {
    let (mut h, _) = harness(plain());
    tool(&mut h, Tool::Distance);
    // The middle of the line from (200, 150) to (260, 150): a snap target while nothing covers it.
    let p = at(&h, 0, 230.0, 150.0);
    settle(&mut h, p);
    assert_eq!(h.state().views[0].measure.snapped, Some(([230.0, 150.0], SnapKind::Midpoint)));
    h.state_mut().overlay_open();
    h.run_steps(4);
    let r = h.ctx.memory(|mem| mem.area_rect(crate::overlay_ui::wizard_id())).expect("the wizard is shown");
    assert!(r.contains(p), "the wizard covers that point");
    h.hover_at(p + egui::vec2(0.5, 0.0));
    h.run_steps(3);
    assert_eq!(h.state().views[0].measure.snapped, None, "no snap target through the wizard");
}

#[test]
fn escape_in_an_inactive_tile_leaves_its_measurement_alone() {
    let mut h = harness_with(2600.0, |app| {
        app.open_bytes("plan.pdf", None, plain()).unwrap();
        app.open_bytes("detail.pdf", None, plain()).unwrap();
        app.set_view_layout(ViewLayout::Tiles);
        app.active = Some(0);
    });
    tool(&mut h, Tool::Perimeter);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 200.0);
    h.state_mut().active = Some(1);
    h.run_steps(2);
    escape(&mut h);
    assert_eq!(h.state().views[0].measure.points.len(), 2, "the inactive tile keeps its perimeter");
    assert_eq!(h.state().quick_tool, QuickTool::Select, "the active tile had nothing to cancel");
}

#[test]
fn backspace_removes_a_vertex_and_never_a_selected_comment_too() {
    let annot = "5 0 obj << /Type /Annot /Subtype /Square /Rect [300 200 360 260] /C [1 0 0] >> endobj\n";
    let (mut h, id) = harness(pdf("", "/Annots [5 0 R]", annot, LINES));
    tool(&mut h, Tool::Perimeter);
    click_at(&mut h, 0, 100.0, 100.0);
    click_at(&mut h, 0, 300.0, 50.0);
    // A comment selected meanwhile (a right-click selects one with any tool).
    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.key_press(egui::Key::Backspace);
    h.run_steps(3);
    assert_eq!(h.state().views[0].measure.points.len(), 1, "Backspace removed the last vertex");
    assert_eq!(h.state().session.get(id).unwrap().info.annotations.len(), 1, "and not the comment");
    untouched(&h, id);
    // Picking a measuring tool leaves no comment selected for a later Backspace to delete.
    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.state_mut().execute("measure.distance");
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.selected, None);
}

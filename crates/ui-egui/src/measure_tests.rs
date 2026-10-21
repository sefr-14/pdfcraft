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
    pos2(r.left() + vx / info.width * r.width(), r.top() + vy / info.height * r.height())
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
        assert!((h.state().views[0].measure.drawing_points - 200.0).abs() < 1.0, "{name}");
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
            Tool::Calibrate => assert!((h.state().views[0].measure.drawing_points - 300f64.hypot(200.0)).abs() < 1e-9),
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
    assert!(h.state().views[0].measure.points.is_empty(), "no unsnapped point while loading");
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

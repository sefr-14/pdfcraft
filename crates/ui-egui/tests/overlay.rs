//! Overlay compare (this fork) in the real shell (egui_kittest): the toolbar button opens the
//! six-step wizard, Back and Next keep every setting, and Generate opens the overlay as a new,
//! unsaved tab with exactly two layers, Old and New, leaving the sources unchanged.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

/// A two-page PDF whose pages draw a box at `x` (so the two revisions differ).
fn drawing(x: u32) -> Vec<u8> {
    drawing_sized(x, [(300, 200), (300, 200)])
}

/// [`drawing`] with each page's sheet size in points.
fn drawing_sized(x: u32, sizes: [(u32, u32); 2]) -> Vec<u8> {
    drawing_moved(x, 0, sizes)
}

/// [`drawing_sized`] with the whole drawing moved `dx` pt to the right.
fn drawing_moved(x: u32, dx: u32, sizes: [(u32, u32); 2]) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into()];
    for (i, (w, h)) in (0..2u32).zip(sizes) {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R >>", 4 + 2 * i));
        let shift = if dx == 0 { String::new() } else { format!("1 0 0 1 {dx} 0 cm ") };
        let body = format!("{shift}0 g 2 w 20 20 m 280 180 l S {} 60 40 40 re f", x + 10 * i);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

/// An `n`-page drawing set whose page `i` draws a box at a height that identifies the sheet.
fn sheets(n: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objs: Vec<String> =
        vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 300 200] >>", kids.join(" "))];
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 4 + 2 * i));
        let body = format!("0 g 20 {} 40 10 re f", 10 + 5 * i);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(drawing(100), drawing(120))
}

/// The app with document A (the active tab) and document B open.
fn harness_with(a: Vec<u8>, b: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 1000.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // B first, so A (the active tab) is the second.
        app.open_bytes("rev-b.pdf", None, b).unwrap();
        app.open_bytes("rev-a.pdf", None, a).unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// The wizard opened on A and taken to step 4, Sheet size & Alignment.
fn to_sheet_and_alignment(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.state_mut().execute("doc.overlay"));
    h.run_steps(3);
    for _ in 0..3 {
        next(h);
    }
    h.get_by_label("4. Sheet size & Alignment");
}

fn next_disabled(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.get_by_label("Next").accesskit_node().is_disabled()
}

fn next(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_label("Next").click();
    h.run_steps(3);
}

fn back(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_label("Back").click();
    h.run_steps(3);
}

/// Run frames until `done` holds (background work finishes on its own threads).
fn wait(h: &mut Harness<'static, PdfCraftApp>, done: impl Fn(&PdfCraftApp) -> bool) {
    for _ in 0..2400 {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("timed out");
}

#[test]
fn the_toolbar_button_opens_the_wizard() {
    let mut h = harness();
    h.get_by_label("Overlay").click();
    h.run_steps(3);
    h.get_by_label("1. Documents");
    h.get_by_label("Document A:");
    // B defaults to the other open document, so Next is open.
    let (a, b) = {
        let w = h.state();
        (w.views[1].id, w.views[0].id)
    };
    assert_eq!(h.state().overlay_documents(), Some((a, Some(b))));
}

#[test]
fn back_and_next_keep_settings_and_generate_opens_a_layered_tab() {
    let mut h = harness();
    let sources: Vec<_> = h.state().views.iter().map(|v| h.state().session.get(v.id).unwrap().bytes.clone()).collect();
    assert!(h.state_mut().execute("doc.overlay"));
    h.run_steps(3);
    h.get_by_label("1. Documents");
    next(&mut h);
    h.get_by_label("2. Pages");
    next(&mut h);
    h.get_by_label("3. Colours");
    // A setting changed here survives a trip back to the start and forward again.
    assert_eq!(h.get_by_label("Keep the colours complementary").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    h.get_by_label("Keep the colours complementary").click();
    h.run_steps(2);
    back(&mut h);
    back(&mut h);
    h.get_by_label("1. Documents");
    next(&mut h);
    next(&mut h);
    h.get_by_label("3. Colours");
    assert_eq!(h.get_by_label("Keep the colours complementary").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
    next(&mut h);
    h.get_by_label("4. Sheet size & Alignment");
    // Six steps; sheet sizes and alignment share step 4.
    for step in ["1 Documents", "2 Pages", "3 Colours", "4 Sheet size & Alignment", "5 Preview", "6 Generate"] {
        h.get_by_label(step);
    }
    assert!(h.query_by_label_contains("Sheet sizes").is_none() && h.query_by_label_contains("7 ").is_none());
    // Equal sheets: 1:1 on one line, nothing to confirm; no points picked is a 1:1 placement.
    h.get_by_label("Old: 300 × 200 pt · New: 300 × 200 pt · Scale: 1:1");
    assert!(!next_disabled(&h));
    next(&mut h);
    h.get_by_label("5. Preview");
    back(&mut h);
    h.get_by_label("4. Sheet size & Alignment");
    next(&mut h);
    h.get_by_label("5. Preview");
    next(&mut h);
    h.get_by_label("6. Generate");
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    let app = h.state();
    let (_, id) = app.active_ids().unwrap();
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.name, "Overlay rev-a vs rev-b.pdf");
    assert!(doc.dirty, "the overlay is a new, unsaved document");
    assert_eq!(doc.info.pages.len(), 2);
    let layers: Vec<&str> = doc.info.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(layers, ["Old", "New"]);
    // The sources are unchanged.
    for (v, before) in app.views.iter().take(2).zip(&sources) {
        assert_eq!(app.session.get(v.id).unwrap().bytes, *before);
    }
    h.get_by_label_contains("Created Overlay rev-a vs rev-b.pdf");
}

/// A primary click at `pos` (the image views pick on a click, not a drag).
fn click_at(h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2) {
    click_with(h, pos, egui::Modifiers::NONE);
}

/// A primary click at `pos` with `modifiers` held throughout.
fn click_with(h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2, modifiers: egui::Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers });
        h.run_steps(1);
    }
    h.run_steps(2);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
}

fn settle(h: &mut Harness<'static, PdfCraftApp>, frames: usize) {
    for _ in 0..frames {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// Where PDF point (x, y) of a 300 × 200 pt page shows in an image view at `view` (the page is
/// fitted and centred).
fn on_page(view: egui::Rect, x: f32, y: f32) -> egui::Pos2 {
    let fit = (view.width() / 300.0).min(view.height() / 200.0);
    let img = egui::Rect::from_center_size(view.center(), egui::vec2(300.0, 200.0) * fit);
    egui::pos2(img.left() + x / 300.0 * img.width(), img.top() + (1.0 - y / 200.0) * img.height())
}

/// Writes screenshots of the steps into the directory PDFCRAFT_SHOT names (for review), with two
/// reference points picked on the Sheet size & Alignment step.
#[test]
fn wizard_screenshots() {
    let Ok(dir) = std::env::var("PDFCRAFT_SHOT") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.get_by_label("Overlay").click();
    h.run_steps(3);
    for step in 1..=6 {
        // Let renders finish on their threads.
        settle(&mut h, if step >= 4 { 200 } else { 20 });
        if step == 4 {
            pick_the_box(&mut h);
        }
        h.render().unwrap().save(dir.join(format!("step-{step}.png"))).unwrap();
        if step < 6 {
            next(&mut h);
        }
    }
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    settle(&mut h, 20);
    h.render().unwrap().save(dir.join("step-6-done.png")).unwrap();
}

/// On the Sheet size & Alignment step: the box's top-left and bottom-right corners on Old (A: 100,
/// 100 and 140, 60), then the same corners on New (B), where the box is 20 pt further right.
fn pick_the_box(h: &mut Harness<'static, PdfCraftApp>) {
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    for (ax, ay) in [(100.0, 100.0), (140.0, 60.0)] {
        click_at(h, on_page(va, ax, ay));
    }
    for (bx, by) in [(120.0, 100.0), (160.0, 60.0)] {
        click_at(h, on_page(vb, bx, by));
    }
}

/// Wait until both drawings have rendered (and, with `lines`, their lines are read for snapping).
fn wait_for_drawings(h: &mut Harness<'static, PdfCraftApp>, lines: bool) {
    for _ in 0..800 {
        h.run_steps(1);
        if h.query_by_label("Old drawing").is_some()
            && h.query_by_label("New drawing").is_some()
            && (!lines || h.query_by_label_contains("Snapping to ").is_some())
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the drawings didn't render");
}

/// Two reference points are enough: two on Old, then a click on New starts New's points. The box
/// moved 20 pt (7.056 mm) to the right in B, so the fit shifts B 7.056 mm left, at scale 1.
#[test]
fn picking_two_points_registers_b_on_a() {
    let mut h = harness();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, false);
    h.get_by_label("Old drawing: select point 1 of 3.");
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    // New takes no point before Old has two.
    click_at(&mut h, on_page(vb, 120.0, 100.0));
    h.get_by_label("That click placed no point: the Old drawing takes point 1 next.");
    click_at(&mut h, on_page(va, 100.0, 100.0));
    h.get_by_label("Old drawing: select point 2 of 3.");
    click_at(&mut h, on_page(va, 140.0, 60.0));
    h.get_by_label("Old drawing: select point 3 of 3.");
    h.get_by_label("Or select point 1 on the New drawing to go on with 2 points.");
    click_at(&mut h, on_page(vb, 120.0, 100.0));
    h.get_by_label("New drawing: select point 2 of 2.");
    assert!(next_disabled(&h), "an unfinished set of points blocks Next");
    click_at(&mut h, on_page(vb, 160.0, 60.0));
    h.get_by_label("Alignment calculated.");
    assert!(!next_disabled(&h));
    let node = h.get_by_label_contains(" mm · rotation ");
    // A label's text is its accessibility value.
    let readout = node.accesskit_node().label().or_else(|| node.value()).unwrap_or_default().to_string();
    let numbers: Vec<f64> = readout.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).filter_map(|t| t.parse::<f64>().ok()).collect();
    let [x, y, rotation, scale] = numbers[..] else { panic!("{readout}") };
    // One pixel of the view is about 0.5 pt here: allow for where a click lands.
    assert!((x + 7.056).abs() < 0.4, "{readout}");
    assert!(y.abs() < 0.4, "{readout}");
    assert!(rotation.abs() < 0.5, "{readout}");
    assert!((scale - 1.0).abs() < 0.01, "{readout}");
    // Exactly two points: no residual check is possible yet.
    h.get_by_label_contains("no residual check is possible");
}

/// The fit's shift, rotation and scale as the readout shows them.
fn readout(h: &Harness<'static, PdfCraftApp>) -> [f64; 4] {
    let node = h.get_by_label_contains(" mm · rotation ");
    let text = node.accesskit_node().label().or_else(|| node.value()).unwrap_or_default().to_string();
    let numbers: Vec<f64> = text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).filter_map(|t| t.parse::<f64>().ok()).collect();
    let [x, y, rotation, scale] = numbers[..] else { panic!("{text}") };
    [x, y, rotation, scale]
}

/// Clicks a few pixels off the box's corners snap onto them, so the fit is exact (B's box is
/// 20 pt, 7.056 mm, right of A's); with Alt held the same clicks stay where they land.
#[test]
fn picks_snap_to_line_ends_and_alt_places_them_freely() {
    let mut h = harness();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    h.get_by_label_contains("Snapping to 5 lines on A and 5 lines on B");
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    // Different misses on A and B, so a fit of the raw clicks isn't a pure shift.
    let pick = |h: &mut Harness<'static, PdfCraftApp>, modifiers: egui::Modifiers| {
        for (ax, ay) in [(100.0, 100.0), (140.0, 60.0)] {
            click_with(h, on_page(va, ax, ay) + egui::vec2(3.0, -2.0), modifiers);
        }
        for (bx, by) in [(120.0, 100.0), (160.0, 60.0)] {
            click_with(h, on_page(vb, bx, by) + egui::vec2(-2.0, 3.0), modifiers);
        }
    };
    pick(&mut h, egui::Modifiers::NONE);
    let [x, y, rotation, scale] = readout(&h);
    assert_eq!((x, y, rotation, scale), (-7.056, 0.0, 0.0, 1.0));
    h.get_by_label("Clear all points").click();
    h.run_steps(3);
    pick(&mut h, egui::Modifiers::ALT);
    let [x, y, _, _] = readout(&h);
    assert!((x + 7.056).abs() > 0.5 && y.abs() > 0.5, "Alt clicks snapped: {x}, {y}");
    // Snapping off: clicks stay where they land, without Alt.
    h.get_by_label("Clear all points").click();
    h.get_by_label("Snap to the drawing").click();
    h.run_steps(3);
    pick(&mut h, egui::Modifiers::NONE);
    let [x, y, _, _] = readout(&h);
    assert!((x + 7.056).abs() > 0.5 && y.abs() > 0.5, "clicks snapped with snapping off: {x}, {y}");
}

/// A label's text, or a combo box's current value.
fn text_of(h: &Harness<'static, PdfCraftApp>, label: &str) -> String {
    let node = h.get_by_label(label);
    node.value().unwrap_or_default().to_string()
}

/// Step 2 with a sheet inserted in New (Old 6 pages, New 7, the new sheet at New p. 3): Compare as
/// on row 3 makes it New only and Next opens once every page has a partner; Undo and Redo; back to
/// Matched reverses the gap; the arrows move a whole row; re-pairing in order asks first, Cancel
/// keeps the pairing, and Undo brings back what re-pairing replaced.
#[test]
fn pages_step_compare_as_arrows_and_re_pairing() {
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 1000.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("new.pdf", None, sheets(7)).unwrap();
        app.open_bytes("old.pdf", None, sheets(6)).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("doc.overlay"));
    h.run_steps(3);
    next(&mut h);
    h.get_by_label("2. Pages");
    h.get_by_label("Compare as");
    let unplaced = "7 output page(s): 6 matched, 0 Old only, 0 New only, 1 without a partner yet";
    let marked = "7 output page(s): 6 matched, 0 Old only, 1 New only";
    h.get_by_label(unplaced);
    assert_eq!(text_of(&h, "Compare output page 7 as"), "No partner yet");
    assert!(h.get_by_label("Next").accesskit_node().is_disabled(), "an unplaced page blocks Next");
    // Row 3: New p. 3 is the inserted sheet.
    let compare = |h: &mut Harness<'static, PdfCraftApp>, row: usize, choice: &str| {
        h.get_by_label(&format!("Compare output page {row} as")).click();
        h.run_steps(2);
        h.get_by_label(choice).click();
        h.run_steps(3);
    };
    compare(&mut h, 3, "New only");
    h.get_by_label(marked);
    assert_eq!(text_of(&h, "Compare output page 3 as"), "New only");
    h.get_by_label("Old sheets from output page 4 on moved down one row.");
    assert!(!h.get_by_label("Next").accesskit_node().is_disabled());
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOT") {
        h.render().unwrap().save(std::path::Path::new(&dir).join("pages-new-only.png")).unwrap();
    }
    // Undo and Redo.
    h.get_by_label("Undo").click();
    h.run_steps(3);
    h.get_by_label(unplaced);
    h.get_by_label("Redo").click();
    h.run_steps(3);
    h.get_by_label(marked);
    // Back to Matched reverses that gap exactly; Undo brings the New-only row back.
    compare(&mut h, 3, "Matched");
    h.get_by_label(unplaced);
    h.get_by_label("Old sheets from output page 4 on moved up one row.");
    h.get_by_label("Undo").click();
    h.run_steps(3);
    h.get_by_label(marked);
    // The arrows move the whole row: the New-only row goes down one, and back up.
    assert!(h.get_by_label("Move output page 1 up").accesskit_node().is_disabled());
    assert!(h.get_by_label("Move output page 7 down").accesskit_node().is_disabled());
    h.get_by_label("Move output page 3 down").click();
    h.run_steps(3);
    assert_eq!(text_of(&h, "Compare output page 3 as"), "Matched");
    assert_eq!(text_of(&h, "Compare output page 4 as"), "New only");
    h.get_by_label(marked);
    h.get_by_label("Move output page 4 up").click();
    h.run_steps(3);
    assert_eq!(text_of(&h, "Compare output page 3 as"), "New only");
    // Re-pairing would replace that: it asks, and Cancel keeps it.
    h.get_by_label("Re-pair in order").click();
    h.run_steps(2);
    h.get_by_label_contains("Re-pairing in order replaces the pairings you made");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    h.get_by_label(marked);
    // Confirmed, it re-pairs; Undo restores the New-only row.
    h.get_by_label("Re-pair in order").click();
    h.run_steps(2);
    h.get_all_by_label("Re-pair in order").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label(unplaced);
    h.get_by_label("Undo").click();
    h.run_steps(3);
    h.get_by_label(marked);
}

/// Different sheet sizes (Old 300 × 200 pt, New 600 × 400 pt): step 4 shows the proposed factor
/// and its basis, and Next waits for the confirmation; 1:1 needs one too, while turning Match
/// sheet sizes off needs none.
#[test]
fn different_sheet_sizes_need_a_confirmed_factor() {
    let mut h = harness_with(drawing(100), drawing_sized(240, [(600, 400), (600, 400)]));
    to_sheet_and_alignment(&mut h);
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: New × 0.5000, to confirm");
    h.get_by_label_contains("Same shape (A 300.0 × 200.0 pt, B 600.0 × 400.0 pt)");
    h.get_by_label("Scale B by the proposed 0.5000");
    assert!(next_disabled(&h));
    h.get_by_label("Confirm the sheet-size choice for output page 1 (Old p. 1 · New p. 1), or turn Match sheet sizes off.");
    h.get_by_label("I confirm this choice").click();
    h.run_steps(3);
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: New × 0.5000");
    // OV6.9: confirmed, the different sizes still need the pair's override and reference points.
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_1);
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOT") {
        settle(&mut h, 200);
        h.render().unwrap().save(std::path::Path::new(&dir).join("sheet-size-different.png")).unwrap();
    }
    // Another choice is confirmed again.
    h.get_by_label("1:1").click();
    h.run_steps(3);
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: 1:1, to confirm");
    assert!(next_disabled(&h));
    h.get_by_label("Match sheet sizes").click();
    h.run_steps(3);
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: 1:1 (Match sheet sizes is off)");
    // OV6.9: Match sheet sizes off doesn't make different sizes acceptable either.
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_1);
}

/// Per pair: New's second sheet is twice the size. With the first pair's choice shared, the pair
/// that differs is named; set apart, it needs its own confirmation, and turning sharing on and
/// off again keeps what was confirmed for it.
#[test]
fn sheet_sizes_per_pair_are_kept() {
    let mut h = harness_with(drawing(100), drawing_sized(120, [(300, 200), (600, 400)]));
    to_sheet_and_alignment(&mut h);
    h.get_by_label("Old: 300 × 200 pt · New: 300 × 200 pt · Scale: 1:1");
    h.get_by_label_contains("Output page(s) 2 have other sheet sizes than the first pair");
    // OV6.9: pair 2's sheets differ, so it waits for its own override and reference points.
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_2);
    h.get_by_label("Use the first pair's sheet size for every pair").click();
    h.run_steps(3);
    assert!(next_disabled(&h));
    h.get_by_label("Confirm the sheet-size choice for output page 2 (Old p. 2 · New p. 2), or turn Match sheet sizes off.");
    h.get_by_label("Page pair").click();
    h.run_steps(2);
    h.get_by_label("Output page 2: Old p. 2 · New p. 2 · sheet size to confirm").click();
    h.run_steps(3);
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: New × 0.5000, to confirm");
    h.get_by_label("I confirm this choice").click();
    h.run_steps(3);
    // Confirmed; OV6.9's override and reference points are still to come.
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_2);
    // Shared again, pair 2 follows the first pair (1:1); set apart again, its own choice is back.
    h.get_by_label("Use the first pair's sheet size for every pair").click();
    h.run_steps(3);
    h.get_by_label("Use the first pair's sheet size for every pair").click();
    h.run_steps(3);
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: New × 0.5000");
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_2);
}

/// The recommended way: three points on Old, then the same three on New in the same order. Clicks
/// snap to the box's corners, so the fit is exact with zero residuals. Undo steps back one point
/// at a time, through a partly picked set and a finished one, and the expected point follows.
#[test]
fn three_old_points_then_three_new_points() {
    let mut h = harness();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    let old = [(100.0, 100.0), (140.0, 60.0), (100.0, 60.0)];
    let near = |view: egui::Rect, (x, y): (f32, f32), dx: f32| on_page(view, x + dx, y) + egui::vec2(2.0, -2.0);
    for (i, p) in old.into_iter().enumerate() {
        h.get_by_label(&format!("Old drawing: select point {} of 3.", i + 1));
        click_at(&mut h, near(va, p, 0.0));
    }
    // Old has three: New's turn, and Old takes no fourth.
    h.get_by_label("New drawing: select point 1 of 3.");
    click_at(&mut h, near(va, (140.0, 100.0), 0.0));
    h.get_by_label("That click placed no point: the New drawing takes point 1 next.");
    // Undo in the middle of the set: Old's third point goes, then comes back.
    h.get_by_label("Undo last point").click();
    h.run_steps(3);
    h.get_by_label("Old drawing: select point 3 of 3.");
    click_at(&mut h, near(va, old[2], 0.0));
    for (i, p) in old.into_iter().enumerate() {
        h.get_by_label(&format!("New drawing: select point {} of 3.", i + 1));
        click_at(&mut h, near(vb, p, 20.0));
    }
    h.get_by_label("Alignment calculated.");
    assert_eq!(readout(&h), [-7.056, 0.0, 0.0, 1.0]);
    h.get_by_label("RMS residual 0.000 mm, largest 0.000 mm, on A's sheet.");
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOT") {
        h.render().unwrap().save(std::path::Path::new(&dir).join("alignment-three-points.png")).unwrap();
    }
    // Undo New's third point: the set is open again, New 3 expected, and Next waits.
    h.get_by_label("Undo last point").click();
    h.run_steps(3);
    h.get_by_label("New drawing: select point 3 of 3.");
    assert!(next_disabled(&h));
    click_at(&mut h, near(vb, old[2], 20.0));
    h.get_by_label("Alignment calculated.");
    assert!(!next_disabled(&h));
    // Clear all points: no registration, which is fine too.
    h.get_by_label("Clear all points").click();
    h.run_steps(3);
    h.get_by_label("Old drawing: select point 1 of 3.");
    h.get_by_label(LOWER_LEFT);
    assert!(!next_disabled(&h));
}

/// Generate's output size. "Expand page to prevent clipping" is off by default: each page keeps
/// A's sheet (300 × 200 pt) and the part of B outside it is clipped, as Preview shows. On, the page
/// grows to hold both aligned drawings: B is registered 20 pt left, so 320 × 200 pt. Either way
/// one page per row, exactly two layers, Old and New, and the sources unchanged.
#[test]
fn expand_page_to_prevent_clipping() {
    let mut h = harness();
    let sources: Vec<_> = h.state().views.iter().map(|v| h.state().session.get(v.id).unwrap().bytes.clone()).collect();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    pick_the_box(&mut h);
    h.get_by_label("Alignment calculated.");
    assert_eq!(readout(&h), [-7.056, 0.0, 0.0, 1.0]);
    next(&mut h);
    h.get_by_label("5. Preview");
    for _ in 0..2400 {
        h.run_steps(1);
        if h.query_by_label_contains("Part of B falls outside A's sheet").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    h.get_by_label_contains("To keep it, turn on Expand page to prevent clipping in step 6.");
    next(&mut h);
    h.get_by_label("6. Generate");
    assert_eq!(h.get_by_label("Expand page to prevent clipping").accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
    h.get_by_label("When enabled, the output page expands to include both aligned drawings. This may create a nonstandard sheet size.");
    let made = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        let (_, id) = app.active_ids().unwrap();
        let doc = app.session.get(id).unwrap();
        let layers: Vec<String> = doc.info.layers.iter().map(|l| l.name.clone()).collect();
        let sizes: Vec<(f32, f32)> = doc.info.pages.iter().map(|p| (p.width, p.height)).collect();
        (layers, sizes)
    };
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    let (layers, sizes) = made(&h);
    assert_eq!(layers, ["Old", "New"]);
    assert_eq!(sizes.len(), 2);
    for (w, ht) in sizes {
        assert!((w - 300.0).abs() < 0.01 && (ht - 200.0).abs() < 0.01, "{w} × {ht}");
    }
    h.get_by_label_contains("Part of B is clipped on page(s) 1, 2");
    // Expanded: the union of both placed sheets.
    h.get_by_label("Expand page to prevent clipping").click();
    h.run_steps(3);
    h.get_by_label("Generate again").click();
    wait(&mut h, |app| app.views.len() == 4);
    let (layers, sizes) = made(&h);
    assert_eq!(layers, ["Old", "New"]);
    assert_eq!(sizes.len(), 2);
    for (w, ht) in sizes {
        assert!((w - 320.0).abs() < 0.01 && (ht - 200.0).abs() < 0.01, "{w} × {ht}");
    }
    assert!(h.query_by_label_contains("Part of B is clipped").is_none());
    let app = h.state();
    for (v, before) in app.views.iter().take(2).zip(&sources) {
        assert_eq!(app.session.get(v.id).unwrap().bytes, *before);
    }
}

// ── OV6.9: different sheet sizes ────────────────────────────────────────────────────────────

const ALLOW: &str = "Allow alignment of different sheet sizes";
const LOWER_LEFT: &str = "No reference points: Old and New aligned at their lower-left corners, 1:1.";
const DIFFER_PAGE_1: &str = "Output page 1 (Old p. 1 · New p. 1): the sheet sizes differ. Check \"Allow alignment of different sheet sizes\" to align them with reference points.";
const DIFFER_PAGE_2: &str = "Output page 2 (Old p. 2 · New p. 2): the sheet sizes differ. Check \"Allow alignment of different sheet sizes\" to align them with reference points.";
const NEED_POINTS_PAGE_1: &str = "Output page 1 (Old p. 1 · New p. 1): different sheet sizes need reference points.";

fn toggled(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

fn click_label(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

/// Show `item` in the Page pair list.
fn choose_pair(h: &mut Harness<'static, PdfCraftApp>, item: &str) {
    click_label(h, "Page pair");
    click_label(h, item);
}

/// Old 300 × 200 pt and New 600 × 400 pt, two pages each, so every pair's sheets differ. New
/// draws Old's drawing 140 pt further right.
fn different_sizes() -> Harness<'static, PdfCraftApp> {
    harness_with(drawing(100), drawing_moved(100, 140, [(600, 400), (600, 400)]))
}

/// The diagonal line's two ends on Old, then on New's twice-as-large sheet (the view shows a
/// 3:2 page, so New's points go in at half their coordinates). Far apart enough for the solver's
/// spread check on the larger sheet. Clicks snap.
fn pick_the_line_on_large_new(h: &mut Harness<'static, PdfCraftApp>) {
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    let nudge = egui::vec2(2.0, -2.0);
    for (x, y) in [(20.0, 20.0), (280.0, 180.0)] {
        click_at(h, on_page(va, x, y) + nudge);
    }
    for (x, y) in [(160.0, 20.0), (420.0, 180.0)] {
        click_at(h, on_page(vb, x / 2.0, y / 2.0) + nudge);
    }
}

/// The matrix the generated overlay's page `page` draws side `state` ("GA" or "GB") with: the
/// first `cm` in its soft-mask group.
fn placed(h: &Harness<'static, PdfCraftApp>, page: usize, state: &str) -> [f64; 6] {
    let app = h.state();
    let (_, id) = app.active_ids().unwrap();
    let doc = pdfcraft_cos::Document::open(app.session.get(id).unwrap().bytes.clone()).unwrap();
    let p = &pdfcraft_model::pages(&doc)[page];
    let dict = |o: &pdfcraft_cos::Object| doc.resolve(o).as_dict().cloned().unwrap();
    let res = dict(p.dict.get(b"Resources").unwrap());
    let gs = dict(dict(res.get(b"ExtGState").unwrap()).get(state.as_bytes()).unwrap());
    let mask = dict(gs.get(b"SMask").unwrap());
    let pdfcraft_cos::Object::Stream(s) = &*doc.resolve(mask.get(b"G").unwrap()) else { panic!("group") };
    let text = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let at = tokens.iter().position(|t| *t == "cm").unwrap();
    let n: Vec<f64> = tokens[at - 6..at].iter().map(|t| t.parse().unwrap()).collect();
    [n[0], n[1], n[2], n[3], n[4], n[5]]
}

/// Different sizes are blocked by default; the override (unchecked at first) applies no scaling:
/// the proposed factor still waits for its own confirmation. With it confirmed and the override
/// on, zero points are still invalid.
#[test]
fn different_sheet_sizes_block_by_default() {
    let mut h = different_sizes();
    to_sheet_and_alignment(&mut h);
    h.get_by_label("Different sheet sizes: Old 105.8 × 70.6 mm (300 × 200 pt), New 211.7 × 141.1 mm (600 × 400 pt).");
    assert!(!toggled(&h, ALLOW));
    // Match sheet sizes off: nothing to confirm, yet the different sizes block.
    click_label(&mut h, "Match sheet sizes");
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_1);
    click_label(&mut h, "Match sheet sizes");
    click_label(&mut h, ALLOW);
    assert!(toggled(&h, ALLOW));
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: New × 0.5000, to confirm");
    assert!(next_disabled(&h));
    h.get_by_label("Confirm the sheet-size choice for output page 1 (Old p. 1 · New p. 1), or turn Match sheet sizes off.");
    click_label(&mut h, "I confirm this choice");
    h.get_by_label("Old: 300 × 200 pt · New: 600 × 400 pt · Scale: New × 0.5000");
    assert!(next_disabled(&h));
    h.get_by_label(NEED_POINTS_PAGE_1);
    h.get_by_label("No reference points yet: different sheet sizes need them.");
}

/// With the override on for both pairs and two point pairs, the existing solver aligns the
/// different sheets (Match sheet sizes off: 140 pt left, 1:1) and the overlay is generated.
/// Match sheet sizes on again proposes 0.5, which waits for its separate confirmation.
#[test]
fn the_override_allows_point_alignment() {
    let mut h = different_sizes();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    click_label(&mut h, "Match sheet sizes");
    click_label(&mut h, ALLOW);
    pick_the_line_on_large_new(&mut h);
    h.get_by_label("Alignment calculated.");
    assert_eq!(readout(&h), [-49.389, 0.0, 0.0, 1.0]);
    // Pair 2 has its own override, still off.
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_2);
    choose_pair(&mut h, "Output page 2: Old p. 2 · New p. 2");
    click_label(&mut h, ALLOW);
    assert!(!next_disabled(&h));
    click_label(&mut h, "Match sheet sizes");
    assert!(next_disabled(&h));
    h.get_by_label("Confirm the sheet-size choice for output page 1 (Old p. 1 · New p. 1), or turn Match sheet sizes off.");
    click_label(&mut h, "Match sheet sizes");
    assert!(!next_disabled(&h));
    next(&mut h);
    h.get_by_label("5. Preview");
    next(&mut h);
    h.get_by_label("6. Generate");
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    let app = h.state();
    let (_, id) = app.active_ids().unwrap();
    let doc = app.session.get(id).unwrap();
    let layers: Vec<&str> = doc.info.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(layers, ["Old", "New"]);
    assert_eq!(doc.info.pages.len(), 2);
    // B is moved 140 pt left at scale 1 on both pages (pair 2 shares pair 1's alignment).
    for page in 0..2 {
        assert_eq!(placed(&h, page, "GB"), [1.0, 0.0, 0.0, 1.0, -140.0, 0.0]);
    }
}

/// The override belongs to the pair on show, even with the first pair's sheet size and alignment
/// shared: ticking pair 1's leaves pair 2's off, and the other way round.
#[test]
fn the_override_is_per_pair() {
    let mut h = different_sizes();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    assert!(toggled(&h, "Use the first pair's sheet size for every pair"));
    assert!(toggled(&h, "Use the first pair's alignment for every pair"));
    click_label(&mut h, "Match sheet sizes");
    click_label(&mut h, ALLOW);
    pick_the_line_on_large_new(&mut h);
    h.get_by_label("Alignment calculated.");
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_2);
    choose_pair(&mut h, "Output page 2: Old p. 2 · New p. 2");
    assert!(!toggled(&h, ALLOW));
    click_label(&mut h, ALLOW);
    choose_pair(&mut h, "Output page 1: Old p. 1 · New p. 1");
    assert!(toggled(&h, ALLOW));
    click_label(&mut h, ALLOW);
    assert!(!toggled(&h, ALLOW));
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_1);
    choose_pair(&mut h, "Output page 2: Old p. 2 · New p. 2");
    assert!(toggled(&h, ALLOW));
}

/// Equal sheets: no points needed. Old and New share their lower-left corners at 1:1 in the
/// output (both drawn with the identity), and there is no override to set.
#[test]
fn equal_sheets_need_no_points() {
    let mut h = harness();
    to_sheet_and_alignment(&mut h);
    h.get_by_label(LOWER_LEFT);
    assert!(h.query_by_label(ALLOW).is_none());
    assert!(!next_disabled(&h));
    next(&mut h);
    h.get_by_label("5. Preview");
    next(&mut h);
    h.get_by_label("6. Generate");
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    for page in 0..2 {
        assert_eq!(placed(&h, page, "GA"), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(placed(&h, page, "GB"), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
}

/// An unfinished point set blocks Next until it is finished or undone; undone to zero points,
/// equal sheets go on aligned at their lower-left corners.
#[test]
fn partial_points_block_until_completed_or_undone() {
    let mut h = harness();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, false);
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    click_at(&mut h, on_page(va, 100.0, 100.0));
    click_at(&mut h, on_page(va, 140.0, 60.0));
    click_at(&mut h, on_page(vb, 120.0, 100.0));
    h.get_by_label("New drawing: select point 2 of 2.");
    assert!(next_disabled(&h));
    for _ in 0..3 {
        click_label(&mut h, "Undo last point");
    }
    h.get_by_label("Old drawing: select point 1 of 3.");
    h.get_by_label(LOWER_LEFT);
    assert!(!next_disabled(&h));
}

/// A typed final nudge on a pair of different sheets survives turning the override off and on
/// again, and so do its points.
#[test]
fn manual_nudge_survives_the_override() {
    let mut h = different_sizes();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    click_label(&mut h, "Match sheet sizes");
    click_label(&mut h, ALLOW);
    pick_the_line_on_large_new(&mut h);
    h.get_by_label("Alignment calculated.");
    choose_pair(&mut h, "Output page 2: Old p. 2 · New p. 2");
    click_label(&mut h, ALLOW);
    choose_pair(&mut h, "Output page 1: Old p. 1 · New p. 1");
    // Shift X of the final nudge (its first number field), set to 1 mm.
    let (target_node, target_tree) = {
        let field = h.query_all(egui_kittest::kittest::By::new().role(egui::accesskit::Role::SpinButton)).next().expect("Shift X field");
        field.accesskit_node().locate()
    };
    h.event(egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::SetValue,
        target_node,
        target_tree,
        data: Some(egui::accesskit::ActionData::NumericValue(1.0)),
    }));
    h.run_steps(3);
    assert_eq!(readout(&h), [1.0, 0.0, 0.0, 1.0]);
    assert!(!h.get_by_label("Back to the fit").accesskit_node().is_disabled());
    assert!(!next_disabled(&h));
    click_label(&mut h, ALLOW);
    assert!(next_disabled(&h));
    h.get_by_label(DIFFER_PAGE_1);
    click_label(&mut h, ALLOW);
    assert_eq!(readout(&h), [1.0, 0.0, 0.0, 1.0]);
    assert!(!h.get_by_label("Back to the fit").accesskit_node().is_disabled());
    assert!(!next_disabled(&h));
}

// ── OV6.10: drawing navigation ──────────────────────────────────────────────────────────────

fn disabled(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// Ctrl (⌘ on a Mac) as egui reports it.
fn ctrl() -> egui::Modifiers {
    egui::Modifiers { ctrl: true, command: true, ..egui::Modifiers::NONE }
}

/// One wheel step of `delta` points over `pos` with `modifiers` held, and the frames it takes to
/// apply in full.
fn wheel(h: &mut Harness<'static, PdfCraftApp>, pos: egui::Pos2, delta: egui::Vec2, modifiers: egui::Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(1);
    h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, phase: egui::TouchPhase::Move, modifiers });
    h.run_steps(8);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
}

/// A drag with `button` from `from` by `by`, in four moves.
fn drag(h: &mut Harness<'static, PdfCraftApp>, button: egui::PointerButton, from: egui::Pos2, by: egui::Vec2) {
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: from, button, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=4 {
        h.event(egui::Event::PointerMoved(from + by * (k as f32 / 4.0)));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton { pos: from + by, button, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
}

/// The two screen spots [`fit_of_spots`] picks, as offsets from each view's centre.
const SPOTS: [egui::Vec2; 2] = [egui::vec2(-40.0, -20.0), egui::vec2(40.0, 20.0)];

/// Where the same screen spots of the two views lie on their pages, as a fit: Alt-clicks (no
/// snapping) on Old's spots, then New's, and the readout; then the points are cleared. With both
/// views at the full page it is the identity.
fn fit_of_spots(h: &mut Harness<'static, PdfCraftApp>) -> [f64; 4] {
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    for d in SPOTS {
        click_with(h, va.center() + d, egui::Modifiers::ALT);
    }
    for d in SPOTS {
        click_with(h, vb.center() + d, egui::Modifiers::ALT);
    }
    let fit = readout(h);
    click_label(h, "Clear all points");
    fit
}

/// OV6.10: on step 4's drawings the wheel pans and Ctrl+wheel zooms about the pointer, a middle
/// drag pans and a left drag does nothing, and none of it selects a point. Each move shows in
/// where the same screen spots land on Old's page (New stays at the full page): the zoom shrinks
/// the fit's scale, the wheel moves only its y shift, Shift+wheel only its x shift. Animation
/// effects are off, so a wheel step applies at once.
#[test]
fn wheel_pans_ctrl_wheel_zooms_and_nothing_selects() {
    let mut h = harness();
    h.state_mut().set_option("animation_effects", "off").unwrap();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, false);
    let no_point = |h: &Harness<'static, PdfCraftApp>| disabled(h, "Undo last point");
    let near = |a: f64, b: f64| (a - b).abs() < 0.002;
    let [x, y, _, scale] = fit_of_spots(&mut h);
    assert!(near(x, 0.0) && near(y, 0.0) && near(scale, 1.0), "{x}, {y}, {scale}");
    // At the full page there is nothing to pan: the view stays (the step may scroll instead).
    let centre = h.get_by_label("Old drawing").rect().center();
    wheel(&mut h, centre, egui::vec2(0.0, -40.0), egui::Modifiers::NONE);
    assert!(disabled(&h, "Fit to page") && no_point(&h));
    let old = h.get_by_label("Old drawing").rect();
    wheel(&mut h, old.center(), egui::vec2(0.0, 200.0), ctrl());
    assert!(!disabled(&h, "Fit to page") && no_point(&h));
    let zoomed = fit_of_spots(&mut h);
    assert!(zoomed[3] < 0.9, "Ctrl+wheel didn't zoom in: {zoomed:?}");
    // Zoomed in, the wheel pans the drawing up and down, and the step stays where it is.
    wheel(&mut h, old.center(), egui::vec2(0.0, -30.0), egui::Modifiers::NONE);
    assert!(no_point(&h));
    assert_eq!(h.get_by_label("Old drawing").rect(), old, "the wheel scrolled the step");
    let down = fit_of_spots(&mut h);
    assert!(near(down[0], zoomed[0]) && (down[1] - zoomed[1]).abs() > 0.5 && near(down[3], zoomed[3]), "{zoomed:?} → {down:?}");
    // Shift+wheel: left and right.
    wheel(&mut h, old.center(), egui::vec2(0.0, -30.0), egui::Modifiers::SHIFT);
    assert!(no_point(&h));
    assert_eq!(h.get_by_label("Old drawing").rect(), old, "Shift+wheel scrolled the step");
    let side = fit_of_spots(&mut h);
    assert!((side[0] - down[0]).abs() > 0.5 && near(side[1], down[1]) && near(side[3], down[3]), "{down:?} → {side:?}");
    // A middle drag pans both ways.
    drag(&mut h, egui::PointerButton::Middle, old.center(), egui::vec2(25.0, 15.0));
    assert!(no_point(&h));
    let dragged = fit_of_spots(&mut h);
    assert!((dragged[0] - side[0]).abs() > 0.5 && (dragged[1] - side[1]).abs() > 0.5 && near(dragged[3], side[3]), "{side:?} → {dragged:?}");
    // A left drag neither pans nor selects.
    drag(&mut h, egui::PointerButton::Primary, old.center(), egui::vec2(25.0, 15.0));
    assert!(no_point(&h));
    assert_eq!(fit_of_spots(&mut h), dragged);
}

/// OV6.10: zoomed in, a click a few pixels off a corner still snaps onto it, on Old and on New,
/// each zoomed about its own corner; Old's zoom leaves New at the full page, where its first
/// corner is picked. Fit to page brings both views back, and the same picks give the same exact
/// fit as before any navigation (B's box is 20 pt, 7.056 mm, right of A's).
#[test]
fn zoomed_picks_snap_and_fit_to_page_restores() {
    let mut h = harness();
    to_sheet_and_alignment(&mut h);
    wait_for_drawings(&mut h, true);
    let (va, vb) = (h.get_by_label("Old drawing").rect(), h.get_by_label("New drawing").rect());
    let miss = egui::vec2(4.0, -3.0);
    let pick_the_box_near = |h: &mut Harness<'static, PdfCraftApp>| {
        for (x, y) in [(100.0, 100.0), (140.0, 60.0)] {
            click_at(h, on_page(va, x, y) + miss);
        }
        for (x, y) in [(120.0, 100.0), (160.0, 60.0)] {
            click_at(h, on_page(vb, x, y) + miss);
        }
    };
    pick_the_box_near(&mut h);
    assert_eq!(readout(&h), [-7.056, 0.0, 0.0, 1.0]);
    click_label(&mut h, "Clear all points");
    // Old: the first corner at the full page, the second zoomed in about it.
    click_at(&mut h, on_page(va, 100.0, 100.0) + miss);
    let corner = on_page(va, 140.0, 60.0);
    wheel(&mut h, corner, egui::vec2(0.0, 300.0), ctrl());
    assert!(!disabled(&h, "Fit to page"));
    click_at(&mut h, corner + miss);
    // New is still at the full page; then zoomed in about its own second corner.
    click_at(&mut h, on_page(vb, 120.0, 100.0) + miss);
    let corner = on_page(vb, 160.0, 60.0);
    wheel(&mut h, corner, egui::vec2(0.0, 300.0), ctrl());
    click_at(&mut h, corner + miss);
    assert_eq!(readout(&h), [-7.056, 0.0, 0.0, 1.0]);
    click_label(&mut h, "Fit to page");
    assert!(disabled(&h, "Fit to page"));
    click_label(&mut h, "Clear all points");
    pick_the_box_near(&mut h);
    assert_eq!(readout(&h), [-7.056, 0.0, 0.0, 1.0]);
}

// ── OV4.10: layers the source doesn't declare ───────────────────────────────────────────────

/// An `n`-page drawing set like [`sheets`] whose pages also draw a red block in one layer, object
/// 5, that `/OCProperties` doesn't declare. With `off`, the configuration's `/OFF` names it, so
/// viewers disagree on whether it shows (OV4.10, U4); without, nothing turns it off (U2).
fn sheets_with_undeclared_layer(n: usize, off: bool) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + 2 * i)).collect();
    let configuration = if off { "/D << /OFF [5 0 R] >>" } else { "/D << >>" };
    let mut objs: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [3 0 R] {configuration} >> >>"),
        format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 300 200] >>", kids.join(" ")),
        "<< /Type /OCG /Name (Declared) >>".into(),
        "<< /Type /OCG /Name (Unused) >>".into(),
        "<< /Type /OCG /Name (Title block) >>".into(),
    ];
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Resources << /Properties << /L 5 0 R >> >> /Contents {} 0 R >>", 7 + 2 * i));
        let body = format!("0 g 20 {} 40 10 re f /OC /L BDC 1 0 0 rg 200 120 30 30 re f EMC", 10 + 5 * i);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

/// From step 1 to step 6 with the default pairing and no points.
fn to_generate(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.state_mut().execute("doc.overlay"));
    h.run_steps(3);
    for _ in 0..5 {
        next(h);
    }
    h.get_by_label("6. Generate");
}

/// Run frames until a label containing `text` shows.
fn wait_for_label(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    for _ in 0..2400 {
        h.run_steps(1);
        if h.query_by_label_contains(text).is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("timed out waiting for {text:?}");
}

#[test]
fn undeclared_layers_shown_by_the_source_need_no_decision() {
    let mut h = harness_with(sheets_with_undeclared_layer(3, false), sheets(3));
    to_generate(&mut h);
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    let app = h.state();
    let (_, id) = app.active_ids().unwrap();
    let doc = app.session.get(id).unwrap();
    let layers: Vec<&str> = doc.info.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(layers, ["Old", "New"]);
    assert_eq!(doc.info.pages.len(), 3);
    assert!(h.query_by_label_contains("can't be resolved with certainty").is_none());
    // The report says the layer was shown as the source shows it.
    h.get_by_label_contains("aren't declared in the document's layer list");
}

#[test]
fn an_undeclared_layer_on_three_pages_is_one_decision() {
    let mut h = harness_with(sheets_with_undeclared_layer(3, true), sheets(3));
    to_generate(&mut h);
    h.get_by_label("Generate").click();
    wait_for_label(&mut h, "Document A, pages 1–3: a layer isn't declared");
    // One row and one control for the layer, not one per page.
    assert_eq!(h.query_all_by_label_contains("Document A, page").count(), 1);
    h.get_by_label_contains("\"Title block\" (5 0 R)");
    h.get_by_label("Show").click();
    h.run_steps(3);
    h.get_by_label("Generate").click();
    wait(&mut h, |app| app.views.len() == 3);
    let app = h.state();
    let (_, id) = app.active_ids().unwrap();
    let layers: Vec<&str> = app.session.get(id).unwrap().info.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(layers, ["Old", "New"]);
    assert!(h.query_by_label_contains("Document A, page").is_none());
}

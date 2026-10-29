//! Tab View and Tile View (OV7.2) and navigation by the hovered tile (OV7.3). These run real
//! frames: the wheel, zoom and middle button reach the views through egui as they do in the app.

use egui::{Pos2, Rect, Vec2, vec2};
use egui_kittest::Harness;
use pdfcraft_engine::DocId;

use crate::canvas::Fit;
use crate::tile_view::MIN_TILE;
use crate::{PdfCraftApp, ViewLayout};

/// Wide enough for three tiles beside the Tools and Inspector panels.
const SIZE: Vec2 = vec2(2600.0, 1000.0);

fn open(app: &mut PdfCraftApp, name: &str, pages: usize) -> DocId {
    let bytes = app.session.create_blank(612.0, 792.0, pages).unwrap();
    app.open_bytes(name, None, bytes.as_ref().clone()).unwrap();
    app.views.last().unwrap().id
}

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(SIZE).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

/// Two documents, tiled, the second one active.
fn two_tiles() -> Harness<'static, PdfCraftApp> {
    harness(|app| {
        open(app, "left.pdf", 30);
        open(app, "right.pdf", 30);
        app.set_view_layout(ViewLayout::Tiles);
    })
}

#[derive(Clone, Debug, PartialEq)]
struct Place {
    current: usize,
    zoom: f32,
    fit: Fit,
    scroll: Vec2,
    back: Vec<usize>,
    forward: Vec<usize>,
}

fn place(h: &Harness<'static, PdfCraftApp>, tab: usize) -> Place {
    let v = &h.state().views[tab];
    Place { current: v.current, zoom: v.zoom, fit: v.fit, scroll: v.last_scroll, back: v.back.clone(), forward: v.forward.clone() }
}

fn close_enough(a: Vec2, b: Vec2) -> bool {
    (a - b).length() < 1.0
}

fn viewport(h: &Harness<'static, PdfCraftApp>, tab: usize) -> Rect {
    h.state().views[tab].viewport_rect()
}

fn wheel_at(h: &mut Harness<'static, PdfCraftApp>, p: Pos2, dy: f32, modifiers: egui::Modifiers) {
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        modifiers,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(8);
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

fn annotations(h: &Harness<'static, PdfCraftApp>, id: DocId) -> usize {
    h.state().session.get(id).unwrap().info.annotations.len()
}

// ── Layout ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn tiles_stand_side_by_side_in_columns_and_scroll_past_the_minimum_width() {
    let mut h = two_tiles();
    let (a, b) = (viewport(&h, 0), viewport(&h, 1));
    assert!(a.max.x <= b.min.x, "side by side: {a:?} {b:?}");
    assert!((a.top() - b.top()).abs() < 1.0, "one row, not a grid");
    assert!((a.width() - b.width()).abs() < 2.0, "equal columns");
    let two = a.width();
    h.state_mut().set_option("panel", "none").ok();
    let id = open(h.state_mut(), "third.pdf", 3);
    h.run_steps(6);
    assert_eq!(h.state().views.last().map(|v| v.id), Some(id));
    let widths: Vec<f32> = (0..3).map(|i| viewport(&h, i).width()).collect();
    assert!(widths.iter().all(|w| (w - widths[0]).abs() < 2.0), "three equal columns: {widths:?}");
    assert!(widths[0] < two, "a third column narrows them");
    for i in 0..3 {
        assert!(viewport(&h, i).max.x <= viewport(&h, (i + 1).min(2)).min.x || i == 2);
    }
    // Eight don't fit: each keeps the minimum width and the row scrolls.
    for k in 0..5 {
        open(h.state_mut(), &format!("more-{k}.pdf"), 2);
    }
    h.run_steps(8);
    let drawn = h.state().tile_frame.clone();
    assert!(!drawn.is_empty() && drawn.len() < 8, "only the tiles in view are drawn: {drawn:?}");
    for i in drawn {
        assert!(viewport(&h, i).width() >= MIN_TILE - 1.0, "tile {i} keeps the minimum width");
    }
}

#[test]
fn tab_view_is_the_default_and_switching_closes_nothing() {
    let mut h = two_tiles();
    assert_eq!(PdfCraftApp::new().layout_mode, ViewLayout::Tabs, "Tab View at every start");
    let id = h.state().views[0].id;
    let generation = h.state().session.get(id).unwrap().edit_generation();
    for layout in [ViewLayout::Tabs, ViewLayout::Tiles, ViewLayout::Tabs] {
        h.state_mut().set_view_layout(layout);
        h.run_steps(4);
        assert_eq!(h.state().views.len(), 2);
        assert_eq!(h.state().open_documents().len(), 2);
        assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "nothing reloaded or edited");
    }
    assert!(h.state_mut().execute("view.tile_view"));
    assert!(!h.state_mut().execute("view.tile_view"), "the arrangement already shown is disabled");
    assert!(h.state_mut().execute("view.tab_view"));
    assert_eq!(h.state().layout_mode, ViewLayout::Tabs);
}

/// Tab 1 on page 3 at 100% at the top of the sheet; tab 2, its duplicate, on page 18 at 150%,
/// scrolled down into the sheet. In Tab View.
fn two_places() -> Harness<'static, PdfCraftApp> {
    let mut h = harness(|app| {
        open(app, "plan.pdf", 30);
    });
    h.state_mut().views[0].set_zoom(1.0);
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(6);
    h.state_mut().duplicate_view(0);
    h.run_steps(4);
    h.state_mut().views[1].set_zoom(1.5);
    h.state_mut().views[1].go_to_page(17);
    h.run_steps(6);
    let p = viewport(&h, 1).center();
    wheel_at(&mut h, p, -300.0, egui::Modifiers::NONE);
    h
}

#[test]
fn switching_between_tab_and_tile_view_keeps_each_views_place() {
    let mut h = two_places();
    let (first, second) = (place(&h, 0), place(&h, 1));
    assert_ne!(first.current, second.current);
    for _ in 0..2 {
        h.state_mut().set_view_layout(ViewLayout::Tiles);
        h.run_steps(8);
        for (tab, before) in [(0, &first), (1, &second)] {
            let now = place(&h, tab);
            assert_eq!(
                (now.current, now.zoom, now.fit, &now.back, &now.forward),
                (before.current, before.zoom, before.fit, &before.back, &before.forward)
            );
            assert!((now.scroll.y - before.scroll.y).abs() < 1.0, "tab {tab} keeps its place in the page: {now:?} {before:?}");
        }
        h.state_mut().set_view_layout(ViewLayout::Tabs);
        h.run_steps(8);
        for (tab, before) in [(1, &second), (0, &first)] {
            h.state_mut().active = Some(tab);
            h.run_steps(6);
            let now = place(&h, tab);
            assert_eq!(now.current, before.current);
            assert_eq!(now.zoom, before.zoom);
            assert!(close_enough(now.scroll, before.scroll), "tab {tab}: {now:?} {before:?}");
        }
    }
}

#[test]
fn a_fit_width_view_keeps_its_fit_and_gets_its_zoom_back() {
    let mut h = two_tiles();
    h.state_mut().set_view_layout(ViewLayout::Tabs);
    h.state_mut().active = Some(0);
    h.run_steps(6);
    let before = place(&h, 0);
    assert_eq!(before.fit, Fit::Width);
    h.state_mut().set_view_layout(ViewLayout::Tiles);
    h.run_steps(8);
    let tiled = place(&h, 0);
    assert_eq!((tiled.fit, tiled.current), (Fit::Width, before.current));
    assert!(tiled.zoom < before.zoom, "a narrower column fits a smaller zoom");
    h.state_mut().set_view_layout(ViewLayout::Tabs);
    h.run_steps(8);
    let back = place(&h, 0);
    assert_eq!((back.fit, back.current), (Fit::Width, before.current));
    assert!((back.zoom - before.zoom).abs() < 1e-3);
}

// ── Navigation follows the pointer ───────────────────────────────────────────────────────────

#[test]
fn the_wheel_scrolls_the_tile_under_the_pointer_without_a_click() {
    let mut h = two_tiles();
    assert_eq!(h.state().active, Some(1));
    let (a, b) = (place(&h, 0), place(&h, 1));
    let left = viewport(&h, 0).center();
    wheel_at(&mut h, left, -300.0, egui::Modifiers::NONE);
    assert!(place(&h, 0).scroll.y > a.scroll.y + 100.0, "the hovered tile scrolls");
    assert_eq!(place(&h, 1), b, "the other doesn't");
    assert_eq!(h.state().active, Some(1), "hovering doesn't change the active tile");
    let a = place(&h, 0);
    let right = viewport(&h, 1).center();
    wheel_at(&mut h, right, -300.0, egui::Modifiers::NONE);
    assert!(place(&h, 1).scroll.y > b.scroll.y + 100.0);
    assert_eq!(place(&h, 0), a);
}

#[test]
fn ctrl_wheel_zooms_only_the_hovered_tile() {
    let mut h = two_tiles();
    let (a, b) = (place(&h, 0), place(&h, 1));
    let left = viewport(&h, 0).center();
    wheel_at(&mut h, left, 120.0, egui::Modifiers::COMMAND);
    assert!(place(&h, 0).zoom > a.zoom * 1.01, "the hovered tile zooms: {} → {}", a.zoom, place(&h, 0).zoom);
    assert_eq!(place(&h, 1).zoom, b.zoom, "the other keeps its zoom");
    assert_eq!(h.state().active, Some(1));
}

#[cfg(target_os = "linux")]
#[test]
fn the_middle_button_scrolls_the_hovered_tile_without_a_click() {
    let mut h = two_tiles();
    let b = place(&h, 1);
    let p = viewport(&h, 0).center();
    let top = h.state().views[0].page_screen_rect(0).unwrap().top();
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Middle, pressed, modifiers: egui::Modifiers::NONE };
    h.event(egui::Event::PointerMoved(p));
    h.event(button(p, true));
    h.event(button(p, false));
    h.run_steps(2);
    h.event(egui::Event::PointerMoved(p + vec2(0.0, 120.0)));
    h.run_steps(12);
    assert!(h.state().views[0].auto_scrolling(), "the inactive tile keeps its auto-scroll");
    assert!(h.state().views[0].page_screen_rect(0).unwrap().top() < top - 20.0, "and scrolls");
    assert_eq!(place(&h, 1), b, "the active tile doesn't move");
    assert_eq!(h.state().active, Some(1));
}

#[test]
fn a_touchpad_swipe_stays_in_its_own_tile() {
    // At a real frame rate: egui eases a swipe's motion over the frames after it.
    let mut h = Harness::builder().with_size(SIZE).with_step_dt(1.0 / 60.0).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        open(&mut app, "left.pdf", 30);
        open(&mut app, "right.pdf", 30);
        app.set_view_layout(ViewLayout::Tiles);
        app
    });
    h.run_steps(10);
    let a = place(&h, 0);
    // A swipe over the right tile, then the pointer moves on to the left one, which is drawn first.
    let right = viewport(&h, 1).center();
    h.hover_at(right);
    h.run_steps(1);
    for _ in 0..3 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -60.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(1);
    }
    let left = viewport(&h, 0).center();
    h.hover_at(left);
    h.run_steps(60);
    assert!(place(&h, 1).scroll.y > 100.0, "the right tile took the swipe");
    assert_eq!(place(&h, 0).scroll, a.scroll, "its tail doesn't reach the left tile");
}

// ── The active tile ──────────────────────────────────────────────────────────────────────────

#[test]
fn a_click_on_another_tile_only_makes_it_active() {
    let mut h = two_tiles();
    h.state_mut().set_option("quick", "ink").unwrap();
    h.run_steps(2);
    let (left, right) = (h.state().views[0].id, h.state().views[1].id);
    let r = viewport(&h, 0);
    let (from, to) = (r.center() - vec2(60.0, 0.0), r.center() + vec2(60.0, 40.0));
    drag(&mut h, from, to);
    assert_eq!(h.state().active, Some(0), "the click picked the tile");
    assert_eq!(annotations(&h, left), 0, "and drew nothing");
    drag(&mut h, from, to);
    assert_eq!(annotations(&h, left), 1, "the next stroke lands in the tile picked");
    assert_eq!(annotations(&h, right), 0);
    // The keyboard goes to the active tile only.
    let b = place(&h, 1);
    let a = place(&h, 0).scroll;
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(4);
    assert!(place(&h, 0).scroll.y > a.y, "the active tile scrolls");
    assert_eq!(place(&h, 1), b);
}

/// A one-page PDF, 200 × 300 points, with "Page 1" set in 24-point Helvetica at (20, 150).
fn text_pdf() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

/// Where a point of page 1 (PDF user space, 200 × 300) is on screen in tab `tab`.
fn page_point(h: &Harness<'static, PdfCraftApp>, tab: usize, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[tab].page_screen_rect(0).unwrap();
    egui::pos2(r.left() + x / 200.0 * r.width(), r.top() + (300.0 - y) / 300.0 * r.height())
}

#[test]
fn a_click_on_another_tile_doesnt_start_a_text_selection_there() {
    let mut h = harness(|app| {
        for name in ["left.pdf", "right.pdf"] {
            app.open_bytes(name, None, text_pdf()).unwrap();
        }
        app.set_view_layout(ViewLayout::Tiles);
    });
    for tab in 0..2 {
        h.state_mut().views[tab].set_zoom(2.0);
    }
    for _ in 0..300 {
        h.run_steps(1);
        if (0..2).all(|tab| h.state().views[tab].texts.contains_key(&0)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!((0..2).all(|tab| h.state().views[tab].texts.contains_key(&0)), "the text layers are read");
    assert_eq!(h.state().active, Some(1));
    // The Select tool (the default) across "Page 1" in the left tile, which isn't active.
    let (from, to) = (page_point(&h, 0, 10.0, 160.0), page_point(&h, 0, 150.0, 160.0));
    drag(&mut h, from, to);
    assert_eq!(h.state().active, Some(0), "the press picked the tile");
    assert_eq!(h.state().views[0].selected_text(), None, "and selected nothing");
    drag(&mut h, from, to);
    assert!(h.state().views[0].selected_text().is_some_and(|t| t.contains("Page")), "the next drag selects in the tile picked");
}

#[test]
fn the_hand_tool_pans_another_tile_with_its_first_drag() {
    let mut h = two_tiles();
    h.state_mut().set_option("quick", "hand").unwrap();
    h.run_steps(2);
    let a = place(&h, 0);
    let r = viewport(&h, 0);
    drag(&mut h, r.center(), r.center() - vec2(0.0, 200.0));
    assert_eq!(h.state().active, Some(0), "the drag picked the tile");
    assert!(place(&h, 0).scroll.y > a.scroll.y + 100.0, "and panned it at once: {:?} → {:?}", a.scroll, place(&h, 0).scroll);
}

#[test]
fn two_tiles_of_one_document_both_render() {
    let mut h = harness(|app| {
        open(app, "plan.pdf", 3);
    });
    h.state_mut().duplicate_view(0);
    h.state_mut().set_view_layout(ViewLayout::Tiles);
    h.run_steps(2);
    h.state_mut().views[0].set_zoom(1.0);
    h.state_mut().views[1].set_zoom(2.5);
    let mut sharp = false;
    for _ in 0..600 {
        h.run_steps(1);
        if h.state().views[0].current_page_sharp() && h.state().views[1].current_page_sharp() {
            sharp = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(sharp, "each tile gets its own page images from the shared document");
    // Closing either duplicate leaves the other where it was.
    let survivor = place(&h, 0);
    let key = h.state().views[1].key;
    h.state_mut().tab_action(crate::tab_context::Action::Close(key));
    h.run_steps(6);
    assert_eq!(h.state().views.len(), 1);
    let now = place(&h, 0);
    assert_eq!((now.current, now.zoom), (survivor.current, survivor.zoom));
}

#[test]
fn the_tab_menu_switches_between_tab_and_tile_view() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut h = harness(|app| {
        open(app, "left.pdf", 3);
        open(app, "right.pdf", 3);
    });
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "left.pdf").click_secondary();
    h.run_steps(2);
    assert!(h.get_by_label("Tab View").accesskit_node().is_disabled(), "Tab View is already shown");
    h.get_by_label("Tile View").click();
    h.run_steps(4);
    assert_eq!(h.state().layout_mode, ViewLayout::Tiles);
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "left.pdf").click_secondary();
    h.run_steps(2);
    assert!(h.get_by_label("Tile View").accesskit_node().is_disabled());
    h.get_by_label("Tab View").click();
    h.run_steps(4);
    assert_eq!(h.state().layout_mode, ViewLayout::Tabs);
}

#[test]
fn the_wheel_over_the_overlay_wizard_moves_no_tile() {
    let mut h = two_tiles();
    assert!(h.state_mut().execute("doc.overlay"));
    h.run_steps(6);
    let (a, b) = (place(&h, 0), place(&h, 1));
    let centre = Rect::from_min_size(Pos2::ZERO, SIZE).center();
    wheel_at(&mut h, centre, -300.0, egui::Modifiers::NONE);
    wheel_at(&mut h, centre, 120.0, egui::Modifiers::COMMAND);
    assert_eq!((place(&h, 0), place(&h, 1)), (a, b), "the wizard's own window takes the wheel");
}

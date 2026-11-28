use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2, ViewportBuilder, ViewportClass, ViewportId, vec2};
use egui_kittest::Harness;
use pdfcraft_engine::{DocId, Session};

use super::*;

// ------------------------------------------------------------------------------- geometry

fn limits(ppp: f32, monitor: Option<Vec2>, usable: Option<Rect>) -> Limits {
    Limits { ppp, monitor, usable }
}

/// A geometry any platform can make: finite, sides within the limits, corner within reach.
fn assert_safe(g: Geometry, l: Limits) {
    let ppp = if l.ppp.is_finite() && l.ppp > 0.0 { l.ppp } else { 1.0 };
    assert!(g.size.x.is_finite() && g.size.y.is_finite(), "{g:?}");
    assert!(g.size.x >= 1.0 && g.size.y >= 1.0, "{g:?}");
    assert!(g.size.x * ppp <= MAX_SIDE_PX + 0.01 && g.size.y * ppp <= MAX_SIDE_PX + 0.01, "{g:?}");
    // Only a monitor size that is a positive number limits anything.
    if let Some(m) = l.monitor {
        assert!(!(m.x.is_finite() && m.x > 0.0) || g.size.x <= m.x + 0.01, "{g:?} on {m:?}");
        assert!(!(m.y.is_finite() && m.y > 0.0) || g.size.y <= m.y + 0.01, "{g:?} on {m:?}");
    }
    if let Some(p) = g.pos {
        assert!(p.x.is_finite() && p.y.is_finite(), "{g:?}");
        assert!(p.x.abs() * ppp <= MAX_COORD_PX + 0.01 && p.y.abs() * ppp <= MAX_COORD_PX + 0.01, "{g:?}");
    }
}

#[test]
fn the_window_requests_that_crashed_eframe_in_w0_come_out_safe() {
    // W0-7 (a): huge, NaN, infinite, negative, zero and far-away requests.
    let requests = [
        (Some(Pos2::new(10.0, 10.0)), vec2(100_000.0, 100_000.0)),
        (Some(Pos2::new(f32::NAN, 10.0)), vec2(f32::NAN, f32::NAN)),
        (Some(Pos2::new(f32::INFINITY, f32::NEG_INFINITY)), vec2(f32::INFINITY, f32::INFINITY)),
        (Some(Pos2::new(-50.0, -50.0)), vec2(-400.0, -300.0)),
        (Some(Pos2::ZERO), vec2(0.0, 0.0)),
        (Some(Pos2::new(1.0e9, 1.0e9)), vec2(500.0, 400.0)),
    ];
    for l in [
        limits(1.0, None, None),
        limits(2.0, Some(vec2(1920.0, 1080.0)), None),
        limits(f32::NAN, Some(vec2(f32::NAN, 0.0)), None),
        limits(1.5, None, Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 984.0)))),
    ] {
        for (pos, size) in requests {
            assert_safe(validate(pos, size, l), l);
        }
    }
    let g = validate(Some(Pos2::new(f32::NAN, 1.0)), vec2(f32::NAN, 500.0), limits(1.0, None, None));
    assert_eq!(g.pos, None, "a position that isn't a number is left to the platform");
    assert_eq!(g.size, vec2(900.0, 500.0), "a side that isn't a number takes the fallback");
}

#[test]
fn sides_stay_between_the_minimum_and_the_monitor() {
    let g = validate(None, vec2(20.0, 5000.0), limits(1.0, Some(vec2(1920.0, 1080.0)), None));
    assert_eq!(g.size, vec2(MIN_SIDE_PT, 1080.0));
    // At 2 physical pixels per point, 16384 px is 8192 points.
    let g = validate(None, vec2(20_000.0, 20_000.0), limits(2.0, None, None));
    assert_eq!(g.size, vec2(8192.0, 8192.0));
    // A display smaller than the minimum: as large as the display.
    let g = validate(None, vec2(500.0, 500.0), limits(1.0, Some(vec2(80.0, 60.0)), None));
    assert_eq!(g.size, vec2(80.0, 60.0));
}

#[test]
fn a_known_usable_area_keeps_the_whole_window_inside() {
    let usable = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(1920.0, 1032.0));
    let g = validate(Some(Pos2::new(1800.0, 1000.0)), vec2(600.0, 400.0), limits(1.0, None, Some(usable)));
    assert_eq!(g.pos, Some(Pos2::new(1320.0, 632.0)));
    let g = validate(Some(Pos2::new(-300.0, -20.0)), vec2(600.0, 400.0), limits(1.0, None, Some(usable)));
    assert_eq!(g.pos, Some(Pos2::new(0.0, 0.0)));
}

#[test]
fn a_new_window_sits_32_pixels_from_its_source() {
    let source = Rect::from_min_size(Pos2::new(100.0, 80.0), vec2(800.0, 600.0));
    assert_eq!(beside(source, vec2(600.0, 400.0), limits(1.0, None, None)).pos, Some(Pos2::new(132.0, 112.0)));
    // 32 physical pixels are 16 points at 200 %.
    assert_eq!(beside(source, vec2(600.0, 400.0), limits(2.0, None, None)).pos, Some(Pos2::new(116.0, 96.0)));
    // Clamped to the usable area when it is known.
    let usable = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0));
    let near_edge = Rect::from_min_size(Pos2::new(1000.0, 500.0), vec2(200.0, 200.0));
    assert_eq!(beside(near_edge, vec2(600.0, 400.0), limits(1.0, None, Some(usable))).pos, Some(Pos2::new(680.0, 320.0)));
}

// ----------------------------------------------------------------------------- the passes

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// The window is there: the body draws it.
    Open,
    /// The platform made no window: the body never runs and nothing panics.
    Refuse,
    /// The body never runs and egui panics, with a message the app must not depend on.
    RefuseAndPanic,
    /// The body draws, then the framework panics.
    FailAfter,
}

/// Stands in for egui's immediate viewports: each window draws in a context of its own, with the
/// app's fonts, so the app's real drawing code runs.
struct Host {
    ctx: egui::Context,
    embedded: bool,
    behaviour: HashMap<ViewportId, Behaviour>,
    close: bool,
    shown: Vec<(ViewportId, Option<String>)>,
    /// Whether the window's own pass in its context ended normally.
    passes_ended: usize,
    /// Input for the next window drawn (keys pressed in it).
    events: Vec<egui::Event>,
}

impl Host {
    fn new(app: &PdfCraftApp) -> Host {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::theme::install_fonts_for(&ctx, crate::theme::CjkPreference::for_language("en"));
        crate::theme::apply(&ctx, app.theme);
        // Fonts set now take effect on the next pass.
        pass(&ctx, egui::RawInput::default(), |_| {});
        Host { ctx, embedded: false, behaviour: HashMap::new(), close: false, shown: Vec::new(), passes_ended: 0, events: Vec::new() }
    }
}

/// One pass of a window's own context; its texture changes go nowhere (no painter here).
fn pass(ctx: &egui::Context, input: egui::RawInput, mut draw: impl FnMut(&mut egui::Ui)) {
    let mut out = ctx.run_ui(input, |ui| draw(ui));
    out.textures_delta.clear();
}

impl ViewportHost for Host {
    fn embedded(&self) -> bool {
        self.embedded
    }

    fn show(&mut self, id: ViewportId, builder: ViewportBuilder, body: &mut dyn FnMut(&mut egui::Ui, ViewportClass)) {
        self.shown.push((id, builder.title.clone()));
        let behaviour = self.behaviour.get(&id).copied().unwrap_or(Behaviour::Open);
        match behaviour {
            Behaviour::Refuse => {}
            Behaviour::RefuseAndPanic => panic!("something unrelated went wrong"),
            Behaviour::Open | Behaviour::FailAfter => {
                let mut input = egui::RawInput { events: std::mem::take(&mut self.events), ..Default::default() };
                if self.close {
                    input.viewports.entry(ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
                }
                pass(&self.ctx, input, |ui| body(ui, ViewportClass::Immediate));
                self.passes_ended += 1;
                if behaviour == Behaviour::FailAfter {
                    panic!("the backend failed after drawing");
                }
            }
        }
    }
}

fn pdf(text: &str) -> Vec<u8> {
    Session::new().create_from_text("t", text).unwrap().to_vec()
}

/// The app with three tabs, a, b and c, drawn a few frames so its fonts are ready.
fn app_with_tabs() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.keep_one_window = false;
        app.force_native_windows = true;
        for name in ["a", "b", "c"] {
            app.open_bytes(&format!("{name}.pdf"), None, pdf(name)).unwrap();
        }
        app
    });
    h.run_steps(4);
    h
}

/// What W2b's "Move Tab to New Window" will do: the tab at `index` goes to a new window.
fn move_to_new_window(app: &mut PdfCraftApp, index: usize) -> WindowId {
    let view = app.win.views.remove(index);
    app.win.active = Some(0);
    let mut ui = WindowUi::new();
    ui.views.push(view);
    ui.active = Some(0);
    let geometry = validate(None, vec2(600.0, 400.0), limits(1.0, None, None));
    app.windows.add(ui, geometry, Some(Origin { window: None, index }))
}

fn ids(views: &[DocView]) -> Vec<DocId> {
    views.iter().map(|v| v.id).collect()
}

fn toast(app: &PdfCraftApp) -> String {
    app.win.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

#[test]
fn another_window_draws_its_own_tabs_and_leaves_the_main_windows_alone() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    let id = move_to_new_window(app, 1);
    let mut host = Host::new(app);
    for _ in 0..3 {
        app.show_other_windows(&mut host);
    }
    assert_eq!(host.passes_ended, 3);
    assert_eq!(ids(&app.win.views), vec![all[0], all[2]], "the main window's tabs are its own");
    assert_eq!(app.win.active, Some(0));
    assert_eq!(app.drawing_window, None);
    let window = &app.windows.others[0];
    assert_eq!(window.id, id);
    assert_eq!(ids(&window.ui.views), vec![all[1]]);
    // The window's own pass ran its own title.
    assert!(window.ui.window_title.contains("b.pdf"), "{}", window.ui.window_title);
    assert!(!app.win.window_title.contains("b.pdf"), "{}", app.win.window_title);
    assert_eq!(host.shown.last().map(|s| s.0), Some(id.viewport()));
}

#[test]
fn a_panic_while_another_window_draws_is_caught_inside_its_pass() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    move_to_new_window(app, 1);
    let mut host = Host::new(app);
    app.panic_in_other_window = true;
    app.show_other_windows(&mut host);
    // Caught inside the closure: egui's pass for that window ended normally, the main window has
    // its own state back, and the window stays with its tab.
    assert_eq!(host.passes_ended, 1, "the window's pass ended");
    assert_eq!(ids(&app.win.views), vec![all[0], all[2]]);
    assert_eq!(app.drawing_window, None);
    assert_eq!(app.windows.others.len(), 1);
    assert_eq!(app.windows.others[0].panicking_passes, 1);
    assert!(toast(app).contains("injected panic in another window"), "{}", toast(app));
    // A later pass of the same context works (an unended nested pass would panic here).
    pass(&host.ctx, egui::RawInput::default(), |_| {});
    // Panicking in pass after pass, the window gives its tab back where it came from.
    for _ in 1..MAX_PANICKING_PASSES {
        app.show_other_windows(&mut host);
    }
    assert!(app.windows.others.is_empty());
    assert_eq!(ids(&app.win.views), all);
    assert_eq!(app.win.active, Some(1), "the tab that came back is active");
}

#[test]
fn a_window_the_platform_refuses_gives_its_tabs_back_where_they_came_from() {
    for behaviour in [Behaviour::Refuse, Behaviour::RefuseAndPanic] {
        let mut h = app_with_tabs();
        let app = h.state_mut();
        let all = ids(&app.win.views);
        let id = move_to_new_window(app, 1);
        let mut host = Host::new(app);
        host.behaviour.insert(id.viewport(), behaviour);
        app.show_other_windows(&mut host);
        assert!(app.windows.others.is_empty());
        assert_eq!(ids(&app.win.views), all, "b is back between a and c");
        assert_eq!(app.win.active, Some(1));
        assert!(toast(app).starts_with("A new window could not be opened"), "{}", toast(app));
    }
}

#[test]
fn a_failure_after_the_window_drew_gives_its_tabs_back() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    let id = move_to_new_window(app, 2);
    let mut host = Host::new(app);
    host.behaviour.insert(id.viewport(), Behaviour::FailAfter);
    app.show_other_windows(&mut host);
    assert!(app.windows.others.is_empty());
    assert_eq!(ids(&app.win.views), all);
    assert_eq!(app.win.active, Some(2));
    assert!(toast(app).contains("the backend failed after drawing"), "{}", toast(app));
}

#[test]
fn closing_another_window_closes_its_tabs_and_the_documents_only_it_showed() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    move_to_new_window(app, 0);
    let docs = app.session.docs().len();
    let mut host = Host::new(app);
    host.close = true;
    // The close is asked for in the window's pass and carried out after all passes.
    app.show_other_windows(&mut host);
    assert!(app.windows.others.is_empty());
    assert_eq!(ids(&app.win.views), vec![all[1], all[2]]);
    assert_eq!(app.session.docs().len(), docs - 1, "a, shown only in that window, closed with it");
    assert!(app.session.get(all[0]).is_none());
}

#[test]
fn tabs_go_back_to_the_window_they_came_from_even_when_it_is_another_one() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    let first = move_to_new_window(app, 1);
    // c moves into the first window as its second tab, then on to a window of its own.
    let c = app.win.views.remove(1);
    app.windows.others[0].ui.views.push(c);
    let c = app.windows.others[0].ui.views.remove(1);
    let mut ui = WindowUi::new();
    ui.views.push(c);
    ui.active = Some(0);
    let second = app.windows.add(ui, validate(None, vec2(600.0, 400.0), limits(1.0, None, None)), Some(Origin { window: Some(first), index: 1 }));
    let mut host = Host::new(app);
    host.behaviour.insert(second.viewport(), Behaviour::Refuse);
    app.show_other_windows(&mut host);
    assert_eq!(app.windows.others.len(), 1);
    assert_eq!(ids(&app.windows.others[0].ui.views), vec![all[1], all[2]], "c is back in the first window");
    assert_eq!(app.windows.others[0].ui.active, Some(1));
    assert_eq!(ids(&app.win.views), vec![all[0]]);
}

#[test]
fn tabs_whose_window_is_gone_go_to_the_end_of_the_main_window() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    let view = app.win.views.remove(0);
    let mut ui = WindowUi::new();
    ui.views.push(view);
    let gone = WindowId(999);
    let id = app.windows.add(ui, validate(None, vec2(600.0, 400.0), limits(1.0, None, None)), Some(Origin { window: Some(gone), index: 0 }));
    let mut host = Host::new(app);
    host.behaviour.insert(id.viewport(), Behaviour::Refuse);
    app.show_other_windows(&mut host);
    assert_eq!(ids(&app.win.views), vec![all[1], all[2], all[0]]);
    assert_eq!(app.win.active, Some(2));
}

#[test]
fn without_native_windows_no_other_window_is_shown_and_its_tabs_come_back() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    move_to_new_window(app, 1);
    let mut host = Host::new(app);
    host.embedded = true;
    app.show_other_windows(&mut host);
    assert!(host.shown.is_empty());
    assert!(app.windows.others.is_empty());
    // Merged: after the main window's own tabs.
    assert_eq!(ids(&app.win.views), vec![all[0], all[2], all[1]]);
}

#[test]
fn the_test_harness_has_no_native_windows_so_the_app_keeps_every_tab_in_the_main_window() {
    // Through the real frame: egui_kittest embeds viewports, as wasm does.
    let mut h = app_with_tabs();
    let all = ids(&h.state().win.views);
    move_to_new_window(h.state_mut(), 2);
    h.run_steps(1);
    assert!(h.state().windows.others.is_empty());
    assert_eq!(ids(&h.state().win.views), all);
}

#[test]
fn the_single_window_preference_shows_no_other_window_and_merges_every_tab() {
    let mut h = app_with_tabs();
    let app = h.state_mut();
    let all = ids(&app.win.views);
    move_to_new_window(app, 0);
    app.keep_one_window = true;
    let mut host = Host::new(app);
    app.show_other_windows(&mut host);
    assert!(host.shown.is_empty());
    assert!(app.windows.others.is_empty());
    assert_eq!(ids(&app.win.views), vec![all[1], all[2], all[0]]);
}

#[test]
fn window_ids_are_never_reused() {
    let mut windows = Windows::default();
    let g = validate(None, vec2(600.0, 400.0), limits(1.0, None, None));
    let a = windows.add(WindowUi::new(), g, None);
    windows.others.clear();
    let b = windows.add(WindowUi::new(), g, None);
    assert_ne!(a, b);
    assert_ne!(a.viewport(), b.viewport());
    assert_ne!(a.viewport(), ViewportId::ROOT);
}

#[path = "w2b_tests.rs"]
mod w2b;

#[path = "w2c_tests.rs"]
mod w2c;

#[path = "w2d_tests.rs"]
mod w2d;

#[path = "w2e_tests.rs"]
mod w2e;

#[path = "w3_tests.rs"]
mod w3;

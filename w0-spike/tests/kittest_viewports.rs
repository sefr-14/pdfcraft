//! W0-11: can egui_kittest reach the widgets of a second immediate viewport?
#![allow(clippy::unwrap_used)]
use egui::{ViewportBuilder, ViewportClass, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

#[derive(Default)]
struct State {
    classes: Vec<&'static str>,
    clicks_root: u32,
    clicks_second: u32,
    second_viewport_id: Option<ViewportId>,
    embed: bool,
}

fn name(c: ViewportClass) -> &'static str {
    match c {
        ViewportClass::Root => "Root",
        ViewportClass::Deferred => "Deferred",
        ViewportClass::Immediate => "Immediate",
        ViewportClass::EmbeddedWindow => "EmbeddedWindow",
    }
}

fn app(ui: &mut egui::Ui, s: &mut State) {
    s.embed = ui.ctx().embed_viewports();
    ui.add_space(400.0); // the embedded window opens at the top left, over this area
    if ui.button("Root button").clicked() {
        s.clicks_root += 1;
    }
    let ctx = ui.ctx().clone();
    ctx.show_viewport_immediate(ViewportId::from_hash_of("second"), ViewportBuilder::default().with_title("Second window"), |ui, class| {
        s.classes.push(name(class));
        s.second_viewport_id = Some(ui.ctx().viewport_id());
        if ui.button("Second button").clicked() {
            s.clicks_second += 1;
        }
    });
}

#[test]
fn kittest_reaches_second_viewport_widgets() {
    let mut h = Harness::new_ui_state(app, State::default());
    h.run();
    h.get_by_label("Second button").click();
    h.run();
    h.get_by_label("Root button").click();
    h.run();
    let s = h.state();
    eprintln!(
        "W0-11: embed_viewports={} classes={:?} second viewport id={:?} clicks root={} second={}",
        s.embed,
        s.classes.iter().collect::<std::collections::BTreeSet<_>>(),
        s.second_viewport_id,
        s.clicks_root,
        s.clicks_second
    );
    // The second "window" is drawn as an egui::Window inside the root viewport.
    assert!(s.classes.iter().all(|c| *c == "EmbeddedWindow"));
    assert_eq!(s.second_viewport_id, Some(ViewportId::ROOT));
    assert_eq!((s.clicks_root, s.clicks_second), (1, 1));
}

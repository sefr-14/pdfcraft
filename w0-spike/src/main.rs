#![allow(deprecated)]
//! TAB.WINDOW.1 W0 feasibility spike (disposable; not production code).
//!
//! One process, several native windows drawn as egui *immediate* viewports from `&mut Spike`,
//! with a per-window `WindowUi` swapped out of the app while its closure runs.
//!
//! Settings: each `name=value` command-line argument, else the env var `SPIKE_<NAME>`.
//!   mode      = basic | h1 | h2 | h3 | fail | clamp | embed | drag | solo | panic | devlost | closes | a11y
//!   renderer  = wgpu | glow
//!   log       = path of the log file
//!   time_scale, inner_guard, fail_case, fail_drop, uncaptured, secondaries, anim, tick_ms, secs
//!   (see `windows/README.md` for what each mode does).
//! Machine-readable lines: `RESULT check=<id> renderer=<r> outcome=<PASS|FAIL|INFO|SKIP> detail=<text>`.
//! Every mode ends itself; the harnesses (`run.sh`, `closes.sh`, `windows/run.ps1`) add OS input.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use eframe::egui;
use egui::{Color32, Id, Pos2, Rect, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId, pos2, vec2};

static START: OnceLock<Instant> = OnceLock::new();
fn now() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
/// The script clock: seconds since the first root pass (startup time varies a lot by renderer).
static SCRIPT0: OnceLock<Instant> = OnceLock::new();
fn st() -> f64 {
    SCRIPT0.get_or_init(Instant::now).elapsed().as_secs_f64() / time_scale()
}
/// `time_scale=5` stretches every scripted time five-fold, for manual runs (Windows).
fn time_scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| setting("time_scale").and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0).unwrap_or(1.0))
}

/// A setting: the `name=value` command-line argument, else the env var `SPIKE_<NAME>`.
fn setting(name: &str) -> Option<String> {
    let key = format!("{name}=");
    std::env::args().skip(1).find_map(|a| a.strip_prefix(&key).map(str::to_owned)).or_else(|| std::env::var(format!("SPIKE_{}", name.to_uppercase())).ok())
}
fn flag(name: &str) -> bool {
    setting(name).is_some_and(|v| !v.is_empty() && v != "0")
}
fn num(name: &str, default: f64) -> f64 {
    setting(name).and_then(|v| v.parse().ok()).filter(|v: &f64| v.is_finite()).unwrap_or(default)
}

static RENDERER: OnceLock<&'static str> = OnceLock::new();
/// One machine-readable result line; the harnesses copy these into `results.csv`.
fn result(check: &str, outcome: &str, detail: &str) {
    let detail = detail.replace(['\n', '\r'], " ");
    log::info!(target: "spike", "RESULT check={check} renderer={} outcome={outcome} detail={detail}", RENDERER.get().copied().unwrap_or("?"));
}

/// wgpu uncaptured errors seen by the handler installed with `uncaptured=1`.
static UNCAPTURED: AtomicU32 = AtomicU32::new(0);

struct FileLogger(Mutex<std::fs::File>);
impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        let t = m.target();
        if t.starts_with("spike") {
            return true;
        }
        let lvl = if t.starts_with("eframe") || t.starts_with("egui_winit") || t.starts_with("egui_wgpu") || t.starts_with("egui_glow") {
            log::Level::Debug
        } else {
            log::Level::Warn
        };
        m.level() <= lvl
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            if let Ok(mut f) = self.0.lock() {
                let _ = writeln!(f, "[{:8.3}] {:5} {}: {}", now(), r.level(), r.target(), r.args());
            }
        }
    }
    fn flush(&self) {}
}

macro_rules! say { ($($t:tt)*) => { log::info!(target: "spike", $($t)*) } }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Basic,
    H1,
    H2,
    H3,
    Fail,
    Embed,
    Drag,
    Solo,
    Panic,
    Clamp,
    DevLost,
    Closes,
    A11y,
}

/// The bad geometries of W0-7 class (a): (name, position, inner size) in points.
const BAD_CASES: [(&str, [f32; 2], [f32; 2]); 6] = [
    ("huge", [10.0, 10.0], [100_000.0, 100_000.0]),
    ("nan", [f32::NAN, f32::NAN], [f32::NAN, f32::NAN]),
    ("inf", [f32::NEG_INFINITY, f32::INFINITY], [f32::INFINITY, 300.0]),
    ("negative", [10.0, 10.0], [-500.0, -500.0]),
    ("zero", [10.0, 10.0], [0.0, 0.0]),
    ("farpos", [1.0e9, -1.0e9], [400.0, 300.0]),
];

/// W0-7 class (a) prevention: bring an app-requested geometry within what every backend accepts.
///
/// * a non-finite or non-positive size becomes the default 640×480;
/// * each side is clamped to [100 pt, the current monitor's side] and to 16384 px (wgpu's common
///   texture limit; X11 itself stores sides as u16, and winit unwraps that conversion);
/// * a non-finite position is dropped (the OS places the window); a finite one is clamped to
///   ±16000 px (X11 stores positions as i16) and to within two monitor widths/heights.
///
/// Returns the clamped (position, size) and whether anything changed.
fn clamp_geometry(pos: Option<[f32; 2]>, size: [f32; 2], monitor: Option<egui::Vec2>, ppp: Option<f32>) -> (Option<[f32; 2]>, [f32; 2], bool) {
    let ppp = ppp.filter(|p| p.is_finite() && *p > 0.0).unwrap_or(1.0);
    let monitor = monitor.filter(|m| m.x.is_finite() && m.y.is_finite() && m.x >= 100.0 && m.y >= 100.0).unwrap_or(egui::vec2(1024.0, 768.0));
    let hard = 16384.0 / ppp;
    let side = |v: f32, mon: f32, default: f32| -> f32 {
        let v = if v.is_finite() && v > 0.0 { v } else { default };
        v.clamp(100.0, mon.min(hard).max(100.0))
    };
    let new_size = [side(size[0], monitor.x, 640.0), side(size[1], monitor.y, 480.0)];
    let limit = 16000.0 / ppp;
    let new_pos = pos.and_then(|p| {
        (p[0].is_finite() && p[1].is_finite()).then(|| [p[0].clamp((-2.0 * monitor.x).max(-limit), (2.0 * monitor.x).min(limit)), p[1].clamp((-2.0 * monitor.y).max(-limit), (2.0 * monitor.y).min(limit))])
    });
    let same = |a: f32, b: f32| a == b;
    let changed = !(same(new_size[0], size[0]) && same(new_size[1], size[1]) && match (pos, new_pos) {
        (Some(a), Some(b)) => same(a[0], b[0]) && same(a[1], b[1]),
        (None, None) => true,
        _ => false,
    });
    (new_pos, new_size, changed)
}

#[derive(Default)]
struct Doc {
    name: String,
    dirty: bool,
}

/// What one native window shows: swapped out of the app while its viewport closure runs.
struct WindowUi {
    id: ViewportId,
    title: String,
    docs: Vec<Doc>,
    pos: Option<[f32; 2]>,
    size: [f32; 2],
    passes: u64,
    /// W0-7 class (a) case shown by this window, and when it was asked for (script clock).
    case: Option<&'static str>,
    /// Drag mode: the desktop drop point (physical px) this window was opened at.
    drop_px: Option<Pos2>,
    created_at: f64,
    close_rx: u32,
    animate: bool,
    held_closes: u32,
    text: String,
    pass_secs: f64,
}

impl WindowUi {
    fn new(key: &str, title: &str, docs: &[(&str, bool)], pos: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            id: ViewportId::from_hash_of(key),
            title: title.to_owned(),
            docs: docs.iter().map(|(n, d)| Doc { name: (*n).to_owned(), dirty: *d }).collect(),
            pos: Some(pos),
            size,
            passes: 0,
            case: None,
            drop_px: None,
            created_at: if SCRIPT0.get().is_some() { st() } else { 0.0 },
            close_rx: 0,
            animate: false,
            held_closes: 0,
            text: String::new(),
            pass_secs: 0.0,
        }
    }
}

struct Spike {
    mode: Mode,
    root: WindowUi,
    windows: Vec<WindowUi>,
    tex: Option<egui::TextureHandle>,
    fired: HashSet<&'static str>,
    root_hidden: bool,
    snap: Vec<(String, f64, u64, u64)>,
    fail_attempts: u32,
    fail_panics: u32,
    /// The panic message of the last caught panic around `show_viewport_immediate`.
    last_panic: String,
    clamp_idx: usize,
    clamp_ok: u32,
    root_clicks: u32,
    root_close_rx: u32,
    device: Option<eframe::egui_wgpu::wgpu::Device>,
    a11y_seen: bool,
    detach_refusals: u32,
    /// Set immediately before the deliberate panic of `mode=panic`.
    panicked: bool,
    /// Drag mode: the window a tab drag started in, while the button is held.
    drag_from: Option<String>,
}

const SHARED_TEXT: &str = "shared_text";
const SHARED_HDR: &str = "shared_hdr";

impl Spike {
    fn new(mode: Mode, ctx: &egui::Context) -> Self {
        if mode == Mode::Embed {
            ctx.set_embed_viewports(true);
        }
        say!("startup: mode={mode:?} embed_viewports={}", ctx.embed_viewports());
        let mut root = WindowUi::new("root", "W0 root", &[("r.pdf", false)], [40.0, 40.0], [700.0, 500.0]);
        root.id = ViewportId::ROOT;
        let windows = match mode {
            Mode::Basic => vec![
                WindowUi::new("A", "W0 sec A", &[("a.pdf", true)], [800.0, 60.0], [420.0, 360.0]),
                WindowUi::new("B", "W0 sec B", &[("b.pdf", false)], [800.0, 520.0], [420.0, 360.0]),
            ],
            Mode::H1 | Mode::H2 | Mode::H3 | Mode::Embed => {
                let mut s = WindowUi::new("S", "W0 sec S", &[("s.pdf", false)], [900.0, 150.0], [500.0, 380.0]);
                s.animate = mode == Mode::H2 && !flag("noanim");
                vec![s]
            }
            Mode::Drag => vec![WindowUi::new("S", "W0 sec S", &[("s.pdf", false)], [800.0, 100.0], [400.0, 300.0])],
            Mode::Solo => vec![],
            Mode::Panic | Mode::DevLost | Mode::A11y => vec![WindowUi::new("S", "W0 sec S", &[("s.pdf", false)], [900.0, 150.0], [500.0, 380.0])],
            Mode::Fail => {
                let case = setting("fail_case").unwrap_or_else(|| "huge".into());
                let (name, pos, size) = BAD_CASES.iter().copied().find(|(n, ..)| *n == case).unwrap_or(BAD_CASES[0]);
                let mut w = WindowUi::new("F", &format!("W0 sec F ({name})"), &[("f.pdf", false)], pos, size);
                w.case = Some(name);
                say!("fail: case={name} pos={pos:?} size={size:?} unclamped; fail_drop={}", flag("fail_drop"));
                vec![w]
            }
            Mode::Clamp => vec![],
            Mode::Closes => {
                let n = num("secondaries", 2.0).clamp(0.0, 8.0) as usize;
                (1..=n).map(|i| {
                    let mut w = WindowUi::new(&format!("C{i}"), &format!("W0 sec C{i}"), &[(&format!("c{i}.pdf"), false)], [760.0 + 40.0 * i as f32, 60.0 + 260.0 * (i as f32 - 1.0)], [420.0, 240.0]);
                    w.animate = i == 1 && flag("anim");
                    w
                }).collect()
            }
        };
        let device = None;
        Self { mode, root, windows, tex: None, fired: HashSet::new(), root_hidden: false, snap: Vec::new(), fail_attempts: 0, fail_panics: 0, last_panic: String::new(), clamp_idx: 0, clamp_ok: 0, root_clicks: 0, root_close_rx: 0, device, a11y_seen: false, detach_refusals: 0, panicked: false, drag_from: None }
    }

    fn due(&mut self, name: &'static str, at: f64) -> bool {
        st() >= at && self.fired.insert(name)
    }

    fn snapshot(&mut self, label: &str) {
        for w in std::iter::once(&self.root).chain(self.windows.iter()) {
            self.snap.push((format!("{label}:{}", w.title), now(), w.passes, (w.pass_secs * 1e6) as u64));
        }
        result(&format!("counters_{label}"), "INFO", &format!("t={:.3} {}", now(), std::iter::once(&self.root).chain(self.windows.iter()).map(|w| format!("{}={}", w.title.replace(' ', "_"), w.passes)).collect::<Vec<_>>().join(" ")));
        say!("COUNTERS {label}: t={:.3} {}", now(), std::iter::once(&self.root).chain(self.windows.iter()).map(|w| format!("{}={} passes ({:.1} ms total in pass)", w.title, w.passes, w.pass_secs * 1e3)).collect::<Vec<_>>().join(", "));
    }

    fn log_viewports(ctx: &egui::Context, label: &str) {
        let infos = ctx.input(|i| i.raw.viewports.clone());
        for (id, info) in infos {
            say!(
                "VIEWPORT {label} {id:?}: parent={:?} title={:?} inner={:?} outer={:?} native_ppp={:?} ctx_ppp={} monitor={:?} focused={:?} minimized={:?} maximized={:?} occluded={:?} visible()={:?}",
                info.parent, info.title, info.inner_rect, info.outer_rect, info.native_pixels_per_point, ctx.pixels_per_point(), info.monitor_size, info.focused, info.minimized, info.maximized, info.occluded, info.visible()
            );
            // Physical pixels, the only unit that compares across monitors of different scale.
            let ppp = info.native_pixels_per_point.unwrap_or(1.0);
            let px = |r: Option<Rect>| r.map(|r| format!("{:.0},{:.0},{:.0}x{:.0}", r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp)).unwrap_or_else(|| "none".into());
            result(&format!("viewport_{label}"), "INFO", &format!("id={id:?} inner_px={} outer_px={} ppp={ppp} monitor_pt={:?} focused={:?} maximized={:?} minimized={:?}", px(info.inner_rect), px(info.outer_rect), info.monitor_size, info.focused, info.maximized, info.minimized));
        }
    }

    fn texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        self.tex
            .get_or_insert_with(|| {
                let (w, h) = (64usize, 64usize);
                let pixels = (0..w * h).map(|i| if i % w < w / 2 { Color32::RED } else { Color32::GREEN }).collect();
                say!("texture created in viewport {:?}", ctx.viewport_id());
                ctx.load_texture("w0-checker", egui::ColorImage::new([w, h], pixels), egui::TextureOptions::NEAREST)
            })
            .id()
    }

    /// The common contents of every window.
    fn contents(&mut self, ui: &mut egui::Ui, w: &mut WindowUi) {
        let tex = self.texture(ui.ctx());
        let vid = ui.ctx().viewport_id();
        ui.label(format!("{}  viewport={vid:?}  pass={}", w.title, w.passes));
        for d in &w.docs {
            ui.label(format!("doc {}{}", d.name, if d.dirty { " *" } else { "" }));
        }
        // W0-12: the same widget ids in every window.
        ui.add(egui::TextEdit::singleline(&mut w.text).id(Id::unique(SHARED_TEXT)));
        let hdr = egui::CollapsingHeader::new("shared header").id_salt(SHARED_HDR).show(ui, |ui| ui.label("inside"));
        if w.passes == 3 {
            say!("IDS in {vid:?}: panel ui.id()={:?} header widget id={:?} persistent_id(shared_hdr)={:?}", ui.id(), hdr.header_response.id, ui.make_persistent_id(SHARED_HDR));
        }
        // W0-2: the texture made once (in whichever pass came first) drawn at a fixed place.
        ui.painter().image(tex, Rect::from_min_size(pos2(10.0, 200.0), vec2(128.0, 128.0)), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        if self.mode == Mode::Drag {
            // The tab strip: the top 40 pt band. A press there starts a tab drag.
            let strip = Rect::from_min_size(ui.max_rect().min, vec2(ui.max_rect().width(), 40.0).min(ui.max_rect().size()));
            ui.painter().rect_filled(strip, 0.0, Color32::from_rgb(60, 90, 160));
            ui.painter().text(strip.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, "tab strip (press here and drag)", egui::FontId::proportional(14.0), Color32::WHITE);
            log_pointer(ui.ctx(), &w.title);
            self.drag_events(ui.ctx(), &w.title);
        }
        if self.mode != Mode::Drag {
            for e in ui.ctx().input(|i| i.raw.events.clone()) {
                if let egui::Event::WindowFocused(f) = e {
                    result("focus", "INFO", &format!("{} focused={f} t={:.3}", w.title, st()));
                }
            }
        }
    }

    /// One secondary window's pass. Returns whether the window stays.
    fn secondary(&mut self, ui: &mut egui::Ui, class: ViewportClass, w: &mut WindowUi, last: bool) -> bool {
        let started = Instant::now();
        w.passes += 1;
        let ctx = ui.ctx().clone();
        if w.passes == 1 {
            say!("{}: first pass class={} ctx.viewport_id={:?} parent={:?} embed={}", w.title, class_name(class), ctx.viewport_id(), ctx.parent_viewport_id(), ctx.embed_viewports());
        }
        if w.passes == 1 {
            if let Some(case) = w.case {
                let info = ctx.input(|i| i.viewport().clone());
                say!("{}: closure ran for case {case}: inner={:?} outer={:?} ppp={:?}", w.title, info.inner_rect, info.outer_rect, info.native_pixels_per_point);
                if self.mode == Mode::Fail {
                    result(&format!("fail_{case}_closure_ran"), "INFO", &format!("unclamped request ran; inner={:?}", info.inner_rect));
                }
            }
        }
        let boom = self.mode == Mode::Panic && w.title.ends_with('S') && self.due("boom", 1.0);
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.contents(ui, w);
            if boom {
                // Mid-draw: inside a panel and a nested layout, with widgets already registered.
                ui.horizontal(|ui| {
                    let _ = ui.button("half-drawn");
                    say!("{}: about to panic mid-draw inside the immediate viewport closure (viewport {:?})", w.title, ctx.viewport_id());
                    self.panicked = true;
                    panic!("spike: deliberate panic inside a secondary viewport");
                });
            }
        });
        if self.mode == Mode::Panic && w.title.ends_with('S') && !boom && self.panicked && self.fired.insert("after_boom") {
            say!("{}: pass after the panic: ctx.viewport_id()={:?} parent={:?} passes={}", w.title, ctx.viewport_id(), ctx.parent_viewport_id(), w.passes);
            result("panic_failed_window_redraws", "PASS", &format!("{} drew a pass after the panic (passes={})", w.title, w.passes));
        }
        if self.mode == Mode::Panic && w.title.ends_with('N') && w.passes == 3 {
            result("panic_new_window", "PASS", "a new secondary window opened after the panic and drew 3 passes");
        }
        if let (Some(drop), 3) = (w.drop_px, w.passes) {
            let info = ctx.input(|i| i.viewport().clone());
            let ppp = info.native_pixels_per_point.unwrap_or(1.0);
            let (dx, dy) = info.outer_rect.map_or((f32::NAN, f32::NAN), |r| (r.min.x * ppp - drop.x, r.min.y * ppp - drop.y));
            let ok = dx.abs() <= 2.0 && dy.abs() <= 2.0;
            result("drag_new_window", if ok { "PASS" } else { "INFO" }, &format!("{} requested at desktop_px=({:.0},{:.0}); outer top-left px off by ({dx:.0},{dy:.0}); ppp={ppp}; inner={:?}", w.title, drop.x, drop.y, info.inner_rect));
        }
        if self.mode == Mode::Clamp && w.passes == 3 {
            let info = ctx.input(|i| i.viewport().clone());
            let case = w.case.unwrap_or("?");
            self.clamp_ok += 1;
            result(&format!("clamp_{case}"), "PASS", &format!("clamped request opened a window: applied pos={:?} size={:?}; inner={:?} ppp={:?}", w.pos, w.size, info.inner_rect, info.native_pixels_per_point));
            return false;
        }
        if self.mode == Mode::Basic && w.title.ends_with('A') {
            if self.due("A_focus", 1.0) {
                ctx.memory_mut(|m| m.request_focus(Id::unique(SHARED_TEXT)));
                ctx.data_mut(|d| d.insert_temp(Id::unique("from_A"), 42_u32));
                say!("A: requested focus of {SHARED_TEXT} and wrote data from_A=42");
            }
            if st() > 1.3 && self.fired.insert("A_focus_log") {
                say!("A pass: memory.focused()={:?} has_focus(shared)={} shared Id={:?} Id::unique(\"panel\")={:?}", ctx.memory(|m| m.focused()), ctx.memory(|m| m.has_focus(Id::unique(SHARED_TEXT))), Id::unique(SHARED_TEXT), Id::unique("panel"));
                let open = egui::collapsing_header::CollapsingState::load(&ctx, Id::unique(SHARED_HDR)).map(|s| s.is_open());
                say!("A pass: shared header state seen from A = {open:?} (root opened it)");
            }
            if self.due("A_shot", 2.0) {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::ScreenshotCallback::new(|img| log_shot("A", &img))));
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Screenshot(egui::ScreenshotCallback::new(|img| log_shot("ROOT", &img))));
                say!("A: sent ViewportCommand::Screenshot to its own (immediate) viewport and to ROOT");
            }
        }
        if w.animate {
            ctx.request_repaint();
        }
        let close = ctx.input(|i| i.viewport().close_requested());
        w.pass_secs += started.elapsed().as_secs_f64();
        if close && self.mode == Mode::Closes {
            w.close_rx += 1;
            say!("CLOSE_RX who={} n={} t={:.3}", w.title, w.close_rx, st());
            return true; // every close is held in this mode
        }
        if close {
            say!("{}: close requested inside its own viewport {:?} (events={:?})", w.title, ctx.viewport_id(), ctx.input(|i| i.viewport().events.clone()));
            if w.docs.iter().any(|d| d.dirty) {
                w.held_closes += 1;
                say!("{}: HELD close (unsaved doc) – keep showing the viewport; CancelClose sent (n={})", w.title, w.held_closes);
                result("close_held", "PASS", &format!("{} has unsaved work: close held, window kept (n={})", w.title, w.held_closes));
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                return true;
            }
            if last && self.root_hidden {
                say!("{}: last window while root hidden -> ViewportCommand::Close to ROOT", w.title);
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
            say!("{}: accepted close -> stop showing the viewport", w.title);
            result("close_accepted", "PASS", &format!("{} closed by its own close request", w.title));
            return false;
        }
        true
    }

    fn script(&mut self, ctx: &egui::Context) {
        match self.mode {
            Mode::Basic => {
                if self.due("root_hdr", 1.0) {
                    let mut s = egui::collapsing_header::CollapsingState::load_with_default_open(ctx, Id::unique(SHARED_HDR), false);
                    s.set_open(true);
                    s.store(ctx);
                    say!("root: opened shared header state");
                }
                if st() > 1.3 && self.fired.insert("root_focus_log") {
                    say!(
                        "root pass: memory.focused()={:?} has_focus(shared)={} data from_A={:?}",
                        ctx.memory(|m| m.focused()),
                        ctx.memory(|m| m.has_focus(Id::unique(SHARED_TEXT))),
                        ctx.data(|d| d.get_temp::<u32>(Id::unique("from_A")))
                    );
                }
                if self.due("vp", 1.5) {
                    Self::log_viewports(ctx, "basic@1.5");
                }
                if self.due("busy_on", 2.0) {
                    self.snapshot("busy-start");
                    if let Some(b) = self.windows.iter_mut().find(|w| w.title.ends_with('B')) {
                        b.animate = true;
                    }
                }
                if self.due("busy_off", 4.0) {
                    self.snapshot("busy-end/idle-start");
                    for w in &mut self.windows {
                        w.animate = false;
                    }
                }
                if self.due("idle_end", 5.0) {
                    self.snapshot("idle-end");
                }
                if self.due("save_A", 6.0) {
                    for w in &mut self.windows {
                        for d in &mut w.docs {
                            d.dirty = false;
                        }
                    }
                    say!("root: 'saved' A's document (clean now)");
                }
                if self.due("close_B", 7.0) {
                    let id = ViewportId::from_hash_of("B");
                    say!("root: ViewportCommand::Close sent to B {id:?} via send_viewport_cmd_to");
                    ctx.send_viewport_cmd_to(id, ViewportCommand::Close);
                }
                if self.due("left", 7.6) {
                    say!("root: windows left = {:?}", self.windows.iter().map(|w| &w.title).collect::<Vec<_>>());
                    Self::log_viewports(ctx, "basic@7.6");
                }
                if self.due("quit", 8.0) {
                    say!("root: Close to ROOT (quit)");
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::H1 => {
                if self.due("vp1", 1.0) {
                    Self::log_viewports(ctx, "h1-before");
                }
                if self.due("vp2", 3.0) {
                    Self::log_viewports(ctx, "h1-after");
                    say!("root now shows docs {:?} title {:?}; secondaries {:?}", self.root.docs.iter().map(|d| &d.name).collect::<Vec<_>>(), self.root.title, self.windows.len());
                    let ok = self.windows.is_empty() && self.root.docs.iter().any(|d| d.name == "s.pdf");
                    result("h1_after", if ok { "PASS" } else { "INFO" }, &format!("root docs={:?} title={:?} secondaries={}", self.root.docs.iter().map(|d| &d.name).collect::<Vec<_>>(), self.root.title, self.windows.len()));
                }
                if self.due("h1_fallback_quit", 8.0) {
                    say!("h1: fallback quit");
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::H2 | Mode::H3 => {
                if self.mode == Mode::H3 && self.due("h3_hide", 0.0) {
                    say!("h3: root hides itself on its first pass");
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                    self.root_hidden = true;
                }
                if self.due("c1", 1.0) {
                    self.snapshot("visible-root-start");
                    Self::log_viewports(ctx, "h2@1");
                }
                if self.due("c2", 2.0) {
                    self.snapshot("visible-root-end");
                }
                if self.due("c3", 2.5) {
                    self.snapshot("hidden-root-start");
                }
                if self.due("c4", 4.5) {
                    self.snapshot("hidden-root-end");
                    Self::log_viewports(ctx, "h2@4.5");
                    for w in &mut self.windows {
                        w.animate = false;
                    }
                }
                if self.due("h2_fallback_quit", 9.0) {
                    say!("h2: fallback quit");
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Fail => {
                if self.due("fail_quit", 3.0) {
                    say!("fail: attempts={} panics caught={} windows={}", self.fail_attempts, self.fail_panics, self.windows.len());
                    Self::log_viewports(ctx, "fail@3");
                    let case = setting("fail_case").unwrap_or_else(|| "huge".into());
                    result(&format!("fail_{case}_survived"), "INFO", &format!("process alive at 3 s; attempts={} panics caught around show_viewport_immediate={} last={:?} windows left={} root passes={}", self.fail_attempts, self.fail_panics, self.last_panic, self.windows.len(), self.root.passes));
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Clamp => {
                let timed_out = self.windows.first().is_some_and(|w| w.passes < 3 && st() - w.created_at > 3.0);
                if timed_out {
                    let w = self.windows.remove(0);
                    result(&format!("clamp_{}", w.case.unwrap_or("?")), "FAIL", &format!("closure did not reach 3 passes in 3 s (passes={}, last panic={:?})", w.passes, self.last_panic));
                }
                if self.windows.is_empty() && self.clamp_idx < BAD_CASES.len() && st() > 0.5 {
                    let (case, pos, size) = BAD_CASES[self.clamp_idx];
                    self.clamp_idx += 1;
                    let (monitor, ppp) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().native_pixels_per_point));
                    let (cpos, csize, changed) = clamp_geometry(Some(pos), size, monitor, ppp);
                    say!("clamp: case={case} requested pos={pos:?} size={size:?} -> pos={cpos:?} size={csize:?} changed={changed} (monitor={monitor:?} ppp={ppp:?})");
                    let mut w = WindowUi::new(&format!("clamp-{case}"), &format!("W0 clamp {case}"), &[], [0.0, 0.0], csize);
                    w.pos = cpos;
                    w.case = Some(case);
                    self.windows.push(w);
                }
                if self.windows.is_empty() && self.clamp_idx >= BAD_CASES.len() && self.fired.insert("clamp_done") {
                    let ok = self.clamp_ok as usize == BAD_CASES.len();
                    result("clamp_all", if ok { "PASS" } else { "FAIL" }, &format!("{}/{} clamped bad requests opened a working window; process alive; root passes={}", self.clamp_ok, BAD_CASES.len(), self.root.passes));
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::DevLost => {
                if self.due("destroy", 1.0) {
                    if let Some(device) = &self.device {
                        say!("devlost: calling wgpu Device::destroy() (simulated device loss), uncaptured handler installed={}", flag("uncaptured"));
                        device.destroy();
                        result("devlost_destroy", "INFO", "Device::destroy() called from app code");
                    } else {
                        result("devlost", "SKIP", "glow renderer: no wgpu device to destroy");
                    }
                }
                if self.due("devlost_check", 2.0) {
                    result("devlost_root_alive", "INFO", &format!("process alive 1 s after device loss; root passes={} secondaries={} uncaptured errors seen={}", self.root.passes, self.windows.len(), UNCAPTURED.load(Ordering::Relaxed)));
                }
                if self.due("devlost_quit", 3.0) {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Closes => {
                if flag("hide_root") && self.due("closes_hide", 0.0) {
                    say!("closes: root hides itself (Visible(false)), as in H2/H3");
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                    self.root_hidden = true;
                }
                if self.due("closes_ready", 0.0) {
                    say!("CLOSES ready: root + {} secondaries; anim={} tick_ms={}", self.windows.len(), flag("anim"), num("tick_ms", 250.0));
                    Self::log_viewports(ctx, "closes");
                }
                if self.due("closes_quit", num("secs", 60.0)) {
                    result("closes_rx_root", "INFO", &format!("count={}", self.root_close_rx));
                    for w in &self.windows {
                        result(&format!("closes_rx_{}", w.title.replace(' ', "_")), "INFO", &format!("count={}", w.close_rx));
                    }
                    self.windows.clear();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::A11y => {
                if self.due("a11y_detach", 2.0) {
                    self.try_detach("x.pdf");
                }
                if self.due("a11y_quit", num("secs", 20.0)) {
                    result("a11y_detected", if self.a11y_seen { "PASS" } else { "INFO" }, &format!("AccessKit active seen={}; single-window fallback on={}; root docs={:?}; secondaries={}; detach refusals={}", self.a11y_seen, self.single_window(), self.root.docs.iter().map(|d| &d.name).collect::<Vec<_>>(), self.windows.len(), self.detach_refusals));
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Embed => {
                if self.due("embed_quit", 1.5) {
                    say!("embed: secondary passes={:?}", self.windows.iter().map(|w| w.passes).collect::<Vec<_>>());
                    result("embed", "INFO", &format!("embed_viewports={} secondary passes={:?} (a debug-build Id assert is expected, see W0-3)", ctx.embed_viewports(), self.windows.iter().map(|w| w.passes).collect::<Vec<_>>()));
                    Self::log_viewports(ctx, "embed");
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Panic => {
                if self.panicked && self.fired.insert("root_after") {
                    say!("ROOT pass after the panic: ctx.viewport_id()={:?} secondaries={} root passes={}", ctx.viewport_id(), self.windows.len(), self.root.passes);
                    result("panic_frame_ended", if ctx.viewport_id() == ViewportId::ROOT { "PASS" } else { "FAIL" }, &format!("a new root pass ran after the panic; ctx.viewport_id()={:?}; panics caught outside the closure={}", ctx.viewport_id(), self.fail_panics));
                }
                if self.due("panic_vp", 2.0) {
                    Self::log_viewports(ctx, "panic@2");
                }
                if self.due("panic_self_close", 3.0) && self.windows.iter().any(|w| w.title.ends_with('S')) {
                    self.windows.retain(|w| !w.title.ends_with('S'));
                    result("panic_failed_window_closed", "FAIL", "no close request reached the failed window by 3 s; the app removed it itself");
                }
                if self.due("panic_new", 3.2) {
                    say!("panic: opening a new secondary window N");
                    self.windows.push(WindowUi::new("N", "W0 sec N", &[("n.pdf", false)], [900.0, 150.0], [500.0, 380.0]));
                }
                if self.due("panic_quit", 4.5) {
                    result("panic_root_clicks", if self.root_clicks > 0 { "PASS" } else { "INFO" }, &format!("root button clicks received after the panic={}", self.root_clicks));
                    result("panic_survived", "PASS", &format!("process alive at 4.5 s and quitting normally; root passes={}", self.root.passes));
                    self.windows.clear();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Solo => {
                if self.due("solo_quit", 8.0) {
                    say!("solo: fallback quit");
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Drag => {
                if self.due("dvp", 0.8) {
                    Self::log_viewports(ctx, "drag@0.8");
                }
                if self.due("drag_quit", num("drag_secs", 14.0)) {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    /// Drag mode: start a tab drag on a press in a tab strip; on release, classify the drop
    /// point (desktop physical pixels) as another window's tab strip (valid: move the tab),
    /// a window's page area (invalid: cancel) or outside every window (new window). Escape
    /// while held cancels. There is no Z-order: overlapping windows take the first match.
    fn drag_events(&mut self, ctx: &egui::Context, who: &str) {
        let (events, me, all) = ctx.input(|i| (i.raw.events.clone(), i.viewport().clone(), i.raw.viewports.clone()));
        let my_ppp = me.native_pixels_per_point.unwrap_or(1.0);
        let Some(my_inner) = me.inner_rect else { return };
        for e in events {
            match e {
                egui::Event::PointerButton { pos, pressed: true, button: egui::PointerButton::Primary, .. } if pos.y <= 40.0 && self.drag_from.is_none() => {
                    self.drag_from = Some(who.to_owned());
                    result("drag_start", "INFO", &format!("tab drag started in {who} at local={pos:?} desktop_px={:?}", (my_inner.min + pos.to_vec2()) * my_ppp));
                }
                egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } if self.drag_from.is_some() => {
                    let from = self.drag_from.take().unwrap_or_default();
                    result("drag_escape", "PASS", &format!("Escape while held cancelled the drag from {from}; key delivered to {who}"));
                }
                egui::Event::PointerButton { pos, pressed: false, button: egui::PointerButton::Primary, .. } if self.drag_from.is_some() => {
                    let from = self.drag_from.take().unwrap_or_default();
                    let p = (my_inner.min + pos.to_vec2()) * my_ppp;
                    let mut target = None;
                    for (id, info) in &all {
                        let (Some(inner), ppp) = (info.inner_rect, info.native_pixels_per_point.unwrap_or(1.0)) else { continue };
                        let r = Rect::from_min_max(inner.min * ppp, inner.max * ppp);
                        if r.contains(p) {
                            let in_strip = p.y <= r.min.y + 40.0 * ppp;
                            target = Some((*id, in_strip));
                            break;
                        }
                    }
                    let source_id = ctx.viewport_id();
                    let (verdict, action) = match target {
                        Some((id, true)) if id != source_id => ("valid", "move the tab to that window"),
                        Some((_, true)) => ("same-window", "reorder within the source window"),
                        Some((_, false)) => ("invalid", "cancel: dropped on a page area"),
                        None => ("desktop", "open the tab in a new window at the drop point"),
                    };
                    if target.is_none() {
                        // "New window at the drop point": the position is given in points, which
                        // winit converts with a scale chosen before the window knows its monitor
                        // (W0-6). The new window reports where it really landed.
                        let n = self.windows.len() + 1;
                        let mut w = WindowUi::new(&format!("drop-{n}-{:.0}-{:.0}", p.x, p.y), &format!("W0 sec D{n}"), &[("dragged.pdf", false)], [p.x / my_ppp, p.y / my_ppp], [360.0, 240.0]);
                        w.drop_px = Some(p);
                        self.windows.push(w);
                    }
                    result("drag_drop", "INFO", &format!("from={from} release delivered to {who} at desktop_px=({:.0},{:.0}) target={:?} verdict={verdict} action={action}", p.x, p.y, target.map(|(id, strip)| (id, if strip { "tab strip" } else { "page" }))));
                }
                _ => {}
            }
        }
    }

    fn on_root_close(&mut self, ctx: &egui::Context) {
        say!("ROOT: close requested; secondaries={} mode={:?}", self.windows.len(), self.mode);
        if self.mode == Mode::Closes && !self.fired.contains("closes_quit") {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.root_close_rx += 1;
            say!("CLOSE_RX who=W0 root n={} t={:.3}", self.root_close_rx, st());
            return;
        }
        if self.windows.is_empty() {
            say!("ROOT: no other windows -> let eframe close (app exits)");
            return;
        }
        match self.mode {
            Mode::H1 => {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                let s = self.windows.remove(0);
                let info = ctx.input(|i| i.raw.viewports.get(&s.id).cloned()).unwrap_or_default();
                say!("H1: CancelClose; re-homing {} (outer={:?} inner={:?} maximized={:?}) into ROOT; root docs {:?} closed", s.title, info.outer_rect, info.inner_rect, info.maximized, self.root.docs.iter().map(|d| &d.name).collect::<Vec<_>>());
                let ppp = info.native_pixels_per_point.unwrap_or(1.0);
                result("h1_rehome", "INFO", &format!("{} moves into the root window; its outer_px={:?} inner_px={:?} ppp={ppp} maximized={:?}", s.title, info.outer_rect.map(|r| (r.min * ppp, r.size() * ppp)), info.inner_rect.map(|r| (r.min * ppp, r.size() * ppp)), info.maximized));
                self.root.docs = s.docs;
                self.root.title = s.title.clone();
                self.root.text = s.text;
                if let Some(outer) = info.outer_rect {
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min));
                }
                if let Some(inner) = info.inner_rect {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(inner.size()));
                }
                ctx.send_viewport_cmd(ViewportCommand::Title(s.title));
                if info.maximized == Some(true) {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
                }
                say!("H1: secondary dropped (not shown again this pass) -> its native window is destroyed at the end of this pass");
            }
            Mode::Basic | Mode::Drag => say!("{:?}: root close accepted with {} secondaries open (baseline: the app quits)", self.mode, self.windows.len()),
            Mode::H2 | Mode::H3 => {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                self.root_hidden = true;
                self.root.docs.clear();
                say!("H2: CancelClose + Visible(false) on ROOT; {} secondaries keep running", self.windows.len());
                result("h2_root_hidden", "INFO", &format!("root hidden; {} secondaries keep running", self.windows.len()));
            }
            Mode::Fail | Mode::Embed | Mode::Solo | Mode::Panic | Mode::Clamp | Mode::DevLost | Mode::A11y | Mode::Closes => {}
        }
    }

    /// The accessible single-window fallback (§13): on while AccessKit is active, or forced.
    fn single_window(&self) -> bool {
        self.a11y_seen || flag("single_window")
    }

    /// "Move Tab to New Window": refused, with the reason, while the single-window fallback is on.
    fn try_detach(&mut self, doc: &str) {
        if self.single_window() {
            self.detach_refusals += 1;
            say!("DETACH refused: 'Move Tab to New Window' is unavailable while a screen reader is active (or single_window=1): secondary windows have no accessibility tree. {doc} stays in this window.");
            result("a11y_detach_refused", "PASS", "detach refused with an explanation; document kept in the root window");
        } else {
            say!("DETACH allowed: {doc} moves to a new window");
            self.windows.push(WindowUi::new("D", "W0 sec D", &[(doc, false)], [900.0, 560.0], [400.0, 260.0]));
            result("a11y_detach_allowed", "INFO", "no assistive technology detected; detach opened a new window");
        }
    }

    /// Turn the fallback on: merge every secondary window's documents back into the root.
    fn merge_all_into_root(&mut self) {
        let n = self.windows.len();
        for w in self.windows.drain(..) {
            self.root.docs.extend(w.docs);
        }
        say!("A11Y fallback: merged {n} secondary windows into the root; root docs now {:?}", self.root.docs.iter().map(|d| &d.name).collect::<Vec<_>>());
        result("a11y_merge", "PASS", &format!("{n} secondary windows merged into the root window"));
    }
}

fn class_name(c: ViewportClass) -> &'static str {
    match c {
        ViewportClass::Root => "Root",
        ViewportClass::Deferred => "Deferred",
        ViewportClass::Immediate => "Immediate",
        ViewportClass::EmbeddedWindow => "EmbeddedWindow",
    }
}

fn log_shot(who: &str, img: &egui::ColorImage) {
    let px = |x: usize, y: usize| img.pixels.get(y * img.size[0] + x).copied();
    say!("SCREENSHOT {who}: size={:?} px(40,264)={:?} px(110,264)={:?}", img.size, px(40, 264), px(110, 264));
}

fn log_pointer(ctx: &egui::Context, who: &str) {
    let (events, inner) = ctx.input(|i| (i.raw.events.clone(), i.viewport().inner_rect));
    let origin = inner.map_or(Pos2::ZERO, |r| r.min);
    for e in events {
        match e {
            egui::Event::PointerMoved(p) => say!("PTR {who}: moved local={p:?} desktop={:?}", p + origin.to_vec2()),
            egui::Event::PointerButton { pos, pressed, button, .. } => {
                say!("PTR {who}: button {button:?} pressed={pressed} local={pos:?} desktop={:?}", pos + origin.to_vec2())
            }
            egui::Event::PointerGone => say!("PTR {who}: PointerGone"),
            egui::Event::Key { key, pressed, .. } => say!("KEY {who}: {key:?} pressed={pressed}"),
            egui::Event::WindowFocused(f) => say!("FOCUS {who}: {f}"),
            _ => {}
        }
    }
}

impl eframe::App for Spike {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        self.root.passes += 1;
        if self.root.passes == 1 {
            let _ = st();
            say!("ROOT first pass, viewport={:?}", ctx.viewport_id());
        }
        self.script(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.on_root_close(&ctx);
        }
        let mut root = std::mem::replace(&mut self.root, WindowUi::new("tmp", "tmp", &[], [0.0; 2], [0.0; 2]));
        let mut button = None;
        let mut a11y_now = false;
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.contents(ui, &mut root);
            let r = ui.button("Root button");
            if r.clicked() {
                self.root_clicks += 1;
                say!("ROOT button clicked n={} t={:.3}", self.root_clicks, st());
            }
            // AccessKit builds nodes only while an assistive technology has asked for the tree.
            a11y_now = ctx.has_accesskit_node(r.id);
            button = Some(r.rect);
        });
        self.root = root;
        if self.root.passes == 5 && let Some(rect) = button {
            let (inner, ppp) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().native_pixels_per_point.unwrap_or(1.0)));
            if let Some(inner) = inner {
                let c = (inner.min + rect.center().to_vec2()) * ppp;
                say!("CLICK_TARGET root_button x={} y={}", c.x.round(), c.y.round());
            }
        }
        if a11y_now && !self.a11y_seen {
            self.a11y_seen = true;
            say!("A11Y: AccessKit became active (an assistive technology requested the tree) at t={:.3}", st());
            result("a11y_active", "INFO", "AccessKit tree requested: an assistive technology is active");
            if self.mode == Mode::A11y {
                self.merge_all_into_root();
            }
        }
        if self.mode == Mode::Basic && self.root.passes == 2 {
            say!("root: Id::unique(\"panel\")={:?}", Id::unique("panel"));
        }

        let taken = std::mem::take(&mut self.windows);
        let n = taken.len();
        let mut kept = Vec::with_capacity(n);
        for (i, mut w) in taken.into_iter().enumerate() {
            let mut builder = ViewportBuilder::default().with_title(w.title.clone()).with_inner_size(w.size);
            if let Some(pos) = w.pos {
                builder = builder.with_position(pos);
            }
            let id = w.id;
            if self.mode == Mode::Fail {
                self.fail_attempts += 1;
            }
            let shown = std::panic::catch_unwind(AssertUnwindSafe(|| ctx.show_viewport_immediate(id, builder, |ui, class| {
                if flag("inner_guard") {
                    // Guard inside the closure, so egui's nested pass of this viewport still ends.
                    match std::panic::catch_unwind(AssertUnwindSafe(|| self.secondary(ui, class, &mut w, i + 1 == n))) {
                        Ok(keep) => keep,
                        Err(_) => {
                            say!("INNER GUARD caught a panic inside the viewport closure; window kept");
                            true
                        }
                    }
                } else {
                    self.secondary(ui, class, &mut w, i + 1 == n)
                }
            })));
            match shown {
                Ok(true) => kept.push(w),
                Ok(false) => {
                    say!("{} removed from the session's window list", w.title);
                    if self.mode == Mode::Panic && w.title.ends_with('S') {
                        result("panic_failed_window_closed", "PASS", "a close request reached the window whose closure panicked; it closed normally");
                    }
                }
                Err(p) => {
                    let msg = p.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                    self.fail_panics += 1;
                    let not_called = msg.contains("user callback was never called");
                    if self.fail_panics <= 3 {
                        say!("PANIC caught around show_viewport_immediate for {} (closure not called={not_called}): {msg:?}", w.title);
                    }
                    self.last_panic = msg;
                    if flag("fail_drop") {
                        // Recovery attempt: abandon the window, keep its documents in the root.
                        say!("fail_drop: abandoning {} after a failed creation; its docs move to the root; ctx.viewport_id() now {:?}", w.title, ctx.viewport_id());
                        result(&format!("fail_{}_dropped", w.case.unwrap_or("?")), "INFO", &format!("window abandoned after creation failure (closure not called={not_called})"));
                        self.root.docs.extend(w.docs);
                    } else {
                        kept.push(w);
                    }
                }
            }
        }
        kept.append(&mut self.windows);
        self.windows = kept;
        // Drive the script clock without a busy loop (`tick_ms=0`: only one wake, at quit time).
        let tick = num("tick_ms", 250.0);
        let wait = if tick > 0.0 { tick / 1000.0 } else { (num("secs", 60.0) - st()).max(0.05) * time_scale() };
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        self.root.pass_secs += started.elapsed().as_secs_f64();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        say!("on_exit: root passes={} snapshots={}", self.root.passes, self.snap.len());
    }
}

fn main() -> eframe::Result {
    let _ = now();
    let path = setting("log").unwrap_or_else(|| "w0-spike.log".into());
    let file = std::fs::File::create(&path).expect("log file");
    let _ = log::set_boxed_logger(Box::new(FileLogger(Mutex::new(file))));
    log::set_max_level(log::LevelFilter::Debug);
    std::panic::set_hook(Box::new(|info| { let bt = std::backtrace::Backtrace::force_capture().to_string(); let frames: Vec<&str> = bt.lines().filter(|l| l.contains("eframe::") || l.contains("w0_spike::") || l.contains("winit::") || l.contains("egui::context")).collect(); log::error!(target: "spike", "panic hook: {info}\n  {}", frames.join("\n  ")) }));

    let mode = match setting("mode").as_deref() {
        Some("h1") => Mode::H1,
        Some("h2") => Mode::H2,
        Some("h3") => Mode::H3,
        Some("fail") => Mode::Fail,
        Some("clamp") => Mode::Clamp,
        Some("embed") => Mode::Embed,
        Some("drag") => Mode::Drag,
        Some("solo") => Mode::Solo,
        Some("panic") => Mode::Panic,
        Some("devlost") => Mode::DevLost,
        Some("closes") => Mode::Closes,
        Some("a11y") => Mode::A11y,
        _ => Mode::Basic,
    };
    let renderer = match setting("renderer").as_deref() {
        Some("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    };
    let _ = RENDERER.set(if renderer == eframe::Renderer::Glow { "glow" } else { "wgpu" });
    let mut viewport = ViewportBuilder::default().with_title("W0 root").with_inner_size([700.0, 500.0]).with_position([40.0, 40.0]);
    if mode == Mode::H3 {
        viewport = viewport.with_visible(false);
    }
    let options = eframe::NativeOptions { viewport, renderer, persistence_path: None, ..Default::default() };
    say!("pid={} renderer={renderer:?} os={} args={:?}", std::process::id(), std::env::consts::OS, std::env::args().skip(1).collect::<Vec<_>>());
    let r = eframe::run_native(
        "w0-spike",
        options,
        Box::new(move |cc| {
            let mut spike = Spike::new(mode, &cc.egui_ctx);
            if let Some(rs) = &cc.wgpu_render_state {
                let info = rs.adapter.get_info();
                say!("wgpu adapter: {} backend={:?} driver={} {}", info.name, info.backend, info.driver, info.driver_info);
                if flag("uncaptured") {
                    rs.device.on_uncaptured_error(Arc::new(|e| {
                        let n = UNCAPTURED.fetch_add(1, Ordering::Relaxed) + 1;
                        if n <= 3 {
                            log::error!(target: "spike", "UNCAPTURED wgpu error #{n}: {e}");
                        }
                    }));
                }
                spike.device = Some(rs.device.clone());
            }
            Ok(Box::new(spike))
        }),
    );
    say!("run_native returned {:?}", r.as_ref().map_err(|e| e.to_string()));
    result("run_native_returned", if r.is_ok() { "PASS" } else { "FAIL" }, &format!("{:?}", r.as_ref().map_err(|e| e.to_string())));
    r
}

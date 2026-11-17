#![allow(deprecated)]
//! TAB.WINDOW.1 W0 feasibility spike (disposable; not production code).
//!
//! One process, several native windows drawn as egui *immediate* viewports from `&mut Spike`,
//! with a per-window `WindowUi` swapped out of the app while its closure runs.
//!
//! Env: SPIKE_MODE = basic | h1 | h2 | h3 | fail | embed | drag
//!      SPIKE_RENDERER = wgpu | glow
//!      SPIKE_LOG = path of the log file
//! Every mode ends itself; the harness (`run.sh`) adds external X11 input at fixed times.

use std::collections::HashSet;
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
/// `SPIKE_TIME_SCALE=5` stretches every scripted time five-fold, for manual runs (Windows).
fn time_scale() -> f64 {
    static SCALE: OnceLock<f64> = OnceLock::new();
    *SCALE.get_or_init(|| std::env::var("SPIKE_TIME_SCALE").ok().and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0).unwrap_or(1.0))
}

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
    pos: [f32; 2],
    size: [f32; 2],
    passes: u64,
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
            pos,
            size,
            passes: 0,
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
                s.animate = mode == Mode::H2 && std::env::var_os("SPIKE_NOANIM").is_none();
                vec![s]
            }
            Mode::Drag => vec![WindowUi::new("S", "W0 sec S", &[("s.pdf", false)], [800.0, 100.0], [400.0, 300.0])],
            Mode::Solo => vec![],
            Mode::Panic => vec![WindowUi::new("S", "W0 sec S", &[("s.pdf", false)], [900.0, 150.0], [500.0, 380.0])],
            Mode::Fail => vec![WindowUi::new("F", "W0 sec F (huge)", &[("f.pdf", false)], [10.0, 10.0], [100_000.0, 100_000.0])],
        };
        Self { mode, root, windows, tex: None, fired: HashSet::new(), root_hidden: false, snap: Vec::new(), fail_attempts: 0, fail_panics: 0 }
    }

    fn due(&mut self, name: &'static str, at: f64) -> bool {
        st() >= at && self.fired.insert(name)
    }

    fn snapshot(&mut self, label: &str) {
        for w in std::iter::once(&self.root).chain(self.windows.iter()) {
            self.snap.push((format!("{label}:{}", w.title), now(), w.passes, (w.pass_secs * 1e6) as u64));
        }
        say!("COUNTERS {label}: t={:.3} {}", now(), std::iter::once(&self.root).chain(self.windows.iter()).map(|w| format!("{}={} passes ({:.1} ms total in pass)", w.title, w.passes, w.pass_secs * 1e3)).collect::<Vec<_>>().join(", "));
    }

    fn log_viewports(ctx: &egui::Context, label: &str) {
        let infos = ctx.input(|i| i.raw.viewports.clone());
        for (id, info) in infos {
            say!(
                "VIEWPORT {label} {id:?}: parent={:?} title={:?} inner={:?} outer={:?} native_ppp={:?} ctx_ppp={} monitor={:?} focused={:?} minimized={:?} maximized={:?} occluded={:?} visible()={:?}",
                info.parent, info.title, info.inner_rect, info.outer_rect, info.native_pixels_per_point, ctx.pixels_per_point(), info.monitor_size, info.focused, info.minimized, info.maximized, info.occluded, info.visible()
            );
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
            log_pointer(ui.ctx(), &w.title);
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
        egui::CentralPanel::default().show_inside(ui, |ui| self.contents(ui, w));
        if self.mode == Mode::Panic && self.due("boom", 1.0) {
            say!("{}: about to panic inside the immediate viewport closure (viewport {:?})", w.title, ctx.viewport_id());
            panic!("spike: deliberate panic inside a secondary viewport");
        }
        if self.mode == Mode::Panic && st() > 1.2 && self.fired.insert("after_boom") {
            say!("{}: pass after the panic: ctx.viewport_id()={:?} parent={:?} passes={}", w.title, ctx.viewport_id(), ctx.parent_viewport_id(), w.passes);
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
        if close {
            say!("{}: close requested inside its own viewport {:?} (events={:?})", w.title, ctx.viewport_id(), ctx.input(|i| i.viewport().events.clone()));
            if w.docs.iter().any(|d| d.dirty) {
                w.held_closes += 1;
                say!("{}: HELD close (unsaved doc) – keep showing the viewport; CancelClose sent (n={})", w.title, w.held_closes);
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                return true;
            }
            if last && self.root_hidden {
                say!("{}: last window while root hidden -> ViewportCommand::Close to ROOT", w.title);
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
            say!("{}: accepted close -> stop showing the viewport", w.title);
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
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Embed => {
                if self.due("embed_quit", 1.5) {
                    say!("embed: secondary passes={:?}", self.windows.iter().map(|w| w.passes).collect::<Vec<_>>());
                    Self::log_viewports(ctx, "embed");
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Mode::Panic => {
                if st() > 1.1 && self.fired.insert("root_after") {
                    say!("ROOT pass after the panic: ctx.viewport_id()={:?} secondaries={} root passes={}", ctx.viewport_id(), self.windows.len(), self.root.passes);
                }
                if self.due("panic_vp", 2.0) {
                    Self::log_viewports(ctx, "panic@2");
                }
                if self.due("panic_quit", 3.0) {
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
                if self.due("drag_quit", std::env::var("SPIKE_DRAG_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(14.0)) {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    fn on_root_close(&mut self, ctx: &egui::Context) {
        say!("ROOT: close requested; secondaries={} mode={:?}", self.windows.len(), self.mode);
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
            }
            Mode::Fail | Mode::Embed | Mode::Solo | Mode::Panic => {}
        }
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
        egui::CentralPanel::default().show_inside(ui, |ui| self.contents(ui, &mut root));
        self.root = root;
        if self.mode == Mode::Basic && self.root.passes == 2 {
            say!("root: Id::unique(\"panel\")={:?}", Id::unique("panel"));
        }

        let taken = std::mem::take(&mut self.windows);
        let n = taken.len();
        let mut kept = Vec::with_capacity(n);
        for (i, mut w) in taken.into_iter().enumerate() {
            let builder = ViewportBuilder::default().with_title(w.title.clone()).with_inner_size(w.size).with_position(w.pos);
            let id = w.id;
            if self.mode == Mode::Fail {
                self.fail_attempts += 1;
            }
            let shown = std::panic::catch_unwind(AssertUnwindSafe(|| ctx.show_viewport_immediate(id, builder, |ui, class| {
                if std::env::var_os("SPIKE_INNER_GUARD").is_some() {
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
                Ok(false) => say!("{} removed from the session's window list", w.title),
                Err(p) => {
                    let msg = p.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                    self.fail_panics += 1;
                    if self.fail_panics <= 3 {
                        say!("PANIC caught around show_viewport_immediate for {}: {msg:?}", w.title);
                    }
                    kept.push(w);
                }
            }
        }
        kept.append(&mut self.windows);
        self.windows = kept;
        // Drive the script clock without a busy loop.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        self.root.pass_secs += started.elapsed().as_secs_f64();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        say!("on_exit: root passes={} snapshots={}", self.root.passes, self.snap.len());
    }
}

fn main() -> eframe::Result {
    let _ = now();
    let path = std::env::var("SPIKE_LOG").unwrap_or_else(|_| "w0-spike.log".into());
    let file = std::fs::File::create(&path).expect("log file");
    let _ = log::set_boxed_logger(Box::new(FileLogger(Mutex::new(file))));
    log::set_max_level(log::LevelFilter::Debug);
    std::panic::set_hook(Box::new(|info| { let bt = std::backtrace::Backtrace::force_capture().to_string(); let frames: Vec<&str> = bt.lines().filter(|l| l.contains("eframe::") || l.contains("w0_spike::") || l.contains("winit::") || l.contains("egui::context")).collect(); log::error!(target: "spike", "panic hook: {info}\n  {}", frames.join("\n  ")) }));

    let mode = match std::env::var("SPIKE_MODE").as_deref() {
        Ok("h1") => Mode::H1,
        Ok("h2") => Mode::H2,
        Ok("h3") => Mode::H3,
        Ok("fail") => Mode::Fail,
        Ok("embed") => Mode::Embed,
        Ok("drag") => Mode::Drag,
        Ok("solo") => Mode::Solo,
        Ok("panic") => Mode::Panic,
        _ => Mode::Basic,
    };
    let renderer = match std::env::var("SPIKE_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    };
    let mut viewport = ViewportBuilder::default().with_title("W0 root").with_inner_size([700.0, 500.0]).with_position([40.0, 40.0]);
    if mode == Mode::H3 {
        viewport = viewport.with_visible(false);
    }
    let options = eframe::NativeOptions { viewport, renderer, persistence_path: None, ..Default::default() };
    say!("pid={} renderer={renderer:?}", std::process::id());
    let r = eframe::run_native("w0-spike", options, Box::new(move |cc| Ok(Box::new(Spike::new(mode, &cc.egui_ctx)))));
    say!("run_native returned {:?}", r.as_ref().map_err(|e| e.to_string()));
    r
}

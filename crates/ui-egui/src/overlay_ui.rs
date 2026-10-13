//! Overlay compare (this fork's overlay feature, step 6): the seven-step wizard that draws two
//! revisions on one page, each in its own colour, so shared content prints dark and every
//! difference shows in colour.
//!
//! Documents, Pages, Colours, Sheet sizes, Alignment, Preview, Generate. Back and Next keep every
//! setting; a change in an earlier step keeps later work keyed to what it belongs to (a page
//! pair's sheet choice and picks follow the pair, not its row). The engine is `pdfcraft-overlay`
//! (registration, compositing, layers and annotations): this module collects the choices, shows
//! what the engine says, and opens the overlay as a new, unsaved tab. The sources are never
//! changed. Rendering and generation run off the UI thread, so a large drawing doesn't freeze the
//! window.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Align, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::DocId;
use pdfcraft_overlay::colour;
use pdfcraft_overlay::snap::{Lines, SnapKind, SnapOptions};
use pdfcraft_overlay::{
    Affine, Fit, LayerStyle, Limits, Matrix, Model, OverlayError, PT_PER_MM, PairPlan, Plan, Point, PointPair, RegistrationKind, RegistrationNote,
    Revision, Roles, Rule, Settings, SheetCase, SheetMatch, Similarity, Unresolved, Visibility,
};
use pdfcraft_overlay::{Pair, Pairing, PairingError, Slot, Summary};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// This fork's interface strings, looked up like `tl!`. They aren't in upstream's language
/// catalogs (fork decision, main overlay PRD, Constraints: untranslated strings show in English),
/// so they stay out of upstream's catalog-coverage scan of `tl!` literals; an entry added to a
/// catalog is still used.
fn tx(s: &str) -> &str {
    crate::i18n::t(s)
}

/// The mode bar's Overlay button (this fork's one hook in the toolbar).
pub(crate) fn toolbar_button(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let tip = tx("Overlay compare: two revisions on one page, each in its own colour");
    if widgets::ghost_button(ui, "blend", tx("Overlay")).on_hover_text(tip).clicked() {
        app.run_command("doc.overlay");
    }
}

/// Longest side, in pixels, of a source page rendered for picking reference points.
const PICK_PIXELS: f64 = 2400.0;
/// How near the pointer, in screen pixels, a line end or crossing must be for a click to snap.
const SNAP_PIXELS: f64 = 10.0;
/// Longest side, in pixels, of a rendered preview.
const PREVIEW_PIXELS: f64 = 2400.0;
/// The pairing table never grows past this (each row is a page of the output).
const MAX_PAIRS: usize = 2000;
/// Flicker never switches faster than this (WCAG: under 3 flashes a second).
const MAX_FLICKER_HZ: f32 = 2.0;

// ── Steps ───────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Documents,
    Pages,
    Colours,
    Sheets,
    Alignment,
    Preview,
    Generate,
}

impl Step {
    pub const ALL: [Step; 7] = [Step::Documents, Step::Pages, Step::Colours, Step::Sheets, Step::Alignment, Step::Preview, Step::Generate];

    fn title(self) -> &'static str {
        match self {
            Step::Documents => "Documents",
            Step::Pages => "Pages",
            Step::Colours => "Colours",
            Step::Sheets => "Sheet sizes",
            Step::Alignment => "Alignment",
            Step::Preview => "Preview",
            Step::Generate => "Generate",
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Step {
        Step::ALL.get(self.index() + 1).copied().unwrap_or(self)
    }

    fn back(self) -> Step {
        self.index().checked_sub(1).and_then(|i| Step::ALL.get(i).copied()).unwrap_or(self)
    }
}

// ── Sources and their snapshot ──────────────────────────────────────────────────────────────

/// A or B.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    A,
    B,
}

impl Side {
    fn letter(self) -> &'static str {
        match self {
            Side::A => "A",
            Side::B => "B",
        }
    }

    fn index(self) -> usize {
        match self {
            Side::A => 0,
            Side::B => 1,
        }
    }

    /// The pairing model's name for this side.
    fn column(self) -> pdfcraft_overlay::Side {
        match self {
            Side::A => pdfcraft_overlay::Side::A,
            Side::B => pdfcraft_overlay::Side::B,
        }
    }
}

/// A source document as the wizard took it: its file and its layers as its tab showed them
/// (main PRD decision 1b). Preview and output use this snapshot only; later changes in the tab
/// apply when the user accepts them.
pub struct Snapshot {
    pub id: DocId,
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub password: Option<String>,
    /// Each layer's group, name and state in the tab.
    pub layers: Vec<((u32, u16), String, bool)>,
    pub labels: Vec<String>,
    /// The file has page labels (`/PageLabels`). Without them `labels` are only page numbers,
    /// and no label is shown: a sheet's label is never taken to be its page number, or the
    /// reverse.
    pub has_labels: bool,
    generation: u64,
    doc: Option<pdfcraft_cos::Document>,
    /// The tab's layer states as explicit decisions, where they differ from the file's default.
    base_decisions: Vec<(ObjRef, bool)>,
    /// Why the document can't be read for the overlay, if so.
    pub error: Option<String>,
    sizes: HashMap<usize, Option<(f64, f64)>>,
}

impl Snapshot {
    fn take(app: &PdfCraftApp, id: DocId) -> Option<Snapshot> {
        let d = app.session.get(id)?;
        let opened = pdfcraft_engine::guard(|| pdfcraft_cos::Document::open_with_password(d.bytes.clone(), d.password.as_deref()));
        let (doc, error) = match opened {
            Ok(Ok(doc)) => (Some(doc), None),
            Ok(Err(e)) => (None, Some(e.to_string())),
            Err(m) => (None, Some(m)),
        };
        let shown: Vec<(ObjRef, bool)> = d.info.layers.iter().map(|l| (ObjRef::new(l.id.0, l.id.1), l.visible)).collect();
        let has_labels = doc
            .as_ref()
            .and_then(|doc| doc.root().and_then(|r| doc.dict(&pdfcraft_cos::Object::Ref(r))))
            .is_some_and(|catalog| catalog.contains(b"PageLabels"));
        let base_decisions =
            doc.as_ref().and_then(|doc| pdfcraft_engine::guard(|| pdfcraft_overlay::snapshot_decisions(doc, &shown)).ok()).unwrap_or_default();
        Some(Snapshot {
            id,
            name: d.name.clone(),
            bytes: d.bytes.clone(),
            password: d.password.clone(),
            layers: d.info.layers.iter().map(|l| (l.id, l.name.clone(), l.visible)).collect(),
            labels: d.info.pages.iter().map(|p| p.label.clone()).collect(),
            has_labels,
            generation: d.edit_generation(),
            doc,
            base_decisions,
            error,
            sizes: HashMap::new(),
        })
    }

    fn pages(&self) -> usize {
        self.labels.len()
    }

    /// The page's physical position in the file, as a viewer numbers it.
    fn page_text(page: usize) -> String {
        crate::i18n::fmt(tx("p. {n}"), &[("n", &(page + 1).to_string())])
    }

    /// The page's PDF label (`/PageLabels`), if the file has labels.
    fn label(&self, page: usize) -> Option<&str> {
        if self.has_labels { self.labels.get(page).map(String::as_str) } else { None }
    }

    /// Physical position and label, kept apart: "p. 15 · label A-201".
    fn page_name(&self, page: usize) -> String {
        match self.label(page) {
            Some(l) => crate::i18n::fmt(tx("{page} · label {label}"), &[("page", &Self::page_text(page)), ("label", l)]),
            None => Self::page_text(page),
        }
    }

    /// What the tab shows now, to tell whether it changed since the snapshot (an edit, or a
    /// layer switched).
    fn signature_of(app: &PdfCraftApp, id: DocId) -> Option<(u64, usize, Vec<bool>)> {
        let d = app.session.get(id)?;
        Some((d.edit_generation(), Arc::as_ptr(&d.bytes) as usize, d.info.layers.iter().map(|l| l.visible).collect()))
    }

    fn signature(&self) -> (u64, usize, Vec<bool>) {
        (self.generation, Arc::as_ptr(&self.bytes) as usize, self.layers.iter().map(|l| l.2).collect())
    }

    /// The displayed sheet size of `page` in points (pdfcraft's own frame).
    fn size(&mut self, page: usize) -> Option<(f64, f64)> {
        if let Some(s) = self.sizes.get(&page) {
            return *s;
        }
        let s = self.doc.as_ref().and_then(|d| pdfcraft_overlay::frame_of_page(d, page).ok()).map(|(f, _)| f.size());
        self.sizes.insert(page, s);
        s
    }

    fn source(&self) -> SourceFile {
        SourceFile { bytes: self.bytes.clone(), password: self.password.clone() }
    }

    /// The renderer's layer overrides for the snapshot.
    fn overrides(&self) -> Vec<(i32, i32, bool)> {
        self.layers.iter().map(|((n, g), _, v)| (i32::try_from(*n).unwrap_or(i32::MAX), i32::from(*g), *v)).collect()
    }

    /// The layers the tab hides: their content isn't snapped to.
    fn hidden(&self) -> HashSet<ObjRef> {
        self.layers.iter().filter(|(_, _, shown)| !shown).map(|((n, g), _, _)| ObjRef::new(*n, *g)).collect()
    }

    fn layer_name(&self, r: ObjRef) -> Option<&str> {
        self.layers.iter().find(|((n, g), _, _)| *n == r.num && *g == r.generation).map(|(_, name, _)| name.as_str())
    }

    fn tab_state(&self, r: ObjRef) -> Option<bool> {
        self.layers.iter().find(|((n, g), _, _)| *n == r.num && *g == r.generation).map(|(_, _, v)| *v)
    }
}

/// What a background job needs of a source.
#[derive(Clone)]
struct SourceFile {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
}

impl SourceFile {
    fn open(&self) -> Result<pdfcraft_cos::Document, String> {
        pdfcraft_cos::Document::open_with_password(self.bytes.clone(), self.password.as_deref()).map_err(|e| e.to_string())
    }
}

// ── Pairs, sheets, alignment ────────────────────────────────────────────────────────────────

/// The pairing as it was before an operation, for Undo and Redo.
#[derive(Clone, Debug, PartialEq)]
struct PairingState {
    range_a: (usize, usize),
    range_b: (usize, usize),
    pairing: Pairing,
}

/// An operation waiting for the user to confirm, because it would replace pairings they made.
#[derive(Clone, Debug, PartialEq)]
enum Confirm {
    /// Re-pair both ranges in order.
    RePair,
    /// A new page range for one document.
    Range { side: Side, range: (usize, usize) },
    /// Another document B.
    ChangeB { id: DocId },
}

/// How B's sheet is matched to A's when Match sheet sizes is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SheetChoice {
    OneToOne,
    Factor(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SheetState {
    choice: SheetChoice,
    /// The user confirmed the choice (a factor is never applied silently).
    confirmed: bool,
}

/// Reference points of one page pair, and the final nudge.
#[derive(Clone, Debug, Default, PartialEq)]
struct Picks {
    /// (on A, on B), each in its document's displayed frame, in points. B's are before any sheet
    /// factor; the factor is applied when fitting.
    picks: Vec<(Point, Point)>,
    pending_a: Option<Point>,
    pending_b: Option<Point>,
    /// Advanced: a general affine fit (changes the drawing's proportions).
    affine: bool,
    /// Typed values that replace the fit (the final nudge).
    manual: Option<Similarity>,
}

/// A pair's registration, as the engine fitted or the user typed it.
struct Registration {
    matrix: Matrix,
    note: RegistrationNote,
    /// The residual report of the picks, when there are enough to check.
    report: Option<FitReport>,
}

struct FitReport {
    residuals_mm: Vec<f64>,
    flagged: Vec<bool>,
    rms_mm: f64,
    max_mm: f64,
    exact: bool,
    verdict: String,
    consistent: bool,
    similarity: Option<Similarity>,
}

impl FitReport {
    fn of<M: Model>(f: &Fit<M>, similarity: Option<Similarity>) -> Self {
        FitReport {
            residuals_mm: f.points.iter().map(|p| p.residual_mm).collect(),
            flagged: f.points.iter().map(|p| p.flagged()).collect(),
            rms_mm: f.rms_mm,
            max_mm: f.max_mm,
            exact: f.exact,
            verdict: f.verdict.to_string(),
            consistent: f.verdict == pdfcraft_overlay::Verdict::Consistent,
            similarity,
        }
    }
}

// ── Background work ─────────────────────────────────────────────────────────────────────────

enum Done {
    Page { side: Side, page: usize, image: Result<egui::ColorImage, String> },
    Lines { side: Side, page: usize, lines: Result<Lines, String> },
    Preview { key: String, result: Result<PreviewOut, Failure> },
    Generated { result: Result<GeneratedOut, Failure> },
}

/// Why a preview or the overlay couldn't be made.
#[derive(Clone, Debug)]
enum Failure {
    Overlay(OverlayError),
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Overlay(e) => write!(f, "{e}"),
            Failure::Other(s) => f.write_str(s),
        }
    }
}

struct PreviewOut {
    both: egui::ColorImage,
    a_only: egui::ColorImage,
    b_only: egui::ColorImage,
    /// B's sheet outline on the output page, normalised to [0, 1] (y down), when part is clipped.
    outline: Option<[Pos2; 4]>,
    report: String,
}

struct GeneratedOut {
    bytes: Arc<Vec<u8>>,
    report: String,
    warnings: Vec<String>,
    pages: usize,
    clipped: Vec<usize>,
}

/// Run `job` off the UI thread (inline on the web), then wake the UI. A panic becomes `failed`'s
/// result: the app and its documents keep running.
fn spawn(ctx: &egui::Context, tx: &Sender<Done>, job: impl FnOnce() -> Done + Send + 'static, failed: impl FnOnce(String) -> Done + Send + 'static) {
    let (ctx, tx) = (ctx.clone(), tx.clone());
    let run = move || {
        let done = pdfcraft_engine::guard(job).unwrap_or_else(|m| failed(format!("an internal error stopped it ({m})")));
        let _ = tx.send(done);
        ctx.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        if std::thread::Builder::new().name("overlay".into()).spawn(run).is_err() {
            log::warn!("overlay: couldn't start a worker thread");
        }
    }
    #[cfg(target_arch = "wasm32")]
    run();
}

fn image_of(out: &pdfcraft_render::RenderedPage) -> Result<egui::ColorImage, String> {
    if let Some(e) = &out.error {
        return Err(e.clone());
    }
    let (w, h) = (out.width as usize, out.height as usize);
    if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(4)) != Some(out.rgba.len()) {
        return Err("the page rendered empty".into());
    }
    Ok(egui::ColorImage::from_rgba_premultiplied([w, h], &out.rgba))
}

fn scale_for(size: (f64, f64), pixels: f64) -> f32 {
    let longest = size.0.max(size.1);
    if !(longest.is_finite() && longest > 0.0) {
        return 1.0;
    }
    (pixels / longest).clamp(0.02, 8.0) as f32
}

fn render_page(file: &SourceFile, overrides: Vec<(i32, i32, bool)>, page: usize, size: (f64, f64), pixels: f64) -> Result<egui::ColorImage, String> {
    let config = RenderConfig { password: file.password.as_deref().map(Arc::from), layers: Arc::new(overrides), ..RenderConfig::default() };
    let mut r = PageRenderer::new(file.bytes.clone(), config);
    image_of(&r.render(RenderRequest { page, scale: scale_for(size, pixels), ..RenderRequest::default() }))
}

/// The longest side a render may have: the wanted size, within the GPU's texture limit.
fn budget(ctx: &egui::Context, wanted: f64) -> f64 {
    let max = ctx.input(|i| i.max_texture_side) as f64;
    // A pixel short of the limit, so rounding in the renderer can't cross it.
    wanted.min((max - 1.0).max(64.0))
}

/// One overlay page from the snapshot, rendered three ways: both layers, A's alone, B's alone.
fn preview(a: SourceFile, b: SourceFile, va: Visibility, vb: Visibility, plan: Plan, pixels: f64) -> Result<PreviewOut, Failure> {
    let (da, db) = (a.open().map_err(Failure::Other)?, b.open().map_err(Failure::Other)?);
    let pair = plan.pairs.first().cloned().ok_or(Failure::Overlay(OverlayError::NoSources))?;
    let (placement, _) = pdfcraft_overlay::pair_placement(&da, &db, &pair, plan.expand, &plan.settings).map_err(Failure::Overlay)?;
    let generated = pdfcraft_overlay::generate(&da, &db, &va, &vb, &plan).map_err(Failure::Overlay)?;
    let bytes = Arc::new(pdfcraft_cos::write_full(&generated.doc, &Default::default()).map_err(|e| Failure::Other(e.to_string()))?);
    let saved = pdfcraft_cos::Document::open(bytes.clone()).map_err(|e| Failure::Other(e.to_string()))?;
    let groups = pdfcraft_overlay::check_output(&saved).map_err(Failure::Overlay)?;
    let off = |r: ObjRef| vec![(i32::try_from(r.num).unwrap_or(i32::MAX), i32::from(r.generation), false)];
    let (old, new) = (groups.old, groups.new);
    let (a_layer, b_layer) = match plan.roles.a {
        Revision::Old => (old, new),
        Revision::New => (new, old),
    };
    let scale = scale_for((placement.width, placement.height), pixels);
    let render = |overrides: Vec<(i32, i32, bool)>| {
        let mut r = PageRenderer::new(bytes.clone(), RenderConfig { layers: Arc::new(overrides), ..RenderConfig::default() });
        image_of(&r.render(RenderRequest { page: 0, scale, ..RenderRequest::default() })).map_err(Failure::Other)
    };
    let outline = (placement.b_clipped && pair.a.is_some() && pair.b.is_some())
        .then(|| placement.b_outline.map(|p| pos2((p.x / placement.width) as f32, (1.0 - p.y / placement.height) as f32)));
    Ok(PreviewOut {
        both: render(Vec::new())?,
        a_only: render(off(b_layer))?,
        b_only: render(off(a_layer))?,
        outline,
        report: generated.report.text(),
    })
}

fn generate(a: SourceFile, b: SourceFile, va: Visibility, vb: Visibility, plan: Plan) -> Result<GeneratedOut, Failure> {
    let (da, db) = (a.open().map_err(Failure::Other)?, b.open().map_err(Failure::Other)?);
    let g = pdfcraft_overlay::generate(&da, &db, &va, &vb, &plan).map_err(Failure::Overlay)?;
    let bytes = pdfcraft_cos::write_full(&g.doc, &Default::default()).map_err(|e| Failure::Other(e.to_string()))?;
    let mut warnings = g.report.warnings();
    warnings.extend(g.warnings.iter().map(|w| w.to_string()));
    Ok(GeneratedOut { bytes: Arc::new(bytes), report: g.report.text(), warnings, pages: plan.pairs.len(), clipped: g.clipped })
}

// ── Image views ─────────────────────────────────────────────────────────────────────────────

struct PageImage {
    texture: egui::TextureHandle,
    size: Vec2,
}

impl PageImage {
    fn new(ctx: &egui::Context, name: &str, image: egui::ColorImage) -> Self {
        let image = fit_texture(image, ctx.input(|i| i.max_texture_side));
        let size = vec2(image.size[0] as f32, image.size[1] as f32);
        PageImage { texture: ctx.load_texture(name, image, egui::TextureOptions::LINEAR), size }
    }
}

/// `image`, shrunk (nearest pixel) so neither side exceeds the GPU's texture limit; renders ask
/// for no more than the limit, so this only catches rounding.
fn fit_texture(image: egui::ColorImage, max_side: usize) -> egui::ColorImage {
    let [w, h] = image.size;
    let max_side = max_side.max(1);
    if w <= max_side && h <= max_side {
        return image;
    }
    let k = (max_side as f64 / w.max(h) as f64).min(1.0);
    let (nw, nh) = (((w as f64 * k).floor() as usize).clamp(1, max_side), ((h as f64 * k).floor() as usize).clamp(1, max_side));
    let mut pixels = Vec::with_capacity(nw.saturating_mul(nh));
    for y in 0..nh {
        let sy = (y * h / nh).min(h.saturating_sub(1));
        for x in 0..nw {
            let sx = (x * w / nw).min(w.saturating_sub(1));
            pixels.push(image.pixels.get(sy * w + sx).copied().unwrap_or(Color32::WHITE));
        }
    }
    egui::ColorImage::new([nw, nh], pixels)
}

/// Zoom and pan of one image view: `zoom` 1 fits the image; `centre` is the view's centre in
/// normalised image coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    zoom: f32,
    centre: Vec2,
}

impl Default for View {
    fn default() -> Self {
        View { zoom: 1.0, centre: vec2(0.5, 0.5) }
    }
}

/// What a click on an image view picked, in normalised image coordinates (0..1, y down).
struct Shown {
    rect: Rect,
    image: Rect,
    clicked: Option<Vec2>,
    /// Where the pointer rests on the image (not while dragging).
    hover: Option<Vec2>,
}

impl Shown {
    fn to_screen(&self, n: Vec2) -> Pos2 {
        self.image.min + n * self.image.size()
    }
}

/// Draw `image` in `size`, with the wheel zooming about the pointer and a drag panning. A click
/// (not a drag) is returned as a pick.
fn image_view(ui: &mut egui::Ui, label: &str, view: &mut View, image: Option<&PageImage>, size: Vec2, placeholder: &str) -> Shown {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    // Until its image arrives the view is named by its placeholder ("Rendering…").
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, ui.is_enabled(), if image.is_some() { label } else { placeholder }));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, t.pasteboard);
    let Some(img) = image else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, placeholder, theme::regular(13.0), t.text_muted);
        return Shown { rect, image: rect, clicked: None, hover: None };
    };
    let fit = (rect.width() / img.size.x).min(rect.height() / img.size.y).max(1e-6);
    let shown_size = img.size * fit * view.zoom;
    if resp.hovered() {
        let (scroll, pointer) = ui.input(|i| (i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0, i.pointer.hover_pos()));
        // The wheel zooms the image, so the step around it doesn't scroll too.
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        if scroll.abs() > 0.0 {
            let old = view.zoom;
            view.zoom = (view.zoom * (scroll / 400.0).exp()).clamp(1.0, 32.0);
            // Keep the point under the pointer where it is.
            if let Some(p) = pointer {
                let before = view.centre + (p - rect.center()) / (img.size * fit * old);
                let after = view.centre + (p - rect.center()) / (img.size * fit * view.zoom);
                view.centre += before - after;
            }
        }
    }
    if resp.dragged() {
        view.centre -= resp.drag_delta() / shown_size;
    }
    let half = vec2(0.5, 0.5) / view.zoom;
    view.centre = vec2(view.centre.x.clamp(half.x.min(0.5), (1.0 - half.x).max(0.5)), view.centre.y.clamp(half.y.min(0.5), (1.0 - half.y).max(0.5)));
    let image_rect = Rect::from_center_size(rect.center() - (view.centre - vec2(0.5, 0.5)) * shown_size, shown_size);
    painter.rect_filled(image_rect, 0.0, Color32::WHITE);
    painter.image(img.texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    let clicked = resp.clicked().then(|| resp.interact_pointer_pos()).flatten().map(|p| (p - image_rect.min) / image_rect.size());
    let on_image = |n: &Vec2| (0.0..=1.0).contains(&n.x) && (0.0..=1.0).contains(&n.y);
    let clicked = clicked.filter(on_image);
    let hover = (!resp.dragged()).then(|| resp.hover_pos()).flatten().map(|p| (p - image_rect.min) / image_rect.size()).filter(on_image);
    if ui.input(|i| i.pointer.any_down()) || resp.hovered() {
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Crosshair });
    }
    Shown { rect, image: image_rect, clicked, hover }
}

// ── The wizard ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewMode {
    Both,
    AOnly,
    BOnly,
    Transparency,
    Flicker,
}

struct Preview {
    pair: usize,
    mode: PreviewMode,
    /// B's opacity over A in Transparency.
    alpha: f32,
    hz: f32,
    view: View,
    key: String,
    pending: Option<String>,
    images: Option<[PageImage; 3]>,
    outline: Option<[Pos2; 4]>,
    report: String,
    error: Option<Failure>,
}

impl Default for Preview {
    fn default() -> Self {
        Preview {
            pair: 0,
            mode: PreviewMode::Both,
            alpha: 0.5,
            hz: 1.0,
            view: View::default(),
            key: String::new(),
            pending: None,
            images: None,
            outline: None,
            report: String::new(),
            error: None,
        }
    }
}

/// The finished overlay, as the wizard reports it.
struct Outcome {
    name: String,
    report: String,
    warnings: Vec<String>,
    pages: usize,
    clipped: Vec<usize>,
}

pub struct Wizard {
    pub step: Step,
    pub open: bool,
    // 1. Documents
    pub a: Option<Snapshot>,
    pub b: Option<Snapshot>,
    pub b_choice: Option<DocId>,
    /// A is `New` and B is `Old` (by default A is `Old`, O1).
    pub swapped: bool,
    /// The tab or its layers changed since the snapshot (A, B): the user is asked at each step.
    stale: [bool; 2],
    /// The tab state the user chose to keep the snapshot against, so they're asked again only if
    /// it changes again.
    kept: [Option<(u64, usize, Vec<bool>)>; 2],
    // 2. Pages (1-based, inclusive)
    pub range_a: (usize, usize),
    pub range_b: (usize, usize),
    /// Each document's column of pages and blanks (`pdfcraft_overlay::pairing`); `pairs` is its
    /// rows, kept in step with it.
    pairing: Pairing,
    pub pairs: Vec<Pair>,
    history: Vec<PairingState>,
    future: Vec<PairingState>,
    /// The ranges as typed, applied at once or after a confirmation.
    range_draft: [(usize, usize); 2],
    confirm: Option<Confirm>,
    /// Why the last pairing operation was refused.
    pairing_note: Option<String>,
    // 3. Colours
    pub old_colour: [u8; 3],
    pub new_colour: [u8; 3],
    pub complementary: bool,
    pub threshold: f64,
    // 4. Sheet sizes
    pub match_sheets: bool,
    pub same_sheet_for_all: bool,
    sheets: HashMap<(usize, usize), SheetState>,
    // 5. Alignment
    pub same_alignment_for_all: bool,
    aligns: HashMap<(usize, usize), Picks>,
    align_pair: usize,
    views: [View; 2],
    pages: HashMap<(Side, usize), PageImage>,
    pages_pending: HashSet<(Side, usize)>,
    page_errors: HashMap<(Side, usize), String>,
    /// Clicks snap to the drawing's line ends and crossings (and, if chosen, midpoints and lines).
    pub snap_enabled: bool,
    pub snap: SnapOptions,
    /// The drawn lines of the pages on show, for snapping.
    lines: HashMap<(Side, usize), Arc<Lines>>,
    lines_pending: HashSet<(Side, usize)>,
    lines_errors: HashMap<(Side, usize), String>,
    // 6. Preview
    preview: Preview,
    // 7. Generate
    pub expand: bool,
    decisions: [HashMap<ObjRef, bool>; 2],
    omit: [HashSet<ObjRef>; 2],
    unresolved: Option<(Vec<Unresolved>, Vec<Unresolved>)>,
    generating: bool,
    generate_requested: bool,
    outcome: Option<Outcome>,
    failure: Option<String>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
}

impl Wizard {
    fn new(app: &PdfCraftApp, a: DocId) -> Wizard {
        let (tx, rx) = channel();
        let snap = Snapshot::take(app, a);
        let n = snap.as_ref().map_or(0, Snapshot::pages).max(1);
        let others: Vec<DocId> = app.views.iter().map(|v| v.id).filter(|id| *id != a).collect();
        let mut w = Wizard {
            step: Step::Documents,
            open: true,
            a: snap,
            b: None,
            b_choice: others.first().copied(),
            swapped: false,
            stale: [false; 2],
            kept: [None, None],
            range_a: (1, n),
            range_b: (1, 1),
            pairing: Pairing::default(),
            pairs: Vec::new(),
            history: Vec::new(),
            future: Vec::new(),
            range_draft: [(1, n), (1, 1)],
            confirm: None,
            pairing_note: None,
            old_colour: pdfcraft_overlay::RED,
            new_colour: pdfcraft_overlay::CYAN,
            complementary: true,
            threshold: 1.0,
            match_sheets: false,
            same_sheet_for_all: true,
            sheets: HashMap::new(),
            same_alignment_for_all: true,
            aligns: HashMap::new(),
            align_pair: 0,
            views: [View::default(); 2],
            pages: HashMap::new(),
            pages_pending: HashSet::new(),
            page_errors: HashMap::new(),
            snap_enabled: true,
            snap: SnapOptions::default(),
            lines: HashMap::new(),
            lines_pending: HashSet::new(),
            lines_errors: HashMap::new(),
            preview: Preview::default(),
            expand: false,
            decisions: [HashMap::new(), HashMap::new()],
            omit: [HashSet::new(), HashSet::new()],
            unresolved: None,
            generating: false,
            generate_requested: false,
            outcome: None,
            failure: None,
            tx,
            rx,
        };
        if let Some(b) = w.b_choice {
            w.set_b(app, b);
        }
        w
    }

    fn snapshot(&self, side: Side) -> Option<&Snapshot> {
        match side {
            Side::A => self.a.as_ref(),
            Side::B => self.b.as_ref(),
        }
    }

    fn snapshot_mut(&mut self, side: Side) -> Option<&mut Snapshot> {
        match side {
            Side::A => self.a.as_mut(),
            Side::B => self.b.as_mut(),
        }
    }

    fn set_b(&mut self, app: &PdfCraftApp, id: DocId) {
        if self.b.as_ref().is_some_and(|b| b.id == id) {
            return;
        }
        self.b = Snapshot::take(app, id);
        let n = self.b.as_ref().map_or(0, Snapshot::pages).max(1);
        self.range_b = (1, n);
        self.range_draft = [self.range_a, self.range_b];
        self.forget_side(Side::B);
        // A different document: its pairing starts in order, and the old one can't be undone
        // into it.
        self.history.clear();
        self.future.clear();
        self.refill_pairs();
    }

    /// Images and errors that belonged to one side's old snapshot.
    fn forget_side(&mut self, side: Side) {
        self.pages.retain(|(s, _), _| *s != side);
        self.page_errors.retain(|(s, _), _| *s != side);
        self.lines.retain(|(s, _), _| *s != side);
        self.lines_errors.retain(|(s, _), _| *s != side);
        self.decisions[side.index()].clear();
        self.omit[side.index()].clear();
        self.unresolved = None;
        self.preview.key.clear();
        self.preview.images = None;
    }

    pub fn roles(&self) -> Roles {
        if self.swapped { Roles::SWAPPED } else { Roles::default() }
    }

    fn colour_of(&self, side: Side) -> [u8; 3] {
        let role = match side {
            Side::A => self.roles().a,
            Side::B => self.roles().b,
        };
        match role {
            Revision::Old => self.old_colour,
            Revision::New => self.new_colour,
        }
    }

    fn role_of(&self, side: Side) -> &'static str {
        match side {
            Side::A => self.roles().a.name(),
            Side::B => self.roles().b.name(),
        }
    }

    // ── Pages ──

    fn range_pages(range: (usize, usize)) -> Vec<usize> {
        let (from, to) = (range.0.max(1), range.1.max(1));
        if from > to {
            return Vec::new();
        }
        (from - 1..to).take(MAX_PAIRS).collect()
    }

    /// Pair the two ranges in order; the longer range's extra pages wait for a choice.
    pub fn refill_pairs(&mut self) {
        self.pairing = self.in_order();
        self.sync_pairs();
    }

    fn in_order(&self) -> Pairing {
        Pairing::in_order(&Self::range_pages(self.range_a), &Self::range_pages(self.range_b))
    }

    fn sync_pairs(&mut self) {
        self.pairs = self.pairing.rows();
        self.align_pair = self.align_pair.min(self.pairs.len().saturating_sub(1));
        self.preview.pair = self.preview.pair.min(self.pairs.len().saturating_sub(1));
    }

    fn state(&self) -> PairingState {
        PairingState { range_a: self.range_a, range_b: self.range_b, pairing: self.pairing.clone() }
    }

    fn restore(&mut self, s: PairingState) {
        (self.range_a, self.range_b, self.pairing) = (s.range_a, s.range_b, s.pairing);
        self.range_draft = [self.range_a, self.range_b];
        self.sync_pairs();
    }

    /// Record the current pairing for Undo, then make `next` current.
    fn commit(&mut self, next: PairingState) {
        if next == self.state() {
            return;
        }
        self.history.push(self.state());
        self.history.truncate(500);
        self.future.clear();
        self.restore(next);
        self.pairing_note = None;
    }

    /// Run a pairing operation; a refused one changes nothing and says why.
    fn pairing_op(&mut self, op: impl FnOnce(&mut Pairing) -> Result<(), PairingError>) {
        let mut next = self.state();
        match op(&mut next.pairing) {
            Ok(()) => self.commit(next),
            Err(e) => self.pairing_note = Some(e.to_string()),
        }
    }

    fn undo_pairing(&mut self) {
        if let Some(prev) = self.history.pop() {
            self.future.push(self.state());
            self.restore(prev);
        }
    }

    fn redo_pairing(&mut self) {
        if let Some(next) = self.future.pop() {
            self.history.push(self.state());
            self.restore(next);
        }
    }

    /// The pairing differs from both ranges paired in order: the user made pairings that a
    /// re-pairing would replace.
    fn manual_edits(&self) -> bool {
        self.pairing != self.in_order()
    }

    /// Re-pair both ranges in order (undoable).
    fn repair(&mut self) {
        let next = PairingState { pairing: self.in_order(), ..self.state() };
        self.commit(next);
    }

    /// A new range for `side`. `keep`: keep the pairing (pages left out are removed from that
    /// column, new pages go at its end); otherwise re-pair in order. Undoable either way.
    fn set_range(&mut self, side: Side, range: (usize, usize), keep: bool) {
        let mut next = self.state();
        match side {
            Side::A => next.range_a = range,
            Side::B => next.range_b = range,
        }
        if keep {
            next.pairing.set_pages(side.column(), &Self::range_pages(range));
        } else {
            next.pairing = Pairing::in_order(&Self::range_pages(next.range_a), &Self::range_pages(next.range_b));
        }
        self.commit(next);
    }

    /// The document that is `Old` (by default A) and the one that is `New`.
    fn old_new(&self) -> (Side, Side) {
        if self.swapped { (Side::B, Side::A) } else { (Side::A, Side::B) }
    }

    /// The summary in Old and New terms: matched, Old only, New only, without a partner, empty.
    fn summary(&self) -> (Summary, usize, usize) {
        let s = self.pairing.summary();
        let (old_only, new_only) = if self.swapped { (s.b_only, s.a_only) } else { (s.a_only, s.b_only) };
        (s, old_only, new_only)
    }

    /// Why the pairing can't be used yet, if so.
    pub fn pairing_problem(&self) -> Option<String> {
        if self.pairs.is_empty() {
            return Some(tx("Choose a page range for each document.").into());
        }
        let (s, _, _) = self.summary();
        if s.unplaced > 0 {
            return Some(crate::i18n::fmt(
                tx("{n} page(s) have no partner yet: mark sheets as inserted or deleted, choose pages, or place the remaining pages against blanks."),
                &[("n", &s.unplaced.to_string())],
            ));
        }
        if s.empty > 0 {
            return Some(tx("An output page has no page from either document. Undo the last change or remove a gap.").into());
        }
        for (side, range) in [(Side::A, self.range_a), (Side::B, self.range_b)] {
            let missing: Vec<String> = self.pairing.missing(side.column(), &Self::range_pages(range)).into_iter().map(Snapshot::page_text).collect();
            if !missing.is_empty() {
                return Some(crate::i18n::fmt(
                    tx("{pages} of document {side} are in its range but in no row. Place them, or change the range."),
                    &[("pages", &missing.join(", ")), ("side", side.letter())],
                ));
            }
            let twice = self.pairing.duplicates(side.column());
            if !twice.is_empty() {
                let list: Vec<String> = twice.into_iter().map(Snapshot::page_text).collect();
                return Some(crate::i18n::fmt(
                    tx("{pages} of document {side} are in more than one row."),
                    &[("pages", &list.join(", ")), ("side", side.letter())],
                ));
            }
        }
        None
    }

    /// The first row with a page on both sides: its sheet choice and alignment apply to every
    /// pair while "the same for every pair" is on.
    fn first_pair(&self) -> Option<(usize, usize)> {
        self.pairs.iter().find_map(Pair::pages)
    }

    // ── Sheets ──

    fn sheet_key(&self, pair: (usize, usize)) -> (usize, usize) {
        if self.same_sheet_for_all { self.first_pair().unwrap_or(pair) } else { pair }
    }

    fn sheet_state(&mut self, pair: (usize, usize)) -> SheetState {
        let key = self.sheet_key(pair);
        if let Some(s) = self.sheets.get(&key) {
            return *s;
        }
        let proposal = self.proposal(key);
        let s = match proposal.as_ref().map(|p| (p.case, p.factor)) {
            Some((SheetCase::SameSheet, _)) | None => SheetState { choice: SheetChoice::OneToOne, confirmed: true },
            Some((SheetCase::SameShape, f)) => SheetState { choice: SheetChoice::Factor(f), confirmed: false },
            Some((SheetCase::DifferentShape, _)) => SheetState { choice: SheetChoice::OneToOne, confirmed: false },
        };
        self.sheets.insert(key, s);
        s
    }

    fn proposal(&mut self, (pa, pb): (usize, usize)) -> Option<pdfcraft_overlay::SheetProposal> {
        let sa = self.a.as_mut()?.size(pa)?;
        let sb = self.b.as_mut()?.size(pb)?;
        pdfcraft_overlay::propose(sa, sb, &Settings::default()).ok()
    }

    /// The sheet match the overlay uses for `pair`.
    fn sheet_match(&mut self, pair: (usize, usize)) -> SheetMatch {
        if !self.match_sheets {
            return SheetMatch::Off;
        }
        match self.sheet_state(pair).choice {
            SheetChoice::OneToOne => SheetMatch::Off,
            SheetChoice::Factor(factor) => SheetMatch::Scaled { factor },
        }
    }

    fn sheets_problem(&mut self) -> Option<String> {
        if !self.match_sheets {
            return None;
        }
        let pairs: Vec<(usize, usize)> = self.pairs.iter().filter_map(Pair::pages).collect();
        for pair in pairs {
            if !self.sheet_state(pair).confirmed {
                return Some(tx("Confirm the sheet-size choice for every pair (or turn Match sheet sizes off).").into());
            }
        }
        None
    }

    // ── Alignment ──

    fn align_key(&self, pair: (usize, usize)) -> (usize, usize) {
        if self.same_alignment_for_all { self.first_pair().unwrap_or(pair) } else { pair }
    }

    fn limits(&mut self, pair: (usize, usize)) -> Option<(Limits, Matrix)> {
        let (pa, pb) = pair;
        let sa = self.a.as_mut()?.size(pa)?;
        let sb = self.b.as_mut()?.size(pb)?;
        let sheet = self.sheet_match(pair);
        let k = sheet.transform(sa, sb).ok()?;
        let factor = match sheet {
            SheetMatch::Off => 1.0,
            SheetMatch::Scaled { factor } => factor,
        };
        let diag = |s: (f64, f64)| s.0.hypot(s.1);
        Some((Limits { diag_a: diag(sa), diag_b: diag(sb) * factor, settings: Settings::default() }, k))
    }

    /// The registration the overlay uses for `pair`: the fit of its picks (those of the first
    /// pair while the alignment is shared), or the typed values; none without picks.
    fn registration(&mut self, pair: (usize, usize)) -> Result<Registration, String> {
        let key = self.align_key(pair);
        let align = self.aligns.get(&key).cloned().unwrap_or_default();
        let Some((limits, k)) = self.limits(key) else { return Err(tx("The pages can't be read for alignment.").into()) };
        let picks: Vec<PointPair> = align
            .picks
            .iter()
            .map(|(a, b)| {
                let (x, y) = k.apply(b.x, b.y);
                PointPair::new(Point::new(x, y), *a)
            })
            .collect();
        let note = |kind, f: Option<&FitReport>| RegistrationNote { kind, rms_mm: f.map(|f| f.rms_mm), max_mm: f.map(|f| f.max_mm) };
        if let Some(sim) = align.manual {
            let report = (picks.len() >= 2).then(|| FitReport::of(&pdfcraft_overlay::report(sim, &picks, &limits), Some(sim)));
            let kind = if report.is_some() { RegistrationKind::Similarity } else { RegistrationKind::Matrix };
            return Ok(Registration { matrix: sim.matrix(), note: note(kind, report.as_ref()), report });
        }
        if picks.is_empty() {
            return Ok(Registration { matrix: Matrix::IDENTITY, note: RegistrationNote::PLACED, report: None });
        }
        if align.affine {
            if picks.len() < Affine::MIN_PAIRS {
                return Err(tx("Affine alignment needs at least three point pairs (four or more to check them).").into());
            }
            let fit = pdfcraft_overlay::fit::<Affine>(&picks, &limits).map_err(|r| r.to_string())?;
            let report = FitReport::of(&fit, None);
            return Ok(Registration { matrix: fit.transform.matrix(), note: note(RegistrationKind::Affine, Some(&report)), report: Some(report) });
        }
        if picks.len() < Similarity::MIN_PAIRS {
            return Err(tx("Pick at least two point pairs (three or more to check them), or remove the pick.").into());
        }
        let fit = pdfcraft_overlay::fit::<Similarity>(&picks, &limits).map_err(|r| r.to_string())?;
        let report = FitReport::of(&fit, Some(fit.transform));
        Ok(Registration { matrix: fit.transform.matrix(), note: note(RegistrationKind::Similarity, Some(&report)), report: Some(report) })
    }

    fn alignment_problem(&mut self) -> Option<String> {
        let pairs: Vec<(usize, usize)> = self.pairs.iter().filter_map(Pair::pages).collect();
        let mut seen = HashSet::new();
        for pair in pairs {
            let key = self.align_key(pair);
            if !seen.insert(key) {
                continue;
            }
            if let Err(e) = self.registration(pair) {
                return Some(crate::i18n::fmt(
                    tx("Pair A {a} / B {b}: {e}"),
                    &[("a", &(key.0 + 1).to_string()), ("b", &(key.1 + 1).to_string()), ("e", &e)],
                ));
            }
        }
        None
    }

    // ── The plan ──

    fn style(&self) -> LayerStyle {
        LayerStyle { old: self.old_colour, new: self.new_colour, threshold: self.threshold }
    }

    fn pair_plan(&mut self, pair: Pair) -> Result<PairPlan, String> {
        let Some(pages) = pair.pages() else { return Ok(PairPlan::placed(pair.a.page(), pair.b.page())) };
        let sheet = self.sheet_match(pages);
        let r = self.registration(pages)?;
        Ok(PairPlan { a: pair.a.page(), b: pair.b.page(), sheet, registration: r.matrix, note: r.note })
    }

    fn plan(&mut self, pairs: &[Pair]) -> Result<Plan, String> {
        let mut out = Vec::with_capacity(pairs.len());
        for p in pairs {
            out.push(self.pair_plan(*p)?);
        }
        Ok(Plan { pairs: out, style: self.style(), roles: self.roles(), expand: self.expand, settings: Settings::default() })
    }

    /// The explicit visibility for one side: the snapshot's layer states where they differ from
    /// the file's default, then the user's decisions on unresolved items.
    fn visibility(&self, side: Side) -> Visibility {
        let Some(s) = self.snapshot(side) else { return Visibility::default() };
        let mut decisions: HashMap<ObjRef, bool> = s.base_decisions.iter().copied().collect();
        decisions.extend(self.decisions[side.index()].iter().map(|(r, v)| (*r, *v)));
        let mut decisions: Vec<(ObjRef, bool)> = decisions.into_iter().collect();
        decisions.sort_by_key(|(r, _)| (r.num, r.generation));
        let mut omit: Vec<ObjRef> = self.omit[side.index()].iter().copied().collect();
        omit.sort_by_key(|r| (r.num, r.generation));
        Visibility { decisions, omit }
    }

    fn can_leave(&mut self, step: Step) -> Option<String> {
        match step {
            Step::Documents => {
                let (Some(a), Some(b)) = (&self.a, &self.b) else {
                    return Some(tx("Choose document B: another open document (open it first with Open a file…).").into());
                };
                for (s, letter) in [(a, "A"), (b, "B")] {
                    if let Some(e) = &s.error {
                        return Some(crate::i18n::fmt(tx("Document {side} can't be read for the overlay: {e}"), &[("side", letter), ("e", e)]));
                    }
                }
                None
            }
            Step::Pages => self.pairing_problem(),
            Step::Colours => None,
            Step::Sheets => self.sheets_problem(),
            Step::Alignment => self.alignment_problem(),
            Step::Preview | Step::Generate => None,
        }
    }

    // ── Background results ──

    fn poll(&mut self, app: &mut PdfCraftApp, ctx: &egui::Context) {
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Page { side, page, image } => {
                    self.pages_pending.remove(&(side, page));
                    match image {
                        Ok(img) => {
                            self.pages.insert((side, page), PageImage::new(ctx, &format!("overlay-{}-{page}", side.letter()), img));
                        }
                        Err(e) => {
                            self.page_errors.insert((side, page), e);
                        }
                    }
                }
                Done::Lines { side, page, lines } => {
                    self.lines_pending.remove(&(side, page));
                    match lines {
                        Ok(l) => {
                            self.lines.insert((side, page), Arc::new(l));
                        }
                        Err(e) => {
                            self.lines_errors.insert((side, page), e);
                        }
                    }
                }
                Done::Preview { key, result } => {
                    if self.preview.pending.as_ref() == Some(&key) {
                        self.preview.pending = None;
                    }
                    self.preview.key = key;
                    match result {
                        Ok(out) => {
                            self.preview.images = Some([
                                PageImage::new(ctx, "overlay-preview-both", out.both),
                                PageImage::new(ctx, "overlay-preview-a", out.a_only),
                                PageImage::new(ctx, "overlay-preview-b", out.b_only),
                            ]);
                            self.preview.outline = out.outline;
                            self.preview.report = out.report;
                            self.preview.error = None;
                            self.unresolved = None;
                        }
                        Err(e) => {
                            self.preview.images = None;
                            if let Failure::Overlay(OverlayError::Unresolved { a, b, .. }) = &e {
                                self.unresolved = Some((a.clone(), b.clone()));
                            }
                            self.preview.error = Some(e);
                        }
                    }
                }
                Done::Generated { result } => {
                    self.generating = false;
                    match result {
                        Ok(out) => {
                            let stem = |s: &Option<Snapshot>| s.as_ref().map(|s| crate::files::strip_pdf(&s.name).to_string()).unwrap_or_default();
                            let name = format!("Overlay {} vs {}.pdf", stem(&self.a), stem(&self.b));
                            app.open_created(&name, out.bytes, "Created the overlay as a new, unsaved document");
                            self.outcome = Some(Outcome { name, report: out.report, warnings: out.warnings, pages: out.pages, clipped: out.clipped });
                            self.unresolved = None;
                            self.failure = None;
                        }
                        Err(e) => {
                            if let Failure::Overlay(OverlayError::Unresolved { a, b, .. }) = &e {
                                self.unresolved = Some((a.clone(), b.clone()));
                            }
                            self.failure = Some(e.to_string());
                        }
                    }
                }
            }
        }
    }

    fn request_page(&mut self, ctx: &egui::Context, side: Side, page: usize) {
        let key = (side, page);
        if self.pages.contains_key(&key) || self.pages_pending.contains(&key) || self.page_errors.contains_key(&key) {
            return;
        }
        let Some(snap) = self.snapshot_mut(side) else { return };
        let Some(size) = snap.size(page) else { return };
        let (file, overrides) = (snap.source(), snap.overrides());
        let pixels = budget(ctx, PICK_PIXELS);
        self.pages_pending.insert(key);
        spawn(
            ctx,
            &self.tx,
            move || Done::Page { side, page, image: render_page(&file, overrides, page, size, pixels) },
            move |m| Done::Page { side, page, image: Err(m) },
        );
    }

    /// Read the drawn lines of `page` for snapping, off the UI thread, once per snapshot.
    fn request_lines(&mut self, ctx: &egui::Context, side: Side, page: usize) {
        let key = (side, page);
        if self.lines.contains_key(&key) || self.lines_pending.contains(&key) || self.lines_errors.contains_key(&key) {
            return;
        }
        let Some(snap) = self.snapshot(side) else { return };
        let (file, hidden) = (snap.source(), snap.hidden());
        self.lines_pending.insert(key);
        spawn(
            ctx,
            &self.tx,
            move || Done::Lines {
                side,
                page,
                lines: file.open().and_then(|doc| pdfcraft_overlay::snap::lines(&doc, page, &hidden).map_err(|e| e.to_string())),
            },
            move |m| Done::Lines { side, page, lines: Err(m) },
        );
    }

    fn request_preview(&mut self, ctx: &egui::Context) {
        let Some(pair) = self.pairs.get(self.preview.pair).copied() else { return };
        let plan = match self.plan(&[pair]) {
            Ok(p) => p,
            Err(e) => {
                self.preview.error = Some(Failure::Other(e));
                return;
            }
        };
        let (va, vb) = (self.visibility(Side::A), self.visibility(Side::B));
        let key = format!("{plan:?}{va:?}{vb:?}");
        if self.preview.key == key || self.preview.pending.as_ref() == Some(&key) {
            return;
        }
        let (Some(a), Some(b)) = (self.a.as_ref().map(Snapshot::source), self.b.as_ref().map(Snapshot::source)) else { return };
        self.preview.pending = Some(key.clone());
        let k2 = key.clone();
        let pixels = budget(ctx, PREVIEW_PIXELS);
        spawn(
            ctx,
            &self.tx,
            move || Done::Preview { key, result: preview(a, b, va, vb, plan, pixels) },
            move |m| Done::Preview { key: k2, result: Err(Failure::Other(m)) },
        );
    }

    fn start_generate(&mut self, ctx: &egui::Context) {
        let pairs = self.pairs.clone();
        let plan = match self.plan(&pairs) {
            Ok(p) => p,
            Err(e) => {
                self.failure = Some(e);
                return;
            }
        };
        let (va, vb) = (self.visibility(Side::A), self.visibility(Side::B));
        let (Some(a), Some(b)) = (self.a.as_ref().map(Snapshot::source), self.b.as_ref().map(Snapshot::source)) else { return };
        self.generating = true;
        self.failure = None;
        self.outcome = None;
        spawn(
            ctx,
            &self.tx,
            move || Done::Generated { result: generate(a, b, va, vb, plan) },
            |m| Done::Generated { result: Err(Failure::Other(m)) },
        );
    }

    fn busy(&self) -> bool {
        self.generating || self.preview.pending.is_some() || !self.pages_pending.is_empty()
    }
}

// ── Opening and showing ─────────────────────────────────────────────────────────────────────

impl PdfCraftApp {
    /// The overlay wizard's documents (A, and B once chosen), while it exists.
    pub fn overlay_documents(&self) -> Option<(DocId, Option<DocId>)> {
        let w = self.overlay.as_ref()?;
        Some((w.a.as_ref()?.id, w.b.as_ref().map(|b| b.id)))
    }

    /// Overlay compare: open the wizard on the active document (A). Opened again from one of its
    /// own documents while they are still open, it shows again where it was left.
    pub fn overlay_open(&mut self) {
        let Some((_, active)) = self.active_ids() else { return };
        let resume = self.overlay.as_ref().is_some_and(|w| {
            let open = |s: &Option<Snapshot>| s.as_ref().is_none_or(|s| self.session.get(s.id).is_some());
            let mine = w.a.as_ref().is_some_and(|a| a.id == active) || w.b.as_ref().is_some_and(|b| b.id == active);
            w.a.is_some() && open(&w.a) && open(&w.b) && mine
        });
        if resume {
            if let Some(w) = self.overlay.as_mut() {
                w.open = true;
            }
            return;
        }
        self.overlay = Some(Box::new(Wizard::new(self, active)));
    }
}

/// The wizard's window, if open. Called every frame.
pub(crate) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(mut w) = app.overlay.take() else { return };
    w.poll(app, ctx);
    if w.open {
        window(app, &mut w, ctx);
    }
    if w.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if app.overlay.is_none() {
        app.overlay = Some(w);
    }
}

fn window(app: &mut PdfCraftApp, w: &mut Wizard, ctx: &egui::Context) {
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let size = vec2((screen.width() - 80.0).clamp(560.0, 1180.0), (screen.height() - 80.0).clamp(420.0, 800.0));
    let mut open = true;
    egui::Window::new(tx("Overlay compare"))
        .id(egui::Id::new("overlay-wizard"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(size)
        .default_pos(screen.center() - size * 0.5)
        .min_size(vec2(540.0, 400.0))
        .max_size(screen.size() - vec2(20.0, 20.0))
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.card).stroke(Stroke::new(1.0, t.border)).inner_margin(egui::Margin::same(14)))
        .show(ctx, |ui| {
            ui.set_min_size(size - vec2(30.0, 60.0));
            steps_bar(ui, w, &t);
            ui.add_space(6.0);
            stale_banner(app, w, ui, &t);
            let footer = 46.0;
            let body = ui.available_height() - footer;
            egui::ScrollArea::vertical().id_salt(("overlay-step", w.step.index())).max_height(body.max(120.0)).auto_shrink([false, false]).show(
                ui,
                |ui| match w.step {
                    Step::Documents => documents(app, w, ui, &t),
                    Step::Pages => pages(w, ui, &t),
                    Step::Colours => colours(w, ui, &t),
                    Step::Sheets => sheets(w, ui, &t),
                    Step::Alignment => alignment(w, ui, &t),
                    Step::Preview => preview_step(w, ui, &t),
                    Step::Generate => generate_step(w, ui, &t),
                },
            );
            ui.separator();
            footer_bar(app, w, ui, &t);
        });
    if std::mem::take(&mut w.generate_requested) {
        // At Generate, as at each step change, a source changed in its tab is reported; the
        // snapshot is used unless the user takes the update.
        check_stale(app, w);
        w.start_generate(ctx);
    }
    if !open {
        w.open = false;
    }
}

fn steps_bar(ui: &mut egui::Ui, w: &mut Wizard, t: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        for s in Step::ALL {
            let current = s == w.step;
            let done = s < w.step;
            let text = format!("{} {}", s.index() + 1, tx(s.title()));
            let rich = if current {
                RichText::new(text).strong().color(t.accent)
            } else if done {
                RichText::new(text).color(t.text)
            } else {
                RichText::new(text).color(t.text_faint)
            };
            // Earlier steps can be revisited directly; later ones only through Next.
            if ui.add_enabled(done || current, egui::Button::new(rich).frame(false)).clicked() && done {
                w.step = s;
            }
            if s != Step::Generate {
                ui.label(RichText::new("›").color(t.text_faint));
            }
        }
    });
}

fn stale_banner(app: &PdfCraftApp, w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    for side in [Side::A, Side::B] {
        if !w.stale[side.index()] {
            continue;
        }
        let name = w.snapshot(side).map(|s| s.name.clone()).unwrap_or_default();
        let mut use_current = false;
        let mut keep = false;
        egui::Frame::new().fill(t.accent_soft).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.label(crate::i18n::fmt(
                tx("Document {side} ({name}) changed in its tab since the wizard took it (an edit, or a layer switched). The overlay still uses what it took."),
                &[("side", side.letter()), ("name", &name)],
            ));
            ui.horizontal(|ui| {
                keep = widgets::pill_button(ui, tx("Keep snapshot"), true).clicked();
                use_current = widgets::pill_button(ui, tx("Use current visibility and content"), false).clicked();
            });
        });
        if keep {
            w.stale[side.index()] = false;
            // Ask again only if it changes again.
            w.kept[side.index()] = w.snapshot(side).and_then(|s| Snapshot::signature_of(app, s.id));
        }
        if use_current && let Some(id) = w.snapshot(side).map(|s| s.id) {
            let snap = Snapshot::take(app, id);
            match side {
                Side::A => w.a = snap,
                Side::B => w.b = snap,
            }
            w.forget_side(side);
            w.stale[side.index()] = false;
            w.kept[side.index()] = None;
        }
        ui.add_space(6.0);
    }
}

/// At each step change (and before generating): has a source changed in its tab? Keep snapshot is
/// the default; the banner offers the update.
fn check_stale(app: &PdfCraftApp, w: &mut Wizard) {
    for side in [Side::A, Side::B] {
        let Some(s) = w.snapshot(side) else { continue };
        let now = Snapshot::signature_of(app, s.id);
        if now.is_some() && now != Some(s.signature()) && now != w.kept[side.index()] {
            w.stale[side.index()] = true;
        }
    }
}

fn footer_bar(app: &mut PdfCraftApp, w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    let problem = w.can_leave(w.step);
    ui.horizontal(|ui| {
        if let Some(p) = &problem {
            ui.label(RichText::new(p).color(t.text_muted).small());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if w.step == Step::Generate {
                if widgets::pill_button(ui, tx("Close"), false).clicked() {
                    w.open = false;
                }
            } else if ui.add_enabled_ui(problem.is_none(), |ui| widgets::pill_button(ui, tx("Next"), true)).inner.clicked() {
                w.step = w.step.next();
                check_stale(app, w);
            }
            if ui.add_enabled_ui(w.step != Step::Documents, |ui| widgets::pill_button(ui, tx("Back"), false)).inner.clicked() {
                w.step = w.step.back();
                check_stale(app, w);
            }
            if ui.button(tx("Start over")).on_hover_text(tx("Take document A and B again and reset every setting")).clicked()
                && let Some(a) = w.a.as_ref().map(|a| a.id)
            {
                let b = w.b.as_ref().map(|b| b.id);
                let mut fresh = Wizard::new(app, a);
                if let Some(b) = b.filter(|b| app.session.get(*b).is_some()) {
                    fresh.set_b(app, b);
                }
                *w = fresh;
            }
            if w.busy() {
                ui.add(egui::Spinner::new());
            }
        });
    });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(theme::semibold(16.0)));
    ui.add_space(4.0);
}

fn muted(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).color(t.text_muted));
}

fn swatch(ui: &mut egui::Ui, c: [u8; 3], size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    ui.painter().rect(rect, 4.0, Color32::from_rgb(c[0], c[1], c[2]), Stroke::new(1.0, Color32::from_gray(140)), egui::StrokeKind::Inside);
    resp
}

// ── Step 1: Documents ───────────────────────────────────────────────────────────────────────

fn documents(app: &mut PdfCraftApp, w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("1. Documents"));
    muted(ui, t, tx("Document A is the tab you started from. Choose document B from the other open documents; to use a file, open it first."));
    ui.add_space(8.0);
    let others: Vec<(DocId, String)> = app.views.iter().filter_map(|v| app.session.get(v.id).map(|d| (d.id, d.name.clone()))).collect();
    egui::Grid::new("overlay-docs").num_columns(3).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label(tx("Document A:"));
        ui.label(RichText::new(w.a.as_ref().map(|a| a.name.clone()).unwrap_or_default()).strong());
        ui.label(
            RichText::new(crate::i18n::fmt(tx("{n} pages"), &[("n", &w.a.as_ref().map_or(0, Snapshot::pages).to_string())])).color(t.text_muted),
        );
        ui.end_row();
        ui.label(tx("Document B:"));
        let shown = w.b.as_ref().map_or_else(|| tx("Choose a document").to_string(), |b| b.name.clone());
        let mut choice = w.b.as_ref().map(|b| b.id);
        egui::ComboBox::from_id_salt("overlay-b").selected_text(shown).width(320.0).show_ui(ui, |ui| {
            for (id, n) in &others {
                let same = w.a.as_ref().is_some_and(|a| a.id == *id);
                let label = if same { format!("{n} ({})", tx("same as A")) } else { n.clone() };
                ui.selectable_value(&mut choice, Some(*id), label);
            }
        });
        ui.label(
            RichText::new(crate::i18n::fmt(tx("{n} pages"), &[("n", &w.b.as_ref().map_or(0, Snapshot::pages).to_string())])).color(t.text_muted),
        );
        ui.end_row();
        if let Some(id) = choice
            && w.b.as_ref().is_none_or(|b| b.id != id)
        {
            // Another B discards the pairing; ask first if the user edited it.
            if w.manual_edits() {
                w.confirm = Some(Confirm::ChangeB { id });
            } else {
                w.set_b(app, id);
            }
        }
    });
    if let Some(Confirm::ChangeB { id }) = w.confirm.clone() {
        let name = app.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
        let text = crate::i18n::fmt(
            tx("Using {name} as document B starts its page pairing again in order, which discards the pairings you made in step 2."),
            &[("name", &name)],
        );
        match confirm_box(ui, t, &text, &[tx("Use it as document B"), tx("Cancel")]) {
            Some(0) => {
                w.confirm = None;
                w.set_b(app, id);
            }
            Some(_) => w.confirm = None,
            None => {}
        }
    }
    ui.add_space(4.0);
    if widgets::ghost_button(ui, "file-input", tx("Open a file…")).clicked() {
        app.open_dialog();
    }
    ui.add_space(12.0);
    heading(ui, tx("Layers in the overlay"));
    muted(ui, t, tx("The overlay has exactly two layers, Old and New: hiding one hides that whole revision. Colours follow the role."));
    ui.add_space(6.0);
    egui::Grid::new("overlay-roles").num_columns(3).spacing([12.0, 8.0]).show(ui, |ui| {
        for side in [Side::A, Side::B] {
            swatch(ui, w.colour_of(side), 16.0);
            ui.label(RichText::new(w.role_of(side)).strong());
            ui.label(crate::i18n::fmt(tx("Document {side}"), &[("side", side.letter())]));
            ui.end_row();
        }
    });
    if widgets::ghost_button(ui, "arrow-up-down", tx("Swap Old and New")).clicked() {
        w.swapped = !w.swapped;
    }
    ui.add_space(12.0);
    heading(ui, tx("Layer visibility snapshot"));
    muted(
        ui,
        t,
        tx(
            "Each source's layers are used as its tab showed them when the wizard started. Switching a layer later changes nothing until you accept it.",
        ),
    );
    for side in [Side::A, Side::B] {
        let Some(s) = w.snapshot(side) else { continue };
        if s.layers.is_empty() {
            ui.label(crate::i18n::fmt(tx("Document {side}: no layers."), &[("side", side.letter())]));
        } else {
            let list: Vec<String> = s.layers.iter().map(|(_, n, v)| format!("{n} {}", if *v { tx("shown") } else { tx("hidden") })).collect();
            ui.label(crate::i18n::fmt(tx("Document {side}: {list}"), &[("side", side.letter()), ("list", &list.join(", "))]));
        }
        if let Some(e) = &s.error {
            ui.colored_label(Color32::from_rgb(200, 60, 40), e);
        }
    }
}

// ── Step 2: Pages ───────────────────────────────────────────────────────────────────────────

/// A pairing operation chosen in the table, applied after it is drawn.
#[derive(Clone, Copy)]
enum PairOp {
    InsertBlank(Side, usize),
    RemoveBlank(Side, usize),
    Move(Side, usize, bool),
    Assign(Side, usize, usize),
}

fn summary_text(w: &Wizard) -> String {
    let (s, old_only, new_only) = w.summary();
    let mut text = crate::i18n::fmt(
        tx("{rows} output page(s): {matched} matched, {old} Old only, {new} New only"),
        &[("rows", &w.pairs.len().to_string()), ("matched", &s.matched.to_string()), ("old", &old_only.to_string()), ("new", &new_only.to_string())],
    );
    if s.unplaced > 0 {
        text.push_str(&crate::i18n::fmt(tx(", {n} without a partner yet"), &[("n", &s.unplaced.to_string())]));
    }
    text
}

fn confirm_box(ui: &mut egui::Ui, t: &Tokens, text: &str, buttons: &[&str]) -> Option<usize> {
    let mut chosen = None;
    egui::Frame::new().fill(t.accent_soft).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.label(text);
        ui.horizontal(|ui| {
            for (i, b) in buttons.iter().enumerate() {
                if widgets::pill_button(ui, b, i == 0).clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    chosen
}

fn pages(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("2. Pages"));
    muted(
        ui,
        t,
        tx(
            "One output page per row. Pair matching sheets; a sheet only one revision has goes opposite a blank. Use a row's ⋯ menu to mark a sheet as inserted or deleted, remove a gap or move a sheet, or choose a page in a cell to pair sheets that moved. \"p.\" is the page's position in the file; a label is the PDF's own page label. The source files are never changed.",
        ),
    );
    ui.add_space(8.0);
    let (old, new) = w.old_new();
    let count = |s: &Option<Snapshot>| s.as_ref().map_or(1, Snapshot::pages).max(1);
    let counts = [count(&w.a), count(&w.b)];
    egui::Grid::new("overlay-ranges").num_columns(5).spacing([10.0, 8.0]).show(ui, |ui| {
        for side in [old, new] {
            let role = w.role_of(side);
            let n = counts[side.index()];
            let draft = &mut w.range_draft[side.index()];
            ui.label(crate::i18n::fmt(tx("{role} (document {side}) pages"), &[("role", role), ("side", side.letter())]));
            ui.add(egui::DragValue::new(&mut draft.0).range(1..=n));
            ui.label(tx("to"));
            ui.add(egui::DragValue::new(&mut draft.1).range(1..=n));
            ui.label(RichText::new(crate::i18n::fmt(tx("of {n}"), &[("n", &n.to_string())])).color(t.text_muted));
            ui.end_row();
        }
    });
    // A changed range applies at once, unless it would replace pairings the user made.
    if w.confirm.is_none() {
        for side in [Side::A, Side::B] {
            let applied = if side == Side::A { w.range_a } else { w.range_b };
            let draft = w.range_draft[side.index()];
            if draft != applied {
                if w.manual_edits() {
                    w.confirm = Some(Confirm::Range { side, range: draft });
                } else {
                    w.set_range(side, draft, false);
                }
                break;
            }
        }
    }
    if w.pairs.is_empty() && w.history.is_empty() {
        w.refill_pairs();
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(!w.history.is_empty(), egui::Button::new(tx("Undo"))).on_hover_text(tx("Undo the last pairing change")).clicked() {
            w.undo_pairing();
        }
        if ui.add_enabled(!w.future.is_empty(), egui::Button::new(tx("Redo"))).clicked() {
            w.redo_pairing();
        }
        if ui
            .add_enabled(w.manual_edits(), egui::Button::new(tx("Re-pair in order")))
            .on_hover_text(tx("Pair both ranges page by page again"))
            .clicked()
        {
            w.confirm = Some(Confirm::RePair);
        }
        let (s, _, _) = w.summary();
        if ui.add_enabled(s.unplaced > 0, egui::Button::new(tx("Place the remaining pages against blanks"))).clicked() {
            w.pairing_op(|p| {
                p.place_unplaced_against_blanks();
                Ok(())
            });
        }
    });
    match w.confirm.clone() {
        Some(Confirm::RePair) => {
            match confirm_box(
                ui,
                t,
                tx("Re-pairing in order replaces the pairings you made (marked insertions and deletions, moved sheets). Undo brings them back."),
                &[tx("Re-pair in order"), tx("Cancel")],
            ) {
                Some(0) => {
                    w.repair();
                    w.confirm = None;
                }
                Some(_) => w.confirm = None,
                None => {}
            }
        }
        Some(Confirm::Range { side, range }) => {
            let text = crate::i18n::fmt(
                tx(
                    "The new range for document {side} would change pairings you made. Keep your pairing (pages left out of the range leave their rows; new pages go at the end), or re-pair both ranges in order?",
                ),
                &[("side", side.letter())],
            );
            match confirm_box(ui, t, &text, &[tx("Keep my pairing"), tx("Re-pair in order"), tx("Cancel")]) {
                Some(0) => {
                    w.set_range(side, range, true);
                    w.confirm = None;
                }
                Some(1) => {
                    w.set_range(side, range, false);
                    w.confirm = None;
                }
                Some(_) => {
                    w.range_draft = [w.range_a, w.range_b];
                    w.confirm = None;
                }
                None => {}
            }
        }
        _ => {}
    }
    if let Some(note) = &w.pairing_note {
        ui.colored_label(Color32::from_rgb(200, 110, 20), crate::i18n::fmt(tx("Not changed: {why}."), &[("why", note)]));
    }
    ui.label(RichText::new(summary_text(w)).strong());
    ui.add_space(4.0);

    // The table: Old and New columns whatever document each is, each cell a physical page with
    // its label shown apart, or a blank.
    let widths = [56.0, 150.0, 120.0, 150.0, 120.0, 120.0, 34.0];
    let header = |ui: &mut egui::Ui, i: usize, text: String| {
        ui.add_sized([widths[i], 18.0], egui::Label::new(RichText::new(text).strong()).truncate());
    };
    ui.horizontal(|ui| {
        header(ui, 0, tx("Output").into());
        header(ui, 1, crate::i18n::fmt(tx("Old · document {side}"), &[("side", old.letter())]));
        header(ui, 2, tx("Old label").into());
        header(ui, 3, crate::i18n::fmt(tx("New · document {side}"), &[("side", new.letter())]));
        header(ui, 4, tx("New label").into());
        header(ui, 5, tx("Kind").into());
    });
    let mut op: Option<PairOp> = None;
    let range_pages = [Wizard::range_pages(w.range_a), Wizard::range_pages(w.range_b)];
    let row_height = 26.0;
    let table_height = (ui.available_height() - 8.0).clamp(160.0, 520.0);
    egui::ScrollArea::vertical().id_salt("overlay-pair-rows").max_height(table_height).auto_shrink([false, true]).show_rows(
        ui,
        row_height,
        w.pairs.len(),
        |ui, rows| {
            for i in rows {
                let slot_of = |side: Side| w.pairing.slot(side.column(), i);
                let (so, sn) = (slot_of(old), slot_of(new));
                ui.horizontal(|ui| {
                    ui.set_height(row_height);
                    ui.add_sized([widths[0], row_height], egui::Label::new((i + 1).to_string()));
                    for (k, side, slot) in [(1usize, old, so), (3, new, sn)] {
                        let snap = w.snapshot(side);
                        let shown = match slot {
                            Slot::Page(p) => RichText::new(Snapshot::page_text(p)),
                            Slot::Blank => RichText::new(tx("blank")).italics().color(t.text_muted),
                            Slot::Unplaced => RichText::new(tx("— no partner —")).color(Color32::from_rgb(200, 110, 20)),
                        };
                        egui::ComboBox::from_id_salt(("overlay-cell", i, side.index())).selected_text(shown).width(widths[k] - 10.0).show_ui(
                            ui,
                            |ui| {
                                for &q in &range_pages[side.index()] {
                                    let name = snap.map_or_else(|| Snapshot::page_text(q), |s| s.page_name(q));
                                    if ui.selectable_label(slot == Slot::Page(q), name).clicked() && slot != Slot::Page(q) {
                                        op = Some(PairOp::Assign(side, i, q));
                                    }
                                }
                            },
                        );
                        let label = match (slot, snap) {
                            (Slot::Page(p), Some(s)) => s.label(p).map(str::to_string),
                            _ => None,
                        };
                        match label {
                            Some(l) => ui.add_sized([widths[k + 1], row_height], egui::Label::new(l).truncate()),
                            None => ui.add_sized([widths[k + 1], row_height], egui::Label::new(RichText::new("—").color(t.text_faint))),
                        };
                    }
                    let kind = match (so, sn) {
                        (Slot::Page(_), Slot::Page(_)) => RichText::new(tx("Matched")),
                        (Slot::Unplaced, _) | (_, Slot::Unplaced) => RichText::new(tx("No partner yet")).color(Color32::from_rgb(200, 110, 20)),
                        (Slot::Page(_), _) => {
                            RichText::new(tx("Old only")).color(Color32::from_rgb(w.old_colour[0], w.old_colour[1], w.old_colour[2]))
                        }
                        (_, Slot::Page(_)) => {
                            RichText::new(tx("New only")).color(Color32::from_rgb(w.new_colour[0], w.new_colour[1], w.new_colour[2]))
                        }
                        _ => RichText::new(tx("Empty")).color(Color32::from_rgb(200, 60, 40)),
                    };
                    ui.add_sized([widths[5], row_height], egui::Label::new(kind));
                    ui.menu_button("⋯", |ui| {
                        ui.set_min_width(300.0);
                        if let Slot::Page(q) = sn
                            && so != Slot::Blank
                            && ui
                                .button(crate::i18n::fmt(
                                    tx("Mark New {page} as inserted (blank on the Old side)"),
                                    &[("page", &Snapshot::page_text(q))],
                                ))
                                .clicked()
                        {
                            op = Some(PairOp::InsertBlank(old, i));
                        }
                        if let Slot::Page(p) = so
                            && sn != Slot::Blank
                            && ui
                                .button(crate::i18n::fmt(
                                    tx("Mark Old {page} as deleted (blank on the New side)"),
                                    &[("page", &Snapshot::page_text(p))],
                                ))
                                .clicked()
                        {
                            op = Some(PairOp::InsertBlank(new, i));
                        }
                        if so == Slot::Blank && ui.button(tx("Remove the gap on the Old side (later Old pages move up)")).clicked() {
                            op = Some(PairOp::RemoveBlank(old, i));
                        }
                        if sn == Slot::Blank && ui.button(tx("Remove the gap on the New side (later New pages move up)")).clicked() {
                            op = Some(PairOp::RemoveBlank(new, i));
                        }
                        ui.separator();
                        for (side, slot, role) in [(old, so, "Old"), (new, sn, "New")] {
                            if slot == Slot::Unplaced {
                                continue;
                            }
                            if ui.button(crate::i18n::fmt(tx("Move this {role} cell up"), &[("role", role)])).clicked() {
                                op = Some(PairOp::Move(side, i, false));
                            }
                            if ui.button(crate::i18n::fmt(tx("Move this {role} cell down"), &[("role", role)])).clicked() {
                                op = Some(PairOp::Move(side, i, true));
                            }
                        }
                    });
                });
            }
        },
    );
    if let Some(op) = op {
        w.pairing_op(|p| match op {
            PairOp::InsertBlank(side, row) => p.insert_blank(side.column(), row),
            PairOp::RemoveBlank(side, row) => p.remove_blank(side.column(), row),
            PairOp::Move(side, row, down) => p.move_cell(side.column(), row, down),
            PairOp::Assign(side, row, page) => p.assign(side.column(), row, page),
        });
    }
}

// ── Step 3: Colours ─────────────────────────────────────────────────────────────────────────

fn colours(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("3. Colours"));
    muted(
        ui,
        t,
        tx(
            "Each revision prints in its layer's colour; content in both prints as their product, dark. With the complementary checkbox on, picking one colour sets the other to its complement.",
        ),
    );
    ui.add_space(8.0);
    ui.add(widgets::checkbox(&mut w.complementary, tx("Keep the colours complementary")));
    ui.add_space(6.0);
    for (is_old, label) in [(true, "Old"), (false, "New")] {
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 20.0], egui::Label::new(RichText::new(tx(label)).strong()));
            let mut picked = None;
            for (x, y) in colour::PALETTE {
                for c in [x, y] {
                    let current = if is_old { w.old_colour } else { w.new_colour };
                    let r = swatch(ui, c, 22.0);
                    if current == c {
                        ui.painter().rect_stroke(r.rect.expand(2.0), 5.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
                    }
                    if r.on_hover_text(format!("RGB {}, {}, {}", c[0], c[1], c[2])).clicked() {
                        picked = Some(c);
                    }
                }
            }
            ui.add_space(8.0);
            let mut custom = if is_old { w.old_colour } else { w.new_colour };
            if ui.color_edit_button_srgb(&mut custom).changed() {
                picked = Some(custom);
            }
            if let Some(c) = picked {
                if is_old {
                    w.old_colour = c;
                    if w.complementary {
                        w.new_colour = colour::complement(c);
                    }
                } else {
                    w.new_colour = c;
                    if w.complementary {
                        w.old_colour = colour::complement(c);
                    }
                }
            }
        });
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        for (c, label) in [(w.old_colour, "Only in Old"), (w.new_colour, "Only in New"), (colour::overlap(w.old_colour, w.new_colour), "In both")] {
            swatch(ui, c, 28.0);
            ui.label(tx(label));
            ui.add_space(14.0);
        }
    });
    let warnings = colour::check(w.old_colour, w.new_colour);
    for warning in &warnings {
        ui.colored_label(Color32::from_rgb(200, 110, 20), warning.text());
    }
    if !warnings.is_empty() {
        muted(ui, t, tx("These are advice only: the overlay can still be made."));
    }
    ui.add_space(12.0);
    heading(ui, tx("Light tones"));
    muted(ui, t, tx("Tones lighter than the threshold print as paper (hatching, screened backgrounds, off-white scans). 1.00 keeps every tone."));
    ui.spacing_mut().slider_width = 260.0;
    ui.add(egui::Slider::new(&mut w.threshold, 0.5..=1.0).step_by(0.01).fixed_decimals(2).text(tx("Paper threshold")));
}

// ── Step 4: Sheet sizes ─────────────────────────────────────────────────────────────────────

fn sheets(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("4. Sheet sizes"));
    muted(
        ui,
        t,
        tx(
            "Off means 1:1: B is placed at its own size. When on, a factor proposed from the sheet sizes is applied only after you confirm it; it is a hint, not proof of the drawing's scale.",
        ),
    );
    ui.add_space(8.0);
    ui.add(widgets::checkbox(&mut w.match_sheets, tx("Match sheet sizes")));
    if !w.match_sheets {
        return;
    }
    ui.add(widgets::checkbox(&mut w.same_sheet_for_all, tx("Use the first pair's choice for every pair")));
    ui.add_space(6.0);
    let keys: Vec<(usize, usize)> = {
        let mut seen = HashSet::new();
        let pairs: Vec<(usize, usize)> = w.pairs.iter().filter_map(Pair::pages).collect();
        pairs.into_iter().map(|p| w.sheet_key(p)).filter(|k| seen.insert(*k)).take(50).collect()
    };
    for key in keys {
        let proposal = w.proposal(key);
        let mut state = w.sheet_state(key);
        egui::Frame::new().stroke(Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::fmt(
                    tx("A page {a} with B page {b}"),
                    &[("a", &(key.0 + 1).to_string()), ("b", &(key.1 + 1).to_string())],
                ))
                .strong(),
            );
            let Some(p) = proposal else {
                ui.label(tx("The sheet sizes can't be read."));
                return;
            };
            ui.label(RichText::new(&p.note).color(t.text_muted));
            let before = state;
            ui.horizontal(|ui| {
                let proposed = if p.case == SheetCase::SameShape { p.factor } else { p.fit };
                let mut scaled = matches!(state.choice, SheetChoice::Factor(_));
                widgets::radio_value(ui, &mut scaled, false, tx("1:1"));
                if p.case != SheetCase::SameSheet {
                    let label = if p.case == SheetCase::SameShape {
                        crate::i18n::fmt(tx("Scale B by the proposed {f}"), &[("f", &format!("{proposed:.4}"))])
                    } else {
                        crate::i18n::fmt(tx("Fit B uniformly ({f})"), &[("f", &format!("{proposed:.4}"))])
                    };
                    widgets::radio_value(ui, &mut scaled, true, label);
                }
                match (scaled, state.choice) {
                    (false, _) => state.choice = SheetChoice::OneToOne,
                    (true, SheetChoice::OneToOne) => state.choice = SheetChoice::Factor(proposed),
                    (true, SheetChoice::Factor(_)) => {}
                }
                if let SheetChoice::Factor(mut f) = state.choice {
                    ui.label(tx("factor"));
                    if ui.add(egui::DragValue::new(&mut f).range(0.05..=20.0).speed(0.001).max_decimals(5)).changed() {
                        state.choice = SheetChoice::Factor(f);
                    }
                }
            });
            if state.choice != before.choice {
                state.confirmed = false;
            }
            ui.add(widgets::checkbox(&mut state.confirmed, tx("I confirm this choice")));
        });
        w.sheets.insert(key, state);
        ui.add_space(4.0);
    }
}

// ── Step 5: Alignment ───────────────────────────────────────────────────────────────────────

fn alignment(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("5. Alignment"));
    muted(
        ui,
        t,
        tx(
            "Optional. Click the same feature on A, then on B (or B, then A); each pair of clicks is one reference point. Two points fix shift, rotation and scale; three or more let the residuals check them. Clicks snap to the nearest line end or crossing, so the same point is picked on both. Scroll to zoom, drag to pan.",
        ),
    );
    ui.add_space(6.0);
    let pairs: Vec<(usize, (usize, usize))> = w.pairs.iter().enumerate().filter_map(|(i, p)| p.pages().map(|k| (i, k))).collect();
    if pairs.is_empty() {
        ui.label(tx("No row has a page on both sides, so there is nothing to align."));
        return;
    }
    if !pairs.iter().any(|(i, _)| *i == w.align_pair) {
        w.align_pair = pairs.first().map_or(0, |(i, _)| *i);
    }
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut w.same_alignment_for_all, tx("Use the first pair's alignment for every pair")));
        if !w.same_alignment_for_all {
            let current = pairs.iter().find(|(i, _)| *i == w.align_pair).map(|(_, k)| *k).unwrap_or((0, 0));
            egui::ComboBox::from_id_salt("overlay-align-pair")
                .selected_text(crate::i18n::fmt(
                    tx("A page {a} with B page {b}"),
                    &[("a", &(current.0 + 1).to_string()), ("b", &(current.1 + 1).to_string())],
                ))
                .show_ui(ui, |ui| {
                    for (i, k) in &pairs {
                        ui.selectable_value(
                            &mut w.align_pair,
                            *i,
                            crate::i18n::fmt(tx("A page {a} with B page {b}"), &[("a", &(k.0 + 1).to_string()), ("b", &(k.1 + 1).to_string())]),
                        );
                    }
                });
        }
    });
    let pair = pairs.iter().find(|(i, _)| *i == w.align_pair).map(|(_, k)| *k).unwrap_or((0, 0));
    let key = w.align_key(pair);
    let ctx = ui.ctx().clone();
    w.request_page(&ctx, Side::A, key.0);
    w.request_page(&ctx, Side::B, key.1);
    // Lines are kept for the pages on show only: a dense sheet's are a few megabytes.
    w.lines.retain(|k, _| *k == (Side::A, key.0) || *k == (Side::B, key.1));
    if w.snap_enabled {
        w.request_lines(&ctx, Side::A, key.0);
        w.request_lines(&ctx, Side::B, key.1);
    }
    let snapping = w.snap_enabled && !ui.input(|i| i.modifiers.alt);
    let options = w.snap;
    let lines = [w.lines.get(&(Side::A, key.0)).cloned(), w.lines.get(&(Side::B, key.1)).cloned()];
    let mut align = w.aligns.get(&key).cloned().unwrap_or_default();
    let width = ((ui.available_width() - 12.0) / 2.0).max(200.0);
    let height = (ui.available_height() - 200.0).clamp(260.0, 620.0);
    let sizes = [w.a.as_mut().and_then(|s| s.size(key.0)), w.b.as_mut().and_then(|s| s.size(key.1))];
    let mut clicks: [Option<Point>; 2] = [None, None];
    ui.horizontal(|ui| {
        for (i, side, page) in [(0usize, Side::A, key.0), (1, Side::B, key.1)] {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(crate::i18n::fmt(tx("Document {side}, page {p}"), &[("side", side.letter()), ("p", &(page + 1).to_string())]))
                        .strong(),
                );
                let placeholder = w.page_errors.get(&(side, page)).cloned().unwrap_or_else(|| tx("Rendering…").to_string());
                let mut view = w.views[i];
                let label = crate::i18n::fmt(tx("Pick on document {side}"), &[("side", side.letter())]);
                let shown = image_view(ui, &label, &mut view, w.pages.get(&(side, page)), vec2(width, height), &placeholder);
                w.views[i] = view;
                // Markers: each pick, numbered, and the pending one.
                let Some(size) = sizes[i] else { return };
                let to_norm = |p: Point| vec2((p.x / size.0) as f32, (1.0 - p.y / size.1) as f32);
                let painter = ui.painter_at(shown.rect);
                let pts: Vec<Point> = align.picks.iter().map(|(a, b)| if i == 0 { *a } else { *b }).collect();
                for (n, p) in pts.iter().enumerate() {
                    let at = shown.to_screen(to_norm(*p));
                    painter.circle_stroke(at, 7.0, Stroke::new(2.0, Color32::from_rgb(230, 80, 20)));
                    painter.text(
                        at + vec2(9.0, -9.0),
                        egui::Align2::LEFT_BOTTOM,
                        (n + 1).to_string(),
                        theme::semibold(13.0),
                        Color32::from_rgb(230, 80, 20),
                    );
                }
                let pending = if i == 0 { align.pending_a } else { align.pending_b };
                if let Some(p) = pending {
                    let at = shown.to_screen(to_norm(p));
                    painter.circle_stroke(at, 7.0, Stroke::new(2.0, Color32::from_rgb(20, 120, 230)));
                    painter.line_segment([at - vec2(10.0, 0.0), at + vec2(10.0, 0.0)], Stroke::new(1.0, Color32::from_rgb(20, 120, 230)));
                    painter.line_segment([at - vec2(0.0, 10.0), at + vec2(0.0, 10.0)], Stroke::new(1.0, Color32::from_rgb(20, 120, 230)));
                }
                // A click lands on the nearest line end or crossing within reach (SNAP_PIXELS at
                // the current zoom), or where it is when there is none or Alt is held.
                let tolerance = SNAP_PIXELS * size.0 / f64::from(shown.image.width()).max(1e-6);
                let snapped = |n: Vec2| -> (Point, Option<SnapKind>) {
                    let p = Point::new(f64::from(n.x) * size.0, (1.0 - f64::from(n.y)) * size.1);
                    match lines.get(i).and_then(Option::as_ref).filter(|_| snapping).and_then(|l| l.snap(p, tolerance, options)) {
                        Some(s) => (s.point, Some(s.kind)),
                        None => (p, None),
                    }
                };
                if let Some(n) = shown.hover
                    && let (p, Some(kind)) = snapped(n)
                {
                    snap_marker(&painter, shown.to_screen(to_norm(p)), kind);
                }
                if let Some(n) = shown.clicked {
                    clicks[i] = Some(snapped(n).0);
                }
            });
        }
    });
    if let Some(p) = clicks[0] {
        align.pending_a = Some(p);
    }
    if let Some(p) = clicks[1] {
        align.pending_b = Some(p);
    }
    if let (Some(a), Some(b)) = (align.pending_a, align.pending_b) {
        align.picks.push((a, b));
        align.pending_a = None;
        align.pending_b = None;
        align.manual = None;
    }
    snapping_controls(w, ui, t, key);
    ui.horizontal(|ui| {
        if ui.add_enabled(!align.picks.is_empty(), egui::Button::new(tx("Remove the last point"))).clicked() {
            align.picks.pop();
            align.manual = None;
        }
        if ui.add_enabled(!align.picks.is_empty() || align.manual.is_some(), egui::Button::new(tx("Clear the alignment"))).clicked() {
            align = Picks { affine: align.affine, ..Picks::default() };
        }
        if ui.add(widgets::checkbox(&mut align.affine, tx("Advanced: affine (allows stretch and skew)"))).changed() {
            align.manual = None;
        }
        if ui.add_enabled(w.views.iter().any(|v| *v != View::default()), egui::Button::new(tx("Fit pages"))).clicked() {
            w.views = [View::default(); 2];
        }
    });
    if align.affine {
        ui.colored_label(
            Color32::from_rgb(200, 110, 20),
            tx("Affine alignment changes the drawing's proportions. Use it only for stretched or skewed scans; the output records it."),
        );
    }
    w.aligns.insert(key, align.clone());
    ui.add_space(6.0);
    match w.registration(pair) {
        Err(e) => {
            ui.colored_label(Color32::from_rgb(200, 60, 40), e);
        }
        Ok(r) => registration_readout(w, ui, t, key, &r, &align),
    }
}

/// Whether and to what clicks snap, and what the two pages' lines allow.
fn snapping_controls(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens, key: (usize, usize)) {
    ui.horizontal_wrapped(|ui| {
        ui.add(widgets::checkbox(&mut w.snap_enabled, tx("Snap to the drawing")));
        if w.snap_enabled {
            ui.add(widgets::checkbox(&mut w.snap.ends, tx("Line ends")));
            ui.add(widgets::checkbox(&mut w.snap.intersections, tx("Intersections")));
            ui.add(widgets::checkbox(&mut w.snap.midpoints, tx("Midpoints")));
            ui.add(widgets::checkbox(&mut w.snap.on_line, tx("Along lines")));
            muted(ui, t, tx("Hold Alt to place a point without snapping."));
        }
    });
    if !w.snap_enabled {
        return;
    }
    let mut notes = Vec::new();
    let mut counts = Vec::new();
    for (side, page) in [(Side::A, key.0), (Side::B, key.1)] {
        let s = side.letter();
        if let Some(e) = w.lines_errors.get(&(side, page)) {
            notes.push(crate::i18n::fmt(tx("{side} can't snap: {error}"), &[("side", s), ("error", e)]));
        } else if let Some(l) = w.lines.get(&(side, page)) {
            if l.is_empty() {
                notes.push(crate::i18n::fmt(tx("{side} has no drawn lines to snap to (a scan, or text only)."), &[("side", s)]));
            } else if l.truncated {
                notes.push(crate::i18n::fmt(tx("{side} has more lines than snapping reads; only the first ones snap."), &[("side", s)]));
            } else if l.unreadable > 0 {
                notes.push(crate::i18n::fmt(tx("Some of {side}'s content can't be read; snapping uses the rest."), &[("side", s)]));
            }
            counts.push(crate::i18n::fmt(tx("{n} lines on {side}"), &[("n", &l.segments().len().to_string()), ("side", s)]));
        } else {
            notes.push(crate::i18n::fmt(tx("Reading {side}'s lines…"), &[("side", s)]));
        }
    }
    for n in notes {
        ui.colored_label(Color32::from_rgb(200, 110, 20), n);
    }
    if counts.len() == 2 {
        muted(ui, t, &crate::i18n::fmt(tx("Snapping to {counts}."), &[("counts", &counts.join(tx(" and ")))]));
    }
}

/// The snap target under the pointer, marked as drawing programs mark them: a square on a line
/// end, a cross on a crossing, a triangle on a midpoint, an hourglass on a line; with its name.
fn snap_marker(painter: &egui::Painter, at: Pos2, kind: SnapKind) {
    let colour = Color32::from_rgb(0, 150, 70);
    let stroke = Stroke::new(2.0, colour);
    let r = 6.0;
    let segs: Vec<[Vec2; 2]> = match kind {
        SnapKind::End => vec![[vec2(-r, -r), vec2(r, -r)], [vec2(r, -r), vec2(r, r)], [vec2(r, r), vec2(-r, r)], [vec2(-r, r), vec2(-r, -r)]],
        SnapKind::Intersection => vec![[vec2(-r, -r), vec2(r, r)], [vec2(-r, r), vec2(r, -r)]],
        SnapKind::Midpoint => vec![[vec2(-r, r), vec2(r, r)], [vec2(r, r), vec2(0.0, -r)], [vec2(0.0, -r), vec2(-r, r)]],
        SnapKind::OnLine => vec![[vec2(-r, -r), vec2(r, -r)], [vec2(r, -r), vec2(-r, r)], [vec2(-r, r), vec2(r, r)], [vec2(r, r), vec2(-r, -r)]],
    };
    for [a, b] in segs {
        painter.line_segment([at + a, at + b], stroke);
    }
    let name = match kind {
        SnapKind::End => tx("End"),
        SnapKind::Intersection => tx("Intersection"),
        SnapKind::Midpoint => tx("Midpoint"),
        SnapKind::OnLine => tx("On line"),
    };
    // On a white backing, so the name reads over dense linework.
    let galley = painter.layout_no_wrap(name.to_string(), theme::semibold(12.0), colour);
    let corner = at + vec2(r + 4.0, r + 2.0);
    painter.rect_filled(Rect::from_min_size(corner, galley.size()).expand(2.0), 2.0, Color32::from_white_alpha(230));
    painter.galley(corner, galley, colour);
}

fn registration_readout(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens, key: (usize, usize), r: &Registration, align: &Picks) {
    let m = r.matrix.0;
    let scale = m[0].hypot(m[1]);
    let rotation = m[1].atan2(m[0]).to_degrees();
    ui.label(match r.note.kind {
        RegistrationKind::Placed => tx("No registration: B is placed as the sheet match puts it.").to_string(),
        _ => crate::i18n::fmt(
            tx("Shift {x} mm, {y} mm · rotation {r}° · scale {s}"),
            &[
                ("x", &format!("{:.3}", m[4] / PT_PER_MM)),
                ("y", &format!("{:.3}", m[5] / PT_PER_MM)),
                ("r", &format!("{rotation:.4}")),
                ("s", &format!("{scale:.6}")),
            ],
        ),
    });
    if let Some(f) = &r.report {
        // Registration quality and diagnostics are shown apart: a statistical flag is not a
        // statement that the alignment is unacceptable (step 2 review).
        ui.label(RichText::new(tx("Registration quality")).strong());
        ui.label(crate::i18n::fmt(
            tx("RMS residual {rms} mm, largest {max} mm, on A's sheet."),
            &[("rms", &format!("{:.3}", f.rms_mm)), ("max", &format!("{:.3}", f.max_mm))],
        ));
        let list: Vec<String> = f
            .residuals_mm
            .iter()
            .zip(&f.flagged)
            .enumerate()
            .map(|(i, (r, flag))| format!("{}: {r:.3} mm{}", i + 1, if *flag { " •" } else { "" }))
            .collect();
        ui.label(RichText::new(list.join("   ")).color(t.text_muted));
        ui.label(RichText::new(tx("Diagnostics")).strong());
        if f.exact {
            muted(ui, t, tx("Exactly as many points as the fit needs: no residual check is possible. Add a point to verify the picks."));
        } else {
            let text = if f.consistent {
                f.verdict.clone()
            } else {
                format!("{} {}", f.verdict, tx("(An observation about the picks, not a verdict on the alignment.)"))
            };
            ui.label(RichText::new(text).color(t.text_muted));
        }
    } else if align.picks.len() == 1 {
        muted(ui, t, tx("One point picked: pick another pair to fit."));
    }
    if align.affine {
        return;
    }
    ui.add_space(6.0);
    ui.label(RichText::new(tx("Final nudge")).strong());
    let base = align.manual.or(r.report.as_ref().and_then(|f| f.similarity)).unwrap_or(Similarity {
        scale,
        rotation: rotation.to_radians(),
        tx: m[4],
        ty: m[5],
    });
    let (mut x, mut y, mut rot, mut s) = (base.tx / PT_PER_MM, base.ty / PT_PER_MM, base.rotation.to_degrees(), base.scale);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tx("Shift X (mm)"));
        changed |= ui.add(egui::DragValue::new(&mut x).speed(0.01).max_decimals(3)).changed();
        ui.label(tx("Y (mm)"));
        changed |= ui.add(egui::DragValue::new(&mut y).speed(0.01).max_decimals(3)).changed();
        ui.label(tx("Rotation (°)"));
        changed |= ui.add(egui::DragValue::new(&mut rot).speed(0.001).range(-180.0..=180.0).max_decimals(4)).changed();
        ui.label(tx("Scale"));
        changed |= ui.add(egui::DragValue::new(&mut s).speed(0.0001).range(0.05..=20.0).max_decimals(6)).changed();
        if ui.add_enabled(align.manual.is_some(), egui::Button::new(tx("Back to the fit"))).clicked()
            && let Some(a) = w.aligns.get_mut(&key)
        {
            a.manual = None;
        }
    });
    if changed && let Some(sim) = Similarity::new(s, rot.to_radians(), x * PT_PER_MM, y * PT_PER_MM) {
        w.aligns.entry(key).or_default().manual = Some(sim);
    }
}

// ── Step 6: Preview ─────────────────────────────────────────────────────────────────────────

fn preview_step(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("6. Preview"));
    muted(ui, t, tx("The page Generate would write, drawn by PdfCraft's own renderer. Scroll to zoom, drag to pan."));
    ui.add_space(6.0);
    let n = w.pairs.len();
    ui.horizontal(|ui| {
        ui.label(tx("Output page"));
        let mut p = w.preview.pair + 1;
        if ui.add(egui::DragValue::new(&mut p).range(1..=n.max(1))).changed() {
            w.preview.pair = p.saturating_sub(1).min(n.saturating_sub(1));
        }
        ui.label(RichText::new(crate::i18n::fmt(tx("of {n}"), &[("n", &n.to_string())])).color(t.text_muted));
        ui.add_space(12.0);
        let (ra, rb) = (w.role_of(Side::A), w.role_of(Side::B));
        let a_label = format!("{} ({ra})", tx("A only"));
        let b_label = format!("{} ({rb})", tx("B only"));
        widgets::radio_value(ui, &mut w.preview.mode, PreviewMode::Both, tx("Both"));
        widgets::radio_value(ui, &mut w.preview.mode, PreviewMode::AOnly, a_label);
        widgets::radio_value(ui, &mut w.preview.mode, PreviewMode::BOnly, b_label);
        widgets::radio_value(ui, &mut w.preview.mode, PreviewMode::Transparency, tx("Transparency"));
        widgets::radio_value(ui, &mut w.preview.mode, PreviewMode::Flicker, tx("Flicker"));
    });
    match w.preview.mode {
        PreviewMode::Transparency => {
            ui.add(egui::Slider::new(&mut w.preview.alpha, 0.0..=1.0).text(tx("B over A")));
        }
        PreviewMode::Flicker => {
            ui.add(egui::Slider::new(&mut w.preview.hz, 0.25..=MAX_FLICKER_HZ).text(tx("Flicker rate (Hz, at most 2)")));
        }
        _ => {}
    }
    let ctx = ui.ctx().clone();
    w.request_preview(&ctx);
    if let Some(row) = w.pairs.get(w.preview.pair).copied()
        && let Some(pages) = row.pages()
        && let Ok(r) = w.registration(pages)
    {
        let m = r.matrix.0;
        ui.label(
            RichText::new(format!(
                "{} · {}",
                r.note.text(),
                crate::i18n::fmt(
                    tx("Shift {x} mm, {y} mm · rotation {r}° · scale {s}"),
                    &[
                        ("x", &format!("{:.3}", m[4] / PT_PER_MM)),
                        ("y", &format!("{:.3}", m[5] / PT_PER_MM)),
                        ("r", &format!("{:.4}", m[1].atan2(m[0]).to_degrees())),
                        ("s", &format!("{:.6}", m[0].hypot(m[1])))
                    ]
                )
            ))
            .color(t.text_muted),
        );
    }
    if let Some(e) = &w.preview.error {
        if w.unresolved.is_some() {
            unresolved_panel(w, ui, t);
        } else {
            ui.colored_label(Color32::from_rgb(200, 60, 40), e.to_string());
        }
        return;
    }
    let size = vec2(ui.available_width(), (ui.available_height() - 8.0).clamp(300.0, 900.0));
    let images = w.preview.images.as_ref();
    let mut view = w.preview.view;
    let placeholder = tx("Rendering the preview…");
    let shown = match w.preview.mode {
        PreviewMode::Both => image_view(ui, tx("Overlay preview"), &mut view, images.map(|i| &i[0]), size, placeholder),
        PreviewMode::AOnly => image_view(ui, tx("Overlay preview"), &mut view, images.map(|i| &i[1]), size, placeholder),
        PreviewMode::BOnly => image_view(ui, tx("Overlay preview"), &mut view, images.map(|i| &i[2]), size, placeholder),
        PreviewMode::Flicker => {
            let period = 1.0 / f64::from(w.preview.hz.clamp(0.25, MAX_FLICKER_HZ));
            let phase = (ui.input(|i| i.time) / period * 2.0) as u64 % 2;
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            image_view(ui, tx("Overlay preview"), &mut view, images.map(|i| if phase == 0 { &i[1] } else { &i[2] }), size, placeholder)
        }
        PreviewMode::Transparency => {
            let shown = image_view(ui, tx("Overlay preview"), &mut view, images.map(|i| &i[1]), size, placeholder);
            if let Some(i) = images {
                let alpha = (w.preview.alpha.clamp(0.0, 1.0) * 255.0) as u8;
                ui.painter_at(shown.rect).image(
                    i[2].texture.id(),
                    shown.image,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::from_white_alpha(alpha),
                );
            }
            shown
        }
    };
    w.preview.view = view;
    if view != View::default() && ui.button(tx("Fit page")).clicked() {
        w.preview.view = View::default();
    }
    if let Some(outline) = w.preview.outline {
        let pts: Vec<Pos2> = outline.iter().map(|p| shown.to_screen(p.to_vec2())).collect();
        let mut closed = pts.clone();
        if let Some(first) = pts.first() {
            closed.push(*first);
        }
        ui.painter_at(shown.rect).add(egui::Shape::dashed_line(&closed, Stroke::new(1.5, Color32::from_rgb(230, 120, 20)), 6.0, 4.0));
        ui.label(
            RichText::new(tx("Part of B falls outside A's sheet and is clipped (dashed outline). Generate can expand the page instead."))
                .color(t.text_muted),
        );
    }
}

/// Unresolved optional content or annotations: each item, and the decision that settles it.
fn unresolved_panel(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    let Some((ua, ub)) = w.unresolved.clone() else { return };
    ui.colored_label(
        Color32::from_rgb(200, 60, 40),
        tx("Some layers or annotations can't be resolved with certainty, so nothing is written until you decide each item. Hidden content is never shown without your decision."),
    );
    ui.add_space(4.0);
    let mut seen = HashSet::new();
    for (side, items) in [(Side::A, ua), (Side::B, ub)] {
        for u in items {
            let annotation =
                matches!(u.rule, Rule::AnnotationUndrawnText | Rule::AnnotationUndrawableText | Rule::AnnotationNotDrawn | Rule::AnnotationPlacement);
            egui::Frame::new().stroke(Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.label(crate::i18n::fmt(tx("Document {side}, {item}"), &[("side", side.letter()), ("item", &u.to_string())]));
                if !u.settleable {
                    muted(ui, t, tx("No decision here can settle this. Leave this page out of the pairing, or fix the source."));
                    return;
                }
                let Some((obj, name)) = u.objects.first().cloned() else { return };
                if !seen.insert((side, obj)) {
                    return;
                }
                if annotation {
                    let mut omit = w.omit[side.index()].contains(&obj);
                    if ui.add(widgets::checkbox(&mut omit, tx("Leave this annotation out of the overlay"))).changed() {
                        if omit {
                            w.omit[side.index()].insert(obj);
                        } else {
                            w.omit[side.index()].remove(&obj);
                        }
                    }
                    return;
                }
                let snap = w.snapshot(side);
                let label = name
                    .or_else(|| snap.and_then(|s| s.layer_name(obj).map(str::to_string)))
                    .unwrap_or_else(|| format!("{} {} R", obj.num, obj.generation));
                let hint = snap.and_then(|s| s.tab_state(obj)).map(|v| if v { tx("its tab shows it") } else { tx("its tab hides it") });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).strong());
                    if let Some(h) = hint {
                        ui.label(RichText::new(format!("({h})")).color(t.text_muted));
                    }
                    let mut d = w.decisions[side.index()].get(&obj).copied();
                    let before = d;
                    widgets::radio_value(ui, &mut d, Some(true), tx("Show"));
                    widgets::radio_value(ui, &mut d, Some(false), tx("Hide"));
                    if d != before
                        && let Some(v) = d
                    {
                        w.decisions[side.index()].insert(obj, v);
                    }
                });
            });
        }
    }
}

// ── Step 7: Generate ────────────────────────────────────────────────────────────────────────

fn generate_step(w: &mut Wizard, ui: &mut egui::Ui, t: &Tokens) {
    heading(ui, tx("7. Generate"));
    muted(ui, t, tx("The overlay opens as a new, unsaved tab with two layers, Old and New. The source documents aren't changed."));
    ui.add_space(8.0);
    // What will be written, before it is: one output page per row of step 2.
    ui.label(RichText::new(summary_text(w)).strong());
    let (old, new) = w.old_new();
    for (side, role, other) in [(old, "Old", new), (new, "New", old)] {
        let only: Vec<String> = w
            .pairs
            .iter()
            .filter(|p| {
                let (mine, theirs) = if side == Side::A { (p.a, p.b) } else { (p.b, p.a) };
                mine.page().is_some() && theirs == Slot::Blank
            })
            .filter_map(|p| if side == Side::A { p.a.page() } else { p.b.page() })
            .map(|page| w.snapshot(side).map_or_else(|| Snapshot::page_text(page), |s| s.page_name(page)))
            .collect();
        if !only.is_empty() {
            let shown: Vec<&str> = only.iter().take(30).map(String::as_str).collect();
            let more = if only.len() > shown.len() { format!(" (+{})", only.len() - shown.len()) } else { String::new() };
            ui.label(
                RichText::new(crate::i18n::fmt(
                    tx("{role} only (document {side}, opposite a blank in {other}): {pages}{more}"),
                    &[("role", role), ("side", side.letter()), ("other", other.letter()), ("pages", &shown.join(", ")), ("more", &more)],
                ))
                .color(t.text_muted),
            );
        }
    }
    ui.add_space(8.0);
    if ui.add(widgets::checkbox(&mut w.expand, tx("Expand output to include both documents"))).changed() {
        w.preview.key.clear();
    }
    muted(ui, t, tx("Off keeps document A's sheet size; any part of B outside it is clipped."));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let label = if w.outcome.is_some() { tx("Generate again") } else { tx("Generate") };
        if ui.add_enabled_ui(!w.generating, |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            w.generate_requested = true;
        }
        if w.generating {
            ui.add(egui::Spinner::new());
            ui.label(crate::i18n::fmt(tx("Making {n} page(s)…"), &[("n", &w.pairs.len().to_string())]));
        }
    });
    ui.add_space(8.0);
    if let Some(f) = w.failure.clone() {
        if w.unresolved.is_some() {
            unresolved_panel(w, ui, t);
            ui.add_space(4.0);
            muted(ui, t, tx("Decide the items above, then Generate again."));
        } else {
            ui.colored_label(Color32::from_rgb(200, 60, 40), crate::i18n::fmt(tx("The overlay couldn't be made: {e}"), &[("e", &f)]));
        }
    }
    if let Some(o) = &w.outcome {
        ui.label(RichText::new(crate::i18n::fmt(tx("Created {name} ({n} page(s))."), &[("name", &o.name), ("n", &o.pages.to_string())])).strong());
        if !o.clipped.is_empty() {
            let list: Vec<String> = o.clipped.iter().map(|p| (p + 1).to_string()).collect();
            ui.label(crate::i18n::fmt(tx("Part of B is clipped on page(s) {pages}."), &[("pages", &list.join(", "))]));
        }
        for warning in &o.warnings {
            ui.colored_label(Color32::from_rgb(200, 110, 20), warning);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tx("Generation report")).strong());
        let mut text = o.report.clone();
        ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(12).interactive(true));
    }
}

// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Create a PDF (blank, from images, from text) and Reduce File Size (execution plan M10.2,
//! M11.1). Opening an image or a text file converts it to a new, unsaved PDF, as Acrobat does.

#[cfg(not(target_arch = "wasm32"))]
use crate::file_filter::AddFilterAnyCase as _;
use std::sync::Arc;

use crate::PdfCraftApp;
#[cfg(not(target_arch = "wasm32"))]
use crate::editing::short_name;
use crate::widgets;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResolutionChoice {
    #[default]
    Image,
    Default72,
    Custom,
}

pub struct ImageImport {
    pub images: Vec<(String, Vec<u8>)>,
    pub choice: ResolutionChoice,
    pub dpi: f64,
}

pub(crate) fn image_import_body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    ui.heading(tl!("Create PDF from images"));
    let Some(draft) = app.image_import.as_mut() else { return (false, true) };
    ui.label(crate::i18n::fmt(tl!("Selected image files: {count}"), &[("count", &draft.images.len().to_string())]));
    ui.add_space(8.0);
    widgets::radio_value(ui, &mut draft.choice, ResolutionChoice::Image, tl!("Use image resolution"));
    ui.label(tl!("Use each image's embedded DPI; use 72 DPI when it is absent."));
    widgets::radio_value(ui, &mut draft.choice, ResolutionChoice::Default72, tl!("Use 72 DPI (one point per pixel)"));
    widgets::radio_value(ui, &mut draft.choice, ResolutionChoice::Custom, tl!("Use custom DPI"));
    ui.add_enabled(draft.choice == ResolutionChoice::Custom, egui::DragValue::new(&mut draft.dpi).range(1.0..=1200.0).suffix(" DPI"));
    ui.label(tl!("DPI sets the printed page size without resampling the image."));
    ui.add_space(12.0);
    let mut go = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        go = crate::widgets::pill_button(ui, tl!("Create"), true).clicked();
        cancel = crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked();
    });
    (go, cancel)
}

/// What Create ▸ Clipboard found on the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// RGBA pixels, row by row.
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    Text(String),
}

/// An image on the system clipboard, or else its text.
pub(crate) const MAX_IMAGE_SIDE: usize = 16384;
pub(crate) const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_clipboard() -> Option<Clip> {
    crate::os_clipboard::with(|cb| {
        if let Ok(img) = cb.get_image() {
            let expected_len = img.width.checked_mul(img.height).and_then(|px| px.checked_mul(4));
            if img.width > 0
                && img.height > 0
                && img.width <= MAX_IMAGE_SIDE
                && img.height <= MAX_IMAGE_SIDE
                && expected_len == Some(img.bytes.len())
                && img.bytes.len() <= MAX_IMAGE_BYTES
            {
                return Ok(Some(Clip::Image { width: img.width, height: img.height, rgba: img.bytes.into_owned() }));
            }
            return Ok(None);
        }
        Ok(cb.get_text().ok().filter(|t| !t.trim().is_empty()).map(Clip::Text))
    })
    .ok()
    .flatten()
}

pub(crate) fn png_from_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    let expected = width.checked_mul(height).and_then(|p| p.checked_mul(4));
    if w == 0 || h == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE || expected != Some(rgba.len()) || rgba.len() > MAX_IMAGE_BYTES {
        return Err("the clipboard image is empty, malformed or exceeds size limits".into());
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Why an optimized result wasn't saved (this fork, COMPRESS.1).
#[cfg(not(target_arch = "wasm32"))]
const EDITED_DURING_OPTIMIZE: &str =
    "The document changed while it was being optimized, so nothing was saved. Run it again to include your latest changes.";

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

impl PdfCraftApp {
    /// Convert a non-PDF file (image, text, Office document) into a new tab. Returns `None` when
    /// `bytes` is not something Create understands (the caller then tries to open it as a PDF).
    /// An Office document blocks while LibreOffice runs: File ▸ Open converts it on the open
    /// worker instead ([`Self::open_converted_pdf`]), leaving this for startup and inline opens.
    pub(crate) fn open_converted(&mut self, name: &str, bytes: &[u8]) -> Option<Result<(), String>> {
        use pdfcraft_engine::SourceKind;
        if !matches!(pdfcraft_engine::source_kind(name, bytes), Some(SourceKind::Image | SourceKind::Text | SourceKind::Office)) {
            return None;
        }
        let created = self.session.convert_to_pdf(name, &Arc::new(bytes.to_vec())).map(|(_, pdf)| pdf);
        Some(self.open_converted_pdf(name, created.map_err(|e| e.to_string())))
    }

    /// Open `created`, the PDF converted from the file `name`, as a new, unsaved tab named after
    /// it (`Report.docx` → `Report.pdf`).
    pub(crate) fn open_converted_pdf(&mut self, name: &str, created: Result<Arc<Vec<u8>>, String>) -> Result<(), String> {
        self.open_created_bytes(&format!("{}.pdf", stem(name)), created)
    }

    fn open_created_bytes(&mut self, name: &str, created: Result<Arc<Vec<u8>>, String>) -> Result<(), String> {
        let bytes = created?;
        let id = self.session.open_new(name, bytes).map_err(|e| e.to_string())?;
        // The reading-colours preference is app-wide: the newly created document starts with it.
        self.session.set_reading_colors(id, self.reading_colors);
        let doc = self.session.get(id).ok_or("the new document could not be opened")?;
        let mut view = crate::DocView::new(id, &doc.info, self.view_defaults);
        // This fork (SYNC.16m, SYNC.17): its comment selection starts in this version of the document.
        view.comments.stamp(crate::content_version(doc));
        if self.full_screen {
            view.enter_presentation();
        }
        self.win.views.push(view);
        self.win.active = Some(self.win.views.len() - 1);
        Ok(())
    }

    /// Create ▸ Clipboard: a new document from the image (one page, its size) or the text on
    /// the clipboard, as Acrobat does.
    pub(crate) fn create_from_clipboard(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        let clip = read_clipboard();
        #[cfg(target_arch = "wasm32")]
        let clip: Option<Clip> = None;
        match clip {
            Some(c) => {
                if let Err(e) = self.create_from_clip(c) {
                    self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
                }
            }
            None => self.notify_tr("The clipboard has no image or text"),
        }
    }

    /// Create a new document from clipboard contents.
    pub fn create_from_clip(&mut self, clip: Clip) -> Result<(), String> {
        let created = match clip {
            Clip::Image { width, height, rgba } => {
                let png = png_from_rgba(width, height, &rgba)?;
                self.session.create_from_images(&[("Clipboard.png".into(), png)])
            }
            Clip::Text(t) => self.session.create_from_text("Clipboard", &t),
        };
        self.open_created_bytes("Clipboard.pdf", created.map_err(|e| e.to_string()))
    }

    /// Create ▸ Blank page: a new untitled document on the system region's paper (#5644).
    pub(crate) fn create_blank(&mut self) {
        let (w, h) = self.blank_paper.points();
        let created = self.session.create_blank(w, h, 1).map_err(|e| e.to_string());
        if let Err(e) = self.open_created_bytes("Untitled.pdf", created) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Create ▸ Images: several images, one page each, in one new document.
    pub(crate) fn create_from_images_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dialog = rfd::AsyncFileDialog::new()
                .add_filter_any_case(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"])
                .set_title(tl!("Choose images"));
            self.ask(crate::pickers::Ask::Files(dialog), None, |app, files| {
                let mut images = Vec::new();
                for f in files {
                    match std::fs::read(&f) {
                        Ok(b) => images.push((f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), b)),
                        Err(e) => {
                            app.notify_fmt("Couldn't read {name}: {e}", &[("name", &f.display().to_string()), ("e", &e.to_string())]);
                            return;
                        }
                    }
                }
                app.begin_image_import(images);
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("On the web, open or drop an image to convert it");
    }

    /// One new document from images (tests and automation call this directly).
    pub fn create_from_images(&mut self, images: Vec<(String, Vec<u8>)>) {
        self.create_from_images_with_resolution(images, pdfcraft_engine::ImageResolution::Embedded);
    }

    /// Stage selected images for the DPI chooser; no document is created until confirmed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn begin_image_import_paths(&mut self, paths: &[String]) -> Result<(), String> {
        let images = paths
            .iter()
            .map(|p| {
                let path = std::path::Path::new(p);
                let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Ok((name, bytes))
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.begin_image_import(images);
        Ok(())
    }

    /// Stage selected images for the DPI chooser; no document is created until confirmed.
    pub fn begin_image_import(&mut self, images: Vec<(String, Vec<u8>)>) {
        if images.is_empty() {
            return;
        }
        self.image_import = Some(ImageImport { images, choice: ResolutionChoice::Image, dpi: 300.0 });
        self.dialog = Some(crate::Dialog::CreateImages);
    }

    pub(crate) fn finish_image_import(&mut self) {
        let Some(draft) = self.image_import.take() else { return };
        let resolution = match draft.choice {
            ResolutionChoice::Image => pdfcraft_engine::ImageResolution::Embedded,
            ResolutionChoice::Default72 => pdfcraft_engine::ImageResolution::Dpi(72.0),
            ResolutionChoice::Custom => pdfcraft_engine::ImageResolution::Dpi(draft.dpi),
        };
        self.create_from_images_with_resolution(draft.images, resolution);
    }

    /// Create directly at the requested resolution (also used by UI tests).
    pub fn create_from_images_with_resolution(&mut self, images: Vec<(String, Vec<u8>)>, resolution: pdfcraft_engine::ImageResolution) {
        if images.is_empty() {
            return;
        }
        let name =
            if let Some((name, _)) = images.first().filter(|_| images.len() == 1) { format!("{}.pdf", stem(name)) } else { "Images.pdf".to_string() };
        let created = self.session.create_from_images_with_resolution(&images, resolution).map_err(|e| e.to_string());
        if let Err(e) = self.open_created_bytes(&name, created) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Reduce File Size: write a compacted copy with images downsampled (the open document is
    /// unchanged). It asks where to save first, so a heavy file can finish by itself, then runs in
    /// the background with a progress bar, like the PDF Optimizer.
    pub(crate) fn reduce_file_size(&mut self) {
        use crate::optimize_ui::OptimizeKind::Reduce;
        let settings = pdfcraft_engine::optimize::Settings::default();
        #[cfg(target_arch = "wasm32")]
        self.start_optimize(Reduce, &settings, &[]);
        #[cfg(not(target_arch = "wasm32"))]
        {
            // What can't run says so now, not after asking where to save.
            if !self.commit_form_typing() {
                return;
            }
            let Some((_, id)) = self.active_ids() else { return };
            if self.optimize_run.is_some() {
                self.notify_tr("Optimization is already running");
                return;
            }
            if let Err(e) = self.session.optimize_job(id, &settings, &[]) {
                let generation = self.session.get(id).map_or(0, |d| d.edit_generation());
                return self.save_optimized(id, Reduce, generation, Err(e), None);
            }
            if let Some(path) = self.save_override.clone() {
                return self.run_optimize(id, Reduce, &settings, &[], Some(path));
            }
            let Some(doc) = self.session.get(id) else { return };
            let dialog = rfd::AsyncFileDialog::new().add_filter_any_case("PDF", &["pdf"]).set_file_name(optimized_name(&doc.name, Reduce));
            let dialog = crate::editing::in_document_folder(dialog, doc.path.as_deref());
            // With the target, a document closed, switched from or edited meanwhile isn't compressed.
            self.ask_one(crate::pickers::Ask::Save(dialog), Some(id), move |app, path| {
                app.run_optimize(id, Reduce, &settings, &[], Some(path.to_string_lossy().into_owned()));
            });
        }
    }

    /// Whether document `id` was edited since its edit generation was `generation` (this fork,
    /// COMPRESS.1). A closed document counts as unchanged: saving then reports it as closed.
    #[cfg(not(target_arch = "wasm32"))]
    fn edited_since(&self, id: pdfcraft_engine::DocId, generation: u64) -> bool {
        self.session.get(id).is_some_and(|d| d.edit_generation() != generation)
    }

    /// Save the optimized document (Reduce File Size, Optimize PDF), reporting the saving. A
    /// Reduce File Size result that isn't smaller is not saved (#490).
    ///
    /// This fork (COMPRESS.1): the result is saved the way Save As saves. The user picks where,
    /// and the tab then shows that file: its name, the optimized content, nothing unsaved. The
    /// original file stays on disk as it was. `generation` is the document's edit generation when
    /// its snapshot was taken; if it was edited since (during the run or while the dialog was
    /// open), nothing is saved, so those edits are never replaced by an older optimized state.
    /// Nothing is written over another open document's file, nor over the document's own file
    /// when that changed on disk. `destination` is where the user chose before the run (Reduce
    /// File Size asks first); otherwise they are asked now.
    pub(crate) fn save_optimized(
        &mut self,
        id: pdfcraft_engine::DocId,
        kind: crate::optimize_ui::OptimizeKind,
        generation: u64,
        result: Result<(Arc<Vec<u8>>, String), pdfcraft_engine::EditError>,
        destination: Option<String>,
    ) {
        let Some(doc) = self.session.get(id) else { return };
        let (before, name) = (doc.bytes.len(), optimized_name(&doc.name, kind));
        #[cfg(not(target_arch = "wasm32"))]
        let folder = doc.path.clone();
        let (bytes, detail) = match result {
            Ok(r) => r,
            Err(e) => {
                self.notify_fmt("Couldn't optimize the file: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let after = bytes.len();
        if kind == crate::optimize_ui::OptimizeKind::Reduce && after >= before {
            self.notify_fmt(
                "This file is already as small as it can be made ({size}). No copy was saved.",
                &[("size", &crate::panels::human_size(before))],
            );
            return;
        }
        let saved = move |app: &mut PdfCraftApp, place: String| {
            let (b, a) = (crate::panels::human_size(before), crate::panels::human_size(after));
            if after < before {
                let pct = 100.0 * (1.0 - after as f64 / before.max(1) as f64);
                app.notify_fmt(
                    "Saved {place}: {before} → {after} ({pct}% smaller){detail}",
                    &[("place", &place), ("before", &b), ("after", &a), ("pct", &format!("{pct:.0}")), ("detail", &detail)],
                );
            } else {
                // Optimize PDF still saves: what it discards may matter more than the size.
                app.notify_fmt(
                    "Saved {place}: {before} → {after} (not smaller){detail}",
                    &[("place", &place), ("before", &b), ("after", &a), ("detail", &detail)],
                );
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.edited_since(id, generation) {
                self.notify_tr(EDITED_DURING_OPTIMIZE);
                return;
            }
            let job = OptimizedSave { id, generation, bytes, name, folder, saved: Box::new(saved) };
            match destination.or_else(|| self.save_override.clone()) {
                Some(p) => self.save_optimized_to(job, p),
                None => self.ask_where_to_save_optimized(job),
            }
        }
        // The web build downloads the result; the document stays as it is.
        #[cfg(target_arch = "wasm32")]
        let _ = generation;
        #[cfg(target_arch = "wasm32")]
        {
            let _ = destination;
            match crate::editing::download(&name, &bytes) {
                Ok(()) => saved(self, name),
                Err(e) => self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
            }
        }
    }
}

/// An optimized result waiting to be saved as the document (this fork, COMPRESS.1): its
/// document and the edit generation its snapshot was taken at, the bytes, the suggested name and
/// folder, and what to report once it is saved.
#[cfg(not(target_arch = "wasm32"))]
struct OptimizedSave {
    id: pdfcraft_engine::DocId,
    generation: u64,
    bytes: Arc<Vec<u8>>,
    name: String,
    folder: Option<String>,
    saved: SavedReport,
}

/// What reports an optimized result saved at its place.
#[cfg(not(target_arch = "wasm32"))]
type SavedReport = Box<dyn FnOnce(&mut PdfCraftApp, String) + Send>;

#[cfg(not(target_arch = "wasm32"))]
impl PdfCraftApp {
    /// Ask where the optimized result goes, then save it there ([`Self::save_optimized_to`]).
    fn ask_where_to_save_optimized(&mut self, job: OptimizedSave) {
        let dialog = rfd::AsyncFileDialog::new().add_filter_any_case("PDF", &["pdf"]).set_file_name(&job.name);
        let dialog = crate::editing::in_document_folder(dialog, job.folder.as_deref());
        self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, p| app.save_optimized_to(job, p.to_string_lossy().into_owned()));
    }

    /// Save the optimized result at `path` as the document, as Save As does (COMPRESS.1). Nothing
    /// is saved over edits made since the snapshot, over another open document's file, over the
    /// document's own file when that changed on disk, or when the bytes don't reopen as this
    /// document. A place that can't be written (gone, read-only) loses nothing: the user is asked
    /// for another until it is written or the question is cancelled (upstream's choose-first
    /// Reduce File Size, SYNC.17).
    fn save_optimized_to(&mut self, job: OptimizedSave, path: String) {
        let id = job.id;
        // Edits made while the save dialog was open aren't in the result either.
        if self.edited_since(id, job.generation) {
            self.notify_tr(EDITED_DURING_OPTIMIZE);
            return;
        }
        if self.refuse_open_destination(id, &path) {
            return;
        }
        let own = self.session.get(id).and_then(|d| d.path.as_deref()).is_some_and(|p| crate::same_file(p, &path));
        if own && self.watch.disk_differs(id, &path, self.session.get(id).map(|d| d.saved_bytes())) {
            self.notify_fmt(
                "{name} changed on disk since it was opened, so nothing was saved. Choose another name to keep both.",
                &[("name", &short_name(&path))],
            );
            return;
        }
        // As Save does (#1698): bytes that don't reopen as this document are not written.
        if let Err(e) = self.session.check_save(id, &job.bytes) {
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", &short_name(&path)), ("e", &e.to_string())]);
            return;
        }
        if self.session.get(id).is_none() {
            self.notify_tr("The document was closed before it could be saved.");
            return;
        }
        if let Err(e) = crate::editing::write_atomically(&path, &job.bytes) {
            self.notify_fmt("Couldn't write {path}: {e}", &[("path", &path), ("e", &e.to_string())]);
            return self.ask_where_to_save_optimized(job);
        }
        if self.commit_written_file(id, &path, job.bytes.clone(), false) {
            (job.saved)(self, path);
        }
    }
}

/// The suggested name of an optimized copy of `document` ("notes (reduced).pdf").
fn optimized_name(document: &str, kind: crate::optimize_ui::OptimizeKind) -> String {
    format!("{} ({}).pdf", stem(document), kind.suffix())
}

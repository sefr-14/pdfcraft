// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Help ▸ Check for updates (UPD.1): offer an owner-approved build of this fork and install it.
//!
//! The desktop app supplies how to check ([`PdfCraftApp::update_source`]: one signed channel
//! envelope, verified there) and how to install ([`PdfCraftApp::update_installer`]: download,
//! verify, stage, and an external helper that replaces the program files after the app has quit).
//! This crate has no network or file code for it; without a source (the web build, tests) the
//! command says updates aren't available here.
//!
//! Nothing installs without the user's explicit "Update and Restart". "Remind me later" (or
//! dismissing the offer) postpones that update for a day, a week or a month; only when that
//! reminder comes due does the app check once on its own, quietly, ten seconds after starting.
//! With nothing postponed there is no check at start (the owner's OV8 decision).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use egui::{Align, Layout};
pub use pdfcraft_overlay_update::reminder::{Interval, Reminder};
pub use pdfcraft_overlay_update::{Check, CheckContext, Offer};

use crate::{PdfCraftApp, theme, widgets};

/// Asks for the approved build (blocking; it runs on its own thread).
pub type UpdateSource = Arc<dyn Fn(CheckContext) -> Result<Check, String> + Send + Sync>;

/// A downloaded, verified and staged update, ready for the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// The transaction id the installer knows it by.
    pub id: String,
    pub distribution_version: String,
}

/// Download progress: bytes received and expected, and whether the user cancelled.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

/// Installs updates (the desktop app's; see the module documentation).
pub trait UpdateInstaller: Send + Sync {
    /// Why installing isn't possible in this copy (a development build, a folder that can't be
    /// written…), or `None`.
    fn unavailable(&self) -> Option<String>;
    /// Download, verify and stage `offer` (blocking, on a worker thread).
    fn prepare(&self, offer: &Offer, progress: &Progress) -> Result<Prepared, String>;
    /// How many other PDF Overlay processes use this installation.
    fn others_running(&self) -> usize;
    /// Install `prepared` once the app has quit (the app starts the helper after the window closes).
    fn hand_off(&self, prepared: &Prepared);
    /// Forget a prepared update that won't be installed.
    fn abandon(&self, prepared: &Prepared);
}

/// Where the Updates dialog is.
#[derive(Default)]
pub(crate) enum Stage {
    #[default]
    Idle,
    #[cfg(not(target_arch = "wasm32"))]
    Checking {
        rx: std::sync::mpsc::Receiver<Result<Check, String>>,
        automatic: bool,
    },
    Checked(Result<Check, String>),
    #[cfg(not(target_arch = "wasm32"))]
    Preparing {
        rx: std::sync::mpsc::Receiver<Result<Prepared, String>>,
        progress: Arc<Progress>,
        offer: Offer,
    },
    /// Staged, but something must close first.
    Blocked {
        prepared: Prepared,
        reasons: Vec<String>,
    },
    /// Staged; the app is quitting (the save prompts may be open).
    Quitting(Prepared),
    /// Installing didn't start; nothing changed.
    Failed(String),
    /// What the helper reported after the last update.
    Result {
        ok: bool,
        message: String,
    },
}

#[derive(Default)]
pub(crate) struct Updates {
    pub(crate) stage: Stage,
    /// The Updates dialog is showing.
    pub(crate) open: bool,
    /// The reminder interval selected in the dialog.
    pub(crate) interval: Interval,
    /// The saved reminder (settings `updates`).
    pub(crate) reminder: Reminder,
    /// The automatic check has run (or been ruled out) this session.
    pub(crate) automatic_done: bool,
    /// When this session's frames started (the automatic check waits a little).
    pub(crate) started: Option<web_time::Instant>,
    /// The sequence UPD.1 last installed here (0: none).
    pub(crate) installed_seq: u64,
    /// How long after the first frame a due reminder's check runs (tests shorten it).
    pub(crate) automatic_delay: Option<std::time::Duration>,
}

/// The current time (through `web_time`, which works on the web too).
pub(crate) fn now() -> DateTime<Utc> {
    let since = web_time::SystemTime::now().duration_since(web_time::SystemTime::UNIX_EPOCH).unwrap_or_default();
    DateTime::from_timestamp(i64::try_from(since.as_secs()).unwrap_or(i64::MAX), since.subsec_nanos()).unwrap_or_default()
}

/// How long after the first frame a due reminder's check runs.
const AUTOMATIC_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

impl PdfCraftApp {
    /// Help ▸ Check for updates: look for an approved update and show what was found.
    pub fn check_for_updates(&mut self) {
        self.updates.open = true;
        self.start_update_check(false);
    }

    fn start_update_check(&mut self, automatic: bool) {
        let Some(source) = self.update_source.clone() else {
            self.updates.stage = Stage::Checked(Err(tl!("updates aren't available in this version of PDF Overlay").to_string()));
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            if matches!(self.updates.stage, Stage::Checking { .. } | Stage::Preparing { .. } | Stage::Quitting(_)) {
                return;
            }
            let context = CheckContext { highest_seen_seq: self.updates.reminder.highest_seen_seq };
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
                // The receiver may be gone (the app quit): nothing to report to then.
                let _ = tx.send(source(context));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            self.updates.stage = match spawned {
                Ok(_) => Stage::Checking { rx, automatic },
                Err(e) => Stage::Checked(Err(e.to_string())),
            };
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (source, automatic);
        }
    }

    /// Restore the saved reminder (settings `updates`) and the sequence installed here.
    pub(crate) fn restore_updates(&mut self, saved: &serde_json::Value) {
        let mut reminder = Reminder::from_json(saved);
        reminder.normalize_clock(now());
        reminder.installed(self.updates.installed_seq);
        self.updates.interval = reminder.interval;
        self.updates.reminder = reminder;
    }

    /// The desktop app reports the update sequence installed in this copy (from its receipt).
    pub fn set_installed_update(&mut self, seq: u64) {
        self.updates.installed_seq = seq;
        self.updates.reminder.installed(seq);
    }

    /// How long after starting a due reminder's check waits (ten seconds; tests shorten it).
    pub fn set_automatic_update_delay(&mut self, delay: std::time::Duration) {
        self.updates.automatic_delay = Some(delay);
    }

    /// The saved reminder, as Preferences keep it (for tests and diagnostics).
    pub fn update_reminder(&self) -> &Reminder {
        &self.updates.reminder
    }

    /// Show what the helper reported after an update (`--overlay-update-result`).
    pub fn show_update_result(&mut self, ok: bool, message: impl Into<String>) {
        self.updates.stage = Stage::Result { ok, message: message.into() };
        self.updates.open = true;
        self.updates.automatic_done = true;
    }

    /// Each frame: pick up finished work and run a due reminder's check.
    pub(crate) fn poll_updates(&mut self) {
        let started = *self.updates.started.get_or_insert_with(web_time::Instant::now);
        if !self.updates.automatic_done && started.elapsed() >= self.updates.automatic_delay.unwrap_or(AUTOMATIC_DELAY) {
            self.updates.automatic_done = true;
            if self.update_source.is_some() && self.updates.reminder.due(now()) && matches!(self.updates.stage, Stage::Idle) {
                self.start_update_check(true);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.poll_update_workers();
        if let Stage::Quitting(prepared) = &self.updates.stage
            && !self.allow_quit
            && self.close_request.is_none()
        {
            // The quit was cancelled at a save prompt: nothing is installed.
            if let Some(installer) = &self.update_installer {
                installer.abandon(prepared);
            }
            self.updates.stage = Stage::Failed(tl!("The update was cancelled. Nothing was installed.").to_string());
            self.updates.open = true;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn poll_update_workers(&mut self) {
        use std::sync::mpsc::TryRecvError;
        match &self.updates.stage {
            Stage::Checking { rx, automatic } => {
                let automatic = *automatic;
                let result = match rx.try_recv() {
                    Ok(r) => r,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => Err("the update check stopped unexpectedly".into()),
                };
                if let Ok(check) = &result {
                    self.updates.reminder.saw(check.seen_seq());
                }
                if automatic {
                    // Quiet: only a postponed reminder that came due, or a newer update, is shown.
                    match &result {
                        Ok(Check::Available(offer)) if self.updates.reminder.may_prompt(offer.seq, now()) => {
                            self.updates.open = true;
                            self.updates.stage = Stage::Checked(result);
                        }
                        Err(e) => {
                            log::info!("automatic update check: {e}");
                            self.updates.stage = Stage::Idle;
                        }
                        _ => self.updates.stage = Stage::Idle,
                    }
                } else {
                    self.updates.stage = Stage::Checked(result);
                }
            }
            Stage::Preparing { rx, .. } => {
                let result = match rx.try_recv() {
                    Ok(r) => r,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => Err("the download stopped unexpectedly".into()),
                };
                match result {
                    Ok(prepared) => self.begin_install(prepared),
                    Err(e) => self.updates.stage = Stage::Failed(e),
                }
            }
            _ => {}
        }
    }

    /// Download, verify and stage the offered update.
    fn prepare_update(&mut self, offer: Offer) {
        let Some(installer) = self.update_installer.clone() else { return };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let progress = Arc::new(Progress::default());
            progress.total.store(offer.size, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            let (job_offer, job_progress, ctx) = (offer.clone(), Arc::clone(&progress), self.ctx.clone());
            let spawned = std::thread::Builder::new().name("update-download".into()).spawn(move || {
                let _ = tx.send(installer.prepare(&job_offer, &job_progress));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            self.updates.stage = match spawned {
                Ok(_) => Stage::Preparing { rx, progress, offer },
                Err(e) => Stage::Failed(e.to_string()),
            };
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (installer, offer);
        }
    }

    /// What must finish or close before PDF Overlay can quit for an update.
    pub(crate) fn update_blockers(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.overlay.as_ref().is_some_and(|w| w.busy()) {
            reasons.push(tl!("Overlay compare is still working.").to_string());
        }
        if self.progress_notice.is_some() || self.print_tasks.busy() {
            reasons.push(tl!("A background task (printing, optimizing…) is still running.").to_string());
        }
        if self.opening_documents() {
            reasons.push(tl!("A document is still opening.").to_string());
        }
        let others = self.update_installer.as_ref().map_or(0, |i| i.others_running());
        if others > 0 {
            reasons.push(crate::i18n::fmt(
                tl!("Other PDF Overlay windows or the command-line tool are running from this folder ({n}). Close them first."),
                &[("n", &others.to_string())],
            ));
        }
        reasons
    }

    /// Documents are still being read in the background.
    fn opening_documents(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return !self.pending_opens.is_empty();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// The update is staged: quit through the usual Save / Discard / Cancel questions.
    fn begin_install(&mut self, prepared: Prepared) {
        let reasons = self.update_blockers();
        if !reasons.is_empty() {
            self.updates.stage = Stage::Blocked { prepared, reasons };
            self.updates.open = true;
            return;
        }
        self.updates.stage = Stage::Quitting(prepared);
        self.updates.open = false;
        if let Some(ctx) = self.ctx.clone() {
            self.request_quit(&ctx);
        }
    }

    /// Called as the app quits: hand a staged update to the installer.
    pub(crate) fn hand_off_update(&mut self) {
        if let (Stage::Quitting(prepared), Some(installer)) = (&self.updates.stage, &self.update_installer) {
            installer.hand_off(prepared);
            log::info!("update {} staged; it installs after PDF Overlay closes", prepared.distribution_version);
        }
    }

    fn abandon_update(&mut self) {
        if let (Stage::Blocked { prepared, .. } | Stage::Quitting(prepared), Some(installer)) = (&self.updates.stage, &self.update_installer) {
            installer.abandon(prepared);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Stage::Preparing { progress, .. } = &self.updates.stage {
            progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Postpone `offer` for the selected interval (Remind me later, or dismissing the offer).
    fn postpone_update(&mut self, seq: u64) {
        self.updates.reminder.postpone(seq, self.updates.interval, now());
    }

    /// `t`'s day in Preferences ▸ Date format.
    fn update_date(&self, t: DateTime<Utc>) -> String {
        use chrono::Datelike;
        use pdfcraft_engine::dates::{date_language, format_day};
        let code = self.session.date_language().unwrap_or(crate::i18n::current().code());
        let year = i64::from(t.year());
        match date_language(code).or_else(|| date_language("en")) {
            Some(lang) => format_day(self.session.date_format(), (year, t.month(), t.day()), lang),
            None => t.format("%Y-%m-%d").to_string(),
        }
    }
}

/// What the dialog's buttons asked for.
enum Pick {
    Retry,
    Later(u64),
    Install(Offer),
    InstallAgain(Prepared),
    Cancel,
    Close,
}

/// The Updates dialog.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.updates.open {
        return;
    }
    let t = theme::Tokens::get(ctx);
    let unavailable = app
        .update_installer
        .as_ref()
        .map_or_else(|| Some(tl!("Installing updates isn't supported in this version.").to_string()), |i| i.unavailable());
    let approved = match &app.updates.stage {
        Stage::Checked(Ok(Check::Available(o))) => Some(app.update_date(o.approved_at)),
        _ => None,
    };
    let spanish = crate::i18n::current().code() == "es";
    let mut pick: Option<Pick> = None;
    let mut interval = app.updates.interval;
    let modal = widgets::show_modal(ctx, "updates", |ui| {
        ui.set_width(460.0);
        let title = match &app.updates.stage {
            Stage::Checked(Ok(Check::Available(_))) => tl!("PDF Overlay — Update available"),
            _ => tl!("Check for updates"),
        };
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("cloud", 22.0, t.accent));
            ui.label(egui::RichText::new(title).font(theme::semibold(16.0)));
        });
        ui.add_space(8.0);
        let muted = |ui: &mut egui::Ui, text: &str| {
            ui.label(egui::RichText::new(text).color(t.text_muted));
        };
        // (label, primary, pick) right to left.
        let mut buttons: Vec<(String, bool, Pick)> = Vec::new();
        match &app.updates.stage {
            Stage::Idle => {
                ui.label(tl!("No check has run yet."));
                buttons.push((tl!("Close").into(), true, Pick::Close));
            }
            #[cfg(not(target_arch = "wasm32"))]
            Stage::Checking { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tl!("Checking for updates…"));
                });
                buttons.push((tl!("Close").into(), false, Pick::Close));
            }
            Stage::Checked(Ok(Check::UpToDate { distribution_version, .. })) => {
                ui.label(egui::RichText::new(tl!("PDF Overlay is up to date.")).strong());
                ui.add_space(4.0);
                muted(ui, &crate::i18n::fmt(tl!("Latest approved version: {v}"), &[("v", distribution_version)]));
                buttons.push((tl!("Close").into(), true, Pick::Close));
            }
            Stage::Checked(Ok(Check::Available(offer))) => {
                ui.label(tl!("A newer approved Windows x64 build of PDF Overlay is available."));
                ui.add_space(4.0);
                ui.label(tl!(
                    "The application will download and verify the update, then restart to install it. Documents and preferences will be preserved."
                ));
                ui.add_space(6.0);
                muted(
                    ui,
                    &crate::i18n::fmt(
                        tl!("Version {v}, approved {date}"),
                        &[("v", &offer.distribution_version), ("date", approved.as_deref().unwrap_or_default())],
                    ),
                );
                let notes = if spanish { offer.notes_es.as_ref().or(offer.notes_en.as_ref()) } else { offer.notes_en.as_ref() };
                if let Some(notes) = notes {
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| muted(ui, notes));
                }
                ui.add_space(8.0);
                ui.label(tl!("Remind me in:"));
                for (choice, label) in [(Interval::Day, tl!("1 day")), (Interval::Week, tl!("1 week")), (Interval::Month, tl!("1 month"))] {
                    widgets::radio_value(ui, &mut interval, choice, label);
                }
                if let Some(why) = &unavailable {
                    ui.add_space(6.0);
                    muted(ui, &crate::i18n::fmt(tl!("Update and Restart isn't available: {why}"), &[("why", why)]));
                } else {
                    buttons.push((tl!("Update and Restart").into(), true, Pick::Install(offer.clone())));
                }
                buttons.push((tl!("Remind me later").into(), unavailable.is_some(), Pick::Later(offer.seq)));
            }
            Stage::Checked(Err(e)) => {
                ui.label(crate::i18n::fmt(tl!("Couldn't check for updates: {e}"), &[("e", e)]));
                buttons.push((tl!("Close").into(), false, Pick::Close));
                buttons.push((tl!("Try again").into(), true, Pick::Retry));
            }
            #[cfg(not(target_arch = "wasm32"))]
            Stage::Preparing { progress, offer, .. } => {
                let done = progress.done.load(Ordering::Relaxed);
                let total = progress.total.load(Ordering::Relaxed).max(1);
                ui.label(crate::i18n::fmt(tl!("Downloading and verifying PDF Overlay {v}…"), &[("v", &offer.distribution_version)]));
                ui.add_space(6.0);
                ui.add(egui::ProgressBar::new((done as f64 / total as f64).clamp(0.0, 1.0) as f32).show_percentage());
                ui.add_space(4.0);
                muted(ui, tl!("Nothing is changed until PDF Overlay restarts."));
                buttons.push((tl!("Cancel").into(), false, Pick::Cancel));
            }
            Stage::Blocked { prepared, reasons } => {
                ui.label(tl!("The update is ready, but PDF Overlay can't restart yet:"));
                ui.add_space(4.0);
                for r in reasons {
                    ui.label(format!("• {r}"));
                }
                buttons.push((tl!("Cancel").into(), false, Pick::Cancel));
                buttons.push((tl!("Try again").into(), true, Pick::InstallAgain(prepared.clone())));
            }
            Stage::Quitting(_) => {
                ui.label(tl!("PDF Overlay will close and restart to finish the update."));
                buttons.push((tl!("Close").into(), true, Pick::Close));
            }
            Stage::Failed(e) => {
                ui.label(crate::i18n::fmt(tl!("Couldn't update: {e}"), &[("e", e)]));
                muted(ui, tl!("Nothing was installed."));
                buttons.push((tl!("Close").into(), true, Pick::Close));
            }
            Stage::Result { ok, message } => {
                ui.label(egui::RichText::new(if *ok { tl!("PDF Overlay was updated.") } else { tl!("The update couldn't be completed.") }).strong());
                ui.add_space(4.0);
                ui.label(message);
                buttons.push((tl!("Close").into(), true, Pick::Close));
            }
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for (label, primary, this) in buttons {
                if widgets::pill_button(ui, &label, primary).clicked() {
                    pick = Some(this);
                }
            }
        });
    });
    app.updates.interval = interval;
    if modal.should_close() && pick.is_none() {
        // Dismissing an offer postpones it for the selected interval; it never installs.
        pick = Some(match &app.updates.stage {
            Stage::Checked(Ok(Check::Available(o))) => Pick::Later(o.seq),
            Stage::Blocked { .. } => Pick::Cancel,
            _ => Pick::Close,
        });
    }
    match pick {
        Some(Pick::Retry) => {
            app.updates.stage = Stage::Idle;
            app.start_update_check(false);
        }
        Some(Pick::Later(seq)) => {
            app.postpone_update(seq);
            app.updates.open = false;
            app.updates.stage = Stage::Idle;
        }
        Some(Pick::Install(offer)) => app.prepare_update(offer),
        Some(Pick::InstallAgain(prepared)) => app.begin_install(prepared),
        Some(Pick::Cancel) => {
            app.abandon_update();
            app.updates.stage = Stage::Failed(tl!("The update was cancelled.").to_string());
        }
        Some(Pick::Close) => {
            app.updates.open = false;
            // A finished or failed check is done; work in progress keeps going.
            if matches!(app.updates.stage, Stage::Checked(_) | Stage::Failed(_) | Stage::Result { .. }) {
                app.updates.stage = Stage::Idle;
            }
        }
        None => {}
    }
}

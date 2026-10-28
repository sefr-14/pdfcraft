// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Help ▸ Check for updates: find this fork's latest available Windows build and offer its page.
//!
//! This fork (sefr-14/pdfcraft) publishes no releases: its Windows builds are artifacts of the
//! manually run `overlay-windows.yml` workflow on `Main` (OV8). The desktop app supplies how to
//! ask GitHub ([`PdfCraftApp::update_source`]), so this crate has no network code; without a
//! source (the web build, tests) the command opens the workflow's page. PdfCraft never downloads
//! or installs anything itself, and it doesn't compare builds: several builds can share a
//! version number, so the dialog only says which build is the latest available one.
//! It asks only when the user does: there is no check at start (the owner's decision).

use std::sync::Arc;

use egui::{Align, Layout};

use crate::{PdfCraftApp, theme, widgets};

/// The page listing every run of the Windows build workflow.
pub const WORKFLOW_PAGE: &str = "https://github.com/sefr-14/pdfcraft/actions/workflows/overlay-windows.yml";
/// A run's page is this followed by its id.
pub const RUN_PAGE: &str = "https://github.com/sefr-14/pdfcraft/actions/runs/";

/// A time as GitHub gives it, in UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Utc {
    /// `YYYY-MM-DDTHH:MM:SSZ`, the form GitHub's API writes; anything else is refused.
    pub fn parse(s: &str) -> Option<Utc> {
        let b = s.as_bytes();
        let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':'), (19, b'Z')];
        if b.len() != 20 || separators.iter().any(|&(i, c)| b.get(i) != Some(&c)) {
            return None;
        }
        // Digits only: `str::parse` would also take a sign.
        let field = |from: usize, to: usize| -> Option<u32> {
            let digits = s.get(from..to)?;
            if digits.bytes().all(|c| c.is_ascii_digit()) { digits.parse().ok() } else { None }
        };
        let t = Utc {
            year: i64::from(field(0, 4)?),
            month: field(5, 7)?,
            day: field(8, 10)?,
            hour: field(11, 13)?,
            minute: field(14, 16)?,
            second: field(17, 19)?,
        };
        let days_in_month = match t.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if t.year % 4 == 0 && (t.year % 100 != 0 || t.year % 400 == 0) => 29,
            2 => 28,
            _ => return None,
        };
        (t.day >= 1 && t.day <= days_in_month && t.hour < 24 && t.minute < 60 && t.second < 60).then_some(t)
    }

    /// Seconds since 1970-01-01 (H. Hinnant's days-from-civil; the four-digit year keeps every
    /// step far from overflow).
    pub fn unix(self) -> i64 {
        let (m, d) = (i64::from(self.month), i64::from(self.day));
        let y = if m <= 2 { self.year - 1 } else { self.year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
        days * 86_400 + i64::from(self.hour) * 3_600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }
}

/// A Windows x64 portable build whose artifact can still be downloaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    /// The version in the artifact's name, such as `0.10.1`; `None` when the name doesn't hold
    /// one that reads as a version (nothing is made up then).
    pub version: Option<String>,
    /// The artifact's name, such as `pdfcraft-overlay-0.10.1-6740201-windows-x64-portable`.
    pub artifact: String,
    /// When the artifact was uploaded.
    pub built: Utc,
    /// The workflow run that made it.
    pub run_id: u64,
}

impl Build {
    /// The run's page, where the artifact is downloaded (GitHub may ask the user to sign in).
    /// Built from the id, never from a URL in GitHub's answer.
    pub fn run_url(&self) -> String {
        format!("{RUN_PAGE}{}", self.run_id)
    }
}

/// What a check found. `checked` is how many of the most recent successful runs were looked
/// at, `total` how many successful runs GitHub counts: when it's more, older ones weren't seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Builds {
    /// The newest build that can still be downloaded.
    Found(Build),
    /// None of the checked runs has a Windows portable artifact.
    NoneFound { checked: usize, total: u64 },
    /// The checked runs' Windows portable artifacts have all expired.
    Expired { checked: usize, total: u64 },
}

/// Looks for the latest available build (blocking; it runs on its own thread).
pub type UpdateSource = Arc<dyn Fn() -> Result<Builds, String> + Send + Sync>;

/// Where a check is.
#[derive(Default)]
pub(crate) enum Check {
    #[default]
    Idle,
    #[cfg(not(target_arch = "wasm32"))]
    Running(std::sync::mpsc::Receiver<Result<Builds, String>>),
    Done(Result<Builds, String>),
}

#[derive(Default)]
pub(crate) struct Updates {
    pub(crate) check: Check,
    /// The Updates dialog is showing.
    pub(crate) open: bool,
}

impl PdfCraftApp {
    /// Help ▸ Check for updates: look for the latest available Windows build and show it.
    pub fn check_for_updates(&mut self) {
        let Some(source) = self.update_source.clone() else {
            self.open_url(WORKFLOW_PAGE);
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.updates.open = true;
            if matches!(self.updates.check, Check::Running(_)) {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                // The receiver may be gone (the app quit): nothing to report to then.
                let _ = tx.send(source());
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            self.updates.check = Check::Running(rx);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = source;
            self.open_url(WORKFLOW_PAGE);
        }
    }

    /// Pick up a finished check (each frame).
    pub(crate) fn poll_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Check::Running(rx) = &self.updates.check {
            let result = match rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the update check stopped unexpectedly".into()),
            };
            self.updates.check = Check::Done(result);
        }
    }

    /// `built`'s day in Preferences ▸ Date format, with its time in UTC.
    fn build_date(&self, built: Utc) -> String {
        use pdfcraft_engine::dates::{date_language, format_day};
        let code = self.session.date_language().unwrap_or(crate::i18n::current().code());
        let day = match date_language(code).or_else(|| date_language("en")) {
            Some(lang) => format_day(self.session.date_format(), (built.year, built.month, built.day), lang),
            None => format!("{:04}-{:02}-{:02}", built.year, built.month, built.day),
        };
        format!("{day} {:02}:{:02} UTC", built.hour, built.minute)
    }
}

/// What the dialog's buttons asked for.
enum Pick {
    Open(String),
    Retry,
}

/// The Updates dialog.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.updates.open {
        return;
    }
    let t = theme::Tokens::get(ctx);
    let current = env!("CARGO_PKG_VERSION");
    let built = match &app.updates.check {
        Check::Done(Ok(Builds::Found(b))) => Some(app.build_date(b.built)),
        _ => None,
    };
    let mut close = false;
    let mut pick: Option<Pick> = None;
    let modal = crate::widgets::show_modal(ctx, "updates", |ui| {
        ui.set_width(440.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("cloud", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Check for updates")).font(theme::semibold(16.0)));
        });
        ui.add_space(8.0);
        let muted = |ui: &mut egui::Ui, text: &str| {
            ui.label(egui::RichText::new(text).color(t.text_muted));
        };
        // The page the action button opens, and its label.
        let mut action: Option<(&str, String)> = None;
        let mut retry = false;
        match &app.updates.check {
            Check::Idle => {
                ui.label(tl!("No check has run yet."));
            }
            #[cfg(not(target_arch = "wasm32"))]
            Check::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tl!("Looking for the latest Windows build…"));
                });
            }
            Check::Done(Ok(Builds::Found(b))) => {
                ui.label(egui::RichText::new(tl!("Latest available Windows build")).strong());
                ui.add_space(4.0);
                // No version is made up: a name without one shows the build by its name and date.
                if let Some(v) = &b.version {
                    ui.label(crate::i18n::fmt(tl!("Version: {v}"), &[("v", v)]));
                }
                ui.label(crate::i18n::fmt(tl!("Built: {date}"), &[("date", built.as_deref().unwrap_or_default())]));
                ui.label(crate::i18n::fmt(tl!("Artifact: {name}"), &[("name", &b.artifact)]));
                ui.add_space(6.0);
                muted(ui, &crate::i18n::fmt(tl!("This copy is version {c}."), &[("c", current)]));
                muted(ui, tl!("GitHub may ask you to sign in to download it. Builds are kept for a limited time."));
                action = Some((tl!("View build on GitHub"), b.run_url()));
            }
            Check::Done(Ok(found @ (Builds::NoneFound { checked, total } | Builds::Expired { checked, total }))) => {
                let n = checked.to_string();
                let text = match found {
                    Builds::Expired { .. } => {
                        crate::i18n::fmt(tl!("The Windows builds checked have expired and can't be downloaded anymore ({n} checked)."), &[("n", &n)])
                    }
                    _ if *total == 0 => tl!("No successful Windows build was found.").to_string(),
                    _ => crate::i18n::fmt(
                        tl!("No downloadable Windows build was found in the most recent successful builds ({n} checked)."),
                        &[("n", &n)],
                    ),
                };
                ui.label(text);
                if *total > u64::try_from(*checked).unwrap_or(u64::MAX) {
                    muted(ui, tl!("Older builds weren't checked."));
                }
                action = Some((tl!("View builds on GitHub"), WORKFLOW_PAGE.to_string()));
            }
            Check::Done(Err(e)) => {
                ui.label(crate::i18n::fmt(tl!("Couldn't check for updates: {e}"), &[("e", &e.to_string())]));
                muted(ui, &crate::i18n::fmt(tl!("Windows builds are listed at {page}."), &[("page", WORKFLOW_PAGE)]));
                retry = true;
            }
        }
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(tl!("Asks GitHub for this fork's latest Windows build. Nothing is downloaded or installed automatically."))
                .color(t.text_muted)
                .small(),
        );
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Close"), action.is_none() && !retry).clicked() {
                close = true;
            }
            if let Some((label, url)) = action
                && widgets::pill_button(ui, label, true).clicked()
            {
                close = true;
                pick = Some(Pick::Open(url));
            }
            if retry && widgets::pill_button(ui, tl!("Try again"), true).clicked() {
                pick = Some(Pick::Retry);
            }
        });
    });
    if modal.should_close() {
        close = true;
        pick = None;
    }
    if close {
        app.updates.open = false;
    }
    match pick {
        Some(Pick::Open(url)) => app.open_url(&url),
        Some(Pick::Retry) => {
            app.updates.check = Check::Idle;
            app.check_for_updates();
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_times_are_read_strictly_and_counted_from_1970() {
        let t = Utc::parse("2026-10-21T12:17:14Z").unwrap();
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute, t.second), (2026, 10, 21, 12, 17, 14));
        assert_eq!(t.unix(), 1_792_585_034);
        assert_eq!(Utc::parse("1970-01-01T00:00:00Z").unwrap().unix(), 0);
        assert_eq!(Utc::parse("2000-03-01T00:00:00Z").unwrap().unix(), 951_868_800);
        assert!(Utc::parse("2024-02-29T00:00:00Z").is_some());
        for bad in [
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-21T24:00:00Z",
            "2026-10-21 12:17:14Z",
            "2026-10-21T12:17:14+00:00",
            "+026-10-21T12:17:14Z",
            "2026-1-021T12:17:14Z",
            "",
            "2026-10-21T12:17:14Zé",
        ] {
            assert_eq!(Utc::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_run_page_is_built_from_the_id() {
        let b = Build { version: None, artifact: "a".into(), built: Utc::parse("2026-10-21T12:17:14Z").unwrap(), run_id: 39595600341 };
        assert_eq!(b.run_url(), "https://github.com/sefr-14/pdfcraft/actions/runs/39595600341");
    }
}

// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Reopen the files from the last session (Preferences ▸ Documents and view, #442): which files
//! were open when PdfCraft last closed, the page each showed and the active tab. Local only, like
//! the recent-files list, and kept only while the preference is on.

use serde::Serialize;

use crate::PdfCraftApp;

/// More files than anyone keeps open. Settings are untrusted, so a longer list is cut here.
pub const MAX_FILES: usize = 64;

/// More windows than anyone arranges (TAB.WINDOW.1 W4); a longer list is cut here.
pub const MAX_WINDOWS: usize = 16;

/// The files open when PdfCraft last closed.
///
/// `files` and `active` are every open file and the first window's active tab, as before there
/// were several windows: a build without windows, or one keeping every document in one window,
/// reads just them. `windows` (TAB.WINDOW.1 W4) says which of those files each window after the
/// first showed; the rest are the first window's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LastSession {
    pub files: Vec<SessionFile>,
    /// The path of the tab that was active (in the first window).
    pub active: Option<String>,
    /// The windows after the first, in order; left out when there were none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<SessionWindow>,
}

/// A window after the first: its tabs and where it was (TAB.WINDOW.1 W4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionWindow {
    /// Its tabs' files, in tab order (absolute paths, each also in [`LastSession::files`]).
    pub files: Vec<String>,
    /// The path of its active tab.
    pub active: Option<String>,
    /// Where it was on the desktop, in physical pixels, when the platform said.
    pub place: Option<SavedPlace>,
}

/// A window's place on the desktop, in physical pixels: its outer top-left corner and its inner
/// size, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SavedPlace {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// Coordinates further out than any desktop (settings are untrusted).
const MAX_COORDINATE: i64 = 1_000_000;
/// Sides larger than any window.
const MAX_SIDE: u64 = 100_000;

impl SavedPlace {
    /// Read one, refusing anything out of range rather than guessing.
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let coordinate = |k: &str| v[k].as_i64().filter(|c| c.abs() <= MAX_COORDINATE).and_then(|c| i32::try_from(c).ok());
        let side = |k: &str| v[k].as_u64().filter(|s| (1..=MAX_SIDE).contains(s)).and_then(|s| u32::try_from(s).ok());
        Some(Self {
            x: coordinate("x")?,
            y: coordinate("y")?,
            width: side("width")?,
            height: side("height")?,
            maximized: v["maximized"].as_bool().unwrap_or(false),
        })
    }
}

impl SavedPlace {
    /// Where a window is now, from its last pass's report (physical pixels).
    pub(crate) fn from_placement(p: crate::multi_window::Placement) -> Option<Self> {
        let finite = p.outer.min.x.is_finite() && p.outer.min.y.is_finite() && p.inner_size.x.is_finite() && p.inner_size.y.is_finite();
        if !finite {
            return None;
        }
        let coordinate = |c: f32| (c.abs() <= MAX_COORDINATE as f32).then(|| c.round() as i32);
        let side = |s: f32| (s >= 1.0 && s <= MAX_SIDE as f32).then(|| s.round() as u32);
        Some(Self {
            x: coordinate(p.outer.min.x)?,
            y: coordinate(p.outer.min.y)?,
            width: side(p.inner_size.x)?,
            height: side(p.inner_size.y)?,
            maximized: p.maximized,
        })
    }
}

impl SessionWindow {
    /// Read one; `None` when it names no file. Paths must be among the session's `files`.
    fn from_json(v: &serde_json::Value, known: &[SessionFile]) -> Option<Self> {
        let is_known = |p: &str| known.iter().any(|f| f.path == p);
        let mut files: Vec<String> = Vec::new();
        for p in v["files"].as_array()?.iter().filter_map(serde_json::Value::as_str) {
            if !p.is_empty() && is_known(p) && !files.iter().any(|f| f == p) && files.len() < MAX_FILES {
                files.push(p.to_string());
            }
        }
        if files.is_empty() {
            return None;
        }
        let active = v["active"].as_str().filter(|a| files.iter().any(|f| f == a)).map(str::to_string);
        Some(Self { files, active, place: SavedPlace::from_json(&v["place"]) })
    }
}

/// One open file and where the reader was in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionFile {
    /// Absolute, so a file opened from the command line by a relative path is found again.
    pub path: String,
    /// The page the tab showed (0-based).
    pub page: usize,
}

impl LastSession {
    /// Read what [`PdfCraftApp::persist`] wrote. Malformed entries are skipped and the list is
    /// capped at [`MAX_FILES`].
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let files: Vec<SessionFile> = v["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| {
                        let path = f["path"].as_str().filter(|p| !p.is_empty())?.to_string();
                        // `go_to_page` clamps a page past the end.
                        let page = f["page"].as_u64().and_then(|p| usize::try_from(p).ok()).unwrap_or(0);
                        Some(SessionFile { path, page })
                    })
                    .take(MAX_FILES)
                    .collect()
            })
            .unwrap_or_default();
        // Each file in at most one window: the first that names it (a document shown twice is
        // restored once, as before).
        let mut windows: Vec<SessionWindow> = Vec::new();
        for w in v["windows"].as_array().map(Vec::as_slice).unwrap_or_default() {
            if windows.len() >= MAX_WINDOWS {
                break;
            }
            if let Some(mut w) = SessionWindow::from_json(w, &files) {
                w.files.retain(|f| !windows.iter().any(|o| o.files.contains(f)));
                if w.active.as_ref().is_some_and(|a| !w.files.contains(a)) {
                    w.active = None;
                }
                if !w.files.is_empty() {
                    windows.push(w);
                }
            }
        }
        Self { files, active: v["active"].as_str().map(str::to_string), windows }
    }
}

/// `path` made absolute against the current directory, without resolving links (and without
/// Windows' `\\?\` prefix, which `canonicalize` adds).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

impl PdfCraftApp {
    /// The files open now. Documents that were never saved to a file are left out (crash
    /// recovery keeps their unsaved work), and so is everything on the web, which has no paths.
    pub(crate) fn open_session(&self) -> LastSession {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = |v: &crate::DocView| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute);
            // Every window's tabs, the main window's first; each document once: a document two
            // windows show belongs to the first of them (as a duplicate tab isn't restored).
            let mut seen: Vec<pdfcraft_engine::DocId> = Vec::new();
            let mut files: Vec<SessionFile> = Vec::new();
            let mut windows: Vec<SessionWindow> = Vec::new();
            let mut active = None;
            for (key, ui) in self.window_uis() {
                let mut mine: Vec<String> = Vec::new();
                for v in &ui.views {
                    if seen.contains(&v.id) || files.len() >= MAX_FILES {
                        continue;
                    }
                    seen.push(v.id);
                    if let Some(p) = path(v) {
                        files.push(SessionFile { path: p.clone(), page: v.current });
                        mine.push(p);
                    }
                }
                let front = ui.active.and_then(|i| ui.views.get(i)).and_then(path).filter(|p| mine.contains(p));
                match key {
                    None => active = ui.active.and_then(|i| ui.views.get(i)).and_then(path),
                    Some(id) if !mine.is_empty() && windows.len() < MAX_WINDOWS => {
                        let place = self.windows.others.iter().find(|w| w.id == id).and_then(|w| w.last).and_then(SavedPlace::from_placement);
                        windows.push(SessionWindow { files: mine, active: front, place });
                    }
                    Some(_) => {}
                }
            }
            LastSession { files, active, windows }
        }
        #[cfg(target_arch = "wasm32")]
        LastSession::default()
    }

    /// What to save as the last session: the files open when the quit began, once the app is
    /// really quitting (unsaved tabs close one by one before that), else the files open now.
    pub(crate) fn session_to_save(&self) -> LastSession {
        match &self.quit_session {
            Some(s) if self.allow_quit => s.clone(),
            _ => self.open_session(),
        }
    }

    /// A quit is about to close its first tab: remember everything that is still open.
    pub(crate) fn note_quit_session(&mut self) {
        if self.quit_session.is_none() {
            self.quit_session = Some(self.open_session());
        }
    }

    /// The quit was cancelled: what is open from now on is the session again.
    pub(crate) fn forget_quit_session(&mut self) {
        self.quit_session = None;
    }

    /// At startup, with the preference on: reopen the last session's files at the pages they
    /// showed and bring its active tab forward. Files that are gone are skipped, and so are
    /// `skip` (files the app is about to open anyway, e.g. from the command line) and documents
    /// the Recovery dialog offers, so neither opens twice.
    ///
    /// Only one password prompt can wait at a time: the first encrypted file asks for its
    /// password and later ones stay closed (in the recent-files list, if there is one).
    pub fn reopen_last_files(&mut self, skip: &[String]) {
        let last = std::mem::take(&mut self.last_session);
        #[cfg(not(target_arch = "wasm32"))]
        if self.reopen_last_session {
            let skip: Vec<String> =
                skip.iter().map(|p| absolute(p)).chain(self.recoverable.iter().filter_map(|m| m.path.as_deref().map(absolute))).collect();
            let mut active = None;
            let mut prompt = self.password_prompt.take();
            for f in &last.files {
                let open = |app: &Self| {
                    app.win
                        .views
                        .iter()
                        .any(|v| app.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute).as_deref() == Some(f.path.as_str()))
                };
                if skip.contains(&f.path) || !std::path::Path::new(&f.path).is_file() || open(self) {
                    continue;
                }
                let before = self.win.views.len();
                self.open_path(&f.path);
                if let Some(p) = self.password_prompt.take() {
                    prompt.get_or_insert(p);
                }
                if self.win.views.len() > before
                    && let Some(view) = self.win.views.last_mut()
                {
                    view.go_to_page(f.page);
                    if last.active.as_ref() == Some(&f.path) {
                        active = Some(self.win.views.len() - 1);
                    }
                }
            }
            self.password_prompt = prompt;
            if active.is_some() {
                self.win.active = active;
            }
            // The windows after the first come back once their files are open (W4).
            if !last.windows.is_empty() {
                self.restoring = Some(crate::multi_window::Restore { windows: last.windows.clone(), waited: 0 });
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (last, skip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_sessions_are_skipped_and_capped() {
        let v = serde_json::json!({
            "files": [{"path": 5}, {"path": ""}, "x", {"path": "/a.pdf", "page": -3}, {"path": "/b.pdf", "page": 1e300}, {"path": "/c.pdf", "page": 7}],
            "active": 3,
        });
        let s = LastSession::from_json(&v);
        let pages: Vec<(&str, usize)> = s.files.iter().map(|f| (f.path.as_str(), f.page)).collect();
        assert_eq!(pages, [("/a.pdf", 0), ("/b.pdf", 0), ("/c.pdf", 7)]);
        assert_eq!(s.active, None);
        let many: Vec<serde_json::Value> = (0..1000).map(|i| serde_json::json!({"path": format!("/{i}.pdf")})).collect();
        assert_eq!(LastSession::from_json(&serde_json::json!({"files": many})).files.len(), MAX_FILES);
        assert_eq!(LastSession::from_json(&serde_json::json!("garbage")), LastSession::default());
    }
}

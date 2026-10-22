//! Rulers and guides, remembered per PDF in PdfCraft's settings (this fork, OV11). Never in the
//! PDF: changing them doesn't mark the document as changed, adds no undo step, and nothing of
//! them is printed, exported or put into Overlay output.
//!
//! **Which file.** A stored entry is keyed by the file's absolute path and checked against the
//! file's permanent identifier (the first string of the trailer's `/ID`) or, when either side has
//! none, its page count. Another document saved at the same path doesn't get the entry; it is
//! replaced only once that document's own rulers or guides change. A file that was moved or
//! renamed takes over its old entry only when the identifier and page count match and nothing is
//! at the old path any more. Documents without a file (new, dropped, on the web) keep theirs for
//! the session; Save As gives them a file and an entry.
//!
//! **Which page.** In memory a guide belongs to its page's object, so it follows the page through
//! reordering, insertion, deletion and undo (a deleted page's guides wait for an undo, in memory
//! only). An entry stores each guide under its page's position in the file as last opened or
//! saved, and only guides of pages the document has now. When no page keeps its object (a full
//! rewrite, a reload, undo across one), guides stay with their page numbers and those of pages
//! that are gone are dropped, never moved onto another page.
//!
//! **Where.** Guides are in PDF user space: `x` holds lines of constant x (vertical on the
//! unrotated page) and `y` lines of constant y. Zoom, scrolling, `/Rotate`, view rotation, the
//! CropBox and `/UserUnit` only change where they are drawn.
//!
//! Settings are untrusted: entries, guides and paths are capped, malformed parts are skipped, and
//! nothing here panics.

use std::collections::HashMap;

use pdfcraft_engine::{DocId, Document};
use serde_json::{Value, json};

use crate::units::Unit;

/// Documents remembered; the least recently used go first.
pub(crate) const MAX_DOCS: usize = 200;
/// Guides on one document's pages, all together.
pub(crate) const MAX_GUIDES_PER_DOC: usize = 1000;
/// Guides in one direction on one page.
pub(crate) const MAX_GUIDES_PER_AXIS: usize = 100;
/// The longest path kept, in characters.
pub(crate) const MAX_PATH_CHARS: usize = 4096;
/// More pages than any document a stored entry could describe.
const MAX_PAGES: u64 = 10_000_000;
/// A user-space coordinate further out than this isn't a guide on a page.
const MAX_COORD: f64 = 1.0e7;
/// Guides closer than this (user-space units) are the same guide.
const SAME: f64 = 1.0e-3;
/// The stored format.
const VERSION: u64 = 1;

/// Which coordinate a guide fixes: `X` is a line of constant x, `Y` of constant y (in PDF user
/// space, so on a page shown turned 90° an `X` guide is horizontal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

/// One page's guides, user-space coordinates in the order they were made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageGuides {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
}

impl PageGuides {
    pub fn axis(&self, axis: Axis) -> &[f64] {
        match axis {
            Axis::X => &self.x,
            Axis::Y => &self.y,
        }
    }
    fn axis_mut(&mut self, axis: Axis) -> &mut Vec<f64> {
        match axis {
            Axis::X => &mut self.x,
            Axis::Y => &mut self.y,
        }
    }
    fn len(&self) -> usize {
        self.x.len().saturating_add(self.y.len())
    }
    fn is_empty(&self) -> bool {
        self.x.is_empty() && self.y.is_empty()
    }
}

/// A guide, as views select and drag it: its page (0-based, as the document is now), direction
/// and coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideRef {
    pub page: usize,
    pub axis: Axis,
    pub at: f64,
}

/// A document's ruler and guide settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewPrefs {
    pub rulers: bool,
    pub guides_shown: bool,
    pub unit: Unit,
}

impl ViewPrefs {
    /// A PDF seen for the first time: rulers hidden, guides shown, the app's unit.
    pub fn new(unit: Unit) -> Self {
        Self { rulers: false, guides_shown: true, unit }
    }
}

/// How a page is told apart: by its object (number, generation) when the document has an object
/// graph, else by its position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PageKey {
    Obj(u32, u16),
    Index(usize),
}

/// A sort key for page keys (sets of them are compared sorted).
fn key_order(k: &PageKey) -> (u8, u64, u16) {
    match *k {
        PageKey::Obj(n, g) => (0, u64::from(n), g),
        PageKey::Index(i) => (1, i as u64, 0),
    }
}

/// The pages of `doc`, in order.
fn page_keys(doc: &Document) -> Vec<PageKey> {
    match doc.page_objects() {
        Some(objs) if objs.len() == doc.info.pages.len() => objs.into_iter().map(|(n, g)| PageKey::Obj(n, g)).collect(),
        _ => (0..doc.info.pages.len()).map(PageKey::Index).collect(),
    }
}

/// Who a stored entry belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    path: String,
    /// The trailer's first `/ID` string, in lowercase hex.
    id: Option<String>,
    pages: usize,
}

impl Identity {
    /// The file `doc` was opened from or saved to; `None` without one.
    fn of(doc: &Document) -> Option<Self> {
        let path = absolute(doc.path.as_deref()?)?;
        Some(Self { path, id: doc.file_id().map(|b| hex(&b)), pages: doc.info.pages.len() })
    }

    /// The same document: the same identifier, or the same page count where either has none.
    fn same_document(&self, stored: &Stored) -> bool {
        match (&self.id, &stored.id) {
            (Some(a), Some(b)) => a == b,
            _ => self.pages == stored.pages,
        }
    }
}

/// `path` made absolute against the current directory, without resolving links (as the last
/// session and `same_file` do); `None` when it is empty or too long to keep.
fn absolute(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let path = std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
    #[cfg(target_arch = "wasm32")]
    let path = path.to_string();
    (path.chars().count() <= MAX_PATH_CHARS).then_some(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A remembered document.
#[derive(Clone, Debug, PartialEq)]
struct Stored {
    path: String,
    id: Option<String>,
    pages: usize,
    /// Larger is more recent.
    used: u64,
    rulers: bool,
    guides_shown: bool,
    /// `None` when unreadable: the app's unit then.
    unit: Option<Unit>,
    /// By page position in the file as last saved; no empty pages.
    guides: Vec<(usize, PageGuides)>,
}

impl Stored {
    fn from_json(v: &Value) -> Option<Self> {
        let path = v["path"].as_str().filter(|p| !p.is_empty() && p.chars().count() <= MAX_PATH_CHARS)?.to_string();
        let id = match &v["id"] {
            Value::Null => None,
            // An identifier that isn't one could match the wrong file: the entry is skipped.
            id => Some(
                id.as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 2 * pdfcraft_engine::MAX_FILE_ID && s.bytes().all(|b| b.is_ascii_hexdigit()))?
                    .to_ascii_lowercase(),
            ),
        };
        let pages = usize::try_from(v["pages"].as_u64().filter(|p| *p <= MAX_PAGES)?).ok()?;
        let unit = serde_json::from_value::<Unit>(v["unit"].clone()).ok();
        let mut stored = Stored {
            path,
            id,
            pages,
            used: v["used"].as_u64().unwrap_or(0),
            rulers: v["rulers"].as_bool().unwrap_or(false),
            guides_shown: v["guides_shown"].as_bool().unwrap_or(true),
            unit,
            guides: Vec::new(),
        };
        let mut total = 0usize;
        for g in v["guides"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let Some(page) = g["page"].as_u64().and_then(|p| usize::try_from(p).ok()).filter(|p| *p < pages) else { continue };
            let mut on = match stored.guides.iter().position(|(p, _)| *p == page) {
                Some(i) => stored.guides.swap_remove(i).1,
                None => PageGuides::default(),
            };
            for axis in [Axis::X, Axis::Y] {
                let key = if axis == Axis::X { "x" } else { "y" };
                for at in g[key].as_array().map(Vec::as_slice).unwrap_or_default().iter().filter_map(Value::as_f64) {
                    if total < MAX_GUIDES_PER_DOC && insert(on.axis_mut(axis), at) {
                        total += 1;
                    }
                }
            }
            if !on.is_empty() {
                stored.guides.push((page, on));
            }
        }
        stored.guides.sort_by_key(|(p, _)| *p);
        Some(stored)
    }

    fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "id": self.id,
            "pages": self.pages,
            "used": self.used,
            "rulers": self.rulers,
            "guides_shown": self.guides_shown,
            "unit": self.unit,
            "guides": self.guides.iter().map(|(page, g)| json!({"page": page, "x": g.x, "y": g.y})).collect::<Vec<_>>(),
        })
    }
}

/// Add `at` to `list` unless it is not a coordinate, already there, or the list is full.
fn insert(list: &mut Vec<f64>, at: f64) -> bool {
    if !at.is_finite() || at.abs() > MAX_COORD || list.len() >= MAX_GUIDES_PER_AXIS || list.iter().any(|g| (g - at).abs() < SAME) {
        return false;
    }
    list.push(at);
    true
}

/// An open document's rulers and guides, shared by all of its tabs.
#[derive(Debug)]
pub struct DocState {
    pub prefs: ViewPrefs,
    /// By page; pages the document doesn't have now (deleted, waiting for an undo) included.
    guides: HashMap<PageKey, PageGuides>,
    /// The document's pages now.
    order: Vec<PageKey>,
    /// Its pages in the file as last opened or saved: stored positions refer to these.
    saved: Vec<PageKey>,
    /// Where it is remembered; `None` for the session only.
    binding: Option<Identity>,
    /// A stored entry at `binding`'s path belongs to another document: it is replaced only when
    /// this one's settings change.
    foreign: bool,
    /// The document's edit generation last looked at.
    seen: u64,
}

impl DocState {
    fn new(prefs: ViewPrefs, order: Vec<PageKey>, seen: u64) -> Self {
        Self { prefs, guides: HashMap::new(), saved: order.clone(), order, binding: None, foreign: false, seen }
    }

    /// Guides of page `page` (0-based, as the document is now).
    pub fn page(&self, page: usize) -> Option<&PageGuides> {
        self.guides.get(self.order.get(page)?)
    }

    /// Pages with guides, as the document is now.
    pub fn pages_with_guides(&self) -> impl Iterator<Item = (usize, &PageGuides)> + '_ {
        self.order.iter().enumerate().filter_map(|(i, k)| self.guides.get(k).filter(|g| !g.is_empty()).map(|g| (i, g)))
    }

    /// Whether `guide` still exists (another tab may have moved or deleted it).
    pub fn has(&self, guide: GuideRef) -> bool {
        self.page(guide.page).is_some_and(|g| g.axis(guide.axis).contains(&guide.at))
    }

    fn total(&self) -> usize {
        self.order.iter().filter_map(|k| self.guides.get(k)).map(PageGuides::len).sum()
    }

    /// The document's pages changed (an edit, undo, redo or revert): guides follow their page
    /// object. When the objects were renumbered instead (no page keeps its object, or as many
    /// pages as before with other objects, as undo across a full rewrite brings back), they
    /// follow their page number. `history` says whether undo or redo could still bring a page
    /// back; without it, guides of pages that are gone are dropped.
    fn sync_pages(&mut self, order: Vec<PageKey>, history: bool) {
        if order != self.order {
            let survives = self.order.iter().any(|k| order.contains(k));
            let reshuffled = order.len() == self.order.len() && {
                let (mut a, mut b) = (order.clone(), self.order.clone());
                a.sort_unstable_by_key(key_order);
                b.sort_unstable_by_key(key_order);
                a == b
            };
            if !self.order.is_empty() && (!survives || (order.len() == self.order.len() && !reshuffled)) {
                self.by_position(order);
            } else {
                self.order = order;
            }
        }
        if !history {
            let order = &self.order;
            self.guides.retain(|k, g| order.contains(k) && !g.is_empty());
        }
    }

    /// Guides stay with their page numbers on `order`; those of pages the document doesn't have
    /// now are dropped, so a deleted page's guides never land on another page.
    fn by_position(&mut self, order: Vec<PageKey>) {
        let mut moved = HashMap::new();
        for (old, new) in self.order.iter().zip(&order) {
            if let Some(g) = self.guides.remove(old) {
                moved.insert(*new, g);
            }
        }
        self.guides = moved;
        self.order = order;
    }

    /// The entry to store: guides of pages the document has now, at their position in the saved
    /// file; a page the saved file doesn't have yet waits for the next save.
    fn stored(&self, binding: &Identity, used: u64) -> Stored {
        let mut guides: Vec<(usize, PageGuides)> = self
            .guides
            .iter()
            .filter(|(k, g)| !g.is_empty() && self.order.contains(k))
            .filter_map(|(k, g)| Some((self.saved.iter().position(|s| s == k)?, g.clone())))
            .collect();
        guides.sort_by_key(|(p, _)| *p);
        Stored {
            path: binding.path.clone(),
            id: binding.id.clone(),
            pages: binding.pages,
            used,
            rulers: self.prefs.rulers,
            guides_shown: self.prefs.guides_shown,
            unit: Some(self.prefs.unit),
            guides,
        }
    }

    /// Nothing worth an entry: settings as for a new PDF and no guides.
    fn is_default(&self, unit: Unit) -> bool {
        self.prefs == ViewPrefs::new(unit) && self.guides.values().all(PageGuides::is_empty)
    }
}

/// Every document's rulers and guides: the remembered ones and the open ones.
pub struct DocPrefs {
    stored: Vec<Stored>,
    clock: u64,
    open: HashMap<DocId, DocState>,
    /// Remembered settings changed since they were last written to the settings file.
    changed: bool,
    /// Whether something is at a path (a moved file's old path must be empty to adopt its entry).
    exists: fn(&str) -> bool,
}

impl Default for DocPrefs {
    fn default() -> Self {
        Self { stored: Vec::new(), clock: 0, open: HashMap::new(), changed: false, exists: path_exists }
    }
}

fn path_exists(path: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return std::path::Path::new(path).exists();
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        false
    }
}

impl DocPrefs {
    /// The settings file's `"document_views"`.
    pub fn to_json(&self) -> Value {
        json!({"v": VERSION, "docs": self.stored.iter().map(Stored::to_json).collect::<Vec<_>>()})
    }

    /// Read what [`Self::to_json`] wrote (at startup, before any document opens). Malformed
    /// entries are skipped, a path listed twice keeps its most recent entry, and at most
    /// [`MAX_DOCS`] are kept. Another format version is ignored.
    pub fn load(&mut self, v: &Value) {
        if v["v"].as_u64() != Some(VERSION) {
            return;
        }
        let mut stored: Vec<Stored> = v["docs"].as_array().map(Vec::as_slice).unwrap_or_default().iter().filter_map(Stored::from_json).collect();
        stored.sort_by_key(|s| std::cmp::Reverse(s.used));
        let mut seen = std::collections::HashSet::new();
        stored.retain(|s| seen.insert(s.path.clone()));
        stored.truncate(MAX_DOCS);
        self.clock = stored.iter().map(|s| s.used).max().unwrap_or(0);
        self.stored = stored;
    }

    /// Remembered settings changed since [`Self::written`].
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// The settings file has them now.
    pub fn written(&mut self) {
        self.changed = false;
    }

    pub fn state(&self, id: DocId) -> Option<&DocState> {
        self.open.get(&id)
    }

    /// Follow the open documents, once a frame: start the newly opened ones from what is
    /// remembered, follow page changes and saves (Save As moves the settings to the new file,
    /// leaving the old file's entry as it was), and let go of closed ones.
    pub fn sync<'a>(&mut self, docs: impl Iterator<Item = &'a Document>, unit: Unit) {
        let mut live = Vec::new();
        for doc in docs {
            live.push(doc.id);
            let generation = doc.edit_generation();
            match self.open.get(&doc.id) {
                None => self.attach(doc, unit),
                Some(s) if s.seen == generation => {}
                Some(_) => self.follow(doc, unit),
            }
        }
        self.open.retain(|id, _| live.contains(id));
    }

    fn attach(&mut self, doc: &Document, unit: Unit) {
        let order = page_keys(doc);
        let mut state = DocState::new(ViewPrefs::new(unit), order, doc.edit_generation());
        if let Some(identity) = Identity::of(doc) {
            self.bind(&mut state, identity, unit);
        }
        self.open.insert(doc.id, state);
    }

    /// Apply the entry remembered for `identity`, adopting a moved file's entry, and remember
    /// the document there from now on.
    fn bind(&mut self, state: &mut DocState, identity: Identity, unit: Unit) {
        let at = self.stored.iter().position(|s| s.path == identity.path);
        let found = match at {
            Some(i) if self.stored.get(i).is_some_and(|s| identity.same_document(s)) => Some(i),
            Some(_) => {
                state.foreign = true;
                None
            }
            None => self.adoptable(&identity),
        };
        // One open document per entry: another bound to the same file is now session-only.
        for other in self.open.values_mut() {
            if other.binding.as_ref().is_some_and(|b| b.path == identity.path) {
                other.binding = None;
            }
        }
        if let Some(i) = found {
            self.clock = self.clock.saturating_add(1);
            if let Some(s) = self.stored.get_mut(i) {
                s.path.clone_from(&identity.path);
                s.used = self.clock;
                state.prefs = ViewPrefs { rulers: s.rulers, guides_shown: s.guides_shown, unit: s.unit.unwrap_or(unit) };
                let mut total = 0usize;
                for (page, g) in &s.guides {
                    let Some(key) = state.saved.get(*page) else { continue };
                    let on = state.guides.entry(*key).or_default();
                    for axis in [Axis::X, Axis::Y] {
                        for &at in g.axis(axis) {
                            if total < MAX_GUIDES_PER_DOC && insert(on.axis_mut(axis), at) {
                                total += 1;
                            }
                        }
                    }
                }
            }
            // Adopted from another path: drop any other entry at this one.
            let path = identity.path.clone();
            let mut first = true;
            self.stored.retain(|s| s.path != path || std::mem::take(&mut first));
            self.changed = true;
        }
        state.binding = Some(identity);
    }

    /// A moved or renamed file's entry: the same identifier and page count, and nothing at its
    /// old path any more. Never without an identifier, and never when two entries would do.
    fn adoptable(&self, identity: &Identity) -> Option<usize> {
        identity.id.as_ref()?;
        let mut candidates = self
            .stored
            .iter()
            .enumerate()
            .filter(|(_, s)| s.id == identity.id && s.pages == identity.pages && !(self.exists)(&s.path))
            .map(|(i, _)| i);
        let first = candidates.next()?;
        candidates.next().is_none().then_some(first)
    }

    /// Document `doc` was just saved, or reloaded from its file: its pages are the ones it had,
    /// in the same order, whatever their objects are now (a full rewrite renumbers them), so its
    /// guides stay with their page numbers. Those of pages it no longer has are dropped. The
    /// caller [`Self::sync`]s just before saving or reloading, so "the ones it had" are the pages
    /// as they were then, edits made in the same frame included.
    pub fn rebase(&mut self, doc: &Document) {
        let order = page_keys(doc);
        if let Some(state) = self.open.get_mut(&doc.id) {
            state.by_position(order);
        }
    }

    /// The document changed since last looked at.
    fn follow(&mut self, doc: &Document, unit: Unit) {
        let order = page_keys(doc);
        let history = doc.can_undo().is_some() || doc.can_redo().is_some();
        let Some(state) = self.open.get_mut(&doc.id) else { return };
        state.seen = doc.edit_generation();
        state.sync_pages(order, history);
        if doc.dirty {
            return;
        }
        // As saved (or reverted, or reloaded): stored positions are the pages as they are now.
        state.saved = state.order.clone();
        let Some(identity) = Identity::of(doc) else { return };
        let moved = state.binding.as_ref().is_none_or(|b| b.path != identity.path);
        if moved {
            // Save As: the new file starts from this document's settings, whatever was stored
            // for that path before; the old file keeps its entry.
            let mut state = match self.open.remove(&doc.id) {
                Some(s) => s,
                None => return,
            };
            for other in self.open.values_mut() {
                if other.binding.as_ref().is_some_and(|b| b.path == identity.path) {
                    other.binding = None;
                }
            }
            state.binding = Some(identity);
            state.foreign = false;
            let write = !state.is_default(unit);
            self.open.insert(doc.id, state);
            if write {
                self.store(doc.id);
            }
        } else {
            // Saved in place: a file that had no /ID has one now.
            state.binding = Some(identity);
            if !state.foreign && self.stored.iter().any(|s| state.binding.as_ref().is_some_and(|b| b.path == s.path)) {
                self.store(doc.id);
            }
        }
    }

    /// Write document `id`'s settings to its entry (creating it, or replacing another
    /// document's), keeping at most [`MAX_DOCS`] entries.
    fn store(&mut self, id: DocId) {
        let Some(state) = self.open.get_mut(&id) else { return };
        let Some(binding) = state.binding.clone() else { return };
        state.foreign = false;
        self.clock = self.clock.saturating_add(1);
        let entry = state.stored(&binding, self.clock);
        match self.stored.iter_mut().find(|s| s.path == binding.path) {
            Some(s) => {
                // Only the clock moved: not worth a write of the settings file by itself.
                let mut same = s.clone();
                same.used = entry.used;
                if same != entry {
                    self.changed = true;
                }
                *s = entry;
            }
            None => {
                self.stored.push(entry);
                self.changed = true;
            }
        }
        if self.stored.len() > MAX_DOCS {
            // The least recently used entry of a document that isn't open.
            let open: Vec<&str> = self.open.values().filter_map(|s| s.binding.as_ref().map(|b| b.path.as_str())).collect();
            if let Some(i) =
                self.stored.iter().enumerate().filter(|(_, s)| !open.contains(&s.path.as_str())).min_by_key(|(_, s)| s.used).map(|(i, _)| i)
            {
                self.stored.swap_remove(i);
            }
        }
    }

    /// Change document `id`'s settings with `f`, then remember them.
    fn edit<T>(&mut self, id: DocId, f: impl FnOnce(&mut DocState) -> T) -> Option<T> {
        let out = f(self.open.get_mut(&id)?);
        self.store(id);
        Some(out)
    }

    pub fn set_rulers(&mut self, id: DocId, on: bool) {
        self.edit(id, |s| s.prefs.rulers = on);
    }

    pub fn set_guides_shown(&mut self, id: DocId, on: bool) {
        self.edit(id, |s| s.prefs.guides_shown = on);
    }

    pub fn set_unit(&mut self, id: DocId, unit: Unit) {
        self.edit(id, |s| s.prefs.unit = unit);
    }

    /// Add a guide on `page` (0-based) at user-space coordinate `at`. Returns it (an existing one
    /// at the same place included), or `None` when there's no such page or the page or document
    /// already has as many guides as are kept.
    pub fn add_guide(&mut self, id: DocId, page: usize, axis: Axis, at: f64) -> Option<GuideRef> {
        let state = self.open.get(&id)?;
        let key = *state.order.get(page)?;
        let list = state.page(page).map(|g| g.axis(axis)).unwrap_or_default();
        if let Some(&same) = list.iter().find(|g| (*g - at).abs() < SAME) {
            return Some(GuideRef { page, axis, at: same });
        }
        if !at.is_finite() || at.abs() > MAX_COORD || list.len() >= MAX_GUIDES_PER_AXIS || state.total() >= MAX_GUIDES_PER_DOC {
            return None;
        }
        self.edit(id, |s| insert(s.guides.entry(key).or_default().axis_mut(axis), at))?.then_some(GuideRef { page, axis, at })
    }

    /// Move `guide` to `to` on its page. Returns it where it is now (where it was, if `to` isn't
    /// a coordinate or another guide is there already), `None` if it no longer exists.
    pub fn move_guide(&mut self, id: DocId, guide: GuideRef, to: f64) -> Option<GuideRef> {
        let state = self.open.get(&id)?;
        let key = *state.order.get(guide.page)?;
        let list = state.guides.get(&key)?.axis(guide.axis);
        let i = list.iter().position(|g| *g == guide.at)?;
        if !to.is_finite() || to.abs() > MAX_COORD || list.iter().enumerate().any(|(j, g)| j != i && (g - to).abs() < SAME) {
            return Some(guide);
        }
        self.edit(id, |s| {
            if let Some(slot) = s.guides.get_mut(&key).and_then(|g| g.axis_mut(guide.axis).get_mut(i)) {
                *slot = to;
            }
        })?;
        Some(GuideRef { at: to, ..guide })
    }

    /// Delete `guide`. Returns whether it existed.
    pub fn remove_guide(&mut self, id: DocId, guide: GuideRef) -> bool {
        let Some(key) = self.open.get(&id).filter(|s| s.has(guide)).and_then(|s| s.order.get(guide.page).copied()) else { return false };
        self.edit(id, |s| {
            if let Some(g) = s.guides.get_mut(&key) {
                g.axis_mut(guide.axis).retain(|g| *g != guide.at);
            }
        })
        .is_some()
    }

    /// Clear Guides on Page: delete page `page`'s guides. Returns whether it had any.
    pub fn clear_page(&mut self, id: DocId, page: usize) -> bool {
        let Some(key) = self.open.get(&id).filter(|s| s.page(page).is_some_and(|g| !g.is_empty())).and_then(|s| s.order.get(page).copied()) else {
            return false;
        };
        self.edit(id, |s| s.guides.remove(&key)).is_some()
    }

    /// Clear All Guides: delete every page's guides, a deleted page's waiting for an undo too.
    /// Returns whether a page the document has had any.
    pub fn clear_all(&mut self, id: DocId) -> bool {
        if self.open.get(&id).is_none_or(|s| s.pages_with_guides().next().is_none()) {
            return false;
        }
        self.edit(id, |s| s.guides.clear()).is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use pdfcraft_engine::{Edit, Session};

    /// A minimal `n`-page document, with `/ID [<id> <id>]` when `id` is given.
    fn pdf(n: usize, id: Option<&str>) -> Vec<u8> {
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
        let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
        for _ in 0..n {
            objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>".into());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        let id = id.map(|id| format!(" /ID [<{id}> <{id}>]")).unwrap_or_default();
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{id} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn open(s: &mut Session, path: Option<&str>, bytes: Vec<u8>) -> DocId {
        s.open("t.pdf", path.map(str::to_string), Arc::new(bytes), None).unwrap()
    }

    fn sync(p: &mut DocPrefs, s: &Session) {
        p.sync(s.docs().iter(), Unit::Millimetres);
    }

    fn nowhere(_: &str) -> bool {
        false
    }

    fn everywhere(_: &str) -> bool {
        true
    }

    fn prefs() -> DocPrefs {
        DocPrefs { exists: everywhere, ..Default::default() }
    }

    /// A restart: what was written, read into a fresh store.
    fn restart(p: &DocPrefs) -> DocPrefs {
        let json = serde_json::to_string(&p.to_json()).unwrap();
        let mut fresh = DocPrefs { exists: p.exists, ..Default::default() };
        fresh.load(&serde_json::from_str(&json).unwrap());
        fresh
    }

    fn guides(p: &DocPrefs, id: DocId, page: usize) -> PageGuides {
        p.state(id).and_then(|s| s.page(page)).cloned().unwrap_or_default()
    }

    #[test]
    fn settings_and_guides_round_trip_per_file() {
        let mut s = Session::new();
        let mut p = prefs();
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(3, Some("aa01")));
        let b = open(&mut s, Some("/plans/b.pdf"), pdf(2, Some("bb02")));
        sync(&mut p, &s);
        assert_eq!(p.state(a).unwrap().prefs, ViewPrefs::new(Unit::Millimetres), "a new PDF: rulers hidden, guides shown");
        assert!(!p.changed(), "opening alone remembers nothing");
        p.set_rulers(a, true);
        p.set_unit(a, Unit::Points);
        p.set_guides_shown(b, false);
        assert_eq!(p.add_guide(a, 0, Axis::X, 100.0), Some(GuideRef { page: 0, axis: Axis::X, at: 100.0 }));
        p.add_guide(a, 2, Axis::Y, 50.5).unwrap();
        p.add_guide(a, 2, Axis::Y, 250.0).unwrap();
        assert!(p.changed());
        p.written();

        let mut p = restart(&p);
        let mut s = Session::new();
        let b = open(&mut s, Some("/plans/b.pdf"), pdf(2, Some("bb02")));
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(3, Some("aa01")));
        sync(&mut p, &s);
        let state = p.state(a).unwrap();
        assert_eq!(state.prefs, ViewPrefs { rulers: true, guides_shown: true, unit: Unit::Points });
        assert_eq!(guides(&p, a, 0), PageGuides { x: vec![100.0], y: vec![] });
        assert_eq!(guides(&p, a, 1), PageGuides::default());
        assert_eq!(guides(&p, a, 2), PageGuides { x: vec![], y: vec![50.5, 250.0] });
        assert_eq!(p.state(b).unwrap().prefs, ViewPrefs { rulers: false, guides_shown: false, unit: Unit::Millimetres }, "b keeps its own");
        assert_eq!(guides(&p, b, 0), PageGuides::default());
    }

    #[test]
    fn moving_deleting_and_clearing_are_remembered() {
        let mut s = Session::new();
        let mut p = prefs();
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(2, Some("aa01")));
        sync(&mut p, &s);
        let g = p.add_guide(a, 0, Axis::X, 10.0).unwrap();
        let h = p.add_guide(a, 0, Axis::Y, 20.0).unwrap();
        p.add_guide(a, 1, Axis::Y, 30.0).unwrap();
        assert_eq!(p.add_guide(a, 0, Axis::X, 10.0004), Some(g), "the same place is the same guide");
        let moved = p.move_guide(a, g, 15.0).unwrap();
        assert_eq!(moved.at, 15.0);
        assert_eq!(p.move_guide(a, g, 99.0), None, "it isn't at 10 any more");
        assert!(p.remove_guide(a, h));
        assert!(!p.remove_guide(a, h));
        let mut q = restart(&p);
        let mut s2 = Session::new();
        let a2 = open(&mut s2, Some("/plans/a.pdf"), pdf(2, Some("aa01")));
        sync(&mut q, &s2);
        assert_eq!(guides(&q, a2, 0), PageGuides { x: vec![15.0], y: vec![] });
        assert!(q.clear_page(a2, 1));
        assert_eq!(guides(&restart(&q).tap(&mut s2, a2), a2, 1), PageGuides::default());
        assert!(q.clear_all(a2));
        let r = restart(&q).tap(&mut s2, a2);
        assert!(r.state(a2).unwrap().pages_with_guides().next().is_none());
    }

    impl DocPrefs {
        /// Tests: re-open `id` from what this store remembers.
        fn tap(mut self, s: &mut Session, id: DocId) -> Self {
            self.open.remove(&id);
            sync(&mut self, s);
            self
        }
    }

    #[test]
    fn another_document_at_the_same_path_gets_nothing_and_its_changes_replace_the_entry() {
        let mut s = Session::new();
        let mut p = prefs();
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(2, Some("aa01")));
        sync(&mut p, &s);
        p.set_rulers(a, true);
        p.add_guide(a, 0, Axis::X, 10.0).unwrap();
        // The file is replaced by another document (another /ID) at the same path.
        let mut p = restart(&p);
        let mut s = Session::new();
        let other = open(&mut s, Some("/plans/a.pdf"), pdf(2, Some("cc03")));
        sync(&mut p, &s);
        assert!(!p.state(other).unwrap().prefs.rulers);
        assert_eq!(guides(&p, other, 0), PageGuides::default());
        assert!(!p.changed(), "the old entry stays until this document's own settings change");
        // Without /ID on either side, the page count tells them apart.
        let mut s3 = Session::new();
        let mut p3 = restart(&p);
        p3.open.clear();
        let c = open(&mut s3, Some("/plans/c.pdf"), pdf(2, None));
        sync(&mut p3, &s3);
        p3.add_guide(c, 0, Axis::Y, 5.0).unwrap();
        let mut p4 = restart(&p3);
        let mut s4 = Session::new();
        let c3 = open(&mut s4, Some("/plans/c.pdf"), pdf(3, None));
        sync(&mut p4, &s4);
        assert_eq!(guides(&p4, c3, 0), PageGuides::default(), "three pages aren't the two-page file");

        p.add_guide(other, 1, Axis::Y, 42.0).unwrap();
        let mut q = restart(&p);
        let mut s2 = Session::new();
        let again = open(&mut s2, Some("/plans/a.pdf"), pdf(2, Some("cc03")));
        sync(&mut q, &s2);
        assert_eq!(guides(&q, again, 1), PageGuides { x: vec![], y: vec![42.0] }, "now the entry is this document's");
        assert_eq!(guides(&q, again, 0), PageGuides::default(), "and nothing of the other's");
    }

    #[test]
    fn a_moved_file_takes_its_entry_only_when_the_old_path_is_gone() {
        let setup = |exists: fn(&str) -> bool| {
            let mut s = Session::new();
            let mut p = DocPrefs { exists, ..Default::default() };
            let a = open(&mut s, Some("/old/a.pdf"), pdf(2, Some("aa01")));
            sync(&mut p, &s);
            p.add_guide(a, 1, Axis::X, 77.0).unwrap();
            let mut p = restart(&p);
            let mut s = Session::new();
            let moved = open(&mut s, Some("/new/a.pdf"), pdf(2, Some("aa01")));
            sync(&mut p, &s);
            (p, moved)
        };
        let (p, moved) = setup(nowhere);
        assert_eq!(guides(&p, moved, 1), PageGuides { x: vec![77.0], y: vec![] }, "moved: adopted");
        let json = p.to_json();
        assert_eq!(json["docs"].as_array().unwrap().len(), 1);
        assert!(json["docs"][0]["path"].as_str().unwrap().ends_with("a.pdf") && json["docs"][0]["path"].as_str().unwrap().contains("new"));
        let (p, copy) = setup(everywhere);
        assert_eq!(guides(&p, copy, 1), PageGuides::default(), "a copy beside the original starts fresh");
        // Without an identifier a file is never adopted.
        let mut s = Session::new();
        let mut p = DocPrefs { exists: nowhere, ..Default::default() };
        let a = open(&mut s, Some("/old/b.pdf"), pdf(2, None));
        sync(&mut p, &s);
        p.add_guide(a, 0, Axis::X, 1.0).unwrap();
        let mut p = restart(&p);
        let mut s = Session::new();
        let b = open(&mut s, Some("/new/b.pdf"), pdf(2, None));
        sync(&mut p, &s);
        assert_eq!(guides(&p, b, 0), PageGuides::default());
    }

    #[test]
    fn documents_without_a_file_keep_theirs_for_the_session_until_save_as() {
        let mut s = Session::new();
        let mut p = prefs();
        let new = open(&mut s, None, pdf(2, None));
        sync(&mut p, &s);
        p.set_rulers(new, true);
        p.add_guide(new, 0, Axis::X, 12.0).unwrap();
        assert_eq!(guides(&p, new, 0).x, [12.0], "kept for the session");
        assert_eq!(p.to_json()["docs"].as_array().unwrap().len(), 0, "and not remembered");
        // Save As to another file writes it whole (`save_doc_to`), with an /ID.
        let bytes = s.save_full_bytes(new).unwrap();
        sync(&mut p, &s);
        s.mark_saved(new, bytes, Some("/plans/saved.pdf".into())).unwrap();
        p.rebase(s.get(new).unwrap());
        sync(&mut p, &s);
        let docs = p.to_json()["docs"].as_array().unwrap().clone();
        assert_eq!(docs.len(), 1, "Save As gives it an entry");
        assert_eq!(docs[0]["rulers"], json!(true));
        assert_eq!(docs[0]["guides"][0]["x"], json!([12.0]));
        assert!(docs[0]["id"].is_string(), "the saved file's /ID");
    }

    #[test]
    fn save_as_copies_the_settings_and_the_old_file_keeps_its_own() {
        let mut s = Session::new();
        let mut p = prefs();
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(2, Some("aa01")));
        sync(&mut p, &s);
        p.add_guide(a, 0, Axis::X, 10.0).unwrap();
        let bytes = s.save_full_bytes(a).unwrap();
        sync(&mut p, &s);
        s.mark_saved(a, bytes, Some("/plans/copy.pdf".into())).unwrap();
        p.rebase(s.get(a).unwrap());
        sync(&mut p, &s);
        p.add_guide(a, 1, Axis::Y, 20.0).unwrap();
        let json = p.to_json();
        let docs = json["docs"].as_array().unwrap();
        let by = |name: &str| docs.iter().find(|d| d["path"].as_str().unwrap().ends_with(name)).unwrap().clone();
        assert_eq!(by("a.pdf")["guides"], json!([{"page": 0, "x": [10.0], "y": []}]), "the original as it was");
        assert_eq!(by("copy.pdf")["guides"], json!([{"page": 0, "x": [10.0], "y": []}, {"page": 1, "x": [], "y": [20.0]}]));
    }

    #[test]
    fn saving_in_place_takes_the_new_file_id() {
        let mut s = Session::new();
        let mut p = prefs();
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(2, None));
        sync(&mut p, &s);
        p.add_guide(a, 0, Axis::X, 10.0).unwrap();
        assert_eq!(p.to_json()["docs"][0]["id"], Value::Null);
        s.apply(a, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
        let bytes = s.save_bytes(a).unwrap();
        sync(&mut p, &s);
        s.mark_saved(a, bytes, Some("/plans/a.pdf".into())).unwrap();
        p.rebase(s.get(a).unwrap());
        sync(&mut p, &s);
        let id = p.to_json()["docs"][0]["id"].as_str().map(str::to_string).expect("the /ID the save added");
        assert_eq!(Some(id), s.get(a).unwrap().file_id().map(|b| hex(&b)));
    }

    #[test]
    fn guides_follow_their_page_and_a_deleted_page_is_never_stored() {
        let mut s = Session::new();
        let mut p = prefs();
        let a = open(&mut s, Some("/plans/a.pdf"), pdf(3, Some("aa01")));
        sync(&mut p, &s);
        for page in 0..3 {
            p.add_guide(a, page, Axis::X, 10.0 * (page + 1) as f64).unwrap();
        }
        // Move page 1 to the front: its guide goes with it; unsaved, the entry keeps the file's order.
        s.apply(a, Edit::MovePages { pages: vec![0], to: 3 }).unwrap();
        sync(&mut p, &s);
        assert_eq!((guides(&p, a, 0).x, guides(&p, a, 2).x), (vec![20.0], vec![10.0]));
        let stored = |p: &DocPrefs| p.to_json()["docs"][0]["guides"].clone();
        assert_eq!(stored(&p), json!([{"page": 0, "x": [10.0], "y": []}, {"page": 1, "x": [20.0], "y": []}, {"page": 2, "x": [30.0], "y": []}]));
        // Delete what is now the first page (the original page 2), then undo: back with its guide.
        s.apply(a, Edit::DeletePages { pages: vec![0] }).unwrap();
        sync(&mut p, &s);
        assert_eq!(guides(&p, a, 0).x, [30.0]);
        s.undo(a).unwrap();
        sync(&mut p, &s);
        assert_eq!(guides(&p, a, 0).x, [20.0], "undo brings the page back with its guide");
        // Delete it again and save: the deleted page's guide is stored nowhere, the others at
        // their new positions.
        s.apply(a, Edit::DeletePages { pages: vec![0] }).unwrap();
        let bytes = s.save_bytes(a).unwrap();
        sync(&mut p, &s);
        s.mark_saved(a, bytes, Some("/plans/a.pdf".into())).unwrap();
        p.rebase(s.get(a).unwrap());
        sync(&mut p, &s);
        assert_eq!(stored(&p), json!([{"page": 0, "x": [30.0], "y": []}, {"page": 1, "x": [10.0], "y": []}]));
        let mut q = restart(&p);
        let mut s2 = Session::new();
        let bytes = s.get(a).unwrap().bytes.clone();
        let again = s2.open("a.pdf", Some("/plans/a.pdf".into()), bytes, None).unwrap();
        sync(&mut q, &s2);
        assert_eq!((guides(&q, again, 0).x, guides(&q, again, 1).x), (vec![30.0], vec![10.0]));
        assert!(q.state(again).unwrap().pages_with_guides().all(|(_, g)| !g.x.contains(&20.0)));
    }

    #[test]
    fn renumbered_pages_keep_guides_by_position_but_never_a_deleted_pages() {
        let mut state = DocState::new(ViewPrefs::new(Unit::Inches), vec![PageKey::Obj(3, 0), PageKey::Obj(4, 0), PageKey::Obj(5, 0)], 0);
        for (k, x) in [(PageKey::Obj(3, 0), 1.0), (PageKey::Obj(4, 0), 2.0), (PageKey::Obj(9, 0), 9.0)] {
            state.guides.insert(k, PageGuides { x: vec![x], y: vec![] });
        }
        // Object 9 is a deleted page waiting for an undo. A full rewrite renumbers everything,
        // and one of the new numbers happens to be 9.
        state.sync_pages(vec![PageKey::Obj(9, 0), PageKey::Obj(10, 0), PageKey::Obj(11, 0)], true);
        assert_eq!(state.page(0).map(|g| g.x.clone()), Some(vec![1.0]), "by position");
        assert_eq!(state.page(1).map(|g| g.x.clone()), Some(vec![2.0]));
        assert_eq!(state.page(2), None);
        assert_eq!(state.guides.len(), 2, "the deleted page's guide is gone, not on page 1");
        // Renumbered with one number reused by chance (3, 4 → 4, 5): still by position.
        let mut state = DocState::new(ViewPrefs::new(Unit::Inches), vec![PageKey::Obj(3, 0), PageKey::Obj(4, 0)], 0);
        state.guides.insert(PageKey::Obj(3, 0), PageGuides { x: vec![1.0], y: vec![] });
        state.sync_pages(vec![PageKey::Obj(4, 0), PageKey::Obj(5, 0)], true);
        assert_eq!(state.page(0).map(|g| g.x.clone()), Some(vec![1.0]));
        assert_eq!(state.page(1), None);
        // Reordered pages keep their own guides.
        state.sync_pages(vec![PageKey::Obj(5, 0), PageKey::Obj(4, 0)], true);
        assert_eq!(state.page(1).map(|g| g.x.clone()), Some(vec![1.0]));
        // With no undo history left, guides of pages that are gone are dropped.
        let mut state = DocState::new(ViewPrefs::new(Unit::Inches), vec![PageKey::Obj(3, 0)], 0);
        state.guides.insert(PageKey::Obj(8, 0), PageGuides { x: vec![1.0], y: vec![] });
        state.sync_pages(vec![PageKey::Obj(3, 0)], false);
        assert!(state.guides.is_empty());
    }

    #[test]
    fn the_app_keeps_them_in_its_settings_and_never_in_the_pdf() {
        let path = "/pdfcraft-tests/plans/a.pdf".to_string();
        let mut app = crate::PdfCraftApp::new();
        app.open_bytes("a.pdf", Some(path.clone()), pdf(2, Some("aa01"))).unwrap();
        app.sync_doc_prefs();
        let id = app.views[0].id;
        let before = app.session.save_bytes(id).unwrap();
        app.doc_prefs.set_rulers(id, true);
        app.doc_prefs.set_unit(id, Unit::Centimetres);
        app.doc_prefs.add_guide(id, 1, Axis::Y, 123.5).unwrap();
        let doc = app.session.get(id).unwrap();
        assert!(!doc.dirty, "not a change to save");
        assert_eq!(doc.can_undo(), None, "no undo step");
        assert_eq!(*app.session.save_bytes(id).unwrap(), *before, "the PDF's bytes are the same");
        assert!(app.doc_prefs.changed(), "the settings file is due a write");
        let saved = app.persist();

        // A restart: the settings come back before the file opens again.
        let mut again = crate::PdfCraftApp::new();
        again.restore(&saved);
        again.open_bytes("a.pdf", Some(path), pdf(2, Some("aa01"))).unwrap();
        again.sync_doc_prefs();
        let id = again.views[0].id;
        let state = again.doc_prefs.state(id).unwrap();
        assert_eq!(state.prefs, ViewPrefs { rulers: true, guides_shown: true, unit: Unit::Centimetres });
        assert_eq!(state.page(1).map(|g| g.y.clone()), Some(vec![123.5]));
        // Settings are untrusted: a value that isn't the store is ignored.
        let mut odd = crate::PdfCraftApp::new();
        odd.restore(r#"{"document_views": "everything"}"#);
        assert_eq!(odd.doc_prefs.to_json()["docs"], json!([]));
    }

    #[test]
    fn untrusted_settings_are_skipped_and_capped() {
        let mut p = prefs();
        for garbage in [json!(null), json!("x"), json!({"v": 2, "docs": []}), json!({"v": 1, "docs": "no"})] {
            p.load(&garbage);
            assert!(p.stored.is_empty(), "{garbage}");
        }
        let long_path = "a".repeat(MAX_PATH_CHARS + 1);
        let many: Vec<f64> = (0..500).map(f64::from).collect();
        p.load(&json!({"v": 1, "docs": [
            {"path": "", "pages": 1},
            {"path": long_path, "pages": 1},
            {"path": "/no-pages.pdf"},
            {"path": "/bad-id.pdf", "pages": 1, "id": "not hex"},
            {"path": "/huge.pdf", "pages": 1u64 << 40},
            {"path": "/ok.pdf", "pages": 2, "used": 5, "id": "AB01", "rulers": "yes", "unit": "furlongs",
             "guides": [
                {"page": 0, "x": [1.0, "2", null, 1.0, 1.0e300, -3.5], "y": many},
                {"page": 7, "x": [1.0]},
                {"page": 0, "x": [4.0]},
                "nonsense"
             ]},
            {"path": "/ok.pdf", "pages": 9, "used": 1}
        ]}));
        assert_eq!(p.stored.len(), 1, "{:?}", p.stored);
        let ok = &p.stored[0];
        assert_eq!((ok.pages, ok.id.as_deref(), ok.rulers, ok.unit), (2, Some("ab01"), false, None), "the newest /ok.pdf, defaults for bad values");
        assert_eq!(ok.guides.len(), 1, "page 7 is past the end; page 0 listed twice is merged");
        assert_eq!(ok.guides[0].1.x, [1.0, -3.5, 4.0]);
        assert_eq!(ok.guides[0].1.y.len(), MAX_GUIDES_PER_AXIS);

        // At most MAX_GUIDES_PER_DOC per document.
        let pages: Vec<Value> = (0..20).map(|p| json!({"page": p, "x": (0..100).map(f64::from).collect::<Vec<_>>()})).collect();
        p.load(&json!({"v": 1, "docs": [{"path": "/full.pdf", "pages": 20, "guides": pages}]}));
        assert_eq!(p.stored[0].guides.iter().map(|(_, g)| g.len()).sum::<usize>(), MAX_GUIDES_PER_DOC);
        // At most MAX_DOCS documents, the most recent kept.
        let docs: Vec<Value> = (0..MAX_DOCS + 50).map(|i| json!({"path": format!("/d{i}.pdf"), "pages": 1, "used": i})).collect();
        p.load(&json!({"v": 1, "docs": docs}));
        assert_eq!(p.stored.len(), MAX_DOCS);
        assert!(p.stored.iter().all(|s| s.used >= 50));
    }

    #[test]
    fn the_least_recently_used_entry_goes_beyond_the_limit_and_guides_are_capped() {
        let mut s = Session::new();
        let mut p = prefs();
        let docs: Vec<Value> = (0..MAX_DOCS).map(|i| json!({"path": format!("/d{i}.pdf"), "pages": 1, "used": i + 1})).collect();
        p.load(&json!({"v": 1, "docs": docs}));
        let a = open(&mut s, Some("/plans/new.pdf"), pdf(1, Some("aa01")));
        sync(&mut p, &s);
        p.set_rulers(a, true);
        assert_eq!(p.stored.len(), MAX_DOCS);
        assert!(!p.stored.iter().any(|s| s.path == "/d0.pdf"), "the oldest went");
        assert!(p.stored.iter().any(|s| s.path.ends_with("new.pdf")));
        // Per axis and per page.
        for i in 0..MAX_GUIDES_PER_AXIS {
            assert!(p.add_guide(a, 0, Axis::X, i as f64).is_some());
        }
        assert_eq!(p.add_guide(a, 0, Axis::X, 1000.0), None, "the page is full in that direction");
        assert!(p.add_guide(a, 0, Axis::Y, 1.0).is_some());
        assert_eq!(p.add_guide(a, 0, Axis::X, f64::NAN), None);
        assert_eq!(p.add_guide(a, 5, Axis::X, 1.0), None, "no page 6");
    }
}

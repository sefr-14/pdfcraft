//! Per-viewport native window and raw-input hooks.
//!
//! eframe exposes the root window's handles through [`crate::CreationContext`] only, and runs
//! [`crate::App::raw_input_hook`] for the root and deferred viewports but not for immediate ones.
//! Integrations that need every viewport's native window (for example to match Wayland surfaces
//! named by a protocol the windowing layer doesn't implement) or that must add input to every
//! viewport before egui processes it can set [`crate::NativeOptions::viewport_hooks`].
//!
//! [`native_window`] gives an app the native window of any live viewport, for example to parent
//! a native file dialog to the window that asked for it. eframe keeps only a weak reference, so
//! this never keeps a closed window alive.

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

/// Callbacks eframe makes for every viewport. All methods have empty defaults.
///
/// They run on the event loop thread, inside eframe's frame handling: keep them short and don't
/// call back into eframe or block.
pub trait ViewportHooks: Send + Sync {
    /// eframe created the native window of `viewport` (root, deferred or immediate).
    fn window_created(&self, _viewport: egui::ViewportId, _window: &winit::window::Window) {}

    /// eframe dropped `viewport`; its window is about to be (or has been) destroyed.
    fn window_destroyed(&self, _viewport: egui::ViewportId) {}

    /// The raw input of one viewport (`raw_input.viewport_id`), before egui processes it. Runs for
    /// root, deferred and immediate viewports, before [`crate::App::raw_input_hook`].
    fn raw_input(&self, _ctx: &egui::Context, _raw_input: &mut egui::RawInput) {}
}

static HOOKS: RwLock<Option<Arc<dyn ViewportHooks>>> = RwLock::new(None);

/// The native windows of the live viewports, weakly held.
static WINDOWS: Mutex<Option<Registry<winit::window::Window>>> = Mutex::new(None);

/// The native window of `viewport` (root, deferred or immediate), while it exists.
///
/// The window implements `raw_window_handle`'s `HasWindowHandle` and `HasDisplayHandle`, safely,
/// so it can parent a native dialog (`rfd::FileDialog::set_parent`). Call it on the event loop
/// thread, as egui's UI code runs; it returns `None` for a viewport without a window (embedded
/// viewports, a window not created yet, or one already closed). Holding the `Arc` keeps the
/// window alive: drop it when the dialog is done.
pub fn native_window(viewport: egui::ViewportId) -> Option<Arc<winit::window::Window>> {
    WINDOWS.lock().as_ref().and_then(|r| r.get(viewport))
}

/// eframe has the native window of `viewport` (called whenever it may be new; cheap otherwise).
pub(crate) fn window_live(viewport: egui::ViewportId, window: &Arc<winit::window::Window>) {
    WINDOWS
        .lock()
        .get_or_insert_with(Registry::default)
        .insert(viewport, window);
}

/// eframe dropped `viewport`'s window.
pub(crate) fn window_gone(viewport: egui::ViewportId) {
    if let Some(r) = WINDOWS.lock().as_mut() {
        r.remove(viewport);
    }
}

/// Weak references by viewport.
struct Registry<T> {
    by_viewport: egui::ViewportIdMap<std::sync::Weak<T>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            by_viewport: Default::default(),
        }
    }
}

impl<T> Registry<T> {
    fn insert(&mut self, viewport: egui::ViewportId, item: &Arc<T>) {
        // Dead entries (a viewport closed without a report) don't pile up.
        self.by_viewport.retain(|_, weak| weak.strong_count() > 0);
        let known = self
            .by_viewport
            .get(&viewport)
            .is_some_and(|w| core::ptr::eq(w.as_ptr(), Arc::as_ptr(item)));
        if !known {
            self.by_viewport.insert(viewport, Arc::downgrade(item));
        }
    }

    fn get(&self, viewport: egui::ViewportId) -> Option<Arc<T>> {
        self.by_viewport
            .get(&viewport)
            .and_then(std::sync::Weak::upgrade)
    }

    fn remove(&mut self, viewport: egui::ViewportId) {
        self.by_viewport.remove(&viewport);
    }
}

pub(crate) fn install(hooks: Option<Arc<dyn ViewportHooks>>) {
    *HOOKS.write() = hooks;
}

pub(crate) fn with(f: impl FnOnce(&dyn ViewportHooks)) {
    let hooks = HOOKS.read().clone();
    if let Some(hooks) = hooks {
        f(&*hooks);
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct CountRawInput(AtomicUsize);

    impl ViewportHooks for CountRawInput {
        fn raw_input(&self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
            self.0.fetch_add(1, Ordering::Relaxed);
            raw_input.focused = false;
        }
    }

    #[test]
    fn installed_hooks_run_until_replaced() {
        let hooks = Arc::new(CountRawInput(AtomicUsize::new(0)));
        let options = crate::NativeOptions {
            viewport_hooks: Some(Arc::clone(&hooks) as Arc<dyn ViewportHooks>),
            ..Default::default()
        };
        install(options.clone().viewport_hooks);

        let ctx = egui::Context::default();
        let mut raw_input = egui::RawInput::default();
        with(|h| h.raw_input(&ctx, &mut raw_input));
        assert_eq!(hooks.0.load(Ordering::Relaxed), 1);
        assert!(!raw_input.focused, "the hook can change the input");

        install(None);
        with(|h| h.raw_input(&ctx, &mut raw_input));
        assert_eq!(hooks.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_registry_holds_windows_weakly_by_viewport() {
        use super::Registry;
        let (root, other) = (
            egui::ViewportId::ROOT,
            egui::ViewportId::from_hash_of("other"),
        );
        let mut r = Registry::default();
        let a = Arc::new("root window");
        let b = Arc::new("other window");
        r.insert(root, &a);
        r.insert(other, &b);
        assert!(r.get(root).is_some_and(|w| Arc::ptr_eq(&w, &a)));
        assert!(r.get(other).is_some_and(|w| Arc::ptr_eq(&w, &b)));
        // Weak: a closed window is gone, never kept alive by the registry.
        drop(b);
        assert!(r.get(other).is_none());
        // A new window for the same viewport replaces the old one.
        let c = Arc::new("root window, recreated");
        r.insert(root, &c);
        assert!(r.get(root).is_some_and(|w| Arc::ptr_eq(&w, &c)));
        r.remove(root);
        assert!(r.get(root).is_none());
        assert!(r.get(egui::ViewportId::from_hash_of("never")).is_none());
    }
}

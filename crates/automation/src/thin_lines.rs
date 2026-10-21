//! Thin lines (this fork, OV9): View ▸ Thin lines as a tool (AGENTS.md §3).
//!
//! A per-document view setting, as in Revit: the document's strokes draw one device pixel wide
//! in `page_render` while comments keep their widths. The file itself never changes.

use serde_json::{Value, json};

use crate::{Args, Automation, Result};

impl Automation {
    /// Turn thin lines on (the default) or off for the document; returns the resulting setting.
    pub(crate) fn view_thin_lines(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let on = a.opt_bool("on")?.unwrap_or(true);
        if self.session.set_thin_lines(id, on) {
            // Cached renderers baked the old setting into their config; page_render must re-read it.
            self.renderers.remove(&id);
        }
        Ok(json!({ "on": self.session.thin_lines(id).unwrap_or(false) }))
    }
}

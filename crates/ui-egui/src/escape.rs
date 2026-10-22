//! Escape takes one step back per press (this fork, OV11).
//!
//! One physical press does at most one thing, and whatever does it consumes the key, so no
//! handler later in the frame (or in a later phase of it) sees the same press. In order:
//!
//! 1. Something that owns the press keeps it, and nothing underneath reacts: a dialog or prompt,
//!    a text field (focused now, or until this press unfocused it), an open menu or popup, the
//!    command palette, the Combine preview, or the Overlay wizard window under the pointer.
//!    A text field that belongs to a step below is that step: Escape in the find field closes
//!    the find bar, and in a pending calibration's known-distance field cancels the calibration.
//! 2. With a measuring tool: the points of a measurement in progress, else a calibration waiting
//!    for its known distance (fields back as they were), else the selected measurement or comment.
//! 3. The view's own state: a camera snapshot, else the text selection, else the find bar.
//! 4. A measuring tool with nothing left to cancel gives way to Select.
//! 5. Late in the frame, if nothing took the press: comment tools' own steps run where they always
//!    have (they consume now too), and only then does full screen end ([`PdfCraftApp::escape_late`]).
//!
//! Only the active view takes part: an inactive tile's measurement is left as it is.

use egui::{Key, Modifiers};

use crate::{PdfCraftApp, QuickTool, canvas};

impl PdfCraftApp {
    /// Escape's first steps, from the keyboard shortcuts (after dialogs and middle-button
    /// gestures have had theirs). See the module docs for the order.
    pub(crate) fn escape_step(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.key_pressed(Key::Escape)) {
            return;
        }
        let field = self.keyboard_owner.or_else(|| ctx.memory(|m| m.focused()));
        let measuring = matches!(self.quick_tool, QuickTool::Measure(_));
        let active = self.active.filter(|&i| i < self.views.len());
        if let Some(field) = field {
            if let Some(i) = active {
                let view = &mut self.views[i];
                let step = if measuring && view.measure.calibrating() && view.measure.known_field == Some(field) {
                    view.measure.cancel_calibration()
                } else {
                    field == canvas::find_input_id() && view.find.take().is_some()
                };
                if step {
                    take(ctx);
                    return;
                }
            }
            self.escape_reserved = true;
            return;
        }
        let wizard = ctx.pointer_hover_pos().and_then(|p| ctx.layer_id_at(p)).is_some_and(|l| l.id == crate::overlay_ui::wizard_id());
        if self.palette_shown() || egui::Popup::is_any_open(ctx) || self.combine_tab.preview.is_some() || wizard {
            self.escape_reserved = true;
            return;
        }
        let Some(i) = active else { return };
        let view = &mut self.views[i];
        if measuring {
            let step = if !view.measure.points.is_empty() {
                view.measure.cancel();
                true
            } else if view.measure.cancel_calibration() {
                true
            } else if view.comments.selected.take().is_some() {
                // A measurement picked in the panel's list: its reading goes with it.
                view.measure.reading = None;
                view.measure.scale = None;
                true
            } else {
                false
            };
            if step {
                take(ctx);
                return;
            }
        }
        if canvas::escape_view(view) {
            take(ctx);
            return;
        }
        if measuring {
            view.measure.cancel();
            self.quick_tool = QuickTool::Select;
            take(ctx);
        }
    }

    /// Escape's last step, at the end of the frame: leave full screen, if nothing else took the
    /// press. Also notes which widget has the keyboard, for the next frame's first step (egui lets
    /// go of a focused field on Escape before the app sees the press).
    pub(crate) fn escape_late(&mut self, ctx: &egui::Context) {
        if self.full_screen
            && !self.escape_reserved
            && self.combine_tab.preview.is_none()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.set_full_screen(ctx, false);
        }
        self.keyboard_owner = ctx.memory(|m| m.focused());
    }

    /// The command palette is drawn this frame (full screen shows the page only).
    fn palette_shown(&self) -> bool {
        self.palette_open && !(self.full_screen && self.active.is_some())
    }
}

/// This press is taken: nothing later in the frame reacts to it.
fn take(ctx: &egui::Context) {
    ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
}

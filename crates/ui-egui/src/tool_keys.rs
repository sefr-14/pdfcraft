//! The tool keys (this fork, KEY.1): plain letters that pick a tool, as engineering PDF tools
//! have them. One key, or two typed one after the other (not held together):
//!
//! | Keys | Tool |
//! |---|---|
//! | V | Select |
//! | P, or hold Space | Hand |
//! | H | Highlight |
//! | D, then I | Distance measurement |
//! | D, then L | Draw a line |
//! | R, then C | Revision cloud |
//!
//! Only plain presses count (no Ctrl, ⌘, Alt or Shift; no auto-repeat), and only while nothing
//! else takes typing: a text field (the find bar, a comment, a form field), a dialog, the command
//! palette, an open menu or popup. Each press is consumed, so a `logic` pass that runs again with
//! the same input (egui 0.37, a hidden window) can't take it twice.
//!
//! The first key of a pair waits [`SEQUENCE_TIMEOUT`] seconds for the second, with a hint on the
//! page. Another key, Escape, a click, another tab or a dialog cancels it, and a key that doesn't
//! finish the pair then counts on its own (D then H is Highlight).

use egui::{Event, Key};

use crate::{PdfCraftApp, QuickTool, canvas::ViewKey};

/// What a tool key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Select,
    Hand,
    /// A registered command, run as its toolbar button runs it.
    Command(&'static str),
}

/// Every tool key, the single list behind the keys, the hint and the shortcuts dialog.
pub(crate) const KEYS: [(&[Key], Action); 6] = [
    (&[Key::V], Action::Select),
    (&[Key::P], Action::Hand),
    (&[Key::H], Action::Command("comment.highlight")),
    (&[Key::D, Key::I], Action::Command("measure.distance")),
    (&[Key::D, Key::L], Action::Command("comment.line")),
    (&[Key::R, Key::C], Action::Command("comment.cloud")),
];

/// How long the first key of a pair waits for the second, in seconds.
pub(crate) const SEQUENCE_TIMEOUT: f64 = 1.5;

/// The first key of a pair, typed in `view` at `at` (egui's input time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pending {
    pub first: Key,
    at: f64,
    view: ViewKey,
}

/// The tool `keys` pick, if they are one.
fn action(keys: &[Key]) -> Option<Action> {
    KEYS.iter().find(|(k, _)| *k == keys).map(|(_, a)| *a)
}

/// `key` starts a pair.
fn starts_pair(key: Key) -> bool {
    KEYS.iter().any(|(k, _)| k.len() == 2 && k.first() == Some(&key))
}

/// What the tool a key picks is called, translated.
fn label(action: Action) -> String {
    match action {
        Action::Select => tl!("Select (V)").to_string(),
        Action::Hand => tl!("Hand (P)").to_string(),
        Action::Command(id) => pdfcraft_engine::commands::command(id).map(|c| tl!(c.label).to_string()).unwrap_or_default(),
    }
}

/// The keys as the shortcuts dialog writes them: "H", "D, then I".
fn keys_label(keys: &[Key]) -> String {
    match keys {
        [a, b] => crate::i18n::fmt(tl!("{a}, then {b}"), &[("a", a.name()), ("b", b.name())]),
        _ => keys.iter().map(|k| k.name()).collect::<Vec<_>>().join(""),
    }
}

/// The tool keys' rows of the shortcuts dialog: keys, then what they pick.
pub(crate) fn dialog_rows() -> Vec<(String, String)> {
    KEYS.iter()
        .map(|(keys, action)| match action {
            Action::Hand => (tl!("P / Space (hold)").to_string(), label(*action)),
            _ => (keys_label(keys), label(*action)),
        })
        .collect()
}

impl PdfCraftApp {
    /// The tool keys and Space for Hand (see the module docs), for view `i`, from `logic`.
    pub(crate) fn tool_keys(&mut self, i: usize, ctx: &egui::Context) {
        let (now, space, clicked, modified) =
            ctx.input(|input| (input.time, input.key_down(Key::Space), input.pointer.any_pressed(), !input.modifiers.is_none()));
        if let Some(previous) = self.space_hand
            && !space
        {
            self.space_hand = None;
            if self.quick_tool == QuickTool::Hand {
                self.quick_tool = previous;
            }
        }
        let Some(view) = self.views.get(i).map(|v| v.key) else { return };
        let free = self.dialog.is_none()
            && !self.palette_open
            && !ctx.egui_wants_keyboard_input()
            && self.views.get(i).is_some_and(|v| v.forms.focus.is_none())
            && !craft_menu::bar::is_open(ctx)
            && !craft_menu::bar::is_focused(ctx)
            && !egui::Popup::is_any_open(ctx);
        // A pair waiting too long, in another view, behind a dialog or a click, is dropped.
        if self.pending_keys.is_some_and(|p| now - p.at > SEQUENCE_TIMEOUT || p.view != view || !free || clicked) {
            self.pending_keys = None;
        }
        if !free {
            return;
        }
        // This frame's presses, in order: plain letters are the tool keys' to take; any other
        // press cancels a waiting pair.
        let presses: Vec<(Key, bool)> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, modifiers.is_none())),
                    _ => None,
                })
                .collect()
        });
        for (key, plain) in presses {
            let pending = self.pending_keys.take();
            if !plain {
                continue;
            }
            let run = match pending.and_then(|p| action(&[p.first, key])) {
                Some(done) => Some(done),
                None if starts_pair(key) => {
                    self.pending_keys = Some(Pending { first: key, at: now, view });
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(SEQUENCE_TIMEOUT));
                    None
                }
                None => action(&[key]),
            };
            if run.is_none() && self.pending_keys.is_none() {
                continue;
            }
            // Taken: nothing later in this frame, nor a `logic` pass run again, sees it.
            ctx.input_mut(|input| {
                if let Some(at) = input
                    .events
                    .iter()
                    .position(|e| matches!(e, Event::Key { key: k, pressed: true, repeat: false, modifiers, .. } if *k == key && modifiers.is_none()))
                {
                    input.events.remove(at);
                }
            });
            match run {
                Some(Action::Select) => {
                    self.quick_tool = QuickTool::Select;
                    self.space_hand = None;
                }
                Some(Action::Hand) => {
                    self.quick_tool = QuickTool::Hand;
                    self.space_hand = None;
                }
                Some(Action::Command(id)) => {
                    self.execute(id);
                }
                None => {}
            }
        }
        if space && !modified && self.space_hand.is_none() && self.quick_tool != QuickTool::Hand {
            self.space_hand = Some(self.quick_tool);
            self.quick_tool = QuickTool::Hand;
        }
    }

    /// While the first key of a pair waits: which second keys finish it, at the foot of the page
    /// area ("D…  I Measure distance · L Draw a line").
    pub(crate) fn show_pending_keys(&self, ctx: &egui::Context) {
        let Some(pending) = self.pending_keys else { return };
        let options: Vec<String> = KEYS
            .iter()
            .filter(|(k, _)| k.len() == 2 && k.first() == Some(&pending.first))
            .filter_map(|(k, a)| k.get(1).map(|second| format!("{} {}", second.name(), label(*a))))
            .collect();
        let text = format!("{}…   {}", pending.first.name(), options.join("  ·  "));
        egui::Area::new(egui::Id::unique("tool-keys-pending"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .interactable(false)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(text);
                });
            });
    }
}

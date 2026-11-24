// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
use super::PdfCraftApp;
use super::save::SaveTarget;

/// What the user was doing when we asked whether to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseRequest {
    /// Close this tab. By view key, not tab index: tabs can close and shift while a save panel
    /// is open. Not by document either: Duplicate Tab can show one document in several tabs.
    Tab(crate::canvas::ViewKey),
    /// Quit the application once every dirty document is resolved.
    Quit,
    /// File ▸ Close all: like Quit, but the application stays open.
    All,
    /// This fork (TAB.WINDOW.1 W2c): Close Window. Each document only this window shows asks,
    /// then the window closes (the first window hands its place to another, H1).
    Window(crate::multi_window::WindowKey),
}

impl PdfCraftApp {
    /// Close a tab, asking first if it has unsaved changes.
    ///
    /// While another tab still shows the document (Duplicate Tab), closing this one loses
    /// nothing: the document and its unsaved changes stay open there, so there is nothing to
    /// ask. What was typed in this tab goes into the document first, as clicking away would do;
    /// a field that refuses it keeps the tab open with the field shown. Only typing that can't
    /// be committed that way (a paragraph being edited) still asks. A document under OCR review
    /// stays open until the review is accepted or rejected.
    pub fn request_close_tab(&mut self, index: usize) {
        if self.hold_close_for_ocr_review(index) {
            return;
        }
        let Some(key) = self.win.views.get(index).map(|v| v.key) else { return };
        if self.has_twin(index) {
            if self.has_unsaved_typing(index) && !self.commit_typing_at(index) {
                return;
            }
            let Some(index) = self.view_index(key) else { return };
            if self.has_unsaved_typing(index) {
                self.close_request = Some(CloseRequest::Tab(key));
            } else {
                self.close_tab(index);
            }
            return;
        }
        // Text typed into a field counts as an unsaved change.
        if self.has_unsaved_work(index) {
            self.close_request = Some(CloseRequest::Tab(key));
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    /// While a document is under OCR review, nothing closes: the review comes forward first.
    pub fn close_all(&mut self) {
        // Every window's tabs (TAB.WINDOW.1): run between passes, where each window can be reached.
        if self.drawing_other_window() {
            self.request_window_op(crate::multi_window::WindowOp::Run("file.close_all"));
            return;
        }
        if self.hold_for_any_ocr_review() {
            return;
        }
        let keys: Vec<crate::multi_window::WindowKey> = self.window_uis().iter().map(|(key, _)| *key).collect();
        for key in keys {
            self.with_window(key, |app| {
                for i in (0..app.win.views.len()).rev() {
                    if !app.has_unsaved_work(i) {
                        app.close_tab(i);
                    }
                }
            });
        }
        if let Some(key) = self.first_dirty_window() {
            self.ask_in_window(CloseRequest::All, key);
        }
    }

    /// A document whose OCR review is still open (a run, or a reading not yet accepted or
    /// rejected) does not close under it: closing would leave the review with nothing to apply
    /// to (#6485). The document comes to the front with its review, and a notice says what to
    /// do. Whether the close was held.
    pub(crate) fn hold_close_for_ocr_review(&mut self, index: usize) -> bool {
        let Some(id) = self.win.views.get(index).map(|v| v.id) else { return false };
        if !self.ocr_verify.holds_reading_of(id) {
            return false;
        }
        self.win.active = Some(index);
        self.ocr_verify.open = true;
        self.notify_tr("Accept or reject the OCR review before closing this document");
        true
    }

    /// Close all and Quit close every document, so a document under OCR review holds them too
    /// ([`Self::hold_close_for_ocr_review`]), even when it has no unsaved changes. Whether they
    /// were held.
    fn hold_for_any_ocr_review(&mut self) -> bool {
        self.ocr_review_index().is_some_and(|i| self.hold_close_for_ocr_review(i))
    }

    /// The tab of the document whose OCR review is still open, if any.
    fn ocr_review_index(&self) -> Option<usize> {
        self.win.views.iter().position(|v| self.ocr_verify.holds_reading_of(v.id))
    }

    /// File ▸ Revert (after the confirmation).
    pub fn revert_active(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.revert(id) {
            Ok(()) => {
                self.refresh_document_views(id);
                // Revert discards this source's uncommitted typing as well as model edits, in
                // every tab of it. Otherwise a later Save could write a rejected draft back into
                // the file.
                for view in self.win.views.iter_mut().filter(|v| v.id == id) {
                    view.forms.focus = None;
                    // The focus goes with them, running no exit: a script changing the document
                    // just reverted would only make it unsaved again.
                    view.forms.focused = None;
                    view.forms.committed = None;
                    view.forms.unbaked.clear();
                    view.pending_edit = None;
                    // The redactions applied since the last save are discarded too. Those in the
                    // last web download stay, so later downloads keep the `_Redacted` name.
                    view.redactions_applied = view.redactions_downloaded;
                    view.redactions_undone_after_save = 0;
                    view.saved_redactions_cut_off = false;
                }
                // A signature placed since the last save is gone with the rest: the next save
                // doesn't ask to lock it (#2149).
                self.placed_signatures.remove(&id);
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        (0..self.win.views.len()).find(|&i| self.has_unsaved_work(i))
    }

    /// Answer the save prompt: `Some(true)` save, `Some(false)` discard, `None` cancel.
    pub fn resolve_close(&mut self, ctx: &egui::Context, choice: Option<bool>) {
        let Some(req) = self.close_request.take() else { return };
        // The next question (another dirty document) starts on Save again.
        self.save_prompt_button = Default::default();
        let index = match req {
            CloseRequest::Tab(key) => match self.view_index(key) {
                Some(i) => i,
                None => return, // already closed
            },
            CloseRequest::Quit | CloseRequest::All => match self.first_dirty() {
                Some(i) => i,
                None => {
                    // The next document to ask about is in another window: ask there.
                    if let Some(key) = self.first_dirty_window() {
                        self.ask_in_window(req, key);
                    } else if req == CloseRequest::Quit {
                        self.quit(ctx);
                    }
                    return;
                }
            },
            CloseRequest::Window(key) => match self.first_at_risk() {
                Some(i) if choice.is_some() => i,
                // Cancelled: the window stays as it is.
                _ if choice.is_none() => return,
                // Nothing left to ask about: the window closes.
                _ => {
                    self.request_window_op(crate::multi_window::WindowOp::Close(key));
                    return;
                }
            },
        };
        // Quitting closes the unsaved tabs one by one (and brings each forward to save it):
        // remember what was open, and which tab was active, before the first one goes (#442).
        if req == CloseRequest::Quit {
            match choice {
                Some(_) => self.note_quit_session(),
                None => self.forget_quit_session(),
            }
        }
        match choice {
            None => {} // cancelled: nothing closes
            Some(false) => self.close_and_continue(ctx, index, req),
            Some(true) => {
                let Some(key) = self.win.views.get(index).map(|v| v.key) else { return };
                // Bring the document forward: what's typed in its form is committed and saved
                // with it, and a field that rejects the value is shown where the user can fix it.
                self.win.active = Some(index);
                // Close only once the save has actually been written, which may be on a later
                // frame when the user has to choose where. A failed or cancelled save keeps the
                // document open.
                let ctx = ctx.clone();
                self.save_then(index, SaveTarget::InPlace, move |app| {
                    if let Some(i) = app.view_index(key) {
                        app.close_and_continue(&ctx, i, req);
                    }
                });
            }
        }
    }

    /// Close tab `index` for a close request, then ask about the next dirty document, or quit.
    /// Quitting and Close All answer for the whole document: every tab of it closes (Duplicate
    /// Tab), so its other tabs don't ask again. The save it waited for may have taken a later
    /// frame (the user choosing where), and an OCR review started meanwhile holds the close as it
    /// holds a new one: the document stays open, and Quit or Close all stops there.
    pub(crate) fn close_and_continue(&mut self, ctx: &egui::Context, index: usize, req: CloseRequest) {
        let whole = req == CloseRequest::Quit || req == CloseRequest::All;
        if self.hold_close_for_ocr_review(index) || (whole && self.hold_for_any_ocr_review()) {
            if req == CloseRequest::Quit {
                self.forget_quit_session();
            }
            return;
        }
        match (req, self.win.views.get(index).map(|v| v.id)) {
            (CloseRequest::Quit | CloseRequest::All | CloseRequest::Window(_), Some(id)) => self.close_document(id),
            _ => self.close_tab(index),
        }
        // Close Window: the next document only this window shows asks, or the window closes.
        if let CloseRequest::Window(key) = req {
            if self.close_request.is_none() {
                if self.first_at_risk().is_some() {
                    self.save_prompt_seen = false;
                    self.close_request = Some(req);
                } else {
                    self.request_window_op(crate::multi_window::WindowOp::Close(key));
                }
            }
            return;
        }
        // A newer prompt wins: the user may have started another close while a save was pending,
        // and seen its question, which keeps counting as seen.
        if whole && self.close_request.is_none() {
            match self.first_dirty_window() {
                Some(key) => {
                    // A question about the next document is a new one, in its own window.
                    self.save_prompt_seen = false;
                    self.ask_in_window(req, key);
                }
                None if req == CloseRequest::Quit => self.quit(ctx),
                None => {}
            }
        }
    }

    /// Request a normal application exit, preserving every unsaved document until answered.
    /// Returns false while the existing save/discard/cancel prompt needs a decision, or while an
    /// OCR review is still to be accepted or rejected.
    pub(crate) fn request_quit(&mut self, ctx: &egui::Context) -> bool {
        // Quitting from another window: every tab joins the main window first, which then asks
        // about unsaved work as it always has, so no window's documents are left out.
        if self.drawing_other_window() {
            self.request_window_op(crate::multi_window::WindowOp::Quit);
            return false;
        }
        if self.hold_for_any_ocr_review() {
            ctx.request_repaint();
            false
        } else if let Some(key) = self.first_dirty_window() {
            // The question shows in the window holding the document (T-3); every window and tab
            // stays as it is until it is answered, and a Cancel leaves them so.
            self.ask_in_window(CloseRequest::Quit, key);
            ctx.request_repaint();
            false
        } else {
            self.shutdown_recovery();
            self.quit(ctx);
            true
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_quit = true;
        // An update that waited for this quit installs once the window has closed (UPD.1).
        self.hand_off_update();
        // The main window's close ends the app, whichever window asked.
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Publish whether leaving would lose work, for the browser's reload warning (#812): a
    /// document with unsaved changes, or an OCR review not yet accepted or rejected, which holds
    /// Close and Quit the same way.
    pub(crate) fn sync_unsaved_flag(&self) {
        self.unsaved_flag.store(self.work_at_risk(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether leaving would lose work: a document with unsaved changes, or an OCR review not yet
    /// accepted or rejected. The browser's reload warning and the embedding page (`host_dirty`)
    /// both hear this.
    pub(crate) fn work_at_risk(&self) -> bool {
        self.first_dirty().is_some() || self.ocr_review_index().is_some()
    }

    /// Intercept window close while documents have unsaved changes or an OCR review is open.
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        // egui's default quit shortcut ignores extra Alt/Shift, including Ctrl+Alt+Q for '@'.
        ctx.options_mut(|o| o.quit_shortcuts.clear());
        let shortcut = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: egui::Key::Q, pressed: true, modifiers, .. }
                    if modifiers.matches_exact(egui::Modifiers::COMMAND))
            })
        });
        if shortcut {
            self.request_quit(ctx);
        }
        self.hold_close_for_unsaved(ctx);
    }

    /// The window's close: held back with the "save changes?" question while something is
    /// unsaved. Also run after a frame whose logic panicked before `guard_quit` (#3914).
    pub(crate) fn hold_close_for_unsaved(&mut self, ctx: &egui::Context) {
        self.sync_unsaved_flag();
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // With other windows open, closing the first one closes that window only: its documents
        // ask, then another window takes its place (TAB.WINDOW.1 H1). The app keeps running.
        if !self.allow_quit && !self.windows.others.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_window_op(crate::multi_window::WindowOp::Close(None));
            return;
        }
        if !self.allow_quit && self.hold_for_any_ocr_review() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        } else if !self.allow_quit
            && let Some(key) = self.first_dirty_window()
        {
            // `first_dirty` counts text typed into a field as an unsaved change.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.ask_in_window(CloseRequest::Quit, key);
        } else {
            // A clean quit: nothing is left to recover.
            self.shutdown_recovery();
        }
    }

    /// Ask for the user's attention when the "save changes?" question opens while PdfCraft is in
    /// the background (Quit in the Dock menu, logging out, or an agent quitting), and withdraw the
    /// request once the user comes back or the question is answered.
    ///
    /// The question holds up the quit the user asked for, so the request is critical, as AppKit
    /// makes it for a modal panel that an inactive app presents: the Dock icon bounces until
    /// PdfCraft is active, Windows flashes the taskbar button until the window is in front, and
    /// X11 sets the urgency hint, which stays until it is reset. A question that opens in front
    /// asks nothing, and neither does leaving it open: the user has seen it. Each document asked
    /// about when quitting is a new question, so one that opens in the background asks even after
    /// a question the user saw; a request that still stands carries over to it.
    pub(crate) fn attend_save_prompt(&mut self, ctx: &egui::Context) {
        let open = self.close_request.is_some();
        let focused = ctx.input(|i| i.focused);
        if self.save_prompt_attention {
            if focused || !open {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Reset));
                self.save_prompt_attention = false;
            }
        } else if open && !self.save_prompt_seen && !focused {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
            self.save_prompt_attention = true;
        }
        self.save_prompt_seen = open;
    }
}

//! This fork (sefr-14/pdfcraft, COMMENTS.1): Delete selected, confirmed first.
//!
//! The confirmation states what the engine's dry run says the deletion removes, and holds the
//! targets and the document version they were taken from. Confirming in another version of the
//! document (an edit, undo, a save, another tab's change) deletes nothing; the engine checks
//! every target's slot again besides. Nothing here scrolls or turns the page.

use pdfcraft_engine::{DeleteTarget, DocId, Edit};

use crate::PdfCraftApp;
use crate::theme::Tokens;

/// What Delete selected is waiting to delete.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentDeleteDraft {
    pub doc: DocId,
    pub targets: Vec<DeleteTarget>,
    pub comments: usize,
    pub replies: usize,
    /// The document version the targets and counts were taken in.
    pub seen: (u64, usize),
}

impl PdfCraftApp {
    /// Delete selected: work out what goes and ask first. Refused (the reason as a notice, no
    /// question) when the engine wouldn't delete them.
    pub fn ask_delete_comments(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(view) = self.views.get(i) else { return };
        let targets = view.comments.group.targets(&doc.info.annotations);
        if targets.is_empty() {
            return;
        }
        match doc.comments_deletion_preview(&targets) {
            Ok((comments, replies)) => {
                let seen = crate::content_version(doc);
                self.comment_delete = Some(CommentDeleteDraft { doc: id, targets, comments, replies, seen });
                self.dialog = Some(crate::Dialog::DeleteComments);
            }
            Err(e) => self.notify_fmt("Couldn't delete the comments: {error}", &[("error", &e.to_string())]),
        }
    }

    /// Delete selected, confirmed: delete what was confirmed, unless the document changed since.
    pub fn confirm_delete_comments(&mut self) {
        let Some(draft) = self.comment_delete.take() else { return };
        let Some((i, id)) = self.active_ids() else { return };
        let now = self.session.get(id).map(crate::content_version);
        if id != draft.doc || now != Some(draft.seen) {
            self.notify_tr("The comments changed. Review the selection and try again.");
            return;
        }
        if self.apply_edit(Edit::DeleteAnnotations { targets: draft.targets })
            && let Some(view) = self.views.get_mut(i)
        {
            view.comments.group.clear();
        }
    }
}

/// The confirmation. Returns (delete, cancel).
pub(crate) fn body(ui: &mut egui::Ui, draft: &CommentDeleteDraft, t: &Tokens) -> (bool, bool) {
    ui.set_width(420.0);
    ui.label(egui::RichText::new(tl!("Delete comments")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    let comments = crate::i18n::trn(crate::i18n::current(), draft.comments as u64, "{n} comment", "{n} comments");
    let replies = crate::i18n::trn(crate::i18n::current(), draft.replies as u64, "{n} reply", "{n} replies");
    ui.label(crate::i18n::fmt(tl!("Delete {comments} and {replies}?"), &[("comments", &comments), ("replies", &replies)]));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Their pop-ups go with them. You can undo this.")).small().color(t.text_muted));
    ui.add_space(12.0);
    let (mut delete, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if crate::widgets::pill_button(ui, tl!("Delete"), true).clicked() {
            delete = true;
        }
        if crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (delete, cancel)
}

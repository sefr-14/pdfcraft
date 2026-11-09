// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! The crate's integration tests, linked into one test binary.

mod action_failures;
mod attachment_key;
mod automation;
mod automation_roots;
mod backdrop;
mod batch;
mod batch_failures;
mod batch_secrets;
// This fork (BOOKMARK.1).
mod bookmark_shift;
mod button_caption;
// This fork (COMMENTS.1).
mod comment_delete_many;
mod form_bulk;
mod hostile_args;
mod model_3d;
mod model_render;
mod optimize_content_boundaries;
mod redact_after_stray_q_form;
mod redact_text_after_q;
mod render_warnings;

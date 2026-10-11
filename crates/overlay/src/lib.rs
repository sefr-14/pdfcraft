//! pdfcraft-overlay — overlay comparison (L5, this fork). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

mod error;
pub mod frame;
pub mod register;
pub mod settings;
pub mod sheet;

pub use error::{OverlayError, Warning};
pub use frame::{DisplayFrame, frame, frame_of, frame_of_page};
pub use register::{Affine, Doc, Fit, Flag, Limits, Model, Point, PointPair, PointReport, RANGE_SLACK, Refusal, Similarity, fit, report, residuals};
pub use settings::Settings;
pub use sheet::{SheetCase, SheetMatch, SheetProposal, propose};

/// Points per millimetre (72 / 25.4).
pub const PT_PER_MM: f64 = 72.0 / 25.4;

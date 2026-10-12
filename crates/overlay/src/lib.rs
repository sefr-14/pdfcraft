//! pdfcraft-overlay — overlay comparison (L5, this fork). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod composite;
mod error;
pub mod frame;
pub mod place;
pub mod register;
pub mod settings;
pub mod sheet;

pub use composite::{CYAN, RED, Side, Source, Style, compose, compose_page, light_tone, luminosity, transfer_function};
pub use error::{OverlayError, Warning};
pub use frame::{DisplayFrame, frame, frame_of, frame_of_page};
pub use place::{Placement, place};
pub use register::{
    Affine, Doc, Fit, Flag, Limits, Model, Point, PointPair, PointReport, RANGE_SLACK, Refusal, Similarity, Verdict, fit, report, residuals, verdict,
};
pub use settings::Settings;
pub use sheet::{SheetCase, SheetMatch, SheetProposal, propose};

/// Points per millimetre (72 / 25.4).
pub const PT_PER_MM: f64 = 72.0 / 25.4;

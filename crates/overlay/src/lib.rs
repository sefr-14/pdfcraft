//! pdfcraft-overlay — overlay comparison (L5, this fork). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod annotations;
pub mod colour;
pub mod composite;
mod error;
pub mod frame;
pub mod job;
pub mod layers;
pub mod oc;
pub mod pairing;
pub mod place;
pub mod register;
pub mod report;
pub mod settings;
pub mod sheet;
pub mod snap;

pub use annotations::{AnnotationEntry, AppearanceSource, Exclusion, Outcome, SourceWarning};
pub use composite::{CYAN, RED, Side, Source, Style, compose, compose_page, light_tone, luminosity, transfer_function};
pub use error::{OverlayError, Warning};
pub use frame::{DisplayFrame, frame, frame_of, frame_of_page};
pub use job::{Generated, PairPlan, Plan, generate, pair_frames, pair_placement, snapshot_decisions};
pub use layers::{LayerGroups, LayerStyle, LayeredOutput, RegistrationKind, RegistrationNote, Revision, Roles, check_output, compose_layered};
pub use oc::prepare::{PageReport, Prepared, PreparedSource, prepare};
pub use oc::{Rule, Unresolved, Visibility};
pub use pairing::{Cell, Pair, Pairing, PairingError, Slot, Summary};
/// The matrix type of placements and registrations, for callers without `pdfcraft-content`.
pub use pdfcraft_content::Matrix;
pub use place::{Placement, place};
pub use register::{
    Affine, Doc, Fit, Flag, Limits, Model, Point, PointPair, PointReport, RANGE_SLACK, Refusal, Similarity, Verdict, fit, report, residuals, verdict,
};
pub use report::{DocumentReport, OverlayReport, PageEntry};
pub use settings::Settings;
pub use sheet::{SheetCase, SheetMatch, SheetProposal, propose};

/// Points per millimetre (72 / 25.4).
pub const PT_PER_MM: f64 = 72.0 / 25.4;

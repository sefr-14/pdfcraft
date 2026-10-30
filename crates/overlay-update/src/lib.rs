//! pdfcraft-overlay-update — automatic updates of the PDF Overlay Windows portable build (this
//! fork, UPD.1).
//!
//! The owner approves a build by signing a *channel envelope* (one JSON object holding the exact
//! manifest bytes and an Ed25519 signature over them, [`envelope`]). The app checks the envelope
//! against keys compiled into it, decides whether the signed build is an update for this copy
//! ([`eligibility`]), and offers it; the user postpones it ([`reminder`]) or installs it. The
//! install itself ([`archive`], [`transaction`], [`helper`]) runs in an external helper process
//! after the app has quit: it backs up every application-owned file, replaces it, launches the new
//! build, waits for it to confirm that it started, and otherwise restores the previous files,
//! including removing files the update introduced. A journal makes every step recoverable.
//!
//! This crate has no network and no UI code: the app fetches bytes, the UI shows offers.
//! See `docs/updates.md` for the design (the UPD.1 PRD).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod eligibility;
pub mod envelope;
mod error;
pub mod manifest;
pub mod paths;
pub mod reminder;

#[cfg(feature = "install")]
pub mod archive;
#[cfg(feature = "install")]
pub mod hash;
#[cfg(feature = "install")]
pub mod helper;
#[cfg(feature = "install")]
pub mod journal;
#[cfg(feature = "install")]
pub mod ops;
#[cfg(feature = "install")]
pub mod transaction;

pub use error::Error;

/// The product a manifest must name.
pub const PRODUCT: &str = "pdf-overlay";
/// The only platform this updater installs.
pub const PLATFORM: &str = "windows-x86_64";
/// The only package kind this updater installs.
pub const PACKAGE_KIND: &str = "portable-zip";
/// The update channel this build follows.
pub const CHANNEL: &str = "stable";
/// The repository every approved build must come from (never upstream's).
pub const SOURCE_REPOSITORY: &str = "sefr-14/pdfcraft";
/// The manifest schema this updater reads.
pub const MANIFEST_SCHEMA: u32 = 1;
/// The envelope format this updater reads.
pub const ENVELOPE_VERSION: u32 = 1;
/// This updater's own schema; a manifest may require a newer one (`min_updater_schema`).
pub const UPDATER_SCHEMA: u32 = 1;

/// What a check found, as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// The approved build is the one running (or not newer than the installed one).
    UpToDate {
        /// The approved build's distribution version, such as `0.15.0-ov.7`.
        distribution_version: String,
        /// The highest approved sequence seen in a valid envelope.
        seen_seq: u64,
    },
    /// A newer approved build.
    Available(Offer),
}

impl Check {
    /// The approved sequence of the valid envelope the check read.
    pub fn seen_seq(&self) -> u64 {
        match self {
            Check::UpToDate { seen_seq, .. } => *seen_seq,
            Check::Available(o) => o.seq,
        }
    }
}

/// An approved update, ready to be offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The approved distribution sequence (orders updates; never goes down).
    pub seq: u64,
    /// Such as `0.15.0-ov.7`.
    pub distribution_version: String,
    /// The full source commit.
    pub commit: String,
    /// When the owner approved it.
    pub approved_at: chrono::DateTime<chrono::Utc>,
    /// The package size in bytes (for the progress bar).
    pub size: u64,
    /// Release notes in English and Spanish, if any.
    pub notes_en: Option<String>,
    pub notes_es: Option<String>,
    /// The verified envelope, exactly as received: the installer verifies it again.
    pub envelope: String,
}

/// What the update source needs from the app's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckContext {
    /// The highest approved sequence any earlier valid envelope carried (replay protection).
    pub highest_seen_seq: u64,
}

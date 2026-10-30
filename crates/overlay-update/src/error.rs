//! Why an update was refused or failed. The text is shown to the user (after "Couldn't update: ")
//! and written to the log; it never holds secrets.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The channel envelope isn't the expected JSON object.
    Envelope(String),
    /// This build has no key to check signatures with (none has been compiled in yet).
    NoTrustedKey,
    /// The envelope names a key this build doesn't know.
    UnknownKey(String),
    /// The signature doesn't match the manifest.
    BadSignature,
    /// The manifest isn't the expected JSON.
    Manifest(String),
    /// A signed manifest for another product, platform, package kind, channel or repository.
    NotForThisApp(String),
    /// A manifest or envelope newer than this updater understands.
    Unsupported(String),
    /// The approval has expired.
    Expired,
    /// The approval date lies in the future (beyond the clock-skew allowance).
    NotYetValid,
    /// The signed sequence is lower than one already installed or seen.
    Downgrade { offered: u64, floor: u64 },
    /// A download address outside the approved location.
    Url(String),
    /// The downloaded package doesn't match the signed size or hash.
    Package(String),
    /// The archive has an entry the updater refuses, or differs from the signed file list.
    Archive(String),
    /// A file operation failed.
    Io(String),
    /// Another update, or another program using the installation, is in the way.
    Busy(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Envelope(e) => write!(f, "the update information is malformed ({e})"),
            Error::NoTrustedKey => f.write_str("this build has no update signing key, so it can't verify updates"),
            Error::UnknownKey(k) => write!(f, "the update is signed with a key this build doesn't trust ({k})"),
            Error::BadSignature => f.write_str("the update's signature is invalid"),
            Error::Manifest(e) => write!(f, "the update manifest is malformed ({e})"),
            Error::NotForThisApp(e) => write!(f, "the update is not for this application ({e})"),
            Error::Unsupported(e) => write!(f, "the update needs a newer updater ({e})"),
            Error::Expired => f.write_str("the update approval has expired"),
            Error::NotYetValid => f.write_str("the update approval date is in the future; check the computer's clock"),
            Error::Downgrade { offered, floor } => {
                write!(f, "the offered update (sequence {offered}) is older than one already seen (sequence {floor})")
            }
            Error::Url(e) => write!(f, "the update download address isn't approved ({e})"),
            Error::Package(e) => write!(f, "the downloaded package failed verification ({e})"),
            Error::Archive(e) => write!(f, "the update package was refused ({e})"),
            Error::Io(e) => f.write_str(e),
            Error::Busy(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// An I/O failure with what was being done.
    pub fn io(doing: &str, path: &std::path::Path, e: &std::io::Error) -> Error {
        Error::Io(format!("{doing} {}: {e}", path.display()))
    }
}

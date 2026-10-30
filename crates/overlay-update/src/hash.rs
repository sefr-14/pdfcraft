//! SHA-256 of bytes, files and streams, as lowercase hex.

use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::Error;

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a String never fails.
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// SHA-256 of a regular file, streamed.
pub fn file(path: &Path) -> io::Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(hex(&h.finalize()))
}

/// SHA-256 of a file, `None` when nothing is at `path`. A folder or link there is an error.
pub fn file_opt(path: &Path) -> io::Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
        Ok(_) => file(path).map(Some),
    }
}

/// Copy at most `expected` bytes from `from` to `to`, hashing them; fails when the stream holds
/// more or fewer bytes than `expected` or its hash isn't `sha256`. `progress` gets the running
/// total. Reading never goes past `expected + 1` bytes, whatever the stream claims.
pub fn copy_verified(
    from: &mut dyn Read,
    to: &mut dyn Write,
    expected: u64,
    sha256: &str,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<(), Error> {
    let mut h = Sha256::new();
    let mut limited = from.take(expected.saturating_add(1));
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = limited.read(&mut buf).map_err(|e| Error::Package(format!("download interrupted: {e}")))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        total = total.saturating_add(n as u64);
        if total > expected {
            return Err(Error::Package(format!("larger than the signed size of {expected} bytes")));
        }
        h.update(chunk);
        to.write_all(chunk).map_err(|e| Error::Io(format!("couldn't write the download: {e}")))?;
        if !progress(total) {
            return Err(Error::Busy("the update was cancelled".into()));
        }
    }
    if total != expected {
        return Err(Error::Package(format!("{total} of {expected} bytes received")));
    }
    let got = hex(&h.finalize());
    if got != sha256 {
        return Err(Error::Package("SHA-256 doesn't match the signed value".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn known_digest() {
        assert_eq!(sha256(b"abc"), ABC);
    }

    #[test]
    fn copies_are_checked_for_size_and_hash() {
        let mut out = Vec::new();
        copy_verified(&mut &b"abc"[..], &mut out, 3, ABC, &mut |_| true).unwrap();
        assert_eq!(out, b"abc");
        for (data, size) in [(&b"ab"[..], 3), (&b"abcd"[..], 3)] {
            assert!(matches!(copy_verified(&mut &data[..], &mut Vec::new(), size, ABC, &mut |_| true), Err(Error::Package(_))));
        }
        assert!(matches!(copy_verified(&mut &b"abd"[..], &mut Vec::new(), 3, ABC, &mut |_| true), Err(Error::Package(_))));
        assert!(matches!(copy_verified(&mut &b"abc"[..], &mut Vec::new(), 3, ABC, &mut |_| false), Err(Error::Busy(_))));
    }

    #[test]
    fn an_endless_stream_stops_after_the_signed_size() {
        let mut endless = std::io::repeat(b'x');
        let r = copy_verified(&mut endless, &mut std::io::sink(), 1000, ABC, &mut |_| true);
        assert!(matches!(r, Err(Error::Package(_))));
    }
}

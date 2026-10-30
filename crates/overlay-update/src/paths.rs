//! Which paths a package may hold and which installed files an update may replace.
//!
//! Three categories, relative to the installation folder (the folder of `pdfcraft.exe`):
//! - **A, allowed archive contents** ([`is_allowed`]): what `overlay-windows.yml` packs. Anything
//!   else refuses the whole package.
//! - **B, replaceable application files** ([`is_replaceable`]): A without the portable-mode
//!   markers. Only these are backed up, replaced or (when introduced and then rolled back) removed.
//! - **C, protected**: everything else, above all `PdfCraftData` and the portable markers
//!   `portable.txt` / `PdfCraft.portable`, whose presence and contents an update never changes
//!   (that would move the user's settings between the portable folder and the per-user one).

use crate::Error;

/// The longest relative path accepted (bytes).
pub const MAX_PATH: usize = 200;
/// The deepest relative path accepted (components).
pub const MAX_DEPTH: usize = 8;
/// The longest single component (bytes).
const MAX_COMPONENT: usize = 100;

/// Files beside the executable that switch on portable mode (`crates/ui-egui/src/platform/portable.rs`).
pub const PORTABLE_MARKERS: [&str; 2] = ["portable.txt", "PdfCraft.portable"];
/// The portable data folder, never touched.
pub const DATA_DIR: &str = "PdfCraftData";
/// The application executable, replaced last.
pub const APP_EXE: &str = "pdfcraft.exe";
/// The command-line executable.
pub const CLI_EXE: &str = "pdfcraft-cli.exe";
/// The updater's own folder inside the installation.
pub const UPDATE_DIR: &str = ".pdf-overlay-update";

/// Files a package may hold at its root.
const ROOT_FILES: [&str; 9] = [
    APP_EXE,
    CLI_EXE,
    "portable.txt",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "NOTICE",
    "README.md",
    "LICENSE-document.txt",
    "pdfcraft-document-orange.ico",
];

/// Check the form of a relative package path: `/` separators, no traversal, nothing Windows would
/// read as something else (drives, UNC, alternate data streams, device names, trailing dots).
pub fn check_relative(path: &str) -> Result<(), Error> {
    let refuse = |why: &str| Err(Error::Archive(format!("{path:?}: {why}")));
    if path.is_empty() || path.len() > MAX_PATH {
        return refuse("empty or too long");
    }
    if path.starts_with('/') || path.ends_with('/') {
        return refuse("not a relative file path");
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return refuse("too deep");
    }
    for part in parts {
        if part.is_empty() || part == "." || part == ".." {
            return refuse("empty, '.' or '..' component");
        }
        if part.len() > MAX_COMPONENT {
            return refuse("component too long");
        }
        if part.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')) {
            return refuse("character Windows reserves");
        }
        if part.ends_with('.') || part.ends_with(' ') || part.starts_with(' ') {
            return refuse("leading space or trailing dot or space");
        }
        if is_device_name(part) {
            return refuse("reserved device name");
        }
    }
    Ok(())
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM0-9`, `LPT0-9`, `CONIN$`, `CONOUT$`, with or without an extension.
fn is_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end().to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let digit = |s: &str| s.len() == 1 && s.bytes().all(|c| c.is_ascii_digit());
    stem.strip_prefix("COM").is_some_and(digit) || stem.strip_prefix("LPT").is_some_and(digit)
}

/// A plain file name: letters, digits, `.`, `_`, `-`, not starting with `.`.
fn plain_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// A licence text: `.txt` or `.md`, or the one extension-less file the package carries.
fn licence_name(s: &str) -> bool {
    plain_name(s) && (s.ends_with(".txt") || s.ends_with(".md") || s == "LICENSE_FOXIT")
}

/// Category A: a path the package may hold (after [`check_relative`]).
pub fn is_allowed(path: &str) -> bool {
    if ROOT_FILES.contains(&path) {
        return true;
    }
    match path.split('/').collect::<Vec<_>>().as_slice() {
        ["licenses", name] => licence_name(name),
        ["licenses", "epaint_default_fonts", name] => plain_name(name) && name.ends_with(".txt"),
        _ => false,
    }
}

/// Category B: a path an update may back up, replace and roll back.
pub fn is_replaceable(path: &str) -> bool {
    is_allowed(path) && !PORTABLE_MARKERS.iter().any(|m| m.eq_ignore_ascii_case(path))
}

/// The order files are installed in: documentation and licences first, then the command-line
/// tool, and the application last, so the old application stays runnable as long as possible.
pub fn install_rank(path: &str) -> u8 {
    match path {
        APP_EXE => 2,
        CLI_EXE => 1,
        _ => 0,
    }
}

/// The same path for Windows' case-insensitive file systems (to find duplicates).
pub fn fold(path: &str) -> String {
    path.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portable_package_layout_is_allowed() {
        for p in [
            "pdfcraft.exe",
            "pdfcraft-cli.exe",
            "portable.txt",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "NOTICE",
            "README.md",
            "LICENSE-document.txt",
            "pdfcraft-document-orange.ico",
            "licenses/README.txt",
            "licenses/THIRD-PARTY-CRATES.txt",
            "licenses/ATTRIBUTION.md",
            "licenses/LICENSE_FOXIT",
            "licenses/CGATS_LICENSE.txt",
            "licenses/LICENSE-adobe-cmaps.txt",
            "licenses/epaint_default_fonts/OFL.txt",
            "licenses/epaint_default_fonts/emoji-icon-font-mit-license.txt",
        ] {
            assert_eq!(check_relative(p), Ok(()), "{p}");
            assert!(is_allowed(p), "{p}");
        }
    }

    #[test]
    fn anything_else_is_not() {
        for p in [
            "other.exe",
            "PdfCraftData/app.ron",
            "licenses/tool.exe",
            "licenses/LICENSE_FOXIT.exe",
            "licenses/sub/x.txt",
            "licenses/epaint_default_fonts/font.ttf",
            "PDFCRAFT.EXE",
            "update-receipt.json",
            ".pdf-overlay-update/x",
            "pdfcraft.dll",
        ] {
            assert!(!is_allowed(p), "{p}");
        }
    }

    #[test]
    fn portable_markers_are_never_replaceable() {
        assert!(is_allowed("portable.txt"));
        assert!(!is_replaceable("portable.txt"));
        assert!(!is_replaceable("PdfCraft.portable"));
        assert!(!is_allowed("PdfCraft.portable"));
        assert!(is_replaceable("pdfcraft.exe"));
        assert!(is_replaceable("licenses/README.txt"));
    }

    #[test]
    fn dangerous_paths_are_refused() {
        for p in [
            "",
            "/pdfcraft.exe",
            "../pdfcraft.exe",
            "licenses/../../x.txt",
            "./NOTICE",
            "licenses//README.txt",
            "C:/Windows/x.txt",
            "C:x.txt",
            "\\\\server\\share\\x.txt",
            "licenses\\README.txt",
            "NOTICE:stream",
            "CON",
            "nul.txt",
            "licenses/COM1.txt",
            "licenses/lpt9",
            "CONOUT$",
            "NOTICE.",
            "NOTICE ",
            " NOTICE",
            "a\u{0}b",
            "licenses/",
            "a/b/c/d/e/f/g/h/i",
        ] {
            assert!(check_relative(p).is_err(), "{p:?}");
        }
        assert!(check_relative(&"a".repeat(MAX_PATH + 1)).is_err());
        assert!(check_relative(&"a".repeat(MAX_COMPONENT + 1)).is_err());
        // Device names only when they are the whole stem.
        assert_eq!(check_relative("CONSOLE.txt"), Ok(()));
        assert_eq!(check_relative("COM10.txt"), Ok(()));
    }

    #[test]
    fn the_application_is_installed_last() {
        let mut v = ["pdfcraft.exe", "NOTICE", "pdfcraft-cli.exe", "licenses/README.txt"];
        v.sort_by_key(|p| install_rank(p));
        assert_eq!(v.last(), Some(&"pdfcraft.exe"));
        assert_eq!(v.get(2), Some(&"pdfcraft-cli.exe"));
    }
}

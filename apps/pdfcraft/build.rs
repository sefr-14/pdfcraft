// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Windows only: embed the app icon and version info (VERSIONINFO) into `pdfcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab. A separate PDF document icon
//! uses resource ID 2; the installer references it as `pdfcraft.exe,-2`.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `PDFCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

#[path = "src/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/pdfcraft-document.ico");
    println!("cargo:rerun-if-changed=src/windows_manifest.rs");
    println!("cargo:rerun-if-env-changed=PDFCRAFT_REQUIRE_WINRES");
    build_identity();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/pdfcraft.ico")
        .set_icon_with_id("../../assets/app-icon/pdfcraft-document.ico", "2")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        // This fork's name (LIC.1, BRAND.1); OriginalFilename, InternalName and the app id stay upstream's.
        .set("ProductName", "PDF Overlay")
        .set("FileDescription", "PDF Overlay (unofficial fork of PdfCraft)")
        .set("LegalCopyright", "Copyright (c) the PdfCraft contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "pdfcraft.exe")
        .set("InternalName", "pdfcraft");
    if let Err(e) = res.compile() {
        if std::env::var_os("PDFCRAFT_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=pdfcraft.exe built without icon/version resources: {e}");
    }
}

/// The build identity updates compare (UPD.1): the commit of a build of this fork. GitHub Actions
/// sets `GITHUB_SHA`, `GITHUB_RUN_ID` and `GITHUB_REPOSITORY` for every step, so the Windows
/// workflow needs no change; only builds of `sefr-14/pdfcraft` get one. A controlled local test
/// build may name its commit with `PDF_OVERLAY_BUILD_COMMIT`. Without one, the build is a
/// development build and never installs updates.
fn build_identity() {
    for var in ["GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_REPOSITORY", "PDF_OVERLAY_BUILD_COMMIT"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_ascii_lowercase());
    let commit = |c: String| (c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit())).then_some(c);
    let found = env("PDF_OVERLAY_BUILD_COMMIT")
        .and_then(commit)
        .or_else(|| (env("GITHUB_REPOSITORY").as_deref() == Some("sefr-14/pdfcraft")).then(|| env("GITHUB_SHA")).flatten().and_then(commit));
    if let Some(c) = found {
        println!("cargo:rustc-env=PDF_OVERLAY_COMMIT={c}");
    }
    if let Some(run) = env("GITHUB_RUN_ID").filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit())) {
        println!("cargo:rustc-env=PDF_OVERLAY_RUN_ID={run}");
    }
}

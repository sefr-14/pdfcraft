# PDF Overlay: what this fork changes

PDF Overlay ([sefr-14/pdfcraft](https://github.com/sefr-14/pdfcraft)) is an unofficial fork of
[PdfCraft](https://github.com/storytold/pdfcraft) by the ArtCraft team, based on upstream release
`v0.14.0` (`5a78b22`). This page summarizes what the fork adds and changes, by area. The Git history
is the detailed record of every change, and upstream files the fork has changed say so in a line
near their top (see the fork section of [NOTICE](NOTICE)). The README's
[About this fork](README.md#about-this-fork) describes each feature in full.

This is documentation only. It is not a licence document and does not by itself meet any licence's
notice requirement.

## Features

| Area | What changes | Where |
|---|---|---|
| Overlay compare | Six-step wizard that pairs, aligns and overlays two revisions of a drawing set into one new PDF, with the revisions in two layers (Old, New); resolves the sources' own layers and annotations first | `crates/overlay` (new), `crates/ui-egui/src/overlay_ui.rs` (new) |
| Duplicate Tab | A second tab on the same document, navigated independently | `crates/ui-egui` |
| Tab View and Tile View | All open tabs side by side; navigation follows the pointer | `crates/ui-egui` |
| Thin lines | View › Page display › Thin lines draws every stroke one device pixel wide | `vendor/hayro` (patch), `crates/render`, `crates/ui-egui` |
| Measuring drawings | Engineering units (feet and inches), calibration and known distance, snapping, measurements kept as annotations | `crates/measure`, `crates/automation`, `crates/ui-egui/src/measure_ui.rs` |
| Rulers and guides | Rulers along the page view and guides on the pages, remembered per PDF | `crates/ui-egui` |
| Drawing and Find (OV12) | Shift keeps drawn lines level or plumb; Find's Next/Previous centre the match | `crates/ui-egui` |
| Check for updates | Help › Check for updates finds this fork's latest Windows build (an Actions artifact), not upstream releases | `crates/ui-egui/src/updates.rs`, `apps/pdfcraft/src/updates.rs` |
| Translations | The fork's own labels are in English and Spanish only | `crates/ui-egui/src/i18n/es.tsv` ("This fork" sections) |

## Identity, licensing and branding (LIC.1)

| What changes | Where |
|---|---|
| The visible name is **PDF Overlay** (PdfCraft Overlay until BRAND.1), in every language: window title, Home and About headings, Help › About, and the Windows version information of both executables. The app id, settings and data folders, and the portable data folder keep upstream's names. | `apps/pdfcraft`, `apps/pdfcraft-cli`, `crates/ui-egui`, `crates/engine/src/commands.rs` |
| No Discord button. Help and About open this fork's issues and upstream's repository instead of the ArtCraft community, which does not support the fork. | `crates/engine/src/links.rs`, `crates/ui-egui/src/chrome.rs`, `crates/ui-egui/src/widgets.rs` |
| The ArtCraft marks are removed from the app, the README, the screenshots and `docs/brand/`, as their licence requires for published modified versions. Upstream's credit stays, in plain text. | `docs/brand/`, `ATTRIBUTION.toml`, `README.md`, `NOTICE` |
| The Windows build ships the full third-party licence texts in a `licenses` folder. | `licenses/`, `about.toml`, `.github/workflows/overlay-windows.yml` |

## Repository and automation

| What changes | Where |
|---|---|
| Windows x64 builds on request, as Actions artifacts, never releases | `.github/workflows/overlay-windows.yml` (new) |
| GitHub pull requests are bridged to the authoritative Forgejo repository, and upstream is trial-merged daily for review | `.github/workflows/forgejo-*.yml`, `scripts/automation` (new) |
| Upstream workflows the fork doesn't use are removed or run only on demand | `.github/workflows/` |
| Selected upstream security and robustness fixes from after `v0.14.0` are backported (SYNC.12) | see the Git history |

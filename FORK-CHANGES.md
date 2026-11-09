# PDF Overlay: what this fork changes

PDF Overlay ([sefr-14/pdfcraft](https://github.com/sefr-14/pdfcraft)) is an unofficial fork of
[PdfCraft](https://github.com/storytold/pdfcraft) by the ArtCraft team, based on upstream release
`v0.15.0` (`75360d1`). This page summarizes what the fork adds and changes, by area. The Git history
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
| Thin lines | View › Page display › Thin lines (also Page display in the view menu) draws every stroke one device pixel wide | `vendor/hayro` (patch), `crates/render`, `crates/ui-egui` |
| Measuring drawings | Engineering units (feet and inches), calibration, snapping, measurements kept as annotations; the drawing scale as one equation (Drawing distance = Actual distance), with Metric (1:1 to 1:5000) and Imperial (12" = 1'-0" to 1" = 400'-0") common scales saved as a plan writes them (MEASURE.1, MEASURE.2); Apply scale drawn as the panel's primary action, in the accent with white text (MEASURE.UI.1) | `crates/measure`, `crates/automation`, `crates/ui-egui/src/tools/measure_ui.rs` |
| Rulers and guides | Rulers along the page view and guides on the pages, remembered per PDF; View › Page display › Rulers and Guides | `crates/ui-egui/src/rulers.rs`, `guides.rs` |
| Drawing and Find (OV12) | Shift keeps drawn lines level or plumb; Find's Next/Previous centre the match | `crates/ui-egui` |
| Reduce File Size and Optimize PDF | Save the result like Save As: the same tab becomes the optimized file, unsaved changes included; never over another open document, nor over edits made during the run (COMPRESS.1) | `crates/ui-egui/src/workflows/create_ui.rs`, `workflows/optimize_ui.rs`, `editing/save.rs` |
| Save and Save As | Never write over a file another tab has open, including the same file under another name (symlink, letter case); a Duplicate Tab is the same document (SAVEAS.1) | `crates/ui-egui/src/editing/save.rs` |
| Check for updates | Help › Check for updates offers the latest owner-approved Windows build (a signed channel envelope in this fork's Forgejo package registry) and installs it on Update and Restart: verified download, the usual save questions, an external helper that replaces only program files, a confirmed restart or a complete rollback; Remind me later for a day, a week or a month (UPD.1, [docs/updates.md](docs/updates.md)). Upstream's `craft-update` stays out (SYNC.13) | `crates/overlay-update` (new), `apps/pdfcraft/src/updates/`, `crates/ui-egui/src/platform/updates.rs`, `xtask/src/overlay_publish.rs` |
| Tool keys | H Highlight, P Hand (was H), V Select, and the pairs D I distance, D L line, R C revision cloud, typed one after the other; never while typing (KEY.1) | `crates/ui-egui/src/tool_keys.rs` |
| Comments | Upstream's own multi-selection (#3484, since SYNC.16) is on trial as the only one: Ctrl/⌘- and Shift-click on the page and in the Comments panel, Delete and Backspace delete them all as one undo step, group moves, XFDF copy and paste. The fork keeps Collapse and Expand selected (the panel's "…" menu), `comment_delete_many` with its dry run, and a guard: a selection made before the document changed elsewhere (another tab or tile, automation) or before a page move is dropped, never applied to the comments now at its places (SYNC.16h). COMMENTS.1's own panel selection and its confirmed Delete selected are kept, not compiled, as a fallback (see below) | `crates/ui-egui/src/panels/comments_panel.rs`, `tools/comments/state.rs`, `tools/comments/comments1_fallback/` (inactive), `crates/annot/src/bulk.rs`, `crates/engine` |
| Bookmarks | Select several bookmarks (click, Ctrl+Click, Shift+Click, across levels) and Indent or Outdent them together, as one undo step, refused whole when any of them can't move; `bookmark_indent` and `bookmark_outdent` do the same (BOOKMARK.1); upstream's Cut, Copy and Paste take one bookmark and its subtree, and show disabled for a group (SYNC.16) | `crates/organize/src/outline/shift.rs` (new), `crates/ui-egui/src/panels/bookmarks.rs`, `crates/engine`, `crates/automation` |
| Home | Recommends Organize pages, Combine files, Compare files, Edit a PDF and Protect a PDF (HOME.1); the Combine files card opens the Combine tab directly, keeping a list already open (COMBINE.1) | `crates/ui-egui/src/home.rs` |
| Menus | File › Overlay compare next to Combine; Window › Duplicate tab, Tab view and Tile view; Help lists Keyboard shortcuts, Report a problem, Upstream PdfCraft on GitHub, Check for updates and About | `crates/ui-egui/src/menus.rs` |
| Translations | Upstream's catalogues come from storytold/craft-i18n at `craft-i18n.rev` (since SYNC.16); the fork's own rows, as its in-repo catalogues had them (its labels in English and Spanish, many in other languages too), are merged over them by `build.rs`, which also gives every "PDF Overlay" string the translation of upstream's "PdfCraft" one. The Windows build fails without the translations | `crates/ui-egui/src/i18n/overlay/` (new), `crates/ui-egui/build.rs`, `.github/workflows/overlay-windows.yml` |

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
| Upstream releases come in by true merge of the release tag (`v0.15.0` in SYNC.13, `v0.18.0` in SYNC.15, `v0.19.0` in SYNC.16, resolved against `v0.18.0` as the base); the SYNC.12 and SYNC.14 backports are part of them | see the Git history |
| Between releases, selected upstream fixes come in by `cherry-pick -x` or a manual port that names its upstream commit (SYNC.12; SYNC.14 from upstream main `f47618f5`, listed in the ROADMAP log) | see the Git history |
| Upstream's Windows installer-policy check (`packaging-lint.yml`) runs only in upstream's repository: the fork ships a portable zip, not an MSI | `.github/workflows/packaging-lint.yml` |

## COMMENTS.1 fallback (inactive)

SYNC.16 trials upstream's comment multi-selection (#3484) in place of COMMENTS.1's panel-only
one; the owner decides after the consolidated Windows acceptance run whether to keep it, restore
COMMENTS.1 or combine them. Until then COMMENTS.1's UI is kept, not compiled:
`crates/ui-egui/src/tools/comments/comments1_fallback/` (its README lists what is there) and
`crates/ui-egui/tests/it/comments1_fallback.rs` (COMMENTS.1c's 15 panel tests, unchanged), with
the small hooks commented out where they were (marked `COMMENTS.1 fallback (inactive, SYNC.16;
f3ccab72)`).

Restoring the panel-only model (R-a). SYNC.16f is where the switch happened, but reverting it is
only the start: upstream's comment code came in with SYNC.16a and later commits build on it.

1. On a new branch from Main, `git revert` SYNC.16f, and resolve by reading `git show f3ccab72`
   (and `ce9ade4b`'s `panels/comments_panel.rs`) hunk by hunk against the current files.
2. Turn off upstream's second model, so `CommentView::also` can never be filled: the Ctrl/Shift
   branch of `card()` in `comments_panel.rs` (back to COMMENTS.1c's `Group`), the canvas ⌘/Ctrl-
   and ⇧-click additions in `tools/comments/input.rs`, the multi-key branch of `delete_selected`,
   the group drag and nudge, `comment_props` applying to the whole selection, the paste selecting
   every copy (select the first), and `control/host.rs`'s `selected_comments` (report the group).
3. Expect conflicts with SYNC.16a's resolutions (`comments_panel.rs`, `input.rs`, `paint.rs`,
   `automation/src/comments.rs`), SYNC.16g (Collapse/Expand selected: back to `Group`), SYNC.16h
   (the stale-selection guard and its tests: keep both, pointed at `Group` as well), SYNC.16b (keep
   COMMENTS.1c's Spanish rows in the overlay) and any later upstream sync touching `CommentView`.
4. Check one model is active: a test that Ctrl+Click and Shift+Click in the panel and on the page
   leave `also` empty, and `git grep -n "\.also\b"` showing no write outside `clear_selection` and
   `annotation_deleted`.
5. Before the PR: `tests/it/comments_selection.rs` as at `ce9ade4b` (the fallback file's tests),
   `group/tests.rs`, the COMMENTS.1a/1b suites in annot, engine and automation, the stale-selection
   tests, upstream's comment tests that still apply (list those dropped), and the usual gates.

Combining instead (R-b) keeps upstream's model and brings back chosen pieces over
`CommentView::selection()`; steps 3 to 5 apply.

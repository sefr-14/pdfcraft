<!-- Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history. -->
# PdfCraft Overlay app icon

<img src="pdfcraft.svg" alt="PdfCraft Overlay app icon: a dimensioned drawing sheet with an offset revision outline on a blueprint-blue tile" width="128">

**Design:** "Dimensioned Drawing" (LIC.2). A white drawing sheet carries a part outline with a hole, an orange dimension
line with extension ticks and arrowheads, and a solid title block. An orange outline offset behind the sheet is the
other revision, which is what Overlay compare puts side by side. The strokes are heavy and the title block solid so the
sheet still reads at 16–24 px, because one vector serves every size.

**Palette:**

| Colour | Hex | Used for |
|---|---|---|
| Blueprint blue | `#0F3D6E` | the full-bleed field, the part outline and the title block |
| White | `#FFFFFF` | the sheet and the title block rule |
| Orange | `#E8743B` | the dimension line and the other revision's outline |

**Tile:** `viewBox="0 0 512 512"`, a rounded square with `rx=112` that clips everything; Windows and Linux icons use
the full-bleed tile.

**Provenance:** original geometric artwork by the PdfCraft Overlay contributors, written by hand as SVG primitives
(rectangles, a circle, lines and two triangles). It uses no fonts and no traced or third-party material. Licence:
[LICENSE.txt](LICENSE.txt) (`MIT OR Apache-2.0`, like the repo). It replaces upstream PdfCraft's lion icon, which
this fork keeps only in its legacy macOS assets (below).

## Files

| File | What it is |
|---|---|
| `pdfcraft.svg` | the master vector; every PNG and the `.ico` are rendered from it |
| `pdfcraft-1024.png` | 1024 px on Apple's grid; the runtime Dock icon on macOS |
| `pdfcraft.ico` | Windows icon (16–256 px), embedded in `pdfcraft.exe` by `apps/pdfcraft/build.rs` |
| `pdfcraft-document.svg` | Windows PDF file icon master: a Lucide document outline, blue badge and outlined PDF lettering; no font dependency |
| `pdfcraft-document.ico` | Windows PDF file icon (16–256 px), embedded as resource 2 in `pdfcraft.exe` |
| `pdfcraft-document-orange.svg`, `.ico` | the same document icon with an orange badge; shipped beside `pdfcraft.exe` in the Windows zip for anyone who prefers it, not embedded |
| `hicolor/<n>x<n>/apps/ai.storyteller.pdfcraft.png` | Linux hicolor theme, 16–512 px; the 256 px one is the runtime icon on Windows and Linux |
| `hicolor/scalable/apps/ai.storyteller.pdfcraft.svg` | Linux scalable icon (copy of the master) |
| `pdfcraft.icns` | **legacy, unused:** upstream's lion icon for macOS. This fork builds no macOS packages, and the `.icns` can only be regenerated on macOS (`iconutil`) |

Where it shows: `apps/pdfcraft/src/main.rs` sets the window icon (taskbar, Alt-Tab, launcher) and the Wayland app id
`ai.storyteller.pdfcraft` (unchanged); `packaging/linux/ai.storyteller.pdfcraft.desktop` names the hicolor icon.

Windows PDF files use resource 2 (`"pdfcraft.exe",-2`) when PdfCraft Overlay is chosen as the default reader; the app
keeps resource 1. The document outline is adapted from Lucide `file-text` 1.49.0 (ISC); its badge, outlined lettering
and colours are original (`MIT OR Apache-2.0`), covered by [LICENSE-document.txt](LICENSE-document.txt). To use the
orange variant instead, point the file type at `pdfcraft-document-orange.ico` with a Windows file-type tool; the app has
no setting for it. This provides a file type icon; first-page thumbnails still need a thumbnail provider.

## Regenerate

```sh
packaging/icons.sh        # needs resvg and python3; iconutil (macOS only) for the legacy .icns
cargo xtask assets        # then update the sha256 values in ATTRIBUTION.toml and run with --write
```

<!-- This fork (sefr-14/pdfcraft): RENDER.1 diagnosis. -->
# RENDER.1: thin lines and small text at low zoom (findings)

> **Last reviewed:** 2026-10-30 · **Last updated:** 2026-10-30 · **Change:** major (first measurement, Phase R1) · **Scope:** PDF Overlay page view, Windows x64, `Main` cf8605cf (PdfCraft v0.15.0 + fork)

## Revision history

| Date | Change |
|---|---|
| 2026-10-30 | Phase R1: harness, baseline measurements, root causes, ranked R2 candidates. No renderer change. |

## Summary

Lines and text are not degraded by texture sampling once the view is still. The page is rendered at
exactly the screen's resolution and drawn pixel for pixel. What makes engineering drawings weak at
25–100% is the rasterizer's **policy for things thinner than a pixel**, plus GPU minification while
zooming out:

1. **Thin strokes are never darker than one pixel's worth of ink, and usually half that per pixel.**
   - Every stroke under 1 device pixel is widened to exactly 1 px of ink. That is right, and it is
     what Poppler does.
   - Our 1 px lands wherever the line's geometry puts it. A line centred on a pixel edge becomes
     **two 50% grey pixels**. Poppler snaps it to **one black pixel**.
   - Most lines in a drawing fall somewhere in between, so most hairlines look grey and soft.
2. **Thin filled rules fade away.**
   - A rule drawn as a filled rectangle (`re f`, as CAD and word processors often draw them) keeps
     only its geometric coverage.
   - A 0.25 pt rule at 25% peaks at 4–8% darkness, which is invisible. Poppler draws it as one black
     pixel.
3. **Hairlines inside tiling patterns and Type 3 glyphs vanish.**
   - Hatching drawn as a pattern, and symbols drawn as Type 3 glyphs, get no minimum width at all.
   - A zero-width line ("the thinnest line the device can draw") is 0–1% dark in a pattern and
     **fully absent** in a Type 3 glyph. Poppler draws both as one black pixel.
4. **While zooming out, lines can drop out completely for a moment.**
   - Until the re-render arrives, a stale raster is shown smaller by the GPU with bilinear filtering
     and no mipmaps.
   - Shown at 50%, a 1 px line keeps half its ink. At 25% it can disappear entirely.
   - The same applies permanently to the Overlay wizard's previews, which are drawn smaller than they
     are rendered.
5. **Small text is about as good as its geometry allows.**
   - Glyph ink is 84–100% of a 4× supersampled reference, and matches Poppler for Helvetica.
   - 4–5 pt dimension text at 25% is 1.3–1.7 px tall: no rasterizer makes that legible without
     distorting it.
   - The Courier difference against Poppler (Poppler has about 1.5× the ink) comes from a heavier
     substitute font, not from rasterization.
6. **Thin Lines does not change any of this.** Sub-pixel strokes are already 1 px. Thin Lines only
   narrows strokes wider than a pixel, for example a 1 pt line at 100% and ppp 2 goes from 2.67 px
   to 1 px.

## Method

**Harness:** `crates/render/tests/it/low_zoom.rs`. To re-run:

```
cargo test -p pdfcraft-render --test it low_zoom -- --ignored --nocapture
```

It prints every table below in full and writes grey images to `target/render1/`, which is
git-ignored.

**Rendering:**
- Synthetic pages are built in code; no corpus files.
- They are rendered through `PageRenderer::render` at the page view's scale: `zoom × 96/72 × ppp`
  device pixels per point (`canvas/cache.rs` `render_scale`).
- Zooms 25/50/75/100%; ppp 1, 1.5 and 2; Thin Lines off and on.

**Darkness:** 1 − Rec. 709 luminance of the 8-bit sRGB value, composited over white with the
premultiplied alpha undone.

**Line metrics,** per device pixel of the line's length, from the middle 40% of a 200 pt line:
- **ink:** the sum of darkness across the line;
- **peak:** the darkest pixel across it;
- **effective width:** ink ÷ peak;
- **affected:** pixels across it darker than 0.02.

**Subpixel position:**
- Horizontal and vertical lines are placed so their centre falls on a pixel **edge** (offset 0) or a
  pixel **centre** (offset 0.5) at the scale being measured.
- The PDF geometry is otherwise the same at every scale. That is the controlled variable; no fixture
  was tuned to produce a number.

**References:**
- **4× region of interest:**
  - The same device rectangle is rendered as a tile (`RenderRequest.tile`) at 4× the scale.
  - It is averaged over exact 4 × 4 blocks, so each reference pixel covers exactly one target pixel.
  - Only the region is rendered.
  - Caveat: the reference has the renderer's own 1 px minimum at 4× scale. A hairline's reference
    is therefore ¼ px of ink, not "the truth"; a hairline has no true width.
- **pdftoppm** (Poppler, run as an external process only):
  - `-gray -r (scale × 72) -x -y -W -H` crops the same device rectangle.
  - Its anti-aliasing and stroke adjustment are on by default.
- The dense A1 sheet (2,000 × 0.1 pt lines, 400 texts) is rendered only at the target zoom × ppp
  combinations, for time and memory, never at 4×.

**GPU minification** is simulated as egui's `TextureOptions::LINEAR` draws a texture smaller, with no
mipmaps: one bilinear sample per screen pixel, at its centre, between the four nearest texels.

## Results

### 1. Strokes under a pixel (horizontal, ppp 1)

The same at 25, 50, 75 and 100%, and at ppp 1.5 and 2, for every width below 1 device pixel.

| width | position | ink | peak | eff. width | affected px | pdftoppm ink | pdftoppm peak |
|---|---|---|---|---|---|---|---|
| 0 (hairline) to 0.5 pt (< 1 px) | pixel edge | 1.00 | **0.50** | 2.00 | 2 | 1.00 | **1.00** |
| 0 (hairline) to 0.5 pt (< 1 px) | pixel centre | 1.00 | 1.00 | 1.00 | 1 | — | — |
| 1 pt at 100% (1.33 px) | pixel edge | 1.33 | 0.67 | 2.00 | 2 | 2.00 | 1.00 |
| 1 pt at 100% (1.33 px) | pixel centre | 1.33 | 1.00 | 1.33 | 3 | — | — |

- **Vertical** lines and lines on a **`/Rotate 90`** page measure the same as horizontal ones.
- **Slanted** lines (30°, 45°) carry about 1 px of ink at a 0.91–0.92 peak. Anti-aliasing spreads
  them over about 2 px whatever their position, as it would in any renderer.
- **With Thin Lines on,** strokes under a pixel are unchanged. Wider ones become 1.00 px of ink, for
  example 1 pt at 100%, ppp 2: 2.67 → 1.00.

### 2. Filled rules, and strokes in patterns and Type 3 glyphs (ppp 1)

| fixture | zoom | geometric px | ink | peak (pixel edge / centre) | pdftoppm peak |
|---|---|---|---|---|---|
| rule 0.1 pt | 25% | 0.03 | 0.03 | 0.02 / 0.03 | 1.00 |
| rule 0.25 pt | 25% | 0.08 | 0.08 | 0.04 / 0.08 | 1.00 |
| rule 0.5 pt | 25% | 0.17 | 0.17 | 0.08 / 0.17 | 1.00 |
| rule 0.25 pt | 100% | 0.33 | 0.33 | 0.17 / 0.33 | 1.00 |
| rule 0.5 pt | 100% | 0.67 | 0.67 | 0.33 / 0.67 | 1.00 |
| hairline in a tiling pattern | 25–100% | — | 0.00–0.02 | 0.00 / 0.01 | 1.00 |
| hairline in a Type 3 glyph | 25–100% | — | **0.00** | 0.00 / 0.00 | 1.00 |
| 0.1 pt stroke in a pattern | 25% | 0.03 | 0.03 | 0.02 / 0.03 | 1.00 |
| 0.1 pt stroke in a Type 3 glyph | 25% | 0.03 | 0.03 | 0.02 / 0.03 | 0.13 |

Our rules and exempt strokes agree with the 4× reference to ±0.01. They are geometrically exact and
visually absent.

### 3. Dimension text "1/4" = 1'-0"  12'-5 3/8"  0123456789" (ppp 1)

| font | size | zoom | ink / 4× ref | p95 darkness (ours / ref / pdftoppm) | ≥ 50% dark px (ours / ref / pdftoppm) |
|---|---|---|---|---|---|
| Helvetica | 4 pt | 25% | 0.84 | 0.22 / 0.25 / 0.23 | 0 / 0 / 0 |
| Helvetica | 4 pt | 100% | 0.98 | 0.35 / 0.35 / 0.34 | 30 / 30 / 23 |
| Helvetica | 6 pt | 50% | 1.00 | 0.35 / 0.36 / 0.35 | 10 / 8 / 5 |
| Helvetica | 8 pt | 25% | 0.95 | 0.34 / 0.34 / 0.33 | 0 / 0 / 0 |
| Helvetica | 8 pt | 100% | 0.99 | 0.62 / 0.62 / 0.59 | 286 / 282 / 284 |
| Courier | 6 pt | 100% | 0.98 | 0.35 / 0.35 / 0.49 | 58 / 60 / 173 |

- At 25%, glyph ink is 84–95% of the reference: up to 16% is lost at the smallest sizes, a
  small-coverage and quantization effect.
- From 50% up it is 90–100%.

### 4. Zooming out: a 100% raster shown smaller by the GPU (hairline, centred on a pixel)

| shown at | ink after GPU minification | ink of a fresh render |
|---|---|---|
| 50% | 0.50 | 1.00 |
| 33% | 1.00 (the sample happened to land on it) | 1.00 |
| 25% | **0.00** | 1.00 |

- For pages that render in under 500 ms, the page view re-renders at each new zoom step, so this
  lasts a frame or two.
- For slow pages, a 0.35 s hold applies after each zoom change.
- The Overlay wizard's previews (2400 px rasters drawn at fit) are always shown this way.

### 5. Cost baseline: dense A1 sheet

| zoom | ppp | raster | MiB | render ms |
|---|---|---|---|---|
| 25% | 1 | 562×795 | 1.7 | 41 |
| 50% | 1 | 1123×1590 | 6.8 | 108 |
| 100% | 1 | 2246×3179 | 27.2 | 211 |
| 100% | 1.5 | 3368×4768 | 61.3 | 452 |
| 100% | 2 | 4491×6358 | 108.9 | 687 |

These were measured in this Linux container. Windows timings will differ; ratios are what count for
R2.

## Root causes

| symptom | where | cause |
|---|---|---|
| grey, soft hairlines | rasterization | `set_stroke_properties` (`vendor/hayro/src/renderer.rs:520-581`) widens strokes under 1 px to exactly 1 px but doesn't move them onto the pixel grid; anti-aliasing splits the pixel's ink over two rows. Poppler applies stroke adjustment. |
| vanishing filled rules | rasterization | `fill_path` has no minimum width: coverage is exact, so a rule thinner than ~⅓ px is almost white. |
| vanishing hatching and symbols | rasterization | the minimum width is skipped inside patterns and Type 3 glyphs (`in_pattern` / `in_type3_glyph`), and a zero-width hairline there gets no width at all. |
| lines dropping out while zooming out; weak Overlay previews | presentation | bilinear minification without mipmaps (`TextureOptions::LINEAR`; egui-wgpu creates single-level textures). |
| small text | geometry | text is drawn at its true size; 4–6 pt at 25% is 1.3–2 px tall. |
| HiDPI | both | at ppp 2 a 1 px hairline is half a logical pixel: thinner-looking but sharper. The policies above apply unchanged. |

## R2 candidates, ranked

| # | change | expected effect (from the numbers above) | cost | fidelity risk | touches |
|---|---|---|---|---|---|
| **C1** | Minimum visible width for thin **filled** rules: an axis-aligned fill whose short side is under 1 device px is drawn 1 px wide, pixel-aligned (Poppler-like). | 0.25 pt rule at 25%: peak 0.04–0.08 → about 1.0 | negligible CPU | medium: thin fills used as art get bolder; scoped like OV9 (not text, not annotations, not print) | new `vendor/hayro` patch in `fill_path` |
| **C2** | **Pixel-snap** strokes widened to 1 px (stroke adjustment), axis-aligned only. | hairline peak 0.50 → 1.00 at every position; ink unchanged | negligible | low–medium: lines move ≤ ½ px on screen only; output files untouched | `vendor/hayro` `set_stroke_properties` / stroke path |
| **C3** | Apply the stroke minimum inside **patterns and Type 3 glyphs** too. | pattern and Type 3 hairline 0.00 → 1 px of ink | small; dense hatching gets darker (as in Poppler) | medium: pattern cells are cached at pattern scale | `vendor/hayro` `set_stroke_properties` |
| **C4** | **Minify on the CPU** (box filter) the stale raster shown while zooming out and the Overlay previews, or render previews at their display size. | at 25%: 0.00 → about 0.25–1.0 depending on width; at 50%: 0.50 → 1.00 | CPU downsample per stale texture; no extra memory if replacing | low | `crates/ui-egui/src/canvas/cache.rs`, `overlay_ui.rs` (texture policy: needs approval) |
| C5 | Small-text contrast (stem darkening below about 9 px em) | darker 4–6 pt text, beyond the reference | — | high: changes every glyph; contrary to the measurements, which show no rasterization deficit | `vendor/hayro` glyph fill |
| C6 | Supersampling | none: we already match our own 4× reference; the issues are policies, not sampling | 4–16× CPU and memory | — | rejected |

**Interactions:**
- C1–C3 keep the PDF untouched; they change only on-screen pixels.
- They must skip print rasters (`PRINT_TAG`) and annotations (as OV9 does), and keep
  tile/full-render equality (`tiles_match_full_render`).
- Thin Lines then becomes "every document stroke exactly 1 px", and C2 would make those crisp too.
- Overlay alignment is unaffected: the overlay output is built from PDF geometry, not rasters.

**Recommended R2 scope:**
- **Approve C2 + C3 + C1, in that order.** They are all vendored hayro patches beyond today's
  approved scope.
- **C4 for the Overlay previews and the zoom-out path.** This is a texture-policy change; it also
  needs approval.
- **C5 and C6 not recommended.**
- Each step is measured with this harness, and its characterization tests are updated on purpose,
  stating the new expected values.

## Manual check on real drawings (owner, Windows)

No files are committed and nothing is captured automatically.

1. Open a representative drawing (dimension text, grid lines, hatching, a rotated sheet if
   available).
2. At 25, 50, 75 and 100%, in Normal and with View › Page display › Thin lines:
   - take a screenshot of the same region;
   - or start `pdfcraft --control ctl.json drawing.pdf` and run
     `pdfcraft-cli ui --control ctl.json screenshot --out zoom-25.png` after each zoom step.
3. Note which elements are faint or missing: grid lines, hatching, dimension text and leaders,
   symbols. After R2, repeat at the same zooms for a before/after pair.

## What R1 did not change

- No renderer, vendored-library, cache or texture code.
- The Overlay PDF's Thin Lines limitation is out of scope, as agreed.

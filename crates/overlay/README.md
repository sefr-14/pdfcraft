# pdfcraft-overlay

Layer L5. Overlay comparison, this fork's addition to PdfCraft: two documents drawn on one page,
each in its own colour, so shared content prints dark and every difference shows in colour.

Step 2 of the overlay plan is the geometry, as pure functions with no PDF writing and no UI.
Step 3 adds `composite`, which writes the overlay page:

- `frame`: a page's displayed frame. The visible box (crop ∩ media), upright after `/Rotate`, in
  points after `/UserUnit`, with its lower-left corner at (0, 0). It inverts pdfcraft's own
  display matrix (`pdfcraft_model::view_matrix_for`), so the overlay frames a page exactly as the
  viewer shows it. Where pdfcraft falls back (no usable media box, a crop box outside the media
  box), the overlay does the same and returns a `Warning` that names the page.
- `sheet`: sheet-size proposals (same sheet, same shape with a factor to confirm, different
  shape) and the confirmed `SheetMatch` (off = 1:1, or scaled and centred on A).
- `register`: fits from picked point pairs. `Similarity` (shift, rotation, one scale; the 2D
  Umeyama solution) and `Affine` (six parameters, solved in the principal axes of B's picks),
  with refusals that carry their numbers. `fit` / `report` give residuals, RMS and maximum,
  leave-one-out residuals, the RMS and worst misfit without each pick, the approved
  inconsistency flags (absolute 0.5 mm; robust modified z > 3.5 from 5 picks, skipped when the
  MAD is at or below `min(1e-9 × A's diagonal, 0.01 × tolerance)`), and a `Verdict`: consistent,
  add a pick, a probable wrong pick, ambiguous or unexplained. No pick is ever removed or changed.
- `place`: the output page (A's sheet, or expanded to hold both), the matrices that put A and B
  on it, B's outline, clipping, and an output `/UserUnit` past 14,400 units.
- `settings`: every limit, with the starting values from the PRDs.
- `composite` (step 3): one overlay page from two source pages and a placement, in the overlay
  PRD's structure. Each document draws a rectangle in its layer colour under Multiply and a
  luminosity soft mask; the mask's group draws the source page (copied by
  `pdfcraft_organize::page_as_form`, its `/BBox` set to the visible box and its `/Matrix` to the
  identity) under the placement matrix, written at full precision, over a white backdrop, through
  the light-tone function m_t(L) = clamp(1 − L / t, 0, 1) as one shared Type 2 function. Source
  content is copied, never rewritten, so vectors stay vectors. Sources are only read.
- `layers` (step 4, OV4.1): the output's two layers. `LayeredOutput` builds a document with
  exactly two optional content groups, `Old` and `New` (both on, listed Old then New), and wraps
  each document's fill in its role's marker (`/OC /PCOld BDC … EMC`); with both layers on, a page
  is `composite`'s page plus the markers. `Roles` says which side is which (A Old, B New by
  default) and `LayerStyle` gives each role its colour. A failed page leaves the document as it
  was. `finish` writes the registration into the metadata (`/Subject`, XMP `dc:description`) and
  runs `check_output`, the defensive check: any other optional content, `/OC` entry, annotation,
  form, action, script or unreadable content stream is an error, so no output is returned.
  Until source preparation lands (OV4.3, OV4.4), a source with optional content or annotations is
  refused.

Conventions: points (1/72 in), y up, origin at the displayed sheet's lower-left corner;
matrices in PDF order `[a b c d e f]` (`pdfcraft_content::Matrix`); rotation counter-clockwise
in radians; f64 throughout; every input checked to be finite. Nothing here panics on any input.

Sources: ISO 32000-2 §14.11.2 (page boxes), §7.7.3.3 (`/Rotate`, `/UserUnit`), §11.6.5
(soft masks), §7.10.3 (Type 2 functions); S. Umeyama,
"Least-squares estimation of transformation parameters between two point patterns", IEEE TPAMI
13(4), 1991; B. Iglewicz and D. Hoaglin, "How to Detect and Handle Outliers", ASQC, 1993.

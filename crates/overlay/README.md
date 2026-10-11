# pdfcraft-overlay

Layer L5. Overlay comparison, this fork's addition to PdfCraft: two documents drawn on one page,
each in its own colour, so shared content prints dark and every difference shows in colour.

Step 2 of the overlay plan (this crate so far) is the geometry, as pure functions with no PDF
writing and no UI:

- `frame`: a page's displayed frame. The visible box (crop ∩ media), upright after `/Rotate`, in
  points after `/UserUnit`, with its lower-left corner at (0, 0). It inverts pdfcraft's own
  display matrix (`pdfcraft_model::view_matrix_for`), so the overlay frames a page exactly as the
  viewer shows it. Where pdfcraft falls back (no usable media box, a crop box outside the media
  box), the overlay does the same and returns a `Warning` that names the page.
- `settings`: every limit, with the starting values from the PRD.

Conventions: points (1/72 in), y up, origin at the displayed sheet's lower-left corner;
matrices in PDF order `[a b c d e f]` (`pdfcraft_content::Matrix`); rotation counter-clockwise
in radians; f64 throughout; every input checked to be finite. Nothing here panics on any input.

Sources: ISO 32000-2 §14.11.2 (page boxes), §7.7.3.3 (`/Rotate`, `/UserUnit`).

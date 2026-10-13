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
  Its pages take prepared sources (`oc::prepare`).
- `oc::eval` (step 4, OV4.2): what a source's optional-content reference shows under its default
  configuration and the caller's explicit decisions (`Visibility`). Groups (state from
  `/BaseState`, `/ON`, `/OFF`; intents matched with the configuration's), membership dictionaries
  and auto-states. A membership dictionary's `/VE` and its `/P` over `/OCGs` are evaluated
  independently; when both are present they must both resolve and agree, or the dictionary is
  unresolved (OV4.7, F1). Absent, null or empty `/OCGs` has no effect (ISO 32000-1 Table 99);
  references to deleted objects, which pdfcraft's display counts as groups that are on, leave it
  unresolved whenever they change the result (F2). Every result is either decided by the
  standard or unresolved with a `Rule` and whether a decision settles it; values combine in
  three-valued logic, so a result that doesn't depend on an unresolved operand is still resolved.
  Checked against Adobe's PDF Reference 1.7 §4.10, the Arlington model of ISO 32000-2 and, for
  the membership rules, ISO 32000-1 §8.11.
- `oc::resolve` and `oc::prepare` (step 4, OV4.3): an in-memory copy of a source whose pages show
  what the source shows, with no optional content left. Every stream a page draws (its content,
  forms, tiling patterns, Type 3 glyphs, soft-mask groups) is resolved once: a visible block loses
  its `BDC`/`EMC`, a hidden block and every `Do` of a hidden XObject are removed by byte range
  (the rewritten stream is parsed again and must give exactly the expected operators), and a
  visible XObject loses its `/OC`. Viewers apply the graphics-state changes of hidden content, so
  a state change or clipping path in a hidden block that later content uses is kept on its own.
  In a document with layers, each resource dictionary keeps only what kept content uses, so
  hidden content isn't carried along, and what the overlay will copy is then searched for any
  optional content left. Anything that can't be decided (the evaluator's rules, unbalanced
  markers, a hidden block crossing a save, text object or path, hidden text that moves or clips
  visible content, unreadable streams, forms nested too deep or used with different resources) is
  reported per page as `Unresolved`, and `LayeredOutput::add_page` refuses the page with every
  item of both documents. A document without layers or decisions is copied unchanged.
- `annotations` (step 4, OV4.4): preparation draws each page's annotations into its content as
  the source shows them, after the page's content (closing any graphics states it leaves open),
  and the page keeps none. The normal appearance (the `/AS` state of a state dictionary) is
  placed by Algorithm 8.1, NoRotate keeps it upright about the upper-left corner of `/Rect`, and
  `/CA` applies to it as one transparency group. A comment without an appearance gets the one
  PdfCraft's builder draws; a form field is redrawn from its value only when its appearance is
  missing or `/NeedAppearances` is set (O8). Hidden, NoView, pop-ups, links, annotations whose
  layer is off and appearances that can't be used are excluded and counted. Text the fonts can't
  draw is never drawn as `?` (O15): such an annotation, and media annotations the overlay doesn't
  draw, leave the page unresolved until the caller decides to omit them (`Visibility::omit`).
  Every annotation is in the page's report: drawn, excluded, omitted or unresolved.
- `report` (step 4, OV4.5): `LayeredOutput::finish_with_report` returns the generation report with
  the document: each document's role and warnings (signed signature fields, whose appearance is
  drawn as plain graphics; redaction marks not applied, drawn as they appear and never applied;
  the layer notes the output is correct with), and each page's registration, layers and
  annotations. Nothing in it, or in the output, says the output is signed or redacted; the
  defensive check also refuses any signature dictionary or `/ByteRange`.

Conventions: points (1/72 in), y up, origin at the displayed sheet's lower-left corner;
matrices in PDF order `[a b c d e f]` (`pdfcraft_content::Matrix`); rotation counter-clockwise
in radians; f64 throughout; every input checked to be finite. Nothing here panics on any input.

Sources: ISO 32000-2 §14.11.2 (page boxes), §7.7.3.3 (`/Rotate`, `/UserUnit`), §11.6.5
(soft masks), §7.10.3 (Type 2 functions); S. Umeyama,
"Least-squares estimation of transformation parameters between two point patterns", IEEE TPAMI
13(4), 1991; B. Iglewicz and D. Hoaglin, "How to Detect and Handle Outliers", ASQC, 1993.

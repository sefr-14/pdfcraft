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
  - **Undeclared groups (OV4.10, 20 Oct 2026).** A group that content uses but `/OCProperties
    /OCGs` doesn't list is shown as the source shows it, with one report note naming the groups.
    It stays unresolved only where viewers disagree. This revises the step 4 PRD's row 6, which
    held every such group unresolved.

    | Case | Result |
    |---|---|
    | U1: the document has no `/OCProperties` | shown: readers ignore optional content (ISO 32000-1 §8.11.4.2) |
    | U2: valid configuration, the group in neither `/D /ON` nor `/D /OFF` | shown |
    | U3: in `/D /ON` only | shown |
    | U4: in `/D /OFF` | unresolved: PdfCraft's display hides it, Poppler 24.02 shows it |
    | U5: `/BaseState /OFF`, not in `/D /OFF` | shown: the base state applies to listed groups only (user decision, 20 Oct) |
    | U6: named by an auto-state (`/AS`) | unresolved |
    | U7: a malformed configuration | unresolved, as every group is |
    | U8: no `/OCProperties`, and a membership dictionary PdfCraft's display would hide | unresolved: the standard shows its content |

    - **Evidence:**
      - PdfCraft's display (`vendor/hayro-interpret/src/ocg.rs`, `from_catalog` and
        `begin_single_oc`) hides only groups in its off-set (`/D /OFF`, plus `/OCGs` under
        `/BaseState /OFF`).
      - Poppler 24.02 (`pdftoppm`, synthetic files) showed an undeclared group in every case,
        `/D /OFF` included.
      - The group's `/Intent` makes no difference: both renderers ignore it.
      - Acrobat and pdf.js weren't tested.
    - **Coverage:** annotations, forms, nested forms and membership dictionaries follow the same rule.
    - **Decisions:** explicit decisions still come first.
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
  item of both documents. A document without layers or decisions is copied unchanged, unless its
  pages use optional content anyway (no `/OCProperties`, OV4.10). Such a document is walked
  again with its resources pruned and checked, so its groups don't reach the output.
  A marker naming a property its stream's own resources don't define isn't optional content
  under the standard and is shown, as PdfCraft's display shows it, with a note (OV4.8, found on
  Bluebeam sets). In a document with `/OCProperties`, a name that a surrounding resource
  dictionary (of a form or page drawing the stream, or of the page's ancestors) defines as a
  layer that is off or can't be decided leaves the page unresolved instead (OV4.9): the standard
  doesn't apply that dictionary to the stream, but some renderers look names up there (Poppler
  24.02 does, PdfCraft's display doesn't), so viewers would disagree on whether it is hidden. A
  form reused under other surroundings is decided again there.
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

- `colour` (step 6): the wizard's palette (four complementary pairs, each colour's complement
  also a swatch), complements, what shared content prints as (`A × B / 255`), and the
  readability checks behind its warnings (contrast on white at least 1.9, ΔE at least 50, the
  overlap's contrast at least 7). Warnings are advice; generation works with any pair.
- `pairing` (step 6, the wizard's Pages step): which page of A goes with which page of B on each
  output page, for revised drawing sets with inserted, deleted and moved sheets. Two columns of
  cells, each a page (its 0-based physical index, never its label) or a blank; row `i` pairs the
  columns' cells `i`, and a shorter column leaves the other's extra pages unplaced until the user
  places them. Operations insert or remove a blank (later cells of that column shift by one),
  move a cell up or down, put a page at a row by swapping it with what is there, place unplaced
  pages against blanks, or apply a new page range keeping the existing pairing. None can lose or
  duplicate a page, and one that would leave a row with no page is refused and changes nothing.
  OV6.5 adds what the wizard's table uses: `set_compare` (Compare as: Matched, A only, B only;
  the row's kind, not a reason such as inserted or deleted), `move_row` (a whole output page
  up or down, never across a column's end) and `take` (a page chosen by hand; the row it leaves
  is removed when that leaves it empty, and only the first row without a partner can be
  reached). Matched → one document only inserts a blank, and that column's later cells move down
  one row. One document only → Matched takes a partner only where the pairing points to it: the
  one neighbouring row that holds only the other document's page (merged, nothing else moves),
  or, when everything below is still the run an earlier gap pushed down (both sides in page
  sequence, no blank), the gap is removed. Otherwise it is refused (`NoCounterpart`) and the
  partner is chosen by hand, so pairings made by hand are never re-paired. `Effect` says what
  moved besides the row.
- `job` (step 6, used by the wizard; later `doc_overlay`): a whole overlay from two documents.
  `generate` takes the page pairs (a blank side allowed, never both), each with its confirmed
  sheet match and registration, prepares both sources once for the pages used, and returns
  every unresolved item of every page of both documents in one `Unresolved` error before
  composing anything (O12); otherwise the layered document, its report, frame warnings and the
  pages where B is clipped. `snapshot_decisions` turns a tab's layer states into explicit
  decisions only where they differ from what the file's default configuration resolves to; a
  layer the default can't resolve gets none, so the wizard asks rather than settling it silently.
- `snap` (step 6, the wizard's Alignment step): a page's drawn lines in its displayed frame, so a
  click snaps to the nearest line end or crossing (and, if chosen, a straight line's midpoint or
  the nearest point on a line) and the same feature is picked on A and B. Paths are read as
  `pdfcraft-measure` reads them (painted subpaths, rectangles, curves flattened to 0.2 units,
  nested forms and their matrices), with limits sized for dense sheets (400,000 segments; the
  densest real 36 × 24 in structural sheet tried has about 42,000), and content in a layer the
  tab hides is left out. Text, images, shadings and annotations aren't snap targets. Reading is
  lenient: an undecodable stream is skipped and counted, and a limit stops it with `truncated`.
  The viewer's measuring tools snap with it too (OV10, `crates/ui-egui/src/measure_ui.rs`), so
  a change here changes how they snap.

The wizard itself is `crates/ui-egui/src/overlay_ui.rs` (Overlay compare… on the toolbar, in All
tools ▸ Compare files, and the command `doc.overlay`).

Wizard layout, decision of 15 Oct 2026 (OV6.5–OV6.8, after the user's tests with real drawing
sets of up to 59 sheets). It revises the overlay PRD's seven-step wizard for the interface only;
the PRD's requirements stay as written, and every functional area is still there:

- Six visible steps: Documents, Pages, Colours, Sheet size & Alignment, Preview, Generate. Sheet
  sizes and alignment share one screen: a compact sheet-size section ("Old: 3024 × 2160 pt ·
  New: 3024 × 2160 pt · Scale: 1:1") above the two drawings.
- Match sheet sizes is on by default in the wizard (the PRD had it off). The same sheet is 1:1 with
  nothing to confirm; different sheets show the proposal and its basis, and a factor is applied
  only once confirmed, as before. Each page pair keeps its own choice when the first pair's isn't
  shared, and pairs whose sheets differ from a shared choice are named.
- Pages: each row says what it compares, Matched, Old only or New only (a sheet one revision lacks
  isn't necessarily inserted or deleted), in a "Compare as" dropdown, with arrows that move the
  whole row; the row menu moves only one revision's sheet.
- Reference points: Old's first (three recommended), then the same points on New in the same order,
  matched by number, with both drawings side by side throughout. New may start after two Old
  points; more points can be added in later rounds; zero points, two-point similarity, affine and
  the final nudge are unchanged.
- Generate: "Expand page to prevent clipping" (was "Expand output to include both documents"),
  off by default; the output extent calculation is unchanged.
- Different sheet sizes (OV6.9, 19 Oct 2026). Sheets the same within `same_sheet_pt` need no
  reference points: without any, Old and New are aligned at their lower-left corners, 1:1, and
  the wizard says so. A pair whose physical sizes differ shows both (mm and pt) and blocks Next
  and Generate until its own "Allow alignment of different sheet sizes" is ticked, and then
  until it has reference points; a manual nudge alone isn't enough. The tick belongs to that
  page pair only (never shared through "use the first pair's" choices) and changes nothing
  else: Match sheet sizes, a factor's separate confirmation and the OV2 solver are as before,
  and the tick itself never scales.
- Drawing navigation in step 4 (OV6.10, 19 Oct 2026), CAD-style:
  - a left click selects a point (Alt: without snapping); a left drag does nothing;
  - the wheel pans up and down, Shift+wheel left and right;
  - Ctrl+wheel (⌘ on a Mac) zooms about the pointer, read through the app's `motion::zoom_delta`, so Animation effects off zooms at once;
  - a middle drag pans;
  - Fit to page restores both drawings.

  At the full-page view there is nothing to pan, so the wheel scrolls the step.

  The two views keep their own zoom and pan; points stay in page coordinates and the snap reach stays 10 screen pixels.

  The Preview keeps its own controls: the wheel zooms, any drag pans.
- Unresolved items (OV4.10, 20 Oct 2026): each question shows once per document, with the pages it
  is on ("Document A, pages 1–59: …"), and one Show/Hide (or Leave out) decision settles every
  page listed (`oc::group_unresolved`). The grouping is by object reference, never by name, so two
  layers both named "0" are two questions, and documents A and B are decided apart. Decisions
  were already stored per document and object; only the list repeated.
- Preview and Generate keep their own unresolved lists (OV6.11, 20 Oct 2026). They used to share
  one, so a preview finishing after Generate replaced Generate's list with its one page, or
  cleared it, and step 6 showed the wrong questions or the raw error. Decisions stay shared.

Known limitation (R3, pre-existing; recorded at the upstream 0.8.0 sync, not fixed): a source
page's resources are taken as one dictionary, its own `/Resources` or else its nearest page-tree
ancestor's, both by `pdfcraft_organize::page_as_form`, which `composite` uses to copy the page,
and by the OV4 walker. A page that has its own `/Resources` but relies on an ancestor for some
fonts, XObjects or other named resources shows them in PdfCraft's display, whose renderer looks
names up the tree, yet can lose them in the overlay: lines, text or symbols the source tab shows
may be missing. Upstream 0.8.0 (da9cefa) merges such ancestor-only names when pages are imported,
not in `page_as_form`. Engineering drawing sets are where this matters most; compare an overlay
with its sources, and report a page where something is missing. Still open at the upstream
f0c9901c sync (16 Oct): `page_as_form` still copies one dictionary, and the ancestor merge
(`resources_with_ancestors` in `pdfcraft-organize`) still serves page import only. Still open at
the ec0a9650 sync (19 Oct, after upstream 0.9.0): `crates/organize/src/import.rs` is unchanged.
Still open at the 41649514 sync (21 Oct, after upstream 0.10.1): `crates/organize` is unchanged.
Still open at the v0.11.0 sync (3e306453, 21 Oct): the release changes no code in `crates/organize`.
Still open at the v0.12.0 sync (39844f06, 22 Oct): `page_as_form` is unchanged; upstream's change
to `crates/organize/src/import.rs` is how the objects an import added are found (they are not
always numbered above the others), not which resources a page takes.
Still open at the v0.14.0 sync (5a78b223, 25 Oct): `page_as_form` still takes one resource
dictionary. Upstream's changes to it join the page's content streams within one page decode budget
(`DecodeBudget::page()`: 256 MiB of decoded content, 50 000 forms; 9c9bbe2a) and set the form's
`/Matrix` from the page rotation as before; a page past that budget is refused and the overlay
reports it as a source failure (`OverlayError::Source`) rather than reading without bound.

Optional content at the ec0a9650 sync: upstream's one layer change (4ff53a8d) writes the Layers
panel's choices into the default configuration of the copy made for printing. It doesn't change
how a layer that content uses but `/OCProperties` doesn't declare is evaluated. OV4.10 (20 Oct)
then changed that evaluation in this fork: such content is shown as the source shows it, and it
stays unresolved only where viewers disagree (see `oc::eval` above).

Optional content at the 41649514 sync: upstream's one layer change (d695bc56) is in
`pdfcraft-redact`. Remove hidden information now evaluates a membership dictionary's `/P` and
`/VE` against the default configuration before it removes hidden content. The overlay doesn't call
it (`pdfcraft-redact` is only a dev-dependency here, for `marks`), and `oc::eval` is unchanged, so
what the overlay shows or asks about is the same as before the sync.

Optional content at the v0.11.0 sync (3e306453): the release is the version bump, two dependency
security updates, `xtask version set` and a renderer test. It changes no layer code, and
`oc::eval` is unchanged, so what the overlay shows or asks about is the same as before the sync.

Optional content at the v0.12.0 sync (39844f06): upstream's layer changes are the Layers panel's
Reset to Initial Visibility (fcdf2a21), `layer_set` and `page_render` drawing with the document's
view settings (92990b2e, automation), and set-layer-visibility actions run from links and push
buttons (22752dc9). The overlay calls none of them, and `oc::eval` is unchanged, so what the
overlay shows or asks about is the same as before the sync.

Optional content at the v0.14.0 sync (5a78b223): upstream changes no layer evaluation. Its one
optional-content change is that page import keeps a split widget's `/OC` (organize, #2289); the
overlay doesn't import widgets, and its own `oc::eval` and `oc::prepare` are unchanged, so what the
overlay shows or asks about is the same as before the sync.

Since the f0c9901c sync, `page_as_form` copies through upstream's bounded copier (af62daea): a
source page whose resources nest more than 256 references or containers deep is refused with an
error rather than overflowing the stack, and the overlay reports it as a source failure naming
the side and page (`OverlayError::Source`).

A `/Rotate` that isn't a multiple of 90 (ISO 32000-2 §7.7.3.3 requires one) has read as 0 since
the f0c9901c sync (upstream 4d358d4f), as PdfCraft draws the page, so OV2 frames such a page
upright as the wizard shows it. Before, it rounded down (100 read as 90) while the page was shown
upright. Conforming rotations are unaffected.

Conventions: points (1/72 in), y up, origin at the displayed sheet's lower-left corner;
matrices in PDF order `[a b c d e f]` (`pdfcraft_content::Matrix`); rotation counter-clockwise
in radians; f64 throughout; every input checked to be finite. Nothing here panics on any input.

Sources: ISO 32000-2 §14.11.2 (page boxes), §7.7.3.3 (`/Rotate`, `/UserUnit`), §11.6.5
(soft masks), §7.10.3 (Type 2 functions); S. Umeyama,
"Least-squares estimation of transformation parameters between two point patterns", IEEE TPAMI
13(4), 1991; B. Iglewicz and D. Hoaglin, "How to Detect and Handle Outliers", ASQC, 1993.

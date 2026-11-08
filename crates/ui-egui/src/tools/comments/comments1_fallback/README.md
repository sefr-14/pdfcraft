# COMMENTS.1 fallback (inactive)

This fork (sefr-14/pdfcraft). SYNC.16 trials upstream PdfCraft's comment selection (#3484, in
v0.19.0): Ctrl/⌘- and Shift-click select several comments on the page and in the Comments panel,
the one clicked last has the handles, Delete and Backspace delete them all as one undo step, a
drag moves them together, and they copy and paste as XFDF. It is the only selection model in use.

COMMENTS.1's own panel selection is kept here, **not compiled** (no `mod` declaration), so the
owner can restore it after the consolidated Windows acceptance run:

| File | What it was |
|---|---|
| `group.rs`, `group/tests.rs` | the panel's own card selection (`Group`, `CommentKey`): cards held by object, apart from the active comment; its unit tests |
| `delete_group.rs` | Delete selected: one confirmation with the exact comment and reply counts from the engine's dry run, refused whole if the document changed since |

Also inactive:

- `crates/ui-egui/tests/it/comments1_fallback.rs`: COMMENTS.1c's 15 panel tests exactly as they
  were at Main `ce9ade4b` (not declared in `tests/it/main.rs`). `tests/it/comments_selection.rs`
  keeps those that still hold under upstream's model and adds a counterpart for each that doesn't.
- Lines marked `COMMENTS.1 fallback (inactive, SYNC.16; f3ccab72)`, commented out where they were:
  `tools/comments.rs` (the `mod` lines), `tools/comments/state.rs` (`group`, `delete_request`),
  `tools/comments/input.rs` (Escape, the Delete guard), `canvas.rs`, `editing.rs`, `dialogs.rs`,
  `state.rs` (`Dialog::DeleteComments`), `lib.rs`, `app.rs`, `panels/right.rs`.
- `panels/comments_panel.rs` is upstream's file since SYNC.16f; COMMENTS.1c's version of it is in
  `f3ccab72` (and Main `ce9ade4b`).

Still active, because they are not a selection model: COMMENTS.1a (comments carry their object
reference) and COMMENTS.1b (`Edit::DeleteAnnotations`, its dry run and stale-target refusal,
`comment_delete_many`), and Collapse and Expand selected, which read upstream's selection
(SYNC.16g).

## Restoring COMMENTS.1

`git revert` of SYNC.16f is a starting point, not the whole of it: upstream's comment code came
in with SYNC.16a, and later commits build on it. FORK-CHANGES.md § "COMMENTS.1 fallback" gives the
full procedure: which upstream handlers to turn off so that only one selection model is active,
which later commits will conflict, and the tests to run before the restoring PR.

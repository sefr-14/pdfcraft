# COMMENTS.1 fallback (inactive)

This fork (sefr-14/pdfcraft). SYNC.17 trials upstream PdfCraft's comment selection (#3484, as it
is in v0.21.1): Ctrl/⌘- and Shift-click select several comments on the page and in the Comments
panel, the one clicked last has the handles, Delete and Backspace delete them all as one undo
step, a drag moves them together, and they copy and paste as XFDF. A press on a group's member
picks the member (`CommentView::member`, `target`), and each selected slot is checked against the
document after an edit (`CommentView::from_info`, `document_changed`). It is the only selection
model in use.

COMMENTS.1's own panel selection is kept here, **not compiled** (no `mod` declaration), so the
owner can restore it after the consolidated Windows acceptance run:

| File | What it was |
|---|---|
| `group.rs`, `group/tests.rs` | the panel's own card selection (`Group`, `CommentKey`): cards held by object, apart from the active comment; its unit tests |
| `delete_group.rs` | Delete selected: one confirmation with the exact comment and reply counts from the engine's dry run, refused whole if the document changed since |

Also inactive:

- `crates/ui-egui/tests/it/comments1_fallback.rs`: the COMMENTS.1c panel tests whose behaviour
  upstream's model replaces, exactly as they were at Main `ce9ade4b` (not declared in
  `tests/it/main.rs`). `tests/it/comments_selection.rs` keeps those that still hold under
  upstream's model and adds a counterpart for each that doesn't; the PR's test disposition table
  lists each of the 15.
- Lines marked `COMMENTS.1 fallback (inactive, SYNC.17; f3ccab72)`, commented out where they were:
  `tools/comments.rs` (the `mod` lines), `tools/comments/state.rs` (`group`, `delete_request`),
  `tools/comments/input.rs` (Escape, the Delete guard), `canvas.rs`, `editing.rs`, `dialogs.rs`,
  `state.rs` (`Dialog::DeleteComments`), `lib.rs`, `app.rs`, `panels/right.rs`.
- `panels/comments_panel.rs` is upstream's v0.21.1 file since SYNC.17f, with Collapse and Expand
  selected added back (SYNC.17g); COMMENTS.1c's version of it is in `f3ccab72` (and Main `ce9ade4b`).

Still active, because they are not a selection model: COMMENTS.1a (comments carry their object
reference) and COMMENTS.1b (`Edit::DeleteAnnotations`, its dry run and stale-target refusal,
`comment_delete_many`, following upstream's group rules since SYNC.17a), and Collapse and Expand
selected, which read upstream's selection (SYNC.17g).

## Restoring COMMENTS.1

`git revert` of SYNC.17f is a starting point, not the whole of it: upstream's comment code came in
with SYNC.17a, and later commits build on it. FORK-CHANGES.md § "COMMENTS.1 fallback" gives the
full procedure: which upstream handlers to turn off so that only one selection model is active
(the page and panel Ctrl/Shift handlers, `also`, `member`/`target` and the group pick), which later
commits will conflict, and the tests to run before the restoring PR.

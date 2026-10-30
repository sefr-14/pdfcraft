<!-- This fork (sefr-14/pdfcraft): UPD.1. -->
# PDF Overlay updates (UPD.1)

**Status:** implemented and tested in code; the end-to-end Windows acceptance below is still owed,
and it needs an owner signing key and a published package first. Revision history at the end.

PDF Overlay's Windows x64 portable build updates itself. Help › Check for updates finds an
**owner-approved** build, and **Update and Restart** downloads it, verifies it, closes PDF Overlay
through the usual save questions, replaces the program files, restarts, and keeps the update only
once the new build confirms that it started. Otherwise the previous files are restored. Nothing
installs without the user's explicit click.

## User experience

- **Help › Check for updates** shows one of these: the approved build (*PDF Overlay — Update
  available*), up to date, or why the check failed.
- **Remind me in:** 1 day, **1 week** (the default) or 1 month.
  - *Remind me later* saves that update's sequence and the date in the settings (`app.ron`,
    `"updates"`).
  - Dismissing the dialog (Esc, ✕, a click outside) does the same with the selected interval.
- **No check at start**, the owner's OV8 decision, with one exception: when a postponed reminder
  has come due, the app checks once, quietly, 10 s after starting. Failures are logged and not
  shown. An offer is shown only if it is newer than the postponed one or the reminder is due.
  There is at most one automatic prompt per session, and a due reminder never installs anything.
  A manual check always shows the current offer.
- **Update and Restart:**
  1. Download with progress (cancellable).
  2. Verify.
  3. Check what's still running: overlay compare, printing or optimizing, documents opening,
     other PDF Overlay windows or `pdfcraft-cli` running from the same folder. If anything is,
     it says what and offers *Try again* or *Cancel*.
  4. Quit through the existing Save / Don't save / Cancel questions. Cancel installs nothing.
- After the restart, the dialog reports the result: installed, or "the previous version was
  restored" with the reason.
- A successful update clears the postponement.

## Distribution: Forgejo Generic Package Registry

- **Packages, owned by `sefr`:**
  - `pdf-overlay-windows-x64`, one version per approved build (`<workspace version>-ov.<seq>`,
    e.g. `0.15.0-ov.7`). It holds the inner portable zip from the `overlay-windows.yml`
    artifact, byte for byte.
  - `pdf-overlay-channel`:
    - version `stable` holds `channel.json`, the one mutable pointer;
    - version `<seq>` keeps every approved envelope for the record.
- **Approval is the owner's signature**, not the upload. The app ignores any package that no
  signed envelope names, and a successful CI build is never an approved update.
- **The network path is the existing `git.ferrive.me` under its Cloudflare Zero Trust rules.**
  There is no new hostname, bypass or policy, and no credential ships in the app.
  - The client follows redirects only within that host.
  - A redirect elsewhere (the Access sign-in page), a 401 or 403, or an HTML answer is reported as
    *The update server asked for sign-in or refused access*. It is never parsed.
  - **Open item:** user-based Access rules sign people in through a browser cookie, which the
    app's HTTP client doesn't have. The functional test below must show whether the existing
    rules let an authorized Windows copy download. If they don't, that is reported, not worked
    around.

## Trust: one signed envelope

`channel.json` is one object holding the exact manifest bytes (base64), the key id, the schemas,
and an Ed25519 signature over this message:

```
"PDF-OVERLAY-UPDATE-MANIFEST\0" ‖ u32be(envelope_version) ‖ u32be(manifest_schema) ‖ key_id ‖ "\0" ‖ manifest
```

A client never mixes a manifest and a signature from different publications.

The manifest names:
- the product, platform, package kind and channel;
- the approved **sequence** (the only thing that orders updates);
- the distribution and workspace versions;
- the full commit, the repository and the Actions run and artifact;
- the approval and expiry times;
- the package URL, size, SHA-256 and root folder, and every file with its size and SHA-256;
- release notes in English and Spanish.

The app accepts an envelope only when every one of these holds
(`crates/overlay-update/src/eligibility.rs`):
- The key is trusted.
- The signature is valid.
- The product is `pdf-overlay`, the platform `windows-x86_64`, the kind `portable-zip`, the channel
  `stable` and the repository `sefr-14/pdfcraft`.
- The schema is supported.
- The approval hasn't expired.
- The sequence isn't below the highest one seen (no replays).
- The URL is exactly `https://git.ferrive.me/api/packages/sefr/generic/pdf-overlay-windows-x64/<version>/<file>`.
- Every file is an allowed, safe path.

It is offered only when its commit differs from the running build's and its sequence is above
the installed one.

**Keys.** Public keys are compiled into `apps/pdfcraft/src/updates/mod.rs` (`KEYS`). **The list
is empty today**, so every update is refused as unverifiable until the owner creates a key
(decision 2). The private key never goes into the repository, CI, the zip or Forgejo. Rotation
works by shipping the new key in a build before signing with it.

**Build identity.** `apps/pdfcraft/build.rs` embeds `GITHUB_SHA` and `GITHUB_RUN_ID` for builds of
`sefr-14/pdfcraft`, so `overlay-windows.yml` is unchanged. A build without them is a development
build and never installs updates. After a successful update, `update-receipt.json` beside the
executable holds the accepted envelope. It is re-verified and cross-checked against the running
commit before its sequence counts. A manual install has sequence 0.

## Files: what an update may touch

The rules are in `crates/overlay-update/src/paths.rs`.

- **A, allowed in the package:**
  - `pdfcraft.exe`, `pdfcraft-cli.exe`, `portable.txt`, `LICENSE-MIT`, `LICENSE-APACHE`,
    `NOTICE`, `README.md`, `LICENSE-document.txt` and `pdfcraft-document-orange.ico`;
  - `licenses/*.txt|*.md` and `licenses/LICENSE_FOXIT`;
  - `licenses/epaint_default_fonts/*.txt`.

  Anything else refuses the whole package. So do traversal, absolute, drive or UNC paths,
  alternate data streams, device names, trailing dots, case-insensitive duplicates, links,
  encryption, compression other than stored or deflate, and implausible compression ratios.
- **B, replaceable:** A without the portable markers. Only these are backed up, replaced or
  (when a failed update introduced them) removed.
- **C, never touched:**
  - `PdfCraftData/**`, which holds the settings, Rulers & Guides, recent files, logs, Recovery,
    Digital IDs and the clipboard;
  - `portable.txt` and `PdfCraft.portable`. Their presence and their bytes are both kept, so an
    update never switches between portable and per-user settings;
  - unknown files, user documents and the per-user folders.

## The transaction

Everything lives in `<install>\.pdf-overlay-update\<txn>\`, on the same volume as the
installation:
- `journal.json`
- `package.zip`
- `new\`
- `backup\`
- `restore-or-apply.exe` (the helper)
- `verify.json`
- `update.log`
- `RECOVER.txt`

A lock file allows one update at a time.

1. **Prepared** (the app is running and nothing installed has changed):
   1. Verify the envelope again.
   2. Probe that the folder can be written; check free space for size + 2 × unpacked + 16 MB.
   3. Stream the download, checking size and SHA-256.
   4. Validate the archive and extract it with `create_new`, re-hashing from disk.
   5. Journal every B file: whether it existed, its old hash, its new hash.
   6. Copy the running, known-good `pdfcraft.exe` as the helper and record its hash.
2. Quit through the usual flow. After the window closes, the app starts the helper:
   `restore-or-apply.exe --overlay-update-apply <txn> <pid>`. The helper only works on its own
   folder.
3. **The helper:**
   1. Waits until the app and every process using the installation have exited (60 s).
   2. Re-hashes the staged files.
   3. **BackingUp:** copies, flushes and re-hashes every file it will replace.
   4. **Installing:** moves each file into place with a same-volume rename, the application last,
      journaling each one, and retries sharing violations from antivirus scans. Several files are
      not replaced atomically as a group; the journal makes the update recoverable instead.
   5. **InstalledUnverified:** starts the new build with `--overlay-update-verify <txn> <nonce>`.
4. **Handshake:**
   - The new build checks that its own commit is the approved one. If it isn't, it reports a
     failure and exits.
   - After restoring its settings and drawing its first frame, it writes `verify.json` with the
     nonce and its commit.
   - The helper waits up to 120 s, then watches the process for 10 s.
   - Confirmed: **Verified**, then the receipt is written, then **Committed**. The previous version
     stays in `backup\` until the next update.
   - Exited without confirming, reported a failure, or the wrong identity: roll back once the
     process has exited.
   - Still running without confirming: **no rollback while it runs**. The next start decides.
5. **Rollback** (idempotent; it acts on file content, not only on journal flags):
   1. Every pre-existing file whose content differs from its verified backup is restored through a
      temporary file and a rename.
   2. Every introduced file is removed, but only while it still holds the new content.
   3. Folders the update created are removed when empty.
   4. Then **RolledBack**, or **RecoveryRequired** if anything couldn't be done.
   5. The helper restarts the previous app with `--overlay-update-result <txn>`.

**Startup recovery (every normal start, before any window).** A running `pdfcraft.exe` never
restores, replaces or deletes installed files. It only detects an interrupted update and hands it
to the helper:

| Journal state | The starting app | The helper (after every process has exited) |
|---|---|---|
| Prepared | discards the staging | nothing |
| BackingUp, Installing, RollingBack, InstalledUnverified (another build) | verifies the helper's hash, starts `--overlay-update-recover`, exits | rolls back, then restarts the restored app with the user's files |
| InstalledUnverified (this is the new build) | commits after its first frame | nothing |
| Verified | writes the receipt | nothing |
| RecoveryRequired, or a missing or changed helper | starts normally and shows the manual steps | nothing |

When other windows run from the folder, or the start comes straight from the helper, the repair
waits for a later start: there are no loops, and nothing is restored while anything runs.

**Manual recovery** (`RECOVER.txt`, English and Spanish):
1. Close PDF Overlay.
2. Double-click `restore-or-apply.exe` in the transaction folder. It runs the same rollback and
   restarts the app.
3. Failing that, copy `backup\` back and delete the files the journal marks as introduced.

**Cleanup.** At startup or preparation, with the lock free:
- discard abandoned Prepared transactions;
- remove committed transactions older than the newest one;
- delete RolledBack transactions after 7 days.

## Publishing an approved build (owner, on their own machine)

1. Run `overlay-windows.yml` (unchanged) and download its artifact. It holds
   `pdfcraft-overlay-<ver>-<sha7>-windows-x64-portable.zip` and its `.sha256`.
2. Once only, make the signing key and add its public key to `KEYS` in a reviewed PR:
   ```bash
   cargo xtask overlay-publish keygen --id ov-a --out <offline path>
   ```
   Keep the key offline and backed up.
3. Sign:
   ```bash
   cargo xtask overlay-publish sign --zip <zip> --sha256 <zip>.sha256 --commit <full sha> --run-id <run> --seq <N> --key <key> --key-id ov-a --out <dir> [--previous <current channel.json>] [--notes-en f] [--notes-es f]
   ```
   It checks the zip with the app's own validator and refuses a commit that doesn't match the
   artifact. It writes `channel.json` and `manifest.json` for review, and prints the upload
   commands. **It uploads nothing.**
4. Run the printed `curl` commands with your own Forgejo token, in order: package, archived
   envelope, then delete and re-upload `stable`. Clients that ask in between get a 404 and try
   later.

## Windows acceptance (before merge, with separate owner approval for builds and hosting)

Use two controlled UPD.1-capable builds, A (sequence 1) and B (sequence 2), built with
`PDF_OVERLAY_UPDATE_TEST_KEY=<id>:<hex>` (and `PDF_OVERLAY_UPDATE_TEST_HOST` for a controlled
HTTPS host) and signed with a **test** key. Then:

- **Discovery:**
  - no update; B available; same build;
  - wrong product, platform or channel; expired envelope; bad signature; older sequence;
  - off-host redirect; server down;
  - the Access sign-in redirect reported as such.
- **Reminders:**
  - 1 day, 1 week and 1 month (clock shifted), and after a restart;
  - a manual check while postponed; a newer sequence while postponed;
  - a successful update clears the postponement.
- **Download:** hash mismatch, truncated or interrupted download, corrupt zip, unexpected entry.
- **Install:**
  - A→B success (record the identities, hashes, journal, `verify.json` and receipt);
  - B′ whose exe exits at once → rollback;
  - B″ that adds a licence file and fails → the new file is removed;
  - a locked `NOTICE` (a share-deny handle) → rollback;
  - an unwritable folder; low disk (a small VHD);
  - kill the helper during Installing, then start the app → the helper repairs it after the app
    exits;
  - a corrupted `pdfcraft.exe` → double-click `restore-or-apply.exe`.
- **App:**
  - unsaved document (Save, Don't save, Cancel); active overlay compare;
  - a second `--new-window` and `pdfcraft-cli mcp` running;
  - portable and non-portable modes;
  - preferences, Rulers & Guides, recent files, Digital IDs, recovery data and documents intact.

  The app legitimately rewrites its logs and settings, so `PdfCraftData` is checked by content and
  by the updater's operation log, not byte for byte.
- **Zero Trust:** an authorized Windows copy fetches `channel.json` and the zip from
  `git.ferrive.me` under the existing rules.

No upstream binary is involved anywhere: they aren't signed by these keys and don't name this
product.

## Automated tests

- `crates/overlay-update`:
  - the envelope, manifest, eligibility, paths, archive and reminder rules;
  - fault injection at every operation of the backup and install, with rollback back to the
    exact old tree;
  - an interrupted rollback re-run;
  - foreign files left alone; damaged backups never restored; protected files never touched;
  - the startup table;
  - the helper against fake processes, never changing an installed file while anything runs;
  - `tests/recovery_processes.rs` with real child processes.
- `crates/ui-egui/tests/it/updates.rs`: the dialog, the reminders and their persistence, the due
  check, dismissal, the hand-off, the Cancel at a save question, blockers, and the Spanish strings.
- `apps/pdfcraft`:
  - argument handling and redirect rules;
  - a guard against upstream, `/releases`, `craft-update` and credentials in the code.

## Owner decisions still open

1. Package owner and visibility: `sefr`, readable past Zero Trust without a token.
2. Signing-key custody and the backup key. **Blocks the first real publication.**
3. The sequence and channel scheme: `<ver>-ov.<seq>`, `stable` only.
4. The automatic check and dismissal behaviour, as implemented above.
5. Backup retention: one previous version.
6. Removing obsolete owned files: not done in UPD.1.
7. The `zip` dependency: added (MIT).
8. Whether the existing user-based Zero Trust rules admit a native client. **Blocks end-to-end
   delivery.**

## Revision history

- 2026-10-30 — UPD.1 implemented (crate `pdfcraft-overlay-update`, app installer and helper, UI,
  `xtask overlay-publish`).

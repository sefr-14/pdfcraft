# Building the W0 spike on GitHub Actions: inspection and proposals

Nothing in this folder is applied. Each `.patch` changes a fork workflow, which needs the
owner's explicit approval, in a commit of its own that the pull request (or the scratch branch)
names (AGENTS.md, "This fork", CI exception). Until then the spike is built locally on Windows
with `../build.ps1`.

## The eight workflows on GitHub `Main` (at e2085e24)

| File | Triggers | Inputs | Permissions | Runner | What it builds or does | Can it build the spike unmodified? |
|---|---|---|---|---|---|---|
| `automation-tests.yml` | pull_request and push to Main (paths: scripts/automation, two sync workflows, itself); workflow_dispatch | none | contents: read | ubuntu-latest | Python unit tests of the sync scripts | No: no Rust build, Linux only |
| `forgejo-pr-sync.yml` | pull_request_target into Main; workflow_dispatch | `pr_number` (number, required) | contents: read, pull-requests: read; environment `forgejo-sync` with secrets | ubuntu-latest | Bridges a GitHub PR to Forgejo; checks out Main only | No, and it must not be used: it holds Forgejo secrets |
| `forgejo-upstream-sync.yml` | schedule 05:17 UTC daily; workflow_dispatch | `branch_version` (string, optional) | contents: read; environment `forgejo-sync` with secrets | ubuntu-latest | Trial-merges upstream into Forgejo | No, same reason |
| `fuzz.yml` | schedule (runs only in storytold/pdfcraft); workflow_dispatch | none | contents: read | ubuntu-latest | `cargo xtask corpus` + `cargo xtask fuzz` on the app | No: Linux, app fuzzing only |
| `overlay-windows.yml` | workflow_dispatch only | none | contents: read | windows-latest | The app (`-p pdfcraft -p pdfcraft-cli`, release, MSVC) after testing two crates; uploads a portable zip artifact | No: dispatched on the scratch branch it runs that branch's copy, which is unchanged and builds the app, not `w0-spike/` (its own `[workspace]`, not a workspace member) |
| `packaging-lint.yml` | push to main/release and PRs (paths: workflows, packaging, app icon); workflow_dispatch | none | contents: read | shared workflow (Linux); a Windows job only in storytold/pdfcraft | Workflow and packaging lint | No |
| `release.yml` | push to `release`; workflow_dispatch | `version` (string) | **contents: write**, signing secrets | shared release workflow | Signed release of the app | No, and it must never be used for this |
| `sonarcloud.yml` | push to main; pull_request | none | contents: read | shared workflow, only in storytold/pdfcraft | SonarCloud analysis | No (not dispatchable; skipped in the fork) |

**Result: no registered workflow can build the spike without a change.** Every option below
also needs the spike's commit on GitHub, which means pushing `scratch/tab-window-w0` (or another
branch holding it). That is publishing, which the owner decides.

How dispatch picks the file: for `workflow_dispatch`, GitHub runs the copy of the workflow
file at the ref the run is dispatched on, provided a workflow with that file name exists on
the default branch. `overlay-windows.yml` exists on Main, so a changed copy on the scratch
branch would run when dispatched on that branch, with Main's copy untouched.

## Option A: change `overlay-windows.yml` on the scratch branch only

**Chosen by the owner, subject to review of the exact change.** It is applied on the scratch branch
as its own commit ("W0 spike: scratch-only Windows build of the spike (fork workflow change,
option A)"), with one addition to the first draft: a step that parses the `.ps1` scripts with
Windows PowerShell's parser, since they could not be parsed where they were written. A corrective commit then made
the header neutral, named the exe's build path (`target\x86_64-pc-windows-msvc\release`) and its
packaged path (`target\release`, where `run.ps1` looks), and added checks that the exe and every
required file are present and non-empty in the package and in the zip. The patch here is the
whole change from Main's file to the final file, so applying it to Main's copy reproduces the
scratch branch's file exactly. Nothing is pushed or dispatched until the owner approves.

`A-scratch-branch-overlay-windows.patch` replaces the scratch branch's copy so that it builds
`w0-spike` for `x86_64-pc-windows-msvc` and uploads a zip artifact. Main is not touched.

- Inputs: none (as today). Permissions: `contents: read`. Runner: `windows-latest`. No secrets.
- Ref selection: the run builds the commit at the tip of the branch it is dispatched on.
- Artifact `w0-spike-<sha7>-windows-x64`: `target\release\w0-spike.exe`, `windows\` (these
  scripts and docs), `W0-REPORT.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `COMMIT.txt`, plus a
  `.zip.sha256`. Retention 14 days.
- Steps, after approval:
  1. `git apply w0-spike/windows/proposals/A-scratch-branch-overlay-windows.patch` on
     `scratch/tab-window-w0`; commit it alone ("W0 spike: scratch-only Windows build of the
     spike (owner-approved fork workflow change)").
  2. `git push origin scratch/tab-window-w0` (publishing; owner's decision).
  3. Actions > "overlay windows build" > Run workflow > Branch: `scratch/tab-window-w0`, or
     `gh workflow run overlay-windows.yml --repo sefr-14/pdfcraft --ref scratch/tab-window-w0`.
  4. Download the artifact from the run page, check the SHA-256, unzip, run
     `windows\run.ps1 -Exe <unzipped>\target\release\w0-spike.exe` (no build needed).
- Risks: the branch's workflow has the same name and concurrency group as the real one (group
  includes the ref, so they do not cancel each other). Delete the branch when W0 closes, so no
  stray copy remains.

## Option B: a new dispatch-only workflow on Main

`B-new-workflow-on-Main.patch` adds `.github/workflows/w0-spike-windows.yml` to Main through its
own pull request. It takes one input, the full commit SHA to build, checks that commit out and
builds `w0-spike` there.

- Inputs: `commit` (40 lowercase hex characters, validated). Permissions: `contents: read`.
  Runner: `windows-latest`, 60-minute timeout. No secrets.
- Ref selection: dispatched on Main; the build uses exactly `inputs.commit`, which must already
  be on GitHub (a pushed scratch branch).
- Artifact: as in option A, named after the commit.
- Steps, after approval: a PR into Main containing only this file (commit of its own), merged
  by the owner on Forgejo; push the scratch branch; then
  `gh workflow run w0-spike-windows.yml --repo sefr-14/pdfcraft --ref Main -f commit=<sha>`.
- Cost: a workflow on Main for a disposable spike; remove it when W0 closes.

## Option C (no approval needed): local build

`../README.md`: clone or unbundle on a Windows machine, `build.ps1`, `run.ps1`, `collect.ps1`.

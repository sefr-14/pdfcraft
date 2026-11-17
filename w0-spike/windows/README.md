# W0 spike: Windows validation package

A disposable test tool for TAB.WINDOW.1 W0. It is not PDF Overlay, opens no documents and
writes only its own logs. This folder builds it, runs every check on a Windows machine and
packs the results for return. Nothing is uploaded or published by these scripts.

Files:

| File | What it does |
|---|---|
| `build.ps1` | Builds `..\target\release\w0-spike.exe` (both renderers in one binary), records rustc version and commit |
| `run.ps1` | Runs the automated checks (wgpu, then glow), then pauses for each manual check; writes `w0-results\<timestamp>\` |
| `CHECKLIST.md` | Every check: what to observe, what to record, pass/fail rule |
| `collect.ps1` | Adds system information (Windows version, GPU, monitors and scale) and zips `w0-results\` |
| `proposals\` | Options for building on GitHub Actions instead; none applied (owner approval needed) |

## 1. Prerequisites

- Windows 10 or 11, x64. For the mixed-DPI checks: two monitors, one at 100 % and one at 150 %
  (Settings > System > Display > Scale).
- **Visual Studio 2022 Build Tools** with the **"Desktop development with C++"** workload (the
  MSVC linker and Windows SDK). Installer: <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
- **Rust**, stable, MSVC host: install from <https://rustup.rs> and accept the default
  `x86_64-pc-windows-msvc`. Check: `rustc -vV` shows `host: x86_64-pc-windows-msvc`.
- **Git for Windows**: <https://git-scm.com/download/win>.
- Internet access during the first build: Cargo downloads crates from crates.io and the pinned
  egui and winit forks from github.com/storytold (public), exactly the revisions in
  `w0-spike\Cargo.lock`.
- PowerShell: the built-in Windows PowerShell 5.1 is enough. Run the scripts with
  `powershell -ExecutionPolicy Bypass -File ...` (this changes nothing permanently).

## 2. Get the code

### (a) Once the branch is published (owner decision; not done)

```
git clone https://github.com/sefr-14/pdfcraft.git pdfcraft-w0
cd pdfcraft-w0
git fetch origin scratch/tab-window-w0
git checkout <commit SHA given in the report>
```

### (b) Without publishing: the git bundle

The file `tab-window-w0.bundle` holds the branch `scratch/tab-window-w0` with its history. Copy
it to the Windows machine (USB stick, file share), then:

```
git clone -b scratch/tab-window-w0 tab-window-w0.bundle pdfcraft-w0
cd pdfcraft-w0
git log -1 --format=%H        # compare with the commit SHA given in the report
git bundle verify ..\tab-window-w0.bundle
```

`git clone` from a bundle needs no network. The clone's `origin` points at the bundle file, so
nothing can be pushed anywhere by accident.

### (c) From a GitHub Actions artifact (only if the owner approves a proposal in `proposals\`)

Unzip the artifact; it already contains `target\release\w0-spike.exe` and this folder. Skip
the build.

## 3. Build (5 to 15 minutes the first time)

```
cd pdfcraft-w0\w0-spike
powershell -ExecutionPolicy Bypass -File windows\build.ps1
```

It prints the rustc version and commit and writes `windows\w0-results\build-info.txt`.

## 4. Run

```
powershell -ExecutionPolicy Bypass -File windows\run.ps1
```

- **Automated part (about 10 minutes):** runs each check with wgpu, then with glow. It opens,
  closes and clicks windows by itself: do not touch mouse or keyboard until it pauses.
- **Manual part (about 45 minutes):** at each pause the window shows what to do; press Enter
  to continue. `-SkipManual` runs only the automated part.
- Useful options: `-Only basic,closes` (run some checks; `-List` prints the names),
  `-Renderers glow`, `-Attempts 50` (posted closes per configuration).
- Some FAIL rows are expected by design (they reproduce known crashes); `CHECKLIST.md` says which.

Each run of the spike writes a log next to `results.csv`. Its `RESULT` lines are copied into
`results.csv` (`check,renderer,attempt,outcome,details`).

## 5. Return the results

```
powershell -ExecutionPolicy Bypass -File windows\collect.ps1
```

It writes `windows\w0-results-<computer>-<timestamp>.zip`. Send that file back.

## Spike settings (for running by hand)

`w0-spike.exe mode=<mode> renderer=<wgpu|glow> log=<file> [name=value ...]`; each setting can
also be given as the environment variable `SPIKE_<NAME>`.

| mode | What it shows |
|---|---|
| `basic` | root + A (unsaved) + B; close held then accepted; texture shared; counters |
| `h1`, `h2`, `h3` | root handoff variants (re-home; hide root; root never shown) |
| `drag` | tab strips; drop classification valid / invalid / desktop / Escape; new window at drop point |
| `fail` | `fail_case=huge|nan|inf|negative|zero|farpos`, unclamped; `fail_drop=1` abandons the window |
| `clamp` | the same six requests, clamped first |
| `panic` | panic inside a secondary window; `inner_guard=1` catches it inside the closure |
| `devlost` | wgpu `Device::destroy()` (simulated device loss); `uncaptured=1` installs an error handler |
| `closes` | every close held and counted; `anim=1`, `tick_ms`, `secondaries`, `hide_root=1`, `secs` |
| `a11y` | single-window fallback when a screen reader is active (`single_window=1` forces it) |
| `embed`, `solo` | embedded-viewport fallback; a single window |

`time_scale=N` stretches every scripted time N-fold for manual runs.

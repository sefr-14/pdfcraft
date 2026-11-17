# TAB.WINDOW.1: W0 feasibility spike report

## Revision 1 (2026-11-17)

The owner reviewed the first version of this report (the original text follows from §1 on) and corrected four classifications. They are applied in the §2 table:

- **W0-7: FAIL.** Its acceptance criteria were not demonstrated. It may become "partially mitigated" only for the failure classes actually shown to be contained, and stays FAIL while class (c), failures inside eframe, winit or the graphics backend, is not shown containable. The first version's §7 change 2 described a mitigation; it was never demonstrated.
- **W0-9: INCONCLUSIVE.** The first version said PASS with a flake. Close events were lost (about 6 of 18 root closes with wgpu), so per-window close is not shown reliable.
- **W0-6: PENDING-WINDOWS.** Mixed DPI could not be emulated. The single-DPI Linux observation is not evidence for it.
- **W0-8: Windows PENDING.** H1 was observed on X11 only.

Follow-up work in this revision: §11 (W0-7 by failure class), §12 (W0-9 lost close events), §13 (accessibility of secondary windows and the single-window fallback), §14 (Windows validation package). The §1 rerun commands and §9 Windows steps are superseded by `windows/README.md` and `windows/CHECKLIST.md`.

Statements of the first version that this revision corrects:
- §3 item 2, §7 change 2 and §8 called validation "the" mitigation for creation failure. It prevents class (a) only (§11).
- §8 called the close loss "unexplained" and suspected Mesa. It was the test sender (§12).
- §2a and §8 noted the missing AccessKit in secondary windows. §13 now shows it with a screen-reader view (AT-SPI): one window exposed out of three.
- §10 condition 1 stands, with class (c) added to the open risks.

All Linux evidence in this revision comes from debug builds on Xvfb with software rendering (Mesa llvmpipe/lavapipe).


Evidence tags: **[source]** means read in the pinned source. **[linux-x11]** means observed by running the spike here (Xvfb, Mesa llvmpipe/lavapipe, software rendering). **[windows-pending]** means it cannot be established here. Nothing below claims Windows behaviour from Linux evidence. Paths shortened to `egui/…` mean `~/.cargo/git/checkouts/egui-057e9a0485eda407/725a39f/crates/…`, and `winit/…` means `~/.cargo/git/checkouts/winit-0b02aa607dc0343f/a517b38/…`.

## 1. Workspace and baseline

- **Worktree:** `/home/user/pdfcraft-w0`, branch `scratch/tab-window-w0`, base `e2085e24` (Main). I modified no existing repository file.
- **Spike:** `/home/user/pdfcraft-w0/w0-spike/`, a standalone crate with an empty `[workspace]`. It is committed locally on the scratch branch only (`c20615ce`, `2d0bac76`) and has not been pushed.
  - `src/main.rs`: the spike. Its modes are `basic`, `h1`, `h2`, `h3`, `fail`, `embed`, `drag`, `solo` and `panic`.
  - `src/bin/xclose.rs`: sends X11 `WM_DELETE_WINDOW`, the same message a window manager's close button sends.
  - `run.sh`: the Xvfb harness. It uses xdotool, xwininfo and import, and starts openbox when `WM=openbox` is set.
  - `tests/kittest_viewports.rs`: the W0-11 test.
- **Pinned revisions, identical to the app:**
  - egui, eframe, egui-winit, egui-wgpu, egui_glow and egui_kittest 0.37.0 at storytold/egui `725a39fca940f77149ffbb9c58eef405a16997bc`.
  - winit 0.30.13 at storytold/winit `a517b3846ee8457d84ee4975110bfda6797c44dc`.
  - wgpu 30.0.1.
  - The repository's `[patch.crates-io]` egui and winit lines were copied, and the root `Cargo.lock` was copied before the first build.
  - eframe features: `default_fonts`, `wgpu`, `glow`, `x11`, `wayland`, `accesskit`. egui_kittest features: `eframe`, `wgpu`.
- **Renderers tested:** wgpu (Vulkan on lavapipe/llvmpipe, which logs "Software rasterizer detected") and glow (GLX 1.4 on Mesa).
- **X server:** Xvfb 1920x1080x24.
  - Default DPI gave `native_pixels_per_point` = 1.0833.
  - One run used `-dpi 192`, which gave 2.0.
  - Most runs had no window manager. The drag and H1 runs were repeated under openbox.
- **Build:** `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, target dir `w0-spike/target` (1.5 GB). Free disk stayed at 9.4 GB or more, so the target dir was kept.
- **Rerun:**
  ```
  cd /home/user/pdfcraft-w0/w0-spike
  export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=$PWD/target
  cargo build --bins && cargo test --test kittest_viewports -- --nocapture
  xvfb-run -a -s "-screen 0 1920x1080x24" ./run.sh <mode> <wgpu|glow> <outdir>     # add WM=openbox for a window manager
  ```
  Each run writes `<outdir>/<mode>-<renderer>.log` (the spike and eframe debug log), `.harness.log` and `.screen.png`. Scripted times count from the first root pass. `SPIKE_TIME_SCALE=N` stretches them for manual runs.
- **Raw logs** are under `/tmp/claude-0/-home-user-pdfcraft/0a245263-da88-5626-b700-eddd6fb5d8ba/scratchpad/w0/` in the folders `out`, `out2`, `out3`, `out4`, `dpi`, `guard` and `final`.

## 2. Results W0-1 … W0-13

| # | Verdict | Evidence |
|---|---|---|
| **W0-1** second native window from `&mut` app state | **PASS** [linux-x11][source] | `Spike::ui` takes each `WindowUi` out of `self` with `mem::take`, then calls `ctx.show_viewport_immediate(id, builder, \|ui, class\| self.secondary(ui, class, &mut w, …))`. The closure borrows `&mut self` and the moved-out `WindowUi`, with no `Arc<Mutex>`. Log: `W0 sec A: first pass class=Immediate ctx.viewport_id="4477" parent="FFFF"`. xwininfo showed three mapped OS windows. Source: `egui/egui/src/context.rs:4743-4799` (the `FnMut` closure is run synchronously by the backend's immediate renderer) and `egui/eframe/src/native/wgpu_integration.rs:1222-1300`. |
| **W0-2** wgpu and glow, shared texture | **PASS** [linux-x11] | Both renderers drew root plus two secondaries. A 64×64 red/green texture made once by `ctx.load_texture` in the root pass was drawn in all three windows. Pixel samples from the X screen capture: root (86,329)=red, (163,329)=green; A (910,351)=red, (987,351)=green; B (910,849)=red, (987,849)=green, identical for wgpu and glow. Source: one `tex_manager` per `Context` (`context.rs:429`), one painter or GL context shared by every viewport. Side finding: `ViewportCommand::Screenshot` sent to an immediate viewport never answers. Only the ROOT callback fired, with both renderers. Source: `wgpu_integration.rs:1336` passes `vec![]` screenshot commands for immediate viewports, and glow's immediate path has no screenshot handling. |
| **W0-3** embedded case detectable | **PASS, with a constraint** [source][linux-x11] | `ctx.embed_viewports()` (`context.rs:4615`) and the closure argument `ViewportClass::EmbeddedWindow` (`context.rs:4751-4764`) are reliable. They are set when eframe has no native viewports, when `set_embed_viewports(true)` is called, or when no immediate renderer is installed (kittest). Observed: `embed: …class=EmbeddedWindow ctx.viewport_id="FFFF"`. **Constraint 1:** in embedded mode the same widget Id drawn by root and secondary panics in debug builds, every frame: `DEBUG ASSERT: Widget Id::unique("shared_text") changed layer_id during the frame` (`egui/src/widget_rect.rs:228`) [linux-x11]. **Constraint 2:** `show_embedded_viewport` panics if the `egui::Window` is not shown or is collapsed (`context.rs:4807-4812`). To "behave as today", the app should check the flag and not create secondaries at all, keeping every tab in the root. |
| **W0-4** pointer outside window during held drag | **PARTIAL**: X11 PASS [linux-x11]; Windows [source], PENDING-WINDOWS; macOS [source]; Wayland FAIL for cross-window use [source] | **X11 (Xvfb, with and without openbox):** after a press in root, `PointerMoved` kept arriving at root at positions outside it (local `[1714.2 948.0]`, past the 700-pt width). Release outside every window was delivered to root (`button Primary pressed=false local=[1714.2 948.0]`). egui-winit sends `PointerGone` at each crossing (`egui-winit/src/lib.rs:2193-2195`), and moves continue afterwards. egui keeps the drag alive across `PointerGone` (`egui/src/input_state/mod.rs:1232-1240`). The window entered during the drag got an enter-time `moved` and then `PointerGone`. **Escape:** delivered to the grabbing (focused) root under openbox (`KEY W0 root: Escape pressed=true` while held over the desktop). With no window manager, neither window had keyboard focus and no key arrived. **Windows** [source]: winit calls `SetCapture` on every button down and `ReleaseCapture` on the last up (`winit/src/platform_impl/windows/event_loop.rs:1012-1026`, call sites 1805-1927). `WM_MOUSEMOVE` outside sends `CursorLeft`, then keeps sending `CursorMoved` (lines 1663-1726). `WM_CAPTURECHANGED` resets the count (1946-1955). **macOS** [source]: `mouse_motion` drops outside points only when no button is pressed (`winit/src/platform_impl/macos/view.rs:1417-1442`). **Wayland** [source]: motion continues under the implicit grab, but surface-local only, and window positions are unavailable (see W0-5). |
| **W0-5** desktop coordinates | **PARTIAL**: X11 PASS [linux-x11]; Wayland FAIL [source]; Windows PENDING-WINDOWS | `ViewportInfo.inner_rect`/`outer_rect` in points match xwininfo exactly (e.g. inner 800.3 pt × 1.0833 = 867 px). Under openbox outer ≠ inner (title bar ≈ 18.5 pt), both correct. Pointer desktop position = `inner_rect.min + local` agreed with `xdotool getmouselocation` (200 px / 1.0833 = 184.6 pt, logged 184.6). The two windows' estimates of the same point agreed to < 0.1 pt, well inside 2 px. **Caveats** [source]: (a) winit 0.30 has no global cursor-position API, so this sum is the only source. (b) Points are per window (each window's own `pixels_per_point`), so cross-window hit-testing must be done in physical pixels: `pts × that window's native_ppp`. (c) Wayland's `outer_position`/`inner_position` return `NotSupported` and `set_outer_position` is a no-op (`winit/src/platform_impl/linux/wayland/window/mod.rs:254-276`). (d) There is no Z-order information, so overlapping own windows, or another app's window over ours, cannot be told apart. (e) `ViewportInfo.parent` and `title` arrive as `None`/`""` for all viewports [linux-x11]. Don't rely on them. |
| **W0-6** mixed DPI, several monitors | **PENDING-WINDOWS** [windows-pending]. Only single-DPI behaviour was seen [linux-x11]; the rest is [source] | Each viewport gets its own `native_pixels_per_point = window.scale_factor()`, a `monitor_size` for its current monitor, and its own `ctx.pixels_per_point()` (`egui-winit/src/lib.rs:3681-3729`). `-dpi 192` gave ppp 2.0 with positions scaled consistently [linux-x11]. Windows is per-monitor-v2 aware (`winit/src/platform_impl/windows/dpi.rs:20-31`), and fork W1 applies the `WM_DPICHANGED` suggested rect. **Risks** [source]: `ViewportBuilder::with_position` and `with_inner_size` become `LogicalPosition`/`LogicalSize` (`egui-winit/src/lib.rs:4781-4811`), which winit resolves with a scale chosen before the window knows its monitor. `ViewportCommand::OuterPosition` and `InnerSize` multiply by the target window's current ppp (`egui-winit/src/lib.rs` `process_viewport_command`). So "new window under the pointer at the right scale" and H1 geometry copy between monitors of different DPI need physical-pixel conversion and a Windows test. Mixed DPI could not be emulated here. |
| **W0-7** window-creation failure | **FAIL; partially mitigated for classes (a) and (b) only** [linux-x11][source]; Windows PENDING [windows-pending] | Revision 1, §11. **(a) bad geometry from app code:** unclamped huge/inf crash both renderers (winit X11 `window.rs:316-317` unwrap, retried by eframe outside app code), nan crashes in debug builds (emath `pos2.rs:237` debug assert, reached from eframe); clamping before `show_viewport_immediate` prevented all six cases on both renderers (6/6, 2 runs each). **(b) panic in the secondary closure:** `catch_unwind` inside the closure: 3/3 runs per renderer survive, the frame ends, the root takes a click, the failed window closes on request and a new one opens; without it 3/3 crash (rc 101). **(c) framework failures:** a window the OS refuses (X11 BadValue, non-panicking `Err`) is recoverable by detecting "closure not called" and abandoning the viewport (both renderers); device loss panics inside egui-wgpu (`renderer.rs:1033`) with or without an app error handler, so it is **not containable** without an egui-wgpu/eframe change; glow make-current and surface failures and wgpu secondary surface failures were not forced and are classified from source only. Containment is not complete. |
| **W0-8** root handoff (T-2) | **PARTIAL; Windows PENDING** [windows-pending]: H1 was observed on X11 only [linux-x11]; Wayland degraded [source]; Windows PENDING-WINDOWS. H2 works on X11 but is costly [linux-x11][source]. H3 not supported by pinned eframe [source]. | See §4. |
| **W0-9** per-window close/quit | **INCONCLUSIVE overall; X11 cause found** [linux-x11][source]; Windows PENDING [windows-pending] | Revision 1, §12. Attribution works as before (A's close arrives in A's closure; root close held with `CancelClose`). The lost events of the first run were traced to the test harness: `xclose` exited right after flushing its `SendEvent`, and under rendering load the X server dropped some of those requests (50 of 300 lost, both renderers). With a sender that waits for the server's reply, **0 of 1,700** sends were lost across renderer × target × window manager × animation × repaint pacing × hidden root, and with the old sender kept connected 300 ms, 0 of 100. In every run eframe's count equalled the app's count, so nothing was lost inside eframe or the app. The Windows path (posted `WM_CLOSE`, real clicks) is unmeasured; `windows/run.ps1` counts it (50 per configuration). |
| **W0-10** frame cost | **PASS** (measured; idle windows are *not* skipped) [linux-x11][source] | An immediate viewport cannot paint alone. A repaint of any secondary returns `RepaintNext(parent window)` (`wgpu_integration.rs:649-662`), and the root pass reruns every immediate closure. Measured with B animating and A idle for ~1.85 s, both renderers: **wgpu** root 131 passes (71/s), **idle A 131**, B 131; whole-tree cost 9.0 ms per pass, A's own closure CPU 0.58 ms. **glow**: root 183 (97/s), **idle A 183**, B 183; 6.6 ms per pass, A 0.60 ms. Idle phase: 6 passes/s, all from the spike's own 250 ms clock. Software rendering, so absolute numbers are pessimistic. The shape holds: every frame costs Σ(all windows' UI + paint). |
| **W0-11** test harness | **PARTIAL** [linux-x11][source] | `tests/kittest_viewports.rs` passes. kittest installs no immediate renderer, so the second viewport is embedded as an `egui::Window` (`class=EmbeddedWindow`, `viewport_id=ROOT`). Its widgets are reachable and clickable by label (`clicks root=1 second=1`). Not testable in kittest: native windows, per-window `close_requested`, positions, focus, DPI, and anything keyed by `ctx.viewport_id()` (all ROOT). The W0-3 debug assert applies: two windows showing the same `Id::unique` widgets panic in kittest, which runs debug. |
| **W0-12** ids and memory across viewports | **PASS**, with rules [source][linux-x11] | **Per viewport** (`egui/src/memory/mod.rs`): `areas` (104), `interactions` (107), `focus` (110), `popups` (119); plus `ViewportState` input, pass state, `used_ids` (id-clash checking is per viewport), and the root `Ui` id `Id::unique((viewport_id,"__top_ui"))` (`viewport.rs:158-160`). Observed panel ids `Id::unique(("4477","__top_ui"))…`, so salts below the root `Ui`, e.g. `Panel` state, differ per window. Focus observed per viewport: A `focused()=shared_text`, root `None`. **Global:** `Memory::data` (48; observed `from_A=42` readable in root, and the `CollapsingState` under one `Id::unique` was shared), `caches` (76), `options`, `to_global` (keyed by `LayerId` only, 100), `animation_manager` (context.rs:417), `tex_manager` (429), fonts. **`Id::unique(x)` is deterministic** (hash with fixed seeds, `id.rs:91-97`), not per window: identical in every window (logged). About 105 uses in ui-egui. Area/Modal/LayerId uses are safe (per-viewport `areas`). `data`-keyed ones are shared across windows unless salted, e.g. `pdfcraft-inspector-body`, `custom-hotbar`, `zoom-input`, `pdfcraft-combine-grid-offset`, `can-paste-image`, the preferences search ids, `PROGRESS_HEIGHT`. View-keyed ids (`view_key()`) move correctly with a `DocView`. |
| **W0-13** fork patches | **PASS** (no blocking conflict; see list) [source] | See §2a. |

### 2a. Fork patches touching viewports, windows, capture, close or DPI [source]

- **E1 `NativeOptions::viewport_hooks`** (`egui/eframe/src/native/viewport_hooks.rs`). `window_created` and `window_destroyed` cover every viewport, and `raw_input` covers root, deferred **and immediate** viewports. This matters because `App::raw_input_hook` runs for the root only (`epi_integration.rs:425`). PdfCraft's `raw_input_hook` (`crates/ui-egui/src/app.rs:583-597`: desktop-grab release, wheel locate, file-drag pointer, `prepare_raw_input`) therefore **does not run for secondary windows**. The hook trait is `Send + Sync` and cannot borrow `&mut PdfCraftApp`. The app does not install hooks today.
- **E2+E3** (egui-wgpu surface fit and error scopes). These apply to any surface through `painter.set_window`. For secondaries the error is only logged (W0-7).
- **E4 + local E4/P2b** (Wayland clipboard). The state is per `State`, that is, per window: exact `ViewportId`/`WindowId`; hide or close cancels a pending paste (`egui-winit` `Visible(false)` → `cancel_pending_clipboard_pastes`). PATCHES.md says the focus owner is "Context-global `Memory::focused`". The source keeps `focus` per viewport (`memory/mod.rs:110`, `1082-1088`), so the text overstates sharing. This is not a conflict.
- **Local P7b.** Immediate viewports return their own viewport commands. `process_viewport_commands` gains `&mut State` (per-window IME gate), and `IMEAllowed` is applied before platform output in the immediate path (`wgpu_integration.rs:1340-1352`). This is compatible with immediate secondaries.
- **winit W1** (Windows `WM_DPICHANGED` rect) and **W11** (X11 DPI source). These matter for W0-6. W1 is Windows-pending.
- **W4** (Windows: hidden maximized windows stay hidden). This matters for H2 (hidden maximized root) on Windows and is pending.
- **W12/W14** (macOS). W14 turns termination into `CloseRequested` for all windows. If enabled, quit and multi-close cannot be told apart.
- **W15** (Wayland file drops through a per-seat data device): file drops onto a secondary surface need a Wayland test.
- **W18** (Windows hidden system caret per focused window) and **W20** (X11 one input context per window). Both are per window and fine for several windows.
- **Not a patch, but blocking for accessibility:** eframe calls `init_accesskit` only for the root window (`wgpu_integration.rs:322`, `glow_integration.rs:397`). **Secondary windows have no AccessKit tree.**
- **Doc inconsistency (low):** the egui fork's `PATCHES.md:156` names winit `ae48c756…` as "the workspace pin". Its own `Cargo.toml:179`, and the app, pin `a517b38…`.

## 3. Feasibility of the native-window architecture

**Feasible on Windows, macOS and X11, with conditions; partly on Wayland.**

- **What works.** Immediate viewports plus a shared Session plus `WindowUi` swap-in work as designed on both renderers [linux-x11]. Each secondary's closure can mutate app state directly, textures and fonts are shared, close requests are attributable per window, and moving a value between windows is an ordinary Rust move. That last point is demonstrated by H1 moving `docs` from S into the root.
- **What the design must take into account:**
1. **Panics inside a secondary closure.** A panic that is caught *outside* `show_viewport_immediate`, as the app's existing `guarded_frame` would catch it, leaves egui's Context inside the unfinished nested pass. The next frame panics in `Context::take_root_ui` (`context.rs:1064`) outside any app guard, and the process exits 101 on both renderers [linux-x11]. A `catch_unwind` **inside** each viewport closure fixed it: the window was kept, and later passes were normal on both renderers [linux-x11]. This conflicts with AGENTS.md §4 unless W1 adds a per-closure guard.
2. **Window-creation failure can crash the process** (W0-7). Builders must be validated and clamped before `show_viewport_immediate`: finite values, size within the monitor and ≤ 16384 px (X11 needs u16), position finite. Failure must be detected by "closure not called" and that viewport dropped.
3. **The frame cost of every visible window is paid on every frame** (W0-10). An animation or busy job in one window redraws all of them. Keep secondaries few, and keep per-window UI cheap.
4. **Root-only eframe features** must be re-provided per window or accepted as gaps:
   - `raw_input_hook` (per-viewport input preparation needs E1 hooks with shared state, or a different approach);
   - `logic()`;
   - AccessKit;
   - screenshots (the control channel's `ViewportId::ROOT` capture cannot see secondaries);
   - `Frame` (window handle for rfd dialog parenting; E1 `window_created` can supply secondary handles);
   - persistence (eframe saves only the root geometry).
5. **Id hygiene.** Per-window state stored in `Memory::data` under `Id::unique(...)` must be salted with the window's id, or moved into `WindowUi`. The embedded fallback and kittest panic on duplicate ids in debug builds.

## 4. Root viewport handoff evidence (T-2)

Baseline [source][linux-x11]: closing the root without `CancelClose` sets `integration.close` (`epi_integration.rs:367-372`, `430-437`). eframe then saves, destroys everything and exits, even with secondaries open. The `basic` mode quit while windows were open, confirming this.

**H1, re-home (works on X11; recommended primary).**

Steps, on a root `close_requested`:
1. Send `CancelClose`.
2. Take secondary S's `WindowUi` into the root.
3. Send the root `OuterPosition(S.outer_rect.min)`, `InnerSize(S.inner_rect.size())`, `Title`, and `Maximized` if S was maximized.
4. Stop showing S, so eframe destroys its window at the end of that pass.

Results [linux-x11]:

| Run | Before | After |
|---|---|---|
| No WM, wgpu and glow | root at 43,43, 758×542 px; S at 975,163, 542×412 | root at **975,163, 542×412**, title "W0 sec S" |
| openbox (decorations) | S inner `[[900.9 168.9] - [1401.2 549.2]]` | root inner `[[900.9 168.9] - [1401.2 549.2]]`, exact |

In every run S's X window was gone and exactly one window remained. The process kept running, and a second root close with no other windows exited cleanly (rc 0).

Limits:
- **Wayland:** positions cannot be read or set [source], so H1 can copy only content and size. The surviving window stays where the root was.
- **Windows** [windows-pending]: maximize/restore geometry, Aero-snapped state, monitor and DPI changes (W0-6 risk: `OuterPosition` uses the root's ppp), taskbar order and Alt-Tab order (identity changes), focus and activation, and flicker.
- **Every platform:** the user sees "the other window jumped". Native identity, the stacking position and any OS per-window state (IME composition, jump lists) belong to the old root. The root is the only window with AccessKit, so keeping the root alive is an accessibility *advantage* of H1.

**H2, hidden root (works on X11, but slows every window; impossible on Wayland).**
- Observed [linux-x11]: `CancelClose` plus `Visible(false)` left the root `IsUnMapped` and S fully working, including input and close. Closing the last secondary sent `Close` to ROOT and the app exited (rc 0).
- **Cost:** while the root is hidden, every root pass sleeps 10 ms (`winit_integration.rs:16-29`, `sleep_if_invisible_or_minimized`, root window only). Because secondaries render inside the root pass, an animating secondary dropped from **91 to 49 passes/s (glow)** and **81 to 42/s (wgpu)**.
- **Windows** [source → windows-pending]: invisible windows get no `RedrawRequested`. eframe paints them directly and pushes requested repaints of an invisible window to at least 100 ms later (`run.rs:21-27`, `375-383`, `214-262`), so secondary animations could fall to about 10 fps.
- **Wayland:** `set_visible` is a no-op (`wayland/window/mod.rs:254-256`).
- `ViewportInfo::visible()` reflects only minimized/occluded, not `Visible(false)` (`viewport_info.rs:98-104`). The app must track "root hidden" itself.

**H3, never-shown root (not supported without an eframe patch).** eframe calls `window.set_visible(true)` unconditionally after the root's first painted frame (`epi_integration.rs:445-450`), ignoring `with_visible(false)`. Hiding it in the same pass issues map then unmap [source]. Polling xwininfo about 300 times caught only `IsUnMapped` [linux-x11], so no flash was observed under Xvfb. A one-frame flash on a real desktop is still possible and is untested. H3 also inherits all of H2's costs.

**Conclusion:** H1 is a demonstrated safe handoff on X11 and should be the T-2 mechanism. H2 is a fallback only where H1 is impossible, and is unavailable on Wayland. On Wayland, H1 without positioning is the only option. Windows confirmation is required (§9).

## 5. Cross-window tab drag by platform

| Platform | Pointer keeps arriving while held outside | Release / Escape delivered | Desktop position of pointer and windows | Cross-window tab drag verdict |
|---|---|---|---|---|
| **Windows** | Yes, via `SetCapture` [source]; [windows-pending] | Release yes [source]; Escape to the focused, capturing window expected [windows-pending] | `inner_position` exists [source]; mixed DPI needs physical-px math [windows-pending] | **Likely supported**; must be validated (§9) [windows-pending] |
| **macOS** | Yes, `mouseDragged` while a button is held [source] | Release yes [source]; Escape not verified | Positions available [source]; not verified | **Likely supported** [source]; unverified (no macOS run) |
| **X11** | Yes [linux-x11] | Release yes [linux-x11]; Escape yes with a WM (to the grabbing window), none without a WM [linux-x11] | Yes, within 0.1 pt [linux-x11] | **Supported** [linux-x11] (no Z-order: overlapping windows ambiguous) |
| **Wayland** | Surface-local motion under implicit grab [source] | Release to the grabbing surface [source] | **No**: window positions unavailable [source] | **Not supported**; menu commands only (T-1) [source] |

## 6. Required menu-command fallbacks

These are needed on Wayland, where pointer tracking or positions are unavailable, in embedded mode, or when the platform check fails at run time:
- Move Tab to New Window
- Move Tab to Window ▸ (list of open windows)
- Merge All Windows
- Bring All Windows to Front, or a Window menu listing windows (each window is a menu target)
- Close Window, distinct from Close Tab and Quit

Every one of these also needs an automation tool and `pdfcraft-cli`/MCP parity, per AGENTS.md §3. On Wayland, "new window under pointer" becomes "new window, placed by the compositor".

## 7. Changes needed to the approved architecture

1. **Guard inside every viewport closure.** `catch_unwind` must sit inside the immediate closure (around the `WindowUi` body), not only around `ui`. Demonstrated necessary [linux-x11].
2. **Validate the builder before showing a window.** Clamp to finite values, at most the monitor size or 16384 px, and a position within the virtual desktop. Detect "closure not called" and abandon that window, keeping its tabs in an existing window. Never let eframe retry a bad builder.
3. **Re-provide root-only services per window.**
   - Per-window raw input preparation, through E1 `ViewportHooks` and shared state, or by redesigning `raw_input_hook`'s duties.
   - Per-window screenshot capture in the control channel, with an engine-side render instead of `ViewportCommand::Screenshot`.
   - Window handles for native dialog parenting (E1 `window_created`).
   - An accessibility plan for secondaries, since eframe gives them no AccessKit adapter. This needs an eframe patch or acceptance as a known gap.
4. **T-2:** implement H1. Treat "the root window" as an implementation detail: whichever window survives hosts `ViewportId::ROOT`. Track hidden or closed state in app state, not `ViewportInfo::visible()`.
5. **T-4 persistence is app-owned.** eframe stores only the root geometry. Persist each window's outer rect in **physical pixels** plus its monitor and maximized state. On Wayland, size only.
6. **Ids.** Audit the ~105 `Id::unique` uses. Salt the `data`-keyed ones with the window id or move them into `WindowUi`. Make `migrate_panel_state` (root-only by design, `app/frame.rs:50`) aware that the root's contents can change under H1.
7. **Frame budget.** Accept that every window redraws together. Avoid per-window continuous animations, and measure with real PDFs in W1.
8. **T-3 app-modal:** egui modals are per viewport, so the app must draw a blocking overlay in every other window while a dialog is open. This is feasible; nothing in egui provides it automatically.

## 8. Risks and unresolved blockers

- **Blocker until changed: process crash on window-creation failure** (W0-7). eframe's out-of-app retry turns a single bad size into an uncatchable panic [linux-x11]. Mitigated only by input validation (change 2). An OS-level failure on Windows (`CreateWindowExW` or surface/device loss) is untested.
- **Blocker until changed: a panic in a secondary corrupts the egui Context** unless it is guarded inside the closure (change 1).
- **Accessibility:** no AccessKit in secondary windows [source]. Moving a document to a new window removes it from screen readers.
- **Close-message loss under Xvfb** (W0-9): unexplained. It must be checked on real X11 hardware and on Windows before claiming reliable per-window close.
- **H2 on Windows:** possible 10 fps secondaries [source]. H1 on Windows across monitors with different DPI is unverified.
- **Wayland:** no positions and no hide. Cross-window drag is impossible, H1 is partial and T-4 restores size only.
- **macOS:** W14 would make quit send per-window close requests, which would need an "all windows at once means quit" rule. No macOS run.
- **Testing:** kittest only sees embedded windows. Native-window behaviour needs a spike-style or control-channel harness, and the control channel captures ROOT only.

## 9. Windows-only validation still required

**Build on a Windows machine:**
```
git checkout scratch/tab-window-w0   # or copy w0-spike/
cd w0-spike
cargo build --bins
```
The first build takes about 5–10 minutes. `xclose.exe` is a no-op on Windows.

**Run each mode in PowerShell, once with `wgpu` and once with `glow`.** `SPIKE_TIME_SCALE=4` gives a human time to act: the scripted times below are multiplied by 4.
```
$env:SPIKE_TIME_SCALE=4
$env:SPIKE_MODE=...
$env:SPIKE_RENDERER=...
$env:SPIKE_LOG="$PWD\<mode>-<r>.log"
.\target\debug\w0-spike.exe
```
Then read the `spike` and `eframe` lines in the log.

**Checklist:**

1. **basic** (~35 s each)
   - Three windows open. The red/green square shows in all three (W0-2).
   - Between 22 and 24 s, click the X on "W0 sec A". The log should show `HELD close`.
   - After 26 s, click X on A again. Expect `accepted close`. B closes itself at about 28 s, and the app quits at about 32 s.
   - Record the `COUNTERS` lines (W0-10) and the `VIEWPORT` lines (W0-5): outer/inner rects compared with real window edges, including the invisible-border offset.
2. **h1** (~30 s)
   - Between 6 and 30 s, click X on "W0 root". The root must jump to S's position and size, S must vanish, and the app must keep running.
   - Repeat with S **maximized**, with S **snapped** (Win+←), and with S on a **second monitor at a different scale**. Expect geometry within 2 px; check for a flash.
   - Click X on the survivor; the app should exit.
3. **h2** (~40 s)
   - Between 8 and 10 s, click X on root (the counters measure 4–8 s with the root visible and 10–18 s with it hidden). The root should hide and S keep animating.
   - Compare the `COUNTERS visible-root` rate with `hidden-root`. This checks the 100 ms invisible-window throttle on Windows.
   - Close S; the app should exit.
   - Also test with the root maximized before hiding (fork W4).
4. **h3** (~20 s): watch for a root-window flash at startup, ideally with a screen recording. Close S; the app should exit.
5. **drag**, with `SPIKE_TIME_SCALE=1` and `SPIKE_DRAG_SECS=60` (~1 min; the drag timeout is scaled too)
   - Press in root, drag over the desktop, over S, onto the taskbar, and onto a second monitor.
   - Press Escape while still holding, then release outside.
   - Log: `PTR W0 root: moved` must continue outside, with desktop positions consistent between windows (≤ 2 px), and the release and `KEY … Escape` must arrive at root.
   - Repeat starting in S.
6. **fail** (~12 s when it does not crash): note whether it crashes (exit code 101) or fails gracefully. On Windows the 100000-pt size may hit `CreateWindowExW` or wgpu surface limits instead of the X11 unwrap.
7. **panic** without, then with, `$env:SPIKE_INNER_GUARD=1` (~12 s each; the panic happens at 4 s): expect rc 101 without the guard and a clean exit with it, matching Linux.
8. **embed** (~6 s): confirms the embedded-fallback debug assert on Windows.
9. **Close reliability:** repeat step 1's two closes and step 2's root close 10× each, and count any lost close.
10. **DPI** (~10 min, needs two monitors at 100% and 150%): run h1 and drag across monitors, and check `native_ppp` per window in the `VIEWPORT` lines and placement of a window created on the second monitor.

The whole checklist on one machine takes about 2 hours including the build.

## 10. Recommendation

**W1 can proceed, conditionally.** The core mechanism (immediate viewports drawing `&mut` app state, a shared Session, `WindowUi` swap-in, per-window close attribution, and H1 root handoff) is demonstrated on X11 with both renderers. Nothing found requires `Arc<Mutex>` around the app or a different eframe mode.

Conditions:
1. W1 includes changes 1 and 2 from §7 (per-closure panic guard, validated builders with "closure not called" detection), each with a regression test that panicked before, as house rule 1 requires.
2. T-2 is implemented as H1, and the Windows checks in §9 items 2, 3 and 9 pass before multi-window is enabled by default on Windows. H2 is not used on Windows if item 3 shows the ~10 fps throttle.
3. Cross-window tab dragging is gated per platform. It is off on Wayland, where only the menu commands of §6 exist from the start. On Windows it stays off until §9 item 5 passes; on macOS until it is tested.
4. The accessibility gap of secondary windows (no AccessKit) is accepted by the owner as a known limitation or scheduled as an eframe patch before release.
5. The unexplained close-message loss is re-checked on real X11 hardware and on Windows (§9 item 9) before W3–W5 rely on per-window close prompts.

## 11. W0-7 by failure class (Revision 1)

Method: `mode=fail` (unclamped), `mode=clamp`, `mode=panic` and `mode=devlost`. Each was run under Xvfb with wgpu and glow; logs are in the scratchpad `r2/` folder. "Recover" means the app keeps running, the other windows keep working, and the documents of the failed window are kept, in the root window if the window is gone. Clamping is prevention, not recovery. A `catch_unwind` around the app's ui callback does not protect against failures inside eframe.

| Class | Origin (pinned source) | App can prevent? | App can catch / recover? | Other windows and documents | Demonstrated on Linux | Unverified on Windows |
|---|---|---|---|---|---|---|
| **(a) bad size/position from app code** (100000², NaN, ±inf, negative, 0, 1e9) | winit X11 `x11/window.rs:316-317` `dimensions.try_into().unwrap()` (u32→u16) panics for sides over 65535 px. inf saturates to `u32::MAX` there. eframe retries the window **outside app code**: wgpu `initialized_all_windows` (`wgpu_integration.rs:158-181`, called at 451 and 544 on every window event); glow `handle_viewport_output → initialize_all_windows` (`glow_integration.rs:1621`, 1368-1378). NaN: `debug_assert!` in emath `pos2.rs:237`, hit from eframe's `initialize_or_update_viewport` (debug builds only). Negative and 0: X11 BadValue → winit `Err` (`window.rs:324`), not a panic (c1). | **Yes.** Clamp before `show_viewport_immediate`: finite, at least 100 pt, at most the monitor and 16384 px, position within ±16000 px. Demonstrated with spike `clamp_geometry`. | huge/inf/nan: **no**. The first panic is caught, but eframe's retry panics outside app code even when the app abandons the window at once (`fail_drop=1`: rc 101). negative/zero: yes (c1). farpos: winit saturates, and the window opens off-screen. | huge/inf/nan crash the process: **all windows and all unsaved documents lost**. | Unclamped, both renderers, with and without abandoning (24 runs): huge, inf and nan rc 101; negative, zero and farpos rc 0. Clamped: **6/6 cases open a working window, 2 runs × 2 renderers**, process alive. | Windows limits differ (no u16; `CreateWindowExW` and DWM limits; `surface_fit` covers the GPU limit). Release-build NaN behaviour (the assert is debug-only). Clamping uses the root's current `monitor_size`, which may be too strict for a window spanning monitors. A 1e9 position clamps off-screen; a real app must place the window on a visible monitor. |
| **(b) panic inside the app-owned secondary closure** | App code inside `show_viewport_immediate`. Caught outside the closure, egui's nested pass stays open, and the next frame panics in `Context::take_root_ui` (`egui/src/context.rs:1064`) outside app code. | Only by not panicking (house rule 1). It cannot be ruled out. | **Yes**, with `catch_unwind` **inside** the closure: the nested pass then ends normally. `AssertUnwindSafe` caveat: app state mutated before the panic may be half-updated, and that window's frame is partly drawn. | With the inner guard: root keeps working, the failed window keeps its `WindowUi` (documents kept), redraws, closes on request, and new windows open. Without it the process exits 101 and everything is lost. | Panic mid-draw inside a panel and a nested layout. Inner guard: **3/3 runs per renderer**, all seven checks PASS (`panic_frame_ended`, `panic_failed_window_redraws`, `panic_root_clicks` = a real click on the root, `panic_failed_window_closed` by WM_DELETE_WINDOW, `panic_new_window`, `panic_survived`, rc 0). No inner guard: **3/3 rc 101** per renderer. | MSVC unwinding is expected to behave the same (`panic = "unwind"`). Not run. |
| **(c1) OS refuses the window** (non-panicking `Err`) | wgpu: `Viewport::initialize_window` logs and returns (`wgpu_integration.rs:1147-1177`, log at 1175). glow: `create_window_for_viewport` `?` (200-240) → `initialize_window` `Err`, logged (1375, 1785). The renderer returns without running the closure, and egui panics in the app's call: `out.expect("…user callback was never called")` (`context.rs:4795`). | No (the OS decides). | **Yes.** The panic happens in the app's call after the backend returned, with no egui pass open. The app catches it, stops showing that viewport, and eframe removes it at the end of the frame (`remove_viewports_not_in`, wgpu 1365, glow 1548). If the app keeps showing it, eframe retries every frame and the app catches a panic every frame (survives, noisy). This relies on egui's panic message; a contained egui change (`try_show_viewport_immediate` returning `Option`) would make it an API, about 40 lines in `context.rs`. | Unaffected; the documents move to the root. | Forced with size 0 and negative size (X11 BadValue). `fail_drop=1`: one attempt, survives, rc 0, both renderers. Without dropping: 15-18 caught panics in 3 s, survives. | Windows may accept a 0×0 window, so this path needs a real OS failure there (for example USER-object or desktop-heap exhaustion). Untested. |
| **(c2) wgpu surface creation/configuration fails for a secondary** | `Painter::set_window` (`egui-wgpu/src/winit.rs:280-297`); `add_surface` returns `ConfigureSurface` (329-367, `catch_errors` 79-97). eframe only logs it (`wgpu_integration.rs:1157`, 1323). | No. | **Not detectable**: the closure still runs, and painting returns early for a missing surface (`winit.rs:670-672`). The result is a blank window that takes input, with no `ViewportInfo` signal. A contained eframe change could report it: a per-viewport error in `ViewportInfo` or a `ViewportHooks` callback, about 30-50 lines in eframe and egui. | Others unaffected [source]. That window's documents are invisible but still there. | Not forced. `WGPU_BACKEND=gl` failed already at the root's start-up: `run_native` returned `Err` cleanly (rc 1, no panic). That is the root path (fork E3), not a secondary. | Unverified. |
| **(c3) surface lost/outdated while painting** | `on_surface_status` default (`egui-wgpu/src/lib.rs:404-430`): Outdated → reconfigure; Lost → recreate next frame (`winit.rs:648-660`, 735-748); a recreate error is logged (657). | No. | Recovers by design [source]. | Others unaffected [source]. | Not forced. | Unverified (resize storms, monitor changes, sleep/resume). |
| **(c4) device loss (wgpu)** | egui-wgpu `renderer.rs:1032-1037`: explicit `panic!` when `write_buffer_with` returns `None`. Other calls reach wgpu's default uncaptured-error handler, which panics (`wgpu-30.0.1/src/backend/wgpu_core.rs:692-695`). | No. | **No.** The first panic is in the secondary's paint (inside the app's call). The next is in the root's paint inside eframe's event loop, outside app code. An app-installed `Device::on_uncaptured_error` handler does not change this. | One device is shared by all windows: **every window and every unsaved document is lost**. This is the same in single-window PdfCraft today and not specific to multi-window. | `Device::destroy()` as a simulation: **rc 101 with and without the handler**. | A real TDR or driver reset was not tested. A fix needs egui-wgpu to return errors instead of panicking, plus eframe device-lost handling and render-state recreation: about 200+ lines, larger than a contained change. |
| **(c5) glow surface / context failure** | `initialize_window`: `create_window_surface(…)?` (1445); then the shared context is made not-current with `.unwrap()` (1452-1459) and `make_current(&gl_surface)?` (1461) **consumes** it on failure. After that, `change_gl_context` unwraps (1160-1173) and the root paint unwraps (859-861) panic outside app code. `window_handle().expect` (1434). | No. | Surface-creation error: recoverable as in c1 [source]. **make_current failure: not containable**: the only GL context is gone. | make_current failure: every window loses rendering, then the process panics. | Not forced. | WGL failures were not tested. A fix needs the context kept or recreated and the unwraps turned into errors, about 60-100 lines in `glow_integration.rs`. |

**W0-7 verdict: FAIL.** It is partially mitigated for (a) and (b) only, on Linux/X11 [linux-x11]:
- (a) is prevented by clamping;
- (b) is recovered by a guard inside the closure;
- of class (c), only the non-panicking OS-refusal path (c1) is recoverable;
- device loss (c4) and glow make-current failure (c5) are not containable without eframe or egui-wgpu changes, and a failed secondary surface (c2) is silent.

No containment is claimed for Windows [windows-pending].

## 12. W0-9: the lost close events (Revision 1)

**Hypotheses and source.**
- Event routing: winit turns a `WM_DELETE_WINDOW` ClientMessage into `CloseRequested` directly (`x11/event_processor.rs:477-491`).
- eframe logs every `CloseRequested`:
  - for an immediate secondary, it pushes `ViewportEvent::Close` into that viewport's `info.events` and repaints the viewport and its parent (`wgpu_integration.rs:1044-1066`, glow 1072-1090);
  - the next pass of that viewport delivers it in `raw_input.viewports`, then clears it (`wgpu_integration.rs:1313`, glow 1847);
  - root events are cleared after the root pass (809), or after the logic-only pass of a hidden root (733).
- No path drops an event between eframe's log line and the app, as long as each viewport keeps being drawn while its parent is.
- That leaves the layers below eframe, and the timing, renderer, window-manager and frame-pacing hypotheses the owner listed.

**Method.**
- `mode=closes`: root plus two secondaries, every close held (`CancelClose` for the root; secondaries keep showing).
- Each received close is logged as `CLOSE_RX`.
- `closes.sh` sends N closes at 150 ms intervals and counts three layers:
  - X server accepted (the sender checks the reply);
  - eframe's `Received WindowEvent::CloseRequested`;
  - the app's `CLOSE_RX`.
- `xclose` gained a reply check (`.check()`) and, for comparison, the first run's fire-and-forget mode (`XCLOSE_NOCHECK=1`), with an optional linger (`XCLOSE_LINGER_MS`).

**Counts** [linux-x11]:

| Configuration (50 sends each unless noted) | Runs | Sent | eframe | App |
|---|---|---|---|---|
| reply-checked sender, no WM: wgpu/glow × root/secondary × animation on/off × tick 250 ms / none | 16 | 800 | 800 | 800 |
| same, root hidden (`Visible(false)`, as in H2/H3), secondary target, animation on/off | 4 | 200 | 200 | 200 |
| same, openbox WM: wgpu/glow × root/secondary × animation on/off | 8 | 400 | 400 | 400 |
| reply-checked, animating, root target (repeat) | 2 | 100 | 100 | 100 |
| (earlier smoke runs, 10 sends) | 2 | 20 | 20 | 20 |
| **fire-and-forget sender** (first run's), animating | 6 | 300 | **250** | **250** |
| fire-and-forget, idle (no animation) | 2 | 100 | 100 | 100 |
| fire-and-forget, sender keeps its connection 300 ms | 2 | 100 | 100 | 100 |
| replay of the first run's H2 script, fire-and-forget, 8 runs per renderer | 16 | 32 | 30 (wgpu root close lost in 2 of 8) | 30 |
| replay of the first run's H2 script, reply-checked, 8 runs per renderer | 16 | 32 | 32 | 32 |

**Cause** [linux-x11]:
- The losses came from the test sender. `xclose` queued its `SendEvent`, flushed and exited at once. While the X server was busy with continuous software-rendered frames, some of those requests were never acted on. Losses ran from 8 to 28 % per 50, on both renderers.
- With the sender alive until the server answered, or simply lingering 300 ms, and in idle runs, nothing was lost.
- The first report's lost closes all happened in H2/H3 runs with an animating window, which fits.
- I did not read the X server source, so the exact server-side mechanism (presumably requests discarded when the client's hang-up is processed first) is inferred, not shown.
- **In every run, eframe's count equalled the app's**: no loss inside winit-to-app routing, with either renderer, with or without a WM, with a hidden root, or with event-driven pacing (`tick_ms=0`).

**Conclusion.**
- On X11 the cause is found and shown not to apply to the application's event path.
- W0-9 stays **INCONCLUSIVE** overall because Windows delivers closes differently: the title-bar X goes through `WM_SYSCOMMAND/SC_CLOSE` → `WM_CLOSE` → winit's window procedure. Real-hardware X11 was not run either.

**What the Windows run must measure** (`windows/run.ps1`, `CHECKLIST.md` §10):
- 50 posted `WM_CLOSE` per configuration (root, secondary × animation off/on × wgpu/glow), counted at eframe and at the app;
- 10 real title-bar clicks per window, with and without animation, against the app count.

Pass: zero loss at both layers in every configuration.

## 13. Accessibility of secondary windows (Revision 1)

**Findings** [source]:
- eframe creates the AccessKit adapter only for the root window: `egui_winit.init_accesskit(...)` in `init_run_state` (`wgpu_integration.rs:319-323`; glow 387-399). Secondary windows get an `egui_winit::State` with `accesskit: None` (`wgpu_integration.rs:1160-1168`; glow `initialize_window` 1408-1420).
- egui itself already builds **one AccessKit tree per viewport**. Each viewport pass starts its own `accesskit_state` with a `Window` root node (`egui/src/context.rs:562-575`), and `end_pass` emits it in that viewport's `platform_output.accesskit_update` (`context.rs:3248-3280`).
- The immediate path already hands that output to its own `egui_winit::State` (`wgpu_integration.rs:1354`, glow 1911), which forwards it to an adapter when one exists (`egui-winit/src/lib.rs:3530-3536`). AccessKit events are already routed by window id (`wgpu_integration.rs:555-575`, glow 611-630).
- Turning AccessKit on or off is **Context-global**: `enable_accesskit`/`disable_accesskit` (`context.rs:4315-4322`), called from `on_accesskit_window_event` (`winit_integration.rs:187-216`).
- `accesskit_winit::Adapter` panics if the window is already visible when the adapter is created (`accesskit_winit-0.34.1/src/lib.rs:197-200`). Immediate viewports are created visible.

**Observed** [linux-x11]:
- AT-SPI was run under a private D-Bus session with accessibility enabled (`a11y/a11y.sh`, pyatspi dump).
- With root, A and B open, the screen-reader view shows the application with **one** top-level window: the root's frame, panel, labels, entry, toggle button and push button. A and B are absent.

**Contained eframe change: feasible, not prototyped.** A patched egui build needs about 0.5-1 GB of extra target space, and the shared disk was at about 4.4 GB free against the 3 GB floor. The change:
1. wgpu `Viewport::initialize_window` (`wgpu_integration.rs:1130-1179`) and glow `GlutinWindowContext::initialize_window` (`glow_integration.rs:1382-1500`), for non-root viewports:
   - create the window with `with_visible(false)`;
   - call `egui_winit.init_accesskit(event_loop, &window, proxy)`;
   - then `set_visible(builder.visible.unwrap_or(true))`.

   This needs the event-loop proxy in `SharedState` / `GlutinWindowContext`. Today it lives only in `WgpuWinitApp::repaint_proxy` and `GlowWinitApp::repaint_proxy`.
2. `on_accesskit_window_event`: count active adapters, so that `AccessibilityDeactivated` from one window does not disable the tree of the others. Call `disable_accesskit` only when the count reaches 0.
3. Nothing in egui: per-viewport trees and the action routing (`on_accesskit_action_request` per `State`) are already in place.

Size and risk:
- Size: 3 files (`wgpu_integration.rs`, `glow_integration.rs`, `winit_integration.rs`), about 60-100 lines, no new dependency.
- Risk: low to medium.
  - Window-visibility ordering, with the first-frame flash already handled for the root.
  - Focus: each adapter reports its own focused node, so the platform must follow window focus.
  - Duplicated `accesskit_root_id()` across trees is fine, because each adapter is a separate tree.
  - Windows UIA and macOS NSAccessibility adapters are per window by design; that is not verified.

**Testing a patch:**
- Linux: `a11y/a11y.sh` must list three top-level windows, each with its widgets, and stay correct after a window closes; Orca can be used by hand.
- Windows: Narrator and NVDA move into each window and read its widgets; Accessibility Insights or Inspect shows a UIA tree per HWND; closing a secondary with Narrator running does not crash. Test with both renderers.
- macOS: VoiceOver.
- Also a regression test that `AccessibilityDeactivated` from one window keeps the root's tree.

**Accessible single-window fallback: designed and demonstrated** [linux-x11], `mode=a11y`:
- **Detection.**
  - AccessKit activates when an assistive technology asks for the tree (`InitialTreeRequested` → `enable_accesskit`).
  - egui has no public getter. The app can detect activation with `ctx.has_accesskit_node(<id of a widget it just drew>)`, true only while trees are built.
  - Platform triggers differ: Linux AT-SPI `org.a11y.Status.IsEnabled` (`accesskit_unix` `context.rs:184-185`); Windows UIA `WM_GETOBJECT`; macOS NSAccessibility.
  - Activation can be a false positive (a UIA client that is not a screen reader, such as some input tools) and can come late.
  - So also offer a setting, "Keep all documents in one window", that forces the fallback (`single_window=1` in the spike).
- **Behaviour when on:**
  1. All secondary windows are merged into the root at once (documents appended, nothing closed or lost).
  2. Detach commands (drag-out, "Move Tab to New Window", "Move Tab to Window ▸") are refused with an explanation: "unavailable while a screen reader is active: separate windows are not yet accessible". The document stays put.
  3. No new secondary windows open.
  4. It stays on until restart, or until the setting is turned off. Deactivation does not undo it, to avoid windows appearing under a user.
- **Observed** [linux-x11], both renderers:
  - With AT-SPI enabled, AccessKit activated within 0.2 s.
  - `a11y_merge`: one secondary merged; the root's docs became `r.pdf, s.pdf`.
  - `a11y_detach_refused`: the detach was refused with the message.
  - The AT-SPI dump shows both documents in the root window.
  - Without an AT, the same mode allowed the detach and opened a window.
- **Limitations:**
  - Detection is a heuristic.
  - The merge happens after activation, so a user sees windows collapse once.
  - H1 is unaffected, because the root keeps its adapter.
  - Users who rely on magnifiers or switch access without a UIA client get multi-window as usual.
- **Testing requirements:**
  - The automation tool for the setting (AGENTS.md §3).
  - A kittest unit test of merge/refuse logic with the fallback forced.
  - Windows Narrator and NVDA runs (`CHECKLIST.md` §11).
  - The AT-SPI probe on Linux.

## 14. Windows validation package (Revision 1)

- **Workflows:** none of the eight workflows on GitHub Main can build the spike unmodified. The table is in `windows/proposals/README.md`. `overlay-windows.yml`, dispatched on the scratch branch, would run that branch's unchanged copy, which builds the app.
- **Approval-needing options**, written as patches and not applied: (A) a scratch-branch-only replacement of `overlay-windows.yml`; (B) a new dispatch-only `w0-spike-windows.yml` on Main that takes a commit SHA. Both also need the scratch branch pushed (publishing).
- **Local package:** `windows/README.md`, `build.ps1`, `run.ps1`, `CHECKLIST.md`, `collect.ps1`. The spike's settings are scriptable (`name=value` arguments or `SPIKE_<NAME>`), and it emits `RESULT` lines that `run.ps1` copies into `results.csv`.
  - Automated: basic, H1, H2, H3, fail ×6 ×2, clamp, panic ×2, devlost ×2, embed, closes ×4, a11y without AT.
  - Manual: drag, close clicks, unsaved close, H1 variants, focus and geometry, Narrator.
- **Checked here:**
  - `cargo check --locked --bins --target x86_64-pc-windows-msvc` type-checks (no MSVC linker here, so nothing was linked).
  - Linux debug build and kittest test pass.
  - The PowerShell scripts could not be run or parsed here: no `pwsh` package in apt, and downloading it was refused. Only bracket balance was checked.
  - No Linux release build was made (disk), so release-only behaviour (the NaN case) is untested.

# TAB.WINDOW.1: W0 feasibility spike report

## Revision 1 (2026-11-17)

The owner reviewed the first version of this report (the original text follows from §1 on) and corrected four classifications. They are applied in the §2 table:

- **W0-7: FAIL.** Its acceptance criteria were not demonstrated. It may become "partially mitigated" only for the failure classes actually shown to be contained, and stays FAIL while class (c), failures inside eframe, winit or the graphics backend, is not shown containable. The first version's §7 change 2 described a mitigation; it was never demonstrated.
- **W0-9: INCONCLUSIVE.** The first version said PASS with a flake. Close events were lost (about 6 of 18 root closes with wgpu), so per-window close is not shown reliable.
- **W0-6: PENDING-WINDOWS.** Mixed DPI could not be emulated. The single-DPI Linux observation is not evidence for it.
- **W0-8: Windows PENDING.** H1 was observed on X11 only.

Follow-up work in this revision: §11 (W0-7 by failure class), §12 (W0-9 lost close events), §13 (accessibility of secondary windows and the single-window fallback), §14 (Windows validation package). The §1 rerun commands and §9 Windows steps are superseded by `windows/README.md` and `windows/CHECKLIST.md`.


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
| **W0-7** window-creation failure | **FAIL** [linux-x11][source]: the acceptance criteria (failure contained, other windows and documents intact) are not demonstrated. See Revision 1 and §11 for the per-class work | **Forced failure:** an immediate viewport with inner size 100000×100000. Both renderers **crashed the process** (exit 101, ~50 ms) [linux-x11]. The first panic, in winit `x11/window.rs:317` (`dimensions.try_into().unwrap()`: `TryFromIntError(PosOverflow)`), occurred inside `show_viewport_immediate` and was caught by the spike's `catch_unwind`. eframe then **retries window creation outside app code**: wgpu in `initialized_all_windows` on every window event (`wgpu_integration.rs:544`, `158-181`), glow in `handle_viewport_output → initialize_all_windows` (`glow_integration.rs:1368-1378`). That second panic is not catchable by the app. **Non-panicking failure** [source]: `create_window` `Err` is only logged (`wgpu_integration.rs:1175`, `glow_integration.rs:1784-1787`). The renderer then returns without calling the closure, and `show_viewport_immediate` panics with `"egui backend is implemented incorrectly - the user callback was never called"` (`context.rs:4795`), repeated every pass. A surface error from `painter.set_window` is only logged (`wgpu_integration.rs:1150-1153`): a blank window, nothing reported to the app. No `ViewportInfo` field exposes failure. The only signal is "closure not called", observable as a panic, within one frame. |
| **W0-8** root handoff (T-2) | **PARTIAL; Windows PENDING** [windows-pending]: H1 was observed on X11 only [linux-x11]; Wayland degraded [source]; Windows PENDING-WINDOWS. H2 works on X11 but is costly [linux-x11][source]. H3 not supported by pinned eframe [source]. | See §4. |
| **W0-9** per-window close/quit | **INCONCLUSIVE** [linux-x11][source]: attribution works, but some close events were lost, cause not established. See Revision 1 and §12 | A `WM_DELETE_WINDOW` sent to secondary A arrived as `ViewportEvent::Close` in A's own closure (`W0 sec A: close requested inside its own viewport "4477" (events=[Close])`; eframe pushes it per viewport at `wgpu_integration.rs:1044-1066` and `glow_integration.rs:1072+`). Holding = keep calling `show_viewport_immediate`. `CancelClose` is a no-op for non-root (`egui-winit` `process_viewport_command`: "Need to be handled elsewhere"). Observed: A held while dirty, then accepted after "save". `send_viewport_cmd_to(B, Close)` was attributed to B. Root close is held with `CancelClose` (`epi_integration.rs:367-372`, `430-437`; log `Closing of root viewport canceled`). **Flake:** with two or more windows and no window manager, some `WM_DELETE_WINDOW` messages were never delivered (no eframe `Received WindowEvent::CloseRequested` log). Root closes were lost in about 6 of 18 wgpu runs and 0 of about 13 glow runs. Secondary closes were lost in about 1 of 16 wgpu and 2 of about 20 glow. A single window lost 0 of 10. Not root-caused: winit's handler is trivial (`x11/event_processor.rs:477-491`), so I suspect Mesa's software stack sharing the X connection. Treat it as Xvfb-specific until seen on real hardware. **Quit** [source]: fork W14 (macOS cancelable termination) would send `CloseRequested` to every winit window at once. It is opt-in and the app does not enable it today. |
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

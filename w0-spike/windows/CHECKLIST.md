# W0 Windows checklist

Every item runs with **both renderers** (`run.ps1` does wgpu, then glow). "Auto" means `run.ps1`
records it in `results.csv` without help. "Manual" means it pauses and tells you what to do.
Names in `code` are the `check` column of `results.csv`. A check with no PASS/FAIL rule here is
recorded as INFO for the report. An item that passes on one renderer only counts as a FAIL
for the other.

Linux results for comparison are in `../W0-REPORT.md`, §11 to §13. They were measured on Xvfb
with software rendering. They predict nothing about Windows.

## 1. Two native windows, both renderers (W0-1, W0-2): Auto

- Observe: `winrect` rows show three distinct hwnds (`W0 root`, `W0 sec A`, `W0 sec B`) with
  their frame and client rectangles. `basic_exit` rc=0. `viewport_basic@1.5` rows give egui's
  view of the same windows in physical pixels.
- Pass: three windows exist for each renderer; `basic_exit` PASS. In the `viewport_*` rows,
  each window's `inner_px` matches the `client_px` of its `winrect` row within 2 px.
- Also record (manual glance during the run): the red/green square appears in all three windows.

## 2. Cross-window pointer capture and release (W0-4): Manual (`m-drag`)

- Do: drags 1 to 5 as listed on screen. Each starts with a press on a blue tab strip.
- Observe: in the `m-drag` log, the `PTR <window>: moved` lines keep coming from the window the
  drag started in while the pointer is over the desktop or the other window. `drag_drop` rows
  say `release delivered to <source window>`.
- Pass: for every drag, the release is delivered to the window where it started, with a
  desktop position within 2 px of where you let go. `drag_escape` PASS for drag 4 (Escape
  reaches the dragging window while the button is held).
- Fail: a drag whose release never arrives (no `drag_drop` and no `drag_escape` row after
  `drag_start`), or Escape not delivered.

## 3. Valid and invalid drop targets (W0-4/W0-5): Manual (`m-drag`)

- Observe the `verdict` in each `drag_drop` row:

  | Drag | Drop on | Expected verdict | Meaning |
  |---|---|---|---|
  | 1, 5 | the other window's tab strip | `valid` | move the tab there |
  | 2 | the other window's page area | `invalid` | cancel |
  | 3 | empty desktop | `desktop` | new window at the drop point |
  | 4 | desktop, Escape while held | `drag_escape` | cancel |
  | 7 | the taskbar | record the verdict | no rule yet: tells us what the taskbar counts as |

- Pass: drags 1 to 5 give the expected verdicts. For drag 3, `drag_new_window` is PASS: the new
  window's top-left lands within 2 px of the drop point.

## 4. Mixed DPI, 100 % + 150 % (W0-6): Manual (`m-drag` step 6, `m-h1variants` other-monitor)

- Set up: monitor 1 at 100 %, monitor 2 at 150 %. `collect.ps1` records both in `system.txt`.
- Observe:
  - `viewport_*` rows: `ppp` is 1.0 for a window on monitor 1 and 1.5 on monitor 2.
  - Drags 1 and 3 across monitors: their `drag_drop` and `drag_new_window` rows.
  - `h1_variant_other-monitor`.
- Pass:
  - A drop on the other monitor's window strip gives `valid`.
  - `drag_new_window` is within 2 px when the drop is on the other monitor.
  - `h1_variant_other-monitor` PASS: the root takes S's client rectangle exactly.
- Fail is expected here and is the useful result: a new window or a re-homed root on the wrong
  monitor, or scaled by the wrong factor. Report the offset as recorded.

## 5. Closing secondary and root windows (W0-9): Auto

- Observe: `close_accepted` for A and B in `basic`. In `h1`, closing the last window gives
  `h1_exit` rc=0. In `h2`, closing S while the root is hidden ends the app (`h2_exit` rc=0).
- Pass: all of these PASS on both renderers.

## 6. Root-window handoff, H1 (W0-8): Auto + Manual

- Auto (`h1_handoff`): after the root's close, the surviving window is the old root hwnd, S's
  window is gone, and the root's client rectangle equals S's former one.
- Manual (`m-h1variants`): S maximized, S snapped (Win+Left), S on the other monitor.
- Pass: `h1_handoff` PASS on both renderers. Each `h1_variant_*` PASS, and you saw no flash,
  no jump through another position and no wrong size (`h1_variant_*_observed`).
- Record (INFO): whether a maximized or snapped S comes back maximized or snapped. The spike
  copies only "maximized".
- H2 and H3, extra:
  - `h2_root_invisible` PASS.
  - `h2_secondary_rate` PASS needs at least 30 passes/s for the animating window with the
    root hidden. It tests the Windows invisible-window throttle that the report predicts from
    source.
  - `h3_root_never_visible` PASS if the root was never visible in the samples.

## 7. Unsaved-work close interception (W0-9): Auto + Manual

- Auto:
  - `close_held` PASS: A's first close is held while it has unsaved work.
  - `unsaved_close_window_kept` PASS: A's window still exists after that close.
  - `close_accepted` for A after the "save".
- Manual (`m-unsaved`): the same with real clicks on the title-bar X.
- Pass: all PASS and the observer answers "y".

## 8. Window geometry and focus (W0-5): Auto + Manual (`m-focus`)

- Auto: `winrect` and `viewport_*` rows. `focus` rows log each window's focus change as egui
  sees it.
- Manual: click into root, A, B, root. Each click gives one `focus ... focused=true` row for
  that window, and the previous window gets `focused=false`.
- Pass: the focus rows follow your clicks, and the observer answers "y" (each window draws
  exactly inside its frame).

## 9. Window-creation failures and their containment (W0-7): Auto

The pass rules describe what W0-7 needs. Several FAIL rows below are expected: they reproduce
the known crashes.

- **(a) invalid sizes and positions from app code:**
  - `clamp_*` and `clamp_all` must PASS: clamping prevents the crash.
  - `fail-<case>-drop<0|1>_exit` record the unclamped behaviour; any exit code is accepted.
  - Expected from Linux: rc=101 for huge, inf and nan; rc=0 for negative, zero and farpos.
  - Record the Windows exit codes. Windows has no X11 u16 limit, so huge may behave
    differently.
- **(b) panic inside a secondary window's closure:**
  - `panic-guard1`: `panic_frame_ended`, `panic_failed_window_redraws`,
    `panic_root_clicks` (the automated click on the root's button), `panic_failed_window_closed`,
    `panic_new_window`, `panic_survived` and `panic-guard1_exit` rc=0 must all PASS.
  - `panic-guard0_exit` is expected to be rc=101: without the guard inside the closure, the
    process dies.
- **(c) failures inside eframe, winit or wgpu:**
  - `devlost-uncaptured0/1` (wgpu only) record whether a lost device (simulated with
    `Device::destroy`) ends the process. Linux: rc=101 both times, a panic inside egui-wgpu.
    There is no pass rule: record the result.
  - `fail-zero/negative-drop1` show the OS refusing a window (X11 BadValue on Linux; Windows
    may accept a zero-size window). With `fail_drop=1` the app abandons the window and keeps
    running.
  - Record whether Windows refuses the window at all (`fail_<case>_closure_ran` INFO means it
    did not).

## 10. Repeated close attempts with counts (W0-9): Auto + Manual

- Auto (`closes_root`, `closes_sec`): 50 `WM_CLOSE` messages are posted to the root and to a
  secondary, with and without a continuously animating window, for each renderer. Each row
  gives posted, eframe-received and app-received counts.
  - Pass: app = eframe = 50 in every configuration.
  - A loss "before eframe" means the message did not reach winit's handler. A loss "between
    eframe and app" means eframe got it but the app did not see `close_requested()`.
- Manual (`m-closeclicks`): 10 real clicks on each title-bar X, without and with animation.
  - Pass: `closes_manual` PASS (count received = count clicked) in every configuration.
- Background: on Linux every loss was traced to the test sender quitting before the X server
  had processed its message (report §12). Posted `WM_CLOSE` messages and real clicks take a
  different path on Windows, which is what this check measures.

## 11. Screen reader, extra (W0 accessibility, report §13): Manual (`m-narrator`)

- With Narrator on:
  - `a11y_active` and `a11y_merge` PASS: the secondary window is merged into the root.
  - `a11y_detach_refused` PASS.
  - Then record (`narrator_observed`) what Narrator reads in the root and in A/B.
  - Expected today: the root's widgets only.
- Without Narrator (`a11y-noat`): `a11y_detach_allowed` INFO. No secondary may be merged.

## Reading `results.csv` quickly

```
Import-Csv windows\w0-results\<stamp>\results.csv | Where-Object outcome -eq FAIL | Format-Table -AutoSize
```

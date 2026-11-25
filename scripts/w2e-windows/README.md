# TAB.WINDOW.1 W2e: native multi-window checks on Windows (scratch only)

`run.ps1` drives the release build of `claude/tab-window-w2e` on a GitHub Windows runner: the
window tools (`ui.window.*`) making real native windows, shortcuts pressed in a second window,
a question in the first window holding the second, file dialogs owned by the window that asked
(`GW_OWNER`), and closing the first window with another open (H1). Every result row is evidence
of class ci-vm (a virtual machine with injected input), never physical hardware.

This branch exists for one dispatched run of `.github/workflows/overlay-windows.yml`, whose
scratch copy here adds the three steps marked "W2e scratch". It is never merged.

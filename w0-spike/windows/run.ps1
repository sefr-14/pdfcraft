# TAB.WINDOW.1 W0 spike: run the Windows validation (disposable test tool).
# Usage:  powershell -ExecutionPolicy Bypass -File <w0-spike>\windows\run.ps1 [-Only basic,h1] [-SkipManual]
#         [-Renderers wgpu,glow] [-Attempts 50] [-Exe <path to w0-spike.exe>] [-List]
# Writes windows\w0-results\<timestamp>\ : one log per run, harness.log and results.csv
# (check,renderer,attempt,outcome,details). Automated checks run unattended; manual checks
# stop with instructions (see CHECKLIST.md). Works in Windows PowerShell 5.1 and PowerShell 7.
param(
    [string[]]$Only = @(),
    [string[]]$Renderers = @('wgpu', 'glow'),
    [int]$Attempts = 50,
    [switch]$SkipManual,
    [switch]$List,
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'

$Automated = @('basic', 'h1', 'h2', 'h3', 'fail', 'clamp', 'panic', 'devlost', 'embed', 'closes', 'a11y-noat')
$Manual = @('m-drag', 'm-closeclicks', 'm-unsaved', 'm-h1variants', 'm-focus', 'm-narrator')
if ($List) {
    "Automated: $($Automated -join ', ')"
    "Manual:    $($Manual -join ', ')"
    return
}

$Spike = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $Spike 'target\release\w0-spike.exe' }
if (-not (Test-Path $Exe)) { throw "w0-spike.exe not found at $Exe - run build.ps1 first" }
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Out = Join-Path $PSScriptRoot "w0-results\$Stamp"
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Csv = Join-Path $Out 'results.csv'
$HLog = Join-Path $Out 'harness.log'
'check,renderer,attempt,outcome,details' | Set-Content -Encoding UTF8 $Csv

# --- Win32 helpers: find windows by title, post WM_CLOSE (what the title-bar X ends in),
# read window rectangles in physical pixels, click. Compiled once by Add-Type.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class W0Win {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
    public const uint WM_CLOSE = 0x0010;
    public static bool Close(IntPtr h) { return PostMessage(h, WM_CLOSE, IntPtr.Zero, IntPtr.Zero); }
    public static void Click(int x, int y) {
        SetCursorPos(x, y);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }
    // Visible frame (DWMWA_EXTENDED_FRAME_BOUNDS, without the invisible resize border).
    public static string Frame(IntPtr h) {
        RECT r;
        if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) return "none";
        return r.Left + "," + r.Top + "," + (r.Right - r.Left) + "x" + (r.Bottom - r.Top);
    }
    // Client area in screen coordinates: what egui calls inner_rect.
    public static string Client(IntPtr h) {
        RECT r; POINT p = new POINT();
        if (!GetClientRect(h, out r) || !ClientToScreen(h, ref p)) return "none";
        return p.X + "," + p.Y + "," + r.Right + "x" + r.Bottom;
    }
}
'@
# Physical pixels everywhere, also on scaled monitors.
[void][W0Win]::SetProcessDPIAware()

function Write-H([string]$msg) {
    $line = "[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss.fff'), $msg
    Add-Content -Encoding UTF8 -Path $HLog -Value $line
    Write-Host $line
}
function Add-Row([string]$check, [string]$renderer, $attempt, [string]$outcome, [string]$details) {
    $d = '"' + ($details -replace '"', '""') + '"'
    Add-Content -Encoding UTF8 -Path $Csv -Value ("{0},{1},{2},{3},{4}" -f $check, $renderer, $attempt, $outcome, $d)
}
function Read-Log([string]$path) {
    if (-not (Test-Path $path)) { return @() }
    # The spike keeps the file open; share it for reading.
    $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try {
        $sr = New-Object System.IO.StreamReader($fs)
        $text = $sr.ReadToEnd()
    } finally { $fs.Dispose() }
    return $text -split "`r?`n"
}
function Wait-LogLine($run, [string]$pattern, [double]$timeoutSec = 60) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $timeoutSec) {
        $hit = Read-Log $run.Log | Where-Object { $_ -match $pattern } | Select-Object -First 1
        if ($hit) { return $hit }
        if ($run.Process.HasExited) { return $null }
        Start-Sleep -Milliseconds 50
    }
    return $null
}
function Find-W([string]$title) { return [W0Win]::FindWindow($null, $title) }

# Start one spike run. $settings: name=value strings (see src/main.rs).
function Start-Spike([string]$name, [string]$renderer, [string[]]$settings) {
    $log = Join-Path $Out "$name-$renderer.log"
    $err = Join-Path $Out "$name-$renderer.stderr.txt"
    $argList = @("renderer=$renderer", "`"log=$log`"") + $settings
    Write-H "start $name renderer=$renderer $($settings -join ' ')"
    $p = Start-Process -FilePath $Exe -ArgumentList ($argList -join ' ') -PassThru -RedirectStandardError $err
    $null = $p.Handle  # keeps ExitCode readable after exit
    $run = [pscustomobject]@{ Name = $name; Renderer = $renderer; Log = $log; Process = $p; Clock = $null }
    $first = Wait-LogLine $run 'ROOT first pass' 90
    if (-not $first) { Write-H "  no first root pass seen" }
    $run.Clock = [Diagnostics.Stopwatch]::StartNew()  # the script clock, as in the spike
    return $run
}
function Wait-Until($run, [double]$sec) {
    while ($run.Clock.Elapsed.TotalSeconds -lt $sec -and -not $run.Process.HasExited) { Start-Sleep -Milliseconds 20 }
}
# Wait for exit (kill after $timeoutSec), then copy its RESULT lines and exit code into the CSV.
function Stop-Spike($run, $attempt, [double]$timeoutSec = 30, [string]$expectExit = '0') {
    if (-not $run.Process.WaitForExit([int]($timeoutSec * 1000))) {
        Write-H "  $($run.Name): still running after $timeoutSec s - killing"
        Stop-Process -Id $run.Process.Id -Force
        $run.Process.WaitForExit()
        Add-Row "$($run.Name)_exit" $run.Renderer $attempt 'FAIL' 'killed by the harness (timeout)'
    } else {
        $rc = $run.Process.ExitCode
        $ok = ($expectExit -eq 'any') -or ("$rc" -eq $expectExit)
        $outcome = 'FAIL'
        if ($ok) { $outcome = 'PASS' }
        Add-Row "$($run.Name)_exit" $run.Renderer $attempt $outcome "rc=$rc expected=$expectExit"
        Write-H "  $($run.Name) exited rc=$rc"
    }
    foreach ($l in (Read-Log $run.Log)) {
        if ($l -match 'RESULT check=(\S+) renderer=(\S+) outcome=(\S+) detail=(.*)$') {
            Add-Row $Matches[1] $Matches[2] $attempt $Matches[3] $Matches[4]
        }
    }
}
# Two "x,y,WxH" rectangles within 2 px on every number.
function Near([string]$a, [string]$b) {
    $na = @([regex]::Matches($a, '-?\d+') | ForEach-Object { [int]$_.Value })
    $nb = @([regex]::Matches($b, '-?\d+') | ForEach-Object { [int]$_.Value })
    if ($na.Count -ne 4 -or $nb.Count -ne 4) { return $false }
    for ($i = 0; $i -lt 4; $i++) { if ([math]::Abs($na[$i] - $nb[$i]) -gt 2) { return $false } }
    return $true
}
function Want([string]$check) { return ($Only.Count -eq 0) -or ($Only -contains $check) }
function Pause-Manual([string]$text) {
    Write-Host ''
    Write-Host '==================== MANUAL STEP ====================' -ForegroundColor Yellow
    Write-Host $text
    Write-Host '=====================================================' -ForegroundColor Yellow
}
function Ask([string]$question) { return (Read-Host $question) }

Write-H "w0-spike Windows run; exe=$Exe; results=$Out"
Write-H "Do not touch the mouse or keyboard during automated checks (they click and close windows)."

foreach ($r in $Renderers) {
    # ---------------------------------------------------------------- automated
    if (Want 'basic') {
        # W0-1/2/5/9/10/12: three windows; A has unsaved work: first close held, second accepted.
        $run = Start-Spike 'basic' $r @('mode=basic')
        Wait-Until $run 1.6
        foreach ($t in 'W0 root', 'W0 sec A', 'W0 sec B') {
            $h = Find-W $t
            Add-Row 'winrect' $r 1 'INFO' "$t hwnd=$h frame_px=$([W0Win]::Frame($h)) client_px=$([W0Win]::Client($h))"
        }
        Wait-Until $run 5.5; $a = Find-W 'W0 sec A'; Write-H '  WM_CLOSE A (unsaved: must be held)'; [void][W0Win]::Close($a)
        Wait-Until $run 6.2
        $kept = [W0Win]::IsWindow($a)
        $o = 'FAIL'; if ($kept) { $o = 'PASS' }
        Add-Row 'unsaved_close_window_kept' $r 1 $o "A still exists after its held close: $kept"
        Wait-Until $run 6.5; Write-H '  WM_CLOSE A (saved: must close)'; [void][W0Win]::Close($a)
        Stop-Spike $run 1 30
    }
    if (Want 'h1') {
        # W0-8 H1: closing the root re-homes S into the root window.
        $run = Start-Spike 'h1' $r @('mode=h1')
        Wait-Until $run 1.5
        $root = Find-W 'W0 root'; $s = Find-W 'W0 sec S'
        $sFrame = [W0Win]::Frame($s); $sClient = [W0Win]::Client($s)
        Add-Row 'h1_before' $r 1 'INFO' "root=$root frame=$([W0Win]::Frame($root)); S=$s frame=$sFrame client=$sClient"
        Wait-Until $run 2.0; Write-H '  WM_CLOSE root'; [void][W0Win]::Close($root)
        Wait-Until $run 3.2
        $after = Find-W 'W0 sec S'
        $rootFrame = [W0Win]::Frame($root); $rootClient = [W0Win]::Client($root)
        $sGone = -not [W0Win]::IsWindow($s)
        $same = ($after -eq $root)
        $o = 'FAIL'; if ($same -and $sGone -and (Near $rootClient $sClient)) { $o = 'PASS' }
        Add-Row 'h1_handoff' $r 1 $o "survivor is old root hwnd=$same; S window gone=$sGone; root client now=$rootClient (S client was $sClient); root frame now=$rootFrame (S frame was $sFrame)"
        Wait-Until $run 4.0; Write-H '  WM_CLOSE root again (last window)'; [void][W0Win]::Close($root)
        Stop-Spike $run 1 20
    }
    if (Want 'h2') {
        # W0-8 H2: hidden root; S keeps animating; checks the invisible-window throttle.
        $run = Start-Spike 'h2' $r @('mode=h2')
        Wait-Until $run 2.0; $root = Find-W 'W0 root'; $s = Find-W 'W0 sec S'; [void][W0Win]::Close($root)
        Wait-Until $run 2.4
        $o = 'FAIL'; if (-not [W0Win]::IsWindowVisible($root)) { $o = 'PASS' }
        Add-Row 'h2_root_invisible' $r 1 $o "root visible after hide: $([W0Win]::IsWindowVisible($root))"
        Wait-Until $run 5.0; [void][W0Win]::Close($s)
        Stop-Spike $run 1 20
        $c = @{}
        foreach ($l in (Read-Log $run.Log)) {
            if ($l -match 'RESULT check=counters_(\S+) .*detail=t=([0-9.]+) .*W0_sec_S=(\d+)') { $c[$Matches[1]] = @([double]$Matches[2], [int]$Matches[3]) }
        }
        if ($c['visible-root-start'] -and $c['visible-root-end'] -and $c['hidden-root-start'] -and $c['hidden-root-end']) {
            $vis = ($c['visible-root-end'][1] - $c['visible-root-start'][1]) / ($c['visible-root-end'][0] - $c['visible-root-start'][0])
            $hid = ($c['hidden-root-end'][1] - $c['hidden-root-start'][1]) / ($c['hidden-root-end'][0] - $c['hidden-root-start'][0])
            $o = 'FAIL'; if ($hid -ge 30) { $o = 'PASS' }
            Add-Row 'h2_secondary_rate' $r 1 $o ("animating S: {0:N0} passes/s with the root visible, {1:N0}/s with it hidden (FAIL below 30/s)" -f $vis, $hid)
        }
    }
    if (Want 'h3') {
        # W0-8 H3: a root created hidden must never become visible (flash check).
        $run = Start-Spike 'h3' $r @('mode=h3')
        $root = Find-W 'W0 root'; $seen = 0; $samples = 0
        while ($run.Clock.Elapsed.TotalSeconds -lt 3.0) {
            if ([W0Win]::IsWindowVisible($root)) { $seen++ }
            $samples++; Start-Sleep -Milliseconds 5
        }
        $o = 'FAIL'; if ($seen -eq 0) { $o = 'PASS' }
        Add-Row 'h3_root_never_visible' $r 1 $o "root visible in $seen of $samples samples (first 3 s after the first pass; a flash before that is not sampled)"
        Wait-Until $run 5.0; [void][W0Win]::Close((Find-W 'W0 sec S'))
        Stop-Spike $run 1 20
    }
    if (Want 'fail') {
        # W0-7 class (a), unclamped, without and with abandoning the window after the failure.
        foreach ($case in 'huge', 'nan', 'inf', 'negative', 'zero', 'farpos') {
            foreach ($drop in 0, 1) {
                $run = Start-Spike "fail-$case-drop$drop" $r @('mode=fail', "fail_case=$case", "fail_drop=$drop")
                Stop-Spike $run 1 20 'any'
            }
        }
    }
    if (Want 'clamp') {
        # W0-7 class (a) prevention: the same bad requests, clamped.
        $run = Start-Spike 'clamp' $r @('mode=clamp')
        Stop-Spike $run 1 60
    }
    if (Want 'panic') {
        # W0-7 class (b): a panic inside a secondary window's closure, with and without the guard.
        foreach ($g in 1, 0) {
            $run = Start-Spike "panic-guard$g" $r @('mode=panic', "inner_guard=$g")
            Wait-Until $run 1.5
            $t = Read-Log $run.Log | Where-Object { $_ -match 'CLICK_TARGET root_button x=(-?\d+) y=(-?\d+)' } | Select-Object -First 1
            if ($t -and $t -match 'x=(-?\d+) y=(-?\d+)') { Write-H "  click root button at $($Matches[1]),$($Matches[2])"; [W0Win]::Click([int]$Matches[1], [int]$Matches[2]) }
            Wait-Until $run 2.0; [void][W0Win]::Close((Find-W 'W0 sec S'))
            $expect = '0'; if ($g -eq 0) { $expect = 'any' }
            Stop-Spike $run 1 20 $expect
        }
    }
    if ((Want 'devlost') -and $r -eq 'wgpu') {
        # W0-7 class (c): simulated device loss (Device::destroy), with and without an error handler.
        foreach ($u in 0, 1) {
            $run = Start-Spike "devlost-uncaptured$u" $r @('mode=devlost', "uncaptured=$u")
            Stop-Spike $run 1 20 'any'
        }
    }
    if (Want 'embed') {
        $run = Start-Spike 'embed' $r @('mode=embed')
        Stop-Spike $run 1 20 'any'
    }
    if (Want 'closes') {
        # W0-9: N posted closes per configuration, counted at eframe and at the app.
        foreach ($target in 'root', 'sec') {
            foreach ($anim in 0, 1) {
                $secs = [int](6 + $Attempts * 0.15 + 3)
                $run = Start-Spike "closes-$target-anim$anim" $r @('mode=closes', "anim=$anim", "secs=$secs")
                Wait-Until $run 1.0
                $title = 'W0 root'; if ($target -eq 'sec') { $title = 'W0 sec C1' }
                $h = Find-W $title; $posted = 0
                for ($i = 1; $i -le $Attempts; $i++) {
                    if ([W0Win]::Close($h)) { $posted++ }
                    Start-Sleep -Milliseconds 150
                }
                Start-Sleep -Milliseconds 1500
                $lines = Read-Log $run.Log
                $vid = 'FFFF'
                if ($target -eq 'sec') {
                    $m = $lines | Where-Object { $_ -match 'Creating new viewport "([0-9A-F]+)" \(Some\("W0 sec C1"\)\)' } | Select-Object -First 1
                    if ($m -and $m -match '"([0-9A-F]+)"') { $vid = $Matches[1] }
                }
                $ef = @($lines | Where-Object { $_ -like "*Received WindowEvent::CloseRequested for viewport Some(`"$vid`")*" }).Count
                $app = @($lines | Where-Object { $_ -like "*CLOSE_RX who=$title *" }).Count
                $o = 'FAIL'; if ($app -eq $Attempts -and $ef -eq $Attempts) { $o = 'PASS' }
                Add-Row "closes_$target" $r "anim$anim" $o "posted=$posted/$Attempts eframe=$ef app=$app (lost before eframe=$($posted - $ef), lost between eframe and app=$($ef - $app))"
                Stop-Spike $run "anim$anim" ($secs + 10)
            }
        }
    }
    if (Want 'a11y-noat') {
        # Fallback check without an assistive technology: detaching must still be allowed.
        $run = Start-Spike 'a11y-noat' $r @('mode=a11y', 'secs=4')
        Stop-Spike $run 1 20
    }

    # ---------------------------------------------------------------- manual
    if ($SkipManual) { continue }
    if (Want 'm-drag') {
        $run = Start-Spike 'm-drag' $r @('mode=drag', 'drag_secs=150')
        Pause-Manual @"
CROSS-WINDOW DRAG ($r). Two windows: 'W0 root' and 'W0 sec S'. The blue band at the top of each
is its tab strip; a press there starts a tab drag. Do each, in order:
 1. drag from root's strip and release on S's strip            (expect verdict=valid)
 2. drag from root's strip and release on S's page area        (expect verdict=invalid)
 3. drag from root's strip and release on the empty desktop    (expect verdict=desktop; a new window opens there)
 4. drag from root's strip to the desktop, press Esc while still holding, then release (expect drag_escape)
 5. drag from S's strip and release on root's strip            (expect verdict=valid)
 6. if you have a second monitor at another scale: move S there, repeat 1 and 3 across monitors
 7. drag from root's strip over the taskbar and release there
Then close every W0 window (or wait 150 s).
"@
        [void](Ask 'Press Enter when done')
        Stop-Spike $run 1 160 'any'
    }
    if (Want 'm-closeclicks') {
        foreach ($anim in 0, 1) {
            $run = Start-Spike "m-closeclicks-anim$anim" $r @('mode=closes', "anim=$anim", 'secs=600')
            Pause-Manual @"
REAL CLOSE CLICKS ($r, animation=$anim). Every close is held, so no window will close.
Click the X of 'W0 root' exactly 10 times, then the X of 'W0 sec C1' exactly 10 times,
at your own pace (about one per second).
"@
            $n = Ask 'How many clicks did you make on each window (normally 10)? Then press Enter'
            Start-Sleep -Milliseconds 1000
            $lines = Read-Log $run.Log
            foreach ($t in 'W0 root', 'W0 sec C1') {
                $app = @($lines | Where-Object { $_ -like "*CLOSE_RX who=$t *" }).Count
                $o = 'FAIL'; if ("$app" -eq "$n") { $o = 'PASS' }
                Add-Row 'closes_manual' $r "anim$anim" $o "$t clicked=$n received by the app=$app"
            }
            Stop-Process -Id $run.Process.Id -Force
            Stop-Spike $run "anim$anim" 5 'any'
        }
    }
    if (Want 'm-unsaved') {
        $run = Start-Spike 'm-unsaved' $r @('mode=basic', 'time_scale=4')
        Pause-Manual @"
UNSAVED-WORK CLOSE ($r). Times are from the moment the windows appeared.
 - between 22 and 24 s: click the X of 'W0 sec A' (it has unsaved work): it must NOT close.
 - after 26 s: click the X of 'W0 sec A' again: it must close (its work was 'saved' at 24 s).
 - 'W0 sec B' closes itself at about 28 s; the app quits at about 32 s.
"@
        Stop-Spike $run 1 60
        $held = Ask 'Did A stay open after the first click and close after the second? (y/n)'
        $o = 'FAIL'; if ($held -match '^[yY]') { $o = 'PASS' }
        Add-Row 'unsaved_close_manual' $r 1 $o "observer answer: $held"
    }
    if (Want 'm-h1variants') {
        foreach ($v in 'maximized', 'snapped', 'other-monitor') {
            $run = Start-Spike "m-h1-$v" $r @('mode=h1', 'time_scale=6')
            Pause-Manual @"
ROOT HANDOFF H1, variant '$v' ($r). Within 9 seconds:
 - maximized:     maximize 'W0 sec S'
 - snapped:       click 'W0 sec S' and press Win+Left
 - other-monitor: drag 'W0 sec S' to the second monitor (the one with a different scale)
At 12 s the script closes 'W0 root'. Watch: the root window must take S's place and size,
S must vanish, nothing may flash. At 24 s the script closes the survivor.
"@
            Wait-Until $run 10.5
            $root = Find-W 'W0 root'; $s = Find-W 'W0 sec S'
            $sFrame = [W0Win]::Frame($s); $sClient = [W0Win]::Client($s)
            Wait-Until $run 12.0; [void][W0Win]::Close($root)
            Wait-Until $run 18.0
            $rootClient = [W0Win]::Client($root)
            $o = 'FAIL'; if ((Near $rootClient $sClient) -and -not [W0Win]::IsWindow($s)) { $o = 'PASS' }
            Add-Row "h1_variant_$v" $r 1 $o "S client before=$sClient frame=$sFrame; root client after=$rootClient frame=$([W0Win]::Frame($root))"
            Wait-Until $run 24.0; [void][W0Win]::Close($root)
            Stop-Spike $run 1 40
            $flash = Ask "Variant ${v}: did you see a flash, a jump through another position, or a wrong size? (describe, or Enter for none)"
            Add-Row "h1_variant_${v}_observed" $r 1 'INFO' "observer: $flash"
        }
    }
    if (Want 'm-focus') {
        $run = Start-Spike 'm-focus' $r @('mode=basic', 'time_scale=4')
        Pause-Manual @"
FOCUS AND GEOMETRY ($r). Click once inside each of the three W0 windows, in the order
root, A, B, root. Then look at each window's edges: they must match where the window is
drawn (no offset, no blank band). The app quits by itself at about 32 s.
"@
        Stop-Spike $run 1 60
        $g = Ask 'Did each window take focus when clicked and draw exactly inside its frame? (y/n, and notes)'
        $o = 'FAIL'; if ($g -match '^[yY]') { $o = 'PASS' }
        Add-Row 'focus_geometry_manual' $r 1 $o "observer: $g"
    }
    if (Want 'm-narrator') {
        Pause-Manual @"
SCREEN READER ($r). Start Narrator now (Ctrl+Win+Enter) and leave it running, then press Enter.
The spike opens with a secondary window; with Narrator active it must merge that window into
the root and refuse 'Move Tab to New Window' with an explanation (see the log).
"@
        [void](Ask 'Narrator running? Press Enter')
        $run = Start-Spike 'm-narrator' $r @('mode=a11y', 'secs=12')
        Stop-Spike $run 1 30
        $run = Start-Spike 'm-narrator-basic' $r @('mode=basic', 'time_scale=3')
        Pause-Manual @"
Narrator still on: three windows are open. Move to each one (click it, or Alt+Tab) and listen.
Expected (the known gap): Narrator reads the root's labels and buttons, but finds nothing inside
'W0 sec A' and 'W0 sec B'. Optionally check with Accessibility Insights or Inspect.exe.
"@
        Stop-Spike $run 1 60
        $a = Ask 'What did Narrator read in the root, and in A/B? (free text)'
        Add-Row 'narrator_observed' $r 1 'INFO' "observer: $a"
        Pause-Manual 'You can stop Narrator now (Ctrl+Win+Enter).'
    }
}

Write-H "done. Results: $Csv"
$rows = Import-Csv $Csv
$fails = @($rows | Where-Object { $_.outcome -eq 'FAIL' })
Write-Host ("{0} rows, {1} FAIL" -f $rows.Count, $fails.Count)
$fails | Format-Table check, renderer, attempt, details -AutoSize | Out-String -Width 220 | Write-Host
Write-Host 'Some FAIL rows are expected (unguarded panic, unclamped huge/nan/inf, device loss): see CHECKLIST.md.'
Write-Host "Next: run collect.ps1 to zip $PSScriptRoot\w0-results for return."

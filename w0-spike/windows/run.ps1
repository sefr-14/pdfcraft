# TAB.WINDOW.1 W0 spike: run the Windows validation (disposable test tool).
# Usage:  powershell -ExecutionPolicy Bypass -File <w0-spike>\windows\run.ps1 [-Only basic,h1] [-SkipManual]
#         [-Unattended] [-Evidence <label>] [-Renderers wgpu,glow] [-Attempts 50] [-Exe <path>] [-List]
# -Unattended replaces every manual step with injected input, Win32 and UI Automation queries and
# screenshots, so nothing waits for a person. Each CSV row records where it ran (-Evidence; the
# default is 'ci-vm' on GitHub Actions, otherwise 'owner-pc'). Run it with Windows PowerShell 5.1
# (UI Automation is a .NET Framework assembly).
# Writes windows\w0-results\<timestamp>\ : one log per run, harness.log and results.csv
# (check,renderer,attempt,outcome,details). Automated checks run unattended; manual checks
# stop with instructions (see CHECKLIST.md). Works in Windows PowerShell 5.1 and PowerShell 7.
param(
    [string[]]$Only = @(),
    [string[]]$Renderers = @('wgpu', 'glow'),
    [int]$Attempts = 50,
    [switch]$SkipManual,
    [switch]$Unattended,
    [string]$Evidence = '',
    [switch]$List,
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'

$Automated = @('basic', 'h1', 'h2', 'h3', 'fail', 'clamp', 'panic', 'devlost', 'embed', 'closes', 'a11y-noat')
$Manual = @('m-drag', 'm-closeclicks', 'm-unsaved', 'm-h1variants', 'm-focus', 'm-narrator')
$Unattend = @('u-drag', 'u-closeclicks', 'u-unsaved', 'u-h1variants', 'u-focus', 'u-uia')
if ($List) {
    "Automated: $($Automated -join ', ')"
    "Manual:    $($Manual -join ', ')"
    "Unattended replacements (-Unattended): $($Unattend -join ', ')"
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
if (-not $Evidence) { if ($env:GITHUB_ACTIONS -eq 'true') { $Evidence = 'ci-vm' } else { $Evidence = 'owner-pc' } }
'check,renderer,attempt,outcome,details,evidence' | Set-Content -Encoding UTF8 $Csv

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

# --- Unattended helpers (-Unattended): injected input (the same Win32 input queue a mouse and
# keyboard feed), title-bar buttons, foreground window, display mode, screenshots, UI Automation.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class W0In {
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [StructLayout(LayoutKind.Sequential)] public struct TITLEBARINFOEX {
        public int cbSize; public RECT rcTitleBar;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 6)] public int[] rgstate;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 6)] public RECT[] rgrect;
    }
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, ref TITLEBARINFOEX l);
    // Centre of the title bar's close button (WM_GETTITLEBARINFOEX), "" if the window has none.
    public static string CloseButton(IntPtr h) {
        TITLEBARINFOEX t = new TITLEBARINFOEX();
        t.cbSize = Marshal.SizeOf(typeof(TITLEBARINFOEX)); t.rgstate = new int[6]; t.rgrect = new RECT[6];
        SendMessage(h, 0x033F, IntPtr.Zero, ref t);
        RECT r = t.rgrect[5];
        if (r.Right <= r.Left || r.Bottom <= r.Top) return "";
        return ((r.Left + r.Right) / 2) + "," + ((r.Top + r.Bottom) / 2);
    }
    // Centre of the title bar (for focusing and moving a window), "" if none.
    public static string TitleBar(IntPtr h) {
        TITLEBARINFOEX t = new TITLEBARINFOEX();
        t.cbSize = Marshal.SizeOf(typeof(TITLEBARINFOEX)); t.rgstate = new int[6]; t.rgrect = new RECT[6];
        SendMessage(h, 0x033F, IntPtr.Zero, ref t);
        RECT r = t.rcTitleBar;
        if (r.Right <= r.Left || r.Bottom <= r.Top) return "";
        return ((r.Left + r.Right) / 2) + "," + ((r.Top + r.Bottom) / 2);
    }
    public static void Down() { mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); }
    public static void Up() { mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero); }
    public static void Key(byte vk, bool down) { keybd_event(vk, 0, down ? 0u : 2u, UIntPtr.Zero); }
    public static string Rect(IntPtr h) {
        RECT r; if (!GetWindowRect(h, out r)) return "none";
        return r.Left + "," + r.Top + "," + (r.Right - r.Left) + "x" + (r.Bottom - r.Top);
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Ansi)] public struct DEVMODE {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
        public int dmFields, dmPositionX, dmPositionY, dmDisplayOrientation, dmDisplayFixedOutput;
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public short dmLogPixels;
        public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2, dmPanningWidth, dmPanningHeight;
    }
    [DllImport("user32.dll", CharSet = CharSet.Ansi)] public static extern bool EnumDisplaySettings(string dev, int mode, ref DEVMODE dm);
    [DllImport("user32.dll", CharSet = CharSet.Ansi)] public static extern int ChangeDisplaySettings(ref DEVMODE dm, int flags);
    // The spike's windows reach x=1400, y=880: make the primary display at least w x h.
    public static string EnsureResolution(int w, int h) {
        DEVMODE dm = new DEVMODE(); dm.dmSize = (short)Marshal.SizeOf(typeof(DEVMODE));
        if (!EnumDisplaySettings(null, -1, ref dm)) return "EnumDisplaySettings failed";
        string before = dm.dmPelsWidth + "x" + dm.dmPelsHeight;
        if (dm.dmPelsWidth >= w && dm.dmPelsHeight >= h) return "kept " + before;
        dm.dmPelsWidth = w; dm.dmPelsHeight = h; dm.dmFields = 0x80000 | 0x100000;
        return "from " + before + " to " + w + "x" + h + ": ChangeDisplaySettings rc=" + ChangeDisplaySettings(ref dm, 0);
    }
}
'@
$script:GuiOk = $true
if ($Unattended) {
    Add-Type -AssemblyName System.Drawing, System.Windows.Forms
    try { Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes; $script:UiaOk = $true } catch { $script:UiaOk = $false }
}
function Nums([string]$s) { return @([regex]::Matches($s, '-?\d+') | ForEach-Object { [int]$_.Value }) }
function Ppp($h) { $d = [W0In]::GetDpiForWindow($h); if ($d -le 0) { $d = 96 }; return $d / 96.0 }
# Points in a window's client area, physical pixels: its tab strip (top 40 pt) and its page area.
function Strip-Point($h) { $c = Nums ([W0Win]::Client($h)); $k = Ppp $h; return @(($c[0] + [int](60 * $k)), ($c[1] + [int](20 * $k))) }
function Page-Point($h) { $c = Nums ([W0Win]::Client($h)); return @(($c[0] + [int]($c[2] / 2)), ($c[1] + [int]($c[3] * 0.7))) }
function Inside($h, $pt) { $r = Nums ([W0In]::Rect($h)); if ($r.Count -ne 4) { return $false }; return ($pt[0] -ge $r[0] -and $pt[0] -lt $r[0] + $r[2] -and $pt[1] -ge $r[1] -and $pt[1] -lt $r[1] + $r[3]) }
function On-Screen($pt) { $b = [System.Windows.Forms.SystemInformation]::VirtualScreen; return ($pt[0] -ge $b.Left -and $pt[0] -lt $b.Right -and $pt[1] -ge $b.Top -and $pt[1] -lt $b.Bottom) }
function Cursor() { $p = New-Object W0In+POINT; [void][W0In]::GetCursorPos([ref]$p); return @($p.X, $p.Y) }
# Press at $from, move in steps (the window keeps the capture), optionally press Escape while
# held, release at $to. Returns where the cursor really was at the release.
function Drag-To($from, $to, [switch]$Escape) {
    [void][W0In]::SetCursorPos($from[0], $from[1]); Start-Sleep -Milliseconds 120
    [W0In]::Down(); Start-Sleep -Milliseconds 120
    for ($i = 1; $i -le 25; $i++) {
        [void][W0In]::SetCursorPos([int]($from[0] + ($to[0] - $from[0]) * $i / 25), [int]($from[1] + ($to[1] - $from[1]) * $i / 25))
        Start-Sleep -Milliseconds 25
    }
    if ($Escape) { [W0In]::Key(0x1B, $true); Start-Sleep -Milliseconds 40; [W0In]::Key(0x1B, $false); Start-Sleep -Milliseconds 200 }
    $at = Cursor
    [W0In]::Up(); Start-Sleep -Milliseconds 500
    return $at
}
function Click-Close($h) {
    $c = [W0In]::CloseButton($h)
    if (-not $c) { return $false }
    $xy = Nums $c
    if (-not (On-Screen $xy)) { return $false }
    [W0Win]::Click($xy[0], $xy[1]); return $true
}
# Screenshot of the whole desktop to <name>.png; returns how many of a sample grid inside the
# window $h (or the whole desktop) are not black, -1 if capture failed.
function Shot([string]$name, $h = [IntPtr]::Zero) {
    try {
        $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
        $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
        $area = @($b.Left, $b.Top, $b.Width, $b.Height)
        if ($h -ne [IntPtr]::Zero) { $c = Nums ([W0Win]::Client($h)); if ($c.Count -eq 4) { $area = $c } }
        $lit = 0
        for ($i = 1; $i -le 8; $i++) {
            for ($j = 1; $j -le 6; $j++) {
                $x = $area[0] - $b.Left + [int]($area[2] * $i / 9); $y = $area[1] - $b.Top + [int]($area[3] * $j / 7)
                if ($x -ge 0 -and $y -ge 0 -and $x -lt $b.Width -and $y -lt $b.Height) { $p = $bmp.GetPixel($x, $y); if (($p.R + $p.G + $p.B) -gt 30) { $lit++ } }
            }
        }
        $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        return $lit
    } catch { Write-H "  screenshot $name failed: $_"; return -1 }
}
# Number of UI Automation elements under a window (-1 on error). Asking is what activates
# AccessKit in the app, exactly as a screen reader's first request does.
function Uia-Count($h) {
    try {
        $e = [System.Windows.Automation.AutomationElement]::FromHandle($h)
        return $e.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition).Count
    } catch { return -1 }
}
function Gui-Blocked([string]$check, [string]$r) {
    if ($script:GuiOk) { return $false }
    Add-Row $check $r 1 'BLOCKED' 'no usable interactive desktop (see the probe rows): not run'
    return $true
}
function Log-Count($run, [string]$pattern) { return @(Read-Log $run.Log | Where-Object { $_ -match $pattern }).Count }

function Write-H([string]$msg) {
    $line = "[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss.fff'), $msg
    Add-Content -Encoding UTF8 -Path $HLog -Value $line
    Write-Host $line
}
function Add-Row([string]$check, [string]$renderer, $attempt, [string]$outcome, [string]$details) {
    $d = '"' + ($details -replace '"', '""') + '"'
    Add-Content -Encoding UTF8 -Path $Csv -Value ("{0},{1},{2},{3},{4},{5}" -f $check, $renderer, $attempt, $outcome, $d, $Evidence)
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

if ($Unattended) {
    # Probe: record what this desktop can do, so a missing capability shows as BLOCKED, never PASS.
    Add-Row 'probe_session' '-' 1 'INFO' ("session={0} user_interactive={1} os={2} powershell={3}" -f (Get-Process -Id $PID).SessionId, [Environment]::UserInteractive, [Environment]::OSVersion.VersionString, $PSVersionTable.PSVersion)
    Add-Row 'probe_resolution' '-' 1 'INFO' ([W0In]::EnsureResolution(1920, 1080))
    Start-Sleep -Milliseconds 500
    $screens = @([System.Windows.Forms.Screen]::AllScreens)
    Add-Row 'probe_monitors' '-' 1 'INFO' ("count=$($screens.Count); " + (($screens | ForEach-Object { "$($_.DeviceName) bounds=$($_.Bounds) work=$($_.WorkingArea) primary=$($_.Primary)" }) -join '; '))
    $gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) driver=$($_.DriverVersion) mode=$($_.CurrentHorizontalResolution)x$($_.CurrentVerticalResolution)" })
    Add-Row 'probe_adapters' '-' 1 'INFO' ($gpus -join '; ')
    [void][W0In]::SetCursorPos(321, 234); Start-Sleep -Milliseconds 150
    $c = Cursor
    $inputOk = ($c[0] -eq 321 -and $c[1] -eq 234)
    $o = 'FAIL'; if ($inputOk) { $o = 'PASS' }
    Add-Row 'probe_cursor_injection' '-' 1 $o "SetCursorPos(321,234) then GetCursorPos=($($c[0]),$($c[1]))"
    $script:GuiOk = $inputOk -and [Environment]::UserInteractive
}

foreach ($r in $Renderers) {
    if ($Unattended) {
        # Does this renderer start at all here? (On a machine without OpenGL 3, glow cannot.)
        $run = Start-Spike 'renderer-start' $r @('mode=solo')
        Wait-Until $run 1.5
        $root = Find-W 'W0 root'
        $lit = -1; if ($root -ne [IntPtr]::Zero) { $lit = Shot "renderer-start-$r" $root }
        $started = ($root -ne [IntPtr]::Zero) -and -not $run.Process.HasExited
        $o = 'FAIL'; if ($started -and $lit -gt 0) { $o = 'PASS' }
        Add-Row 'renderer_draws' $r 1 $o "root window=$root; non-black samples inside its client area=$lit of 48 (screenshot renderer-start-$r.png)"
        if ($started -and $lit -le 0) { $script:GuiOk = $false; Add-Row 'probe_screenshot' $r 1 'FAIL' 'the desktop capture shows nothing inside the window: GUI checks are BLOCKED' }
        Stop-Spike $run 1 20 'any'
        if (-not $started) { Add-Row 'renderer_skipped' $r 1 'BLOCKED' "renderer $r did not open a window here; its checks were not run (see renderer-start-$r.stderr.txt)"; continue }
    }
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


    # ---------------------------------------------------------------- unattended (-Unattended)
    if ($Unattended) {
        if ((Want 'u-drag') -and -not (Gui-Blocked 'u_drag' $r)) {
            # W0-4/W0-5: tab drags between windows, to a page area, to the desktop, with Escape.
            $run = Start-Spike 'u-drag' $r @('mode=drag', 'drag_secs=45')
            Wait-Until $run 1.5
            $root = Find-W 'W0 root'; $s = Find-W 'W0 sec S'
            if ($root -eq [IntPtr]::Zero -or $s -eq [IntPtr]::Zero) { Add-Row 'drag' $r 1 'FAIL' "windows not found (root=$root S=$s)"; Stop-Spike $run 1 60 'any'; $run = $null }
        }
        if ((Want 'u-drag') -and $script:GuiOk -and $run -and $run.Name -eq 'u-drag') {
            [void](Shot "u-drag-start-$r")
            $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
            $desk = $null
            foreach ($cand in @(@(($b.Left + 200), ($b.Bottom - 160)), @(($b.Right - 160), ($b.Bottom - 160)), @(($b.Left + 120), ($b.Bottom - 80)))) {
                if (-not (Inside $root $cand) -and -not (Inside $s $cand)) { $desk = $cand; break }
            }
            $steps = @(
                @('root_to_s_strip', 'W0 root', $root, (Strip-Point $s), 'valid', $false),
                @('root_to_s_page', 'W0 root', $root, (Page-Point $s), 'invalid', $false),
                @('root_to_desktop', 'W0 root', $root, $desk, 'desktop', $false),
                @('root_escape', 'W0 root', $root, $desk, 'escape', $true),
                @('s_to_root_strip', 'W0 sec S', $s, (Strip-Point $root), 'valid', $false)
            )
            $tray = [W0Win]::FindWindow('Shell_TrayWnd', $null)
            if ($tray -ne [IntPtr]::Zero) { $tn = Nums ([W0In]::Rect($tray)); $steps += , @('root_to_taskbar', 'W0 root', $root, @(($tn[0] + [int]($tn[2] / 2)), ($tn[1] + [int]($tn[3] / 2))), 'any', $false) }
            foreach ($st in $steps) {
                $name = $st[0]; $who = $st[1]; $src = $st[2]; $to = $st[3]; $want = $st[4]
                $from = Strip-Point $src
                if (-not $to -or -not (On-Screen $from) -or -not (On-Screen $to)) { Add-Row "drag_$name" $r 1 'BLOCKED' "source or target point off screen (from=$from to=$to)"; continue }
                $drops = Log-Count $run 'RESULT check=drag_drop '; $escs = Log-Count $run 'RESULT check=drag_escape '
                $esc = [bool]$st[5]
                $at = Drag-To $from $to -Escape:$esc
                Start-Sleep -Milliseconds 300
                $new = @(Read-Log $run.Log | Where-Object { $_ -match 'RESULT check=drag_drop ' } | Select-Object -Skip $drops)
                if ($want -eq 'escape') {
                    $e = (Log-Count $run 'RESULT check=drag_escape ') - $escs
                    $o = 'FAIL'; if ($e -eq 1 -and $new.Count -eq 0) { $o = 'PASS' }
                    Add-Row "drag_$name" $r 1 $o "Escape while held: escape lines=$e, drops after it=$($new.Count)"
                    continue
                }
                if ($new.Count -ne 1) { Add-Row "drag_$name" $r 1 'FAIL' "expected one drop, saw $($new.Count) (released at $($at -join ','))"; continue }
                $line = $new[0]
                $verdict = ''; if ($line -match 'verdict=(\S+)') { $verdict = $Matches[1] }
                $delivered = $line -match "release delivered to $([regex]::Escape($who)) "
                $o = 'FAIL'; if ($delivered -and ($want -eq 'any' -or $verdict -eq $want)) { $o = 'PASS' }
                if ($want -eq 'any') { $o = 'INFO' }
                Add-Row "drag_$name" $r 1 $o "verdict=$verdict (expected $want); release delivered to the source $who=$delivered"
                if ($line -match 'desktop_px=\((-?\d+),(-?\d+)\)') {
                    $dx = [math]::Abs([int]$Matches[1] - $at[0]); $dy = [math]::Abs([int]$Matches[2] - $at[1])
                    $o = 'FAIL'; if ($dx -le 2 -and $dy -le 2) { $o = 'PASS' }
                    Add-Row "drag_coords_$name" $r 1 $o "app's desktop position of the release vs the real cursor: off by ($dx,$dy) px (limit 2)"
                }
            }
            [void](Shot "u-drag-end-$r")
            Stop-Process -Id $run.Process.Id -Force -ErrorAction SilentlyContinue
            Stop-Spike $run 1 10 'any'
        }
        if ((Want 'u-closeclicks') -and -not (Gui-Blocked 'u_closeclicks' $r)) {
            # W0-9 through the real title-bar X (non-client click -> SC_CLOSE -> WM_CLOSE).
            foreach ($anim in 0, 1) {
                $run = Start-Spike "u-closeclicks-anim$anim" $r @('mode=closes', "anim=$anim", 'secs=30')
                Wait-Until $run 1.0
                foreach ($t in 'W0 root', 'W0 sec C1') {
                    $h = Find-W $t; $n = 0
                    for ($i = 1; $i -le 10; $i++) { if (Click-Close $h) { $n++ }; Start-Sleep -Milliseconds 350 }
                    Start-Sleep -Milliseconds 800
                    $app = Log-Count $run "CLOSE_RX who=$([regex]::Escape($t)) "
                    $o = 'FAIL'; if ($n -eq 10 -and $app -eq 10) { $o = 'PASS' }
                    if ($n -eq 0) { $o = 'BLOCKED' }
                    Add-Row 'closes_titlebar' $r "anim$anim" $o "$t title-bar X clicked=$n received by the app=$app"
                }
                Stop-Process -Id $run.Process.Id -Force -ErrorAction SilentlyContinue
                Stop-Spike $run "anim$anim" 10 'any'
            }
        }
        if ((Want 'u-unsaved') -and -not (Gui-Blocked 'u_unsaved' $r)) {
            # Unsaved-work close through the title-bar X: the first click is held, the second closes.
            $run = Start-Spike 'u-unsaved' $r @('mode=basic')
            Wait-Until $run 5.5; $a = Find-W 'W0 sec A'; $c1 = Click-Close $a
            Wait-Until $run 6.2; $kept = [W0Win]::IsWindow($a)
            Wait-Until $run 6.5; $c2 = Click-Close $a
            Wait-Until $run 7.5; $gone = -not [W0Win]::IsWindow($a)
            $o = 'FAIL'; if ($c1 -and $c2 -and $kept -and $gone) { $o = 'PASS' }
            if (-not ($c1 -and $c2)) { $o = 'BLOCKED' }
            Add-Row 'unsaved_close_titlebar' $r 1 $o "clicked=$c1/$c2; A kept after the held close=$kept; A gone after the accepted close=$gone"
            Stop-Spike $run 1 30
        }
        if ((Want 'u-h1variants') -and -not (Gui-Blocked 'u_h1variants' $r)) {
            # W0-8 H1 with S maximized, snapped and on another monitor. The root's rectangle is
            # sampled every ~2 ms through the handoff: a rectangle that is neither the old root nor
            # the final one and lasts a frame (17 ms) or more is a visible jump.
            foreach ($v in 'maximized', 'snapped', 'other-monitor') {
                $screens = @([System.Windows.Forms.Screen]::AllScreens)
                if ($v -eq 'other-monitor' -and $screens.Count -lt 2) { Add-Row "h1_variant_$v" $r 1 'SKIP' "one monitor here ($($screens.Count)): needs a second monitor (physical test)"; continue }
                $run = Start-Spike "u-h1-$v" $r @('mode=h1', 'time_scale=6')
                Wait-Until $run 1.5
                $root = Find-W 'W0 root'; $s = Find-W 'W0 sec S'
                $tb = Nums ([W0In]::TitleBar($s))
                if ($tb.Count -eq 2) { [W0Win]::Click($tb[0], $tb[1]); Start-Sleep -Milliseconds 300 }
                if ($v -eq 'maximized') { [void][W0In]::ShowWindow($s, 3) }
                if ($v -eq 'snapped') { [W0In]::Key(0x5B, $true); [W0In]::Key(0x25, $true); [W0In]::Key(0x25, $false); [W0In]::Key(0x5B, $false); Start-Sleep -Milliseconds 800; [W0In]::Key(0x1B, $true); [W0In]::Key(0x1B, $false) }
                if ($v -eq 'other-monitor' -and $tb.Count -eq 2) {
                    $second = $screens | Where-Object { -not $_.Primary } | Select-Object -First 1
                    $wa = $second.WorkingArea
                    [void](Drag-To $tb @(($wa.Left + [int]($wa.Width / 2)), ($wa.Top + 60)))
                }
                Wait-Until $run 10.5
                $sRect = [W0In]::Rect($s); $sClient = [W0Win]::Client($s); $rootRect = [W0In]::Rect($root)
                $work = [System.Windows.Forms.Screen]::FromHandle($s).WorkingArea
                if ($v -eq 'snapped') {
                    $sn = Nums $sRect
                    $isSnapped = ([math]::Abs($sn[2] - $work.Width / 2) -le 24) -and ([math]::Abs($sn[0] - $work.Left) -le 16)
                    if (-not $isSnapped) { Add-Row "h1_variant_$v" $r 1 'SKIP' "Win+Left did not snap S here (S=$sRect, work area=$work): snapping not available"; Stop-Process -Id $run.Process.Id -Force -ErrorAction SilentlyContinue; Stop-Spike $run 1 10 'any'; continue }
                }
                [void](Shot "u-h1-$v-before-$r")
                Wait-Until $run 12.0; [void][W0Win]::Close($root)
                $samples = New-Object System.Collections.ArrayList
                $sw = [Diagnostics.Stopwatch]::StartNew()
                while ($sw.Elapsed.TotalSeconds -lt 3.0) { [void]$samples.Add(@($sw.Elapsed.TotalMilliseconds, [W0In]::Rect($root), [W0Win]::IsWindowVisible($root))); Start-Sleep -Milliseconds 1 }
                $final = [W0In]::Rect($root); $rootClient = [W0Win]::Client($root)
                [void](Shot "u-h1-$v-after-$r")
                # Durations of every distinct rectangle seen while visible.
                $spans = @{}; $prev = $null; $t0 = 0.0
                foreach ($x in $samples) {
                    $key = "$($x[1]) visible=$($x[2])"
                    if ($key -ne $prev) { if ($prev) { $spans[$prev] = [double]$spans[$prev] + ($x[0] - $t0) }; $prev = $key; $t0 = $x[0] }
                }
                if ($prev -and $samples.Count) { $spans[$prev] = [double]$spans[$prev] + ($samples[$samples.Count - 1][0] - $t0) }
                $mids = @($spans.Keys | Where-Object { $_ -like '*visible=True' -and -not (Near ($_ -replace ' visible=.*', '') $rootRect) -and -not (Near ($_ -replace ' visible=.*', '') $final) -and $spans[$_] -ge 17 })
                $handoff = (Near $rootClient $sClient) -and -not [W0Win]::IsWindow($s)
                $o = 'FAIL'; if ($handoff -and $mids.Count -eq 0) { $o = 'PASS' }
                Add-Row "h1_variant_$v" $r 1 $o ("S client before=$sClient; root client after=$rootClient; S gone=$(-not [W0Win]::IsWindow($s)); samples=$($samples.Count); in-between rectangles shown >= 1 frame: " + ($(if ($mids.Count) { ($mids | ForEach-Object { "$_ for $([int]$spans[$_]) ms" }) -join '; ' } else { 'none' })))
                Wait-Until $run 24.0; [void][W0Win]::Close($root)
                Stop-Spike $run 1 40
            }
        }
        if ((Want 'u-focus') -and -not (Gui-Blocked 'u_focus' $r)) {
            # Focus and geometry: a click makes the window foreground; the client area lies inside
            # the visible frame and is drawn (screenshot samples).
            $run = Start-Spike 'u-focus' $r @('mode=basic', 'time_scale=4')
            Wait-Until $run 2.0
            foreach ($t in 'W0 root', 'W0 sec A', 'W0 sec B', 'W0 root') {
                $h = Find-W $t; $pt = Page-Point $h
                if (-not (On-Screen $pt)) { Add-Row 'focus_click' $r $t 'BLOCKED' "window centre off screen ($($pt -join ','))"; continue }
                [W0Win]::Click($pt[0], $pt[1]); Start-Sleep -Milliseconds 400
                $fg = [W0In]::GetForegroundWindow()
                $o = 'FAIL'; if ($fg -eq $h) { $o = 'PASS' }
                Add-Row 'focus_click' $r $t $o "clicked $($pt -join ','); foreground is the clicked window=$($fg -eq $h)"
            }
            foreach ($t in 'W0 root', 'W0 sec A', 'W0 sec B') {
                $h = Find-W $t; $f = Nums ([W0Win]::Frame($h)); $c = Nums ([W0Win]::Client($h))
                $inside = ($f.Count -eq 4 -and $c.Count -eq 4 -and $c[0] -ge $f[0] -and $c[1] -ge $f[1] -and ($c[0] + $c[2]) -le ($f[0] + $f[2]) -and ($c[1] + $c[3]) -le ($f[1] + $f[3]))
                $lit = Shot ("u-focus-" + ($t -replace ' ', '_') + "-$r") $h
                $o = 'FAIL'; if ($inside -and $lit -gt 0) { $o = 'PASS' }
                Add-Row 'geometry_drawn' $r $t $o "frame=$([W0Win]::Frame($h)) client=$([W0Win]::Client($h)) client inside frame=$inside; non-black samples in client=$lit of 48"
            }
            Stop-Spike $run 1 60
        }
        if ((Want 'u-uia') -and -not $script:UiaOk) { Add-Row 'uia' $r 1 'BLOCKED' 'UI Automation assemblies not available in this PowerShell (use Windows PowerShell 5.1)' }
        elseif (Want 'u-uia') {
            # Accessibility baseline without a screen reader: ask UI Automation for each window's
            # elements (this activates AccessKit as a screen reader would). Known gap: secondaries
            # have no tree. The spike then turns its single-window fallback on (a11y_* rows).
            $run = Start-Spike 'u-uia' $r @('mode=a11y', 'secs=8')
            Wait-Until $run 0.8
            $root = Find-W 'W0 root'; $s = Find-W 'W0 sec S'
            $sCount = Uia-Count $s
            Add-Row 'uia_secondary_elements' $r 1 'INFO' "W0 sec S: $sCount UI Automation elements (known gap: 0 expected until the eframe patch)"
            [void](Uia-Count $root); Start-Sleep -Milliseconds 1200
            $rCount = Uia-Count $root
            $o = 'FAIL'; if ($rCount -gt 0) { $o = 'PASS' }
            Add-Row 'uia_root_elements' $r 1 $o "W0 root: $rCount UI Automation elements after activation"
            Stop-Spike $run 1 30
        }
    }

    # ---------------------------------------------------------------- manual
    if ($SkipManual -or $Unattended) { continue }
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

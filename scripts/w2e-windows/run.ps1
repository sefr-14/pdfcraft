# TAB.WINDOW.1 W2e: native multi-window checks on a Windows runner (scratch only, never on Main).
# Drives the release build through its opt-in control channel and real Win32 input:
#   T  the window tools make, move and merge real native windows;
#   S  shortcuts pressed in the second window act there;
#   M  while a question is open in the first window, the second window's keys run nothing;
#   P  a file dialog asked for in a window is owned by that window (GW_OWNER);
#   H  closing the first window while another is open keeps the app (H1).
# Every row is recorded as PASS or FAIL with its evidence; the class of evidence is ci-vm (a
# virtual machine on a GitHub runner with injected input), never physical hardware.
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Renderer = 'wgpu'
)
$ErrorActionPreference = 'Stop'
# Absolute paths: .NET file calls don't follow PowerShell's current location.
$Out = (New-Item -ItemType Directory -Force -Path $Out).FullName
$Bin = (Resolve-Path $Bin).Path
$results = Join-Path $Out 'results.txt'
Set-Content -Path $results -Value "# W2e Windows native checks, renderer $Renderer, evidence class ci-vm" -Encoding utf8

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class W2e {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    public static string Text(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
    public static string Class(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
    // The visible top-level windows of process `pid`.
    public static List<IntPtr> Of(uint pid) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) found.Add(h); return true; }, IntPtr.Zero);
        return found;
    }
    public static IntPtr Owner(IntPtr h) { return GetWindow(h, 4); }
}
'@

function Row([string]$outcome, [string]$check, [string]$details) {
    $line = "$outcome $check`: $details"
    Add-Content -Path $results -Value $line -Encoding utf8
    Write-Host $line
}

# A one-page PDF with a line of text.
function New-Pdf([string]$path, [string]$text) {
    $stream = "BT /F1 48 Tf 72 700 Td ($text) Tj ET"
    $objs = @('<< /Type /Catalog /Pages 2 0 R >>', '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>',
        "<< /Length $($stream.Length) >>`nstream`n$stream`nendstream",
        '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>')
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append("%PDF-1.4`n")
    $offsets = @()
    for ($i = 0; $i -lt $objs.Count; $i++) {
        $offsets += $sb.Length
        [void]$sb.Append("$($i + 1) 0 obj`n$($objs[$i])`nendobj`n")
    }
    $xref = $sb.Length
    [void]$sb.Append("xref`n0 $($objs.Count + 1)`n0000000000 65535 f `n")
    foreach ($o in $offsets) { [void]$sb.Append(('{0:D10} 00000 n ' -f $o) + "`n") }
    [void]$sb.Append("trailer`n<< /Size $($objs.Count + 1) /Root 1 0 R >>`nstartxref`n$xref`n%%EOF`n")
    [System.IO.File]::WriteAllText($path, $sb.ToString(), [System.Text.Encoding]::ASCII)
}

$app = Join-Path $Bin 'pdfcraft.exe'
$cli = Join-Path $Bin 'pdfcraft-cli.exe'
$ctl = Join-Path $Out 'ctl.json'
foreach ($n in 'a', 'b', 'c') { New-Pdf (Join-Path $Out "$n.pdf") "Document $n" }

# One control-channel call; the reply as an object, or a string starting "ERROR".
function Ui([string[]]$callArgs) {
    $text = & $cli ui --control $ctl @callArgs 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { return "ERROR $text" }
    try { return ($text | ConvertFrom-Json) } catch { return "ERROR unparsed: $text" }
}
function Windows-State() { return (Ui @('windows')).windows }
function Tabs([int]$n) {
    $w = Windows-State
    if ($w.Count -lt $n) { return 'gone' }
    return ((@($w)[$n - 1].tabs | ForEach-Object { $_.name }) -join ',')
}
function Window-Field([int]$n, [string]$field) {
    $w = @(Windows-State)
    if ($w.Count -lt $n) { return 'gone' }
    return [string]($w[$n - 1].$field)
}

# Keys go to the foreground window: bring `h` there first (an Alt tap lets this process do it).
function Front([IntPtr]$h) {
    for ($i = 0; $i -lt 5; $i++) {
        [W2e]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [W2e]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
        [void][W2e]::SetForegroundWindow($h)
        Start-Sleep -Milliseconds 400
        if ([W2e]::GetForegroundWindow() -eq $h) { return $true }
    }
    return $false
}
# A chord: virtual-key codes held in order, released in reverse.
function Chord([IntPtr]$h, [byte[]]$keys) {
    [void](Front $h)
    foreach ($k in $keys) { [W2e]::keybd_event($k, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
    [array]::Reverse($keys)
    foreach ($k in $keys) { [W2e]::keybd_event($k, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40 }
    Start-Sleep -Milliseconds 1200
}
$CTRL = 0x11; $SHIFT = 0x10; $PLUS = 0xBB; $KEY_W = 0x57; $KEY_O = 0x4F; $ESC = 0x1B

$script:Proc = $null
function Start-App([string]$name) {
    if (Test-Path $ctl) { Remove-Item $ctl -Force }
    $env:PDFCRAFT_RENDERER = $Renderer
    $env:PDFCRAFT_SYSTEM_FONTS = '0'
    $env:RUST_LOG = 'info'
    $files = @((Join-Path $Out 'a.pdf'), (Join-Path $Out 'b.pdf'), (Join-Path $Out 'c.pdf'))
    $script:Proc = Start-Process -FilePath $app -ArgumentList (@('--control', $ctl) + $files) -PassThru `
        -RedirectStandardError (Join-Path $Out "app-$name.log") -RedirectStandardOutput (Join-Path $Out "app-$name.out")
    for ($i = 0; $i -lt 120 -and -not (Test-Path $ctl); $i++) { Start-Sleep -Milliseconds 500 }
    # Until the control channel answers (the first window and its renderer are up).
    for ($i = 0; $i -lt 60 -and ((Ui @('state')) -is [string]); $i++) { Start-Sleep -Seconds 1 }
    Start-Sleep -Seconds 3
    [void](Ui @('set', 'key=keep_one_window', 'value=false'))
}
function Stop-App() {
    if ($script:Proc -and -not $script:Proc.HasExited) { Stop-Process -Id $script:Proc.Id -Force; Start-Sleep -Seconds 2 }
}
function App-Windows() { return @([W2e]::Of([uint32]$script:Proc.Id) | Where-Object { [W2e]::Text($_) -like '*PDF Overlay*' }) }
function Window-Showing([string]$doc) { return @(App-Windows | Where-Object { [W2e]::Text($_) -like "*$doc*" }) | Select-Object -First 1 }

try {
    # --- T: the window tools make, move and merge real native windows.
    Start-App 't'
    $list = Ui @('windows')
    if ($list -is [string]) { Row 'FAIL' 't_list' $list } else { Row ($(if ($list.available) { 'PASS' } else { 'FAIL' })) 't_list' "available=$($list.available) windows=$(@($list.windows).Count)" }
    [void](Ui @('window.new', 'window=1', 'tab=2')); Start-Sleep -Seconds 3
    $native = @(App-Windows).Count
    Row ($(if ($native -eq 2 -and (Tabs 2) -eq 'c.pdf') { 'PASS' } else { 'FAIL' })) 't_new' "native windows=$native, window 2 tabs=$(Tabs 2)"
    [void](Ui @('window.move', 'window=1', 'tab=0', 'to=2')); Start-Sleep -Seconds 2
    Row ($(if ((Tabs 1) -eq 'b.pdf' -and (Tabs 2) -eq 'c.pdf,a.pdf') { 'PASS' } else { 'FAIL' })) 't_move' "window 1=$(Tabs 1) window 2=$(Tabs 2)"
    [void](Ui @('window.merge')); Start-Sleep -Seconds 3
    $native = @(App-Windows).Count
    Row ($(if ($native -eq 1 -and (Tabs 1) -eq 'b.pdf,c.pdf,a.pdf') { 'PASS' } else { 'FAIL' })) 't_merge' "native windows=$native, window 1=$(Tabs 1)"
    Stop-App

    # --- S: shortcuts in the second window act there.
    Start-App 's'
    [void](Ui @('window.new', 'window=1', 'tab=2')); Start-Sleep -Seconds 3
    [void](Ui @('window.move', 'window=1', 'tab=0', 'to=2')); Start-Sleep -Seconds 2
    $second = Window-Showing 'a.pdf'
    $first = Window-Showing 'b.pdf'
    $z0 = [double](Window-Field 2 'zoom_percent'); $f0 = "$(Window-Field 1 'zoom_percent')/$(Window-Field 1 'rotation')/$(Tabs 1)"
    Chord $second @($CTRL, $PLUS)
    $z1 = [double](Window-Field 2 'zoom_percent')
    Row ($(if ($z1 -gt $z0) { 'PASS' } else { 'FAIL' })) 's_zoom' "Ctrl+= in the second window: zoom $z0 -> $z1 (foreground ok=$([W2e]::GetForegroundWindow() -eq $second))"
    Chord $second @($CTRL, $SHIFT, $PLUS)
    $rot = Window-Field 2 'rotation'
    Row ($(if ($rot -eq '90') { 'PASS' } else { 'FAIL' })) 's_rotate' "Ctrl+Shift++ in the second window: rotation $rot"
    Chord $second @($CTRL, $KEY_W)
    Row ($(if ((Tabs 2) -eq 'c.pdf') { 'PASS' } else { 'FAIL' })) 's_close' "Ctrl+W in the second window: its tabs $(Tabs 2)"
    $f1 = "$(Window-Field 1 'zoom_percent')/$(Window-Field 1 'rotation')/$(Tabs 1)"
    Row ($(if ($f0 -eq $f1) { 'PASS' } else { 'FAIL' })) 's_first_untouched' "$f0 -> $f1"
    Stop-App

    # --- M: a question in the first window holds the second window's keys.
    Start-App 'm'
    [void](Ui @('window.new', 'window=1', 'tab=2')); Start-Sleep -Seconds 3
    [void](Ui @('window.move', 'window=1', 'tab=0', 'to=2')); Start-Sleep -Seconds 2
    [void](Ui @('command', 'id=page.rotate')); Start-Sleep -Seconds 1
    $second = Window-Showing 'a.pdf'
    $first = Window-Showing 'b.pdf'
    $before = "$(Window-Field 2 'zoom_percent')/$(Window-Field 2 'rotation')/$(Tabs 2)"
    [void](Ui @('quit')); Start-Sleep -Seconds 2
    $state = Ui @('state')
    $asked = ($state.close_prompt -eq $true) -and (@($state.windows)[0].dialog -eq $true)
    Chord $second @($CTRL, $PLUS); Chord $second @($CTRL, $SHIFT, $PLUS); Chord $second @($CTRL, $KEY_W)
    $during = "$(Window-Field 2 'zoom_percent')/$(Window-Field 2 'rotation')/$(Tabs 2)"
    $still = (Ui @('state')).close_prompt
    Row ($(if ($asked -and $during -eq $before -and $still -eq $true) { 'PASS' } else { 'FAIL' })) 'm_blocked' "question in the first window=$asked; second window $before -> $during; question still open=$still"
    Chord $first @($ESC)
    Chord $second @($CTRL, $SHIFT, $PLUS); Chord $second @($CTRL, $KEY_W)
    $after = "$(Window-Field 2 'zoom_percent')/$(Window-Field 2 'rotation')/$(Tabs 2)"
    Row ($(if ($after -ne $before -and (Tabs 2) -eq 'c.pdf') { 'PASS' } else { 'FAIL' })) 'm_control' "unblocked: second window $before -> $after"
    Stop-App

    # --- P: a file dialog is owned by the window that asked (Ctrl+O there).
    Start-App 'p'
    [void](Ui @('window.new', 'window=1', 'tab=2')); Start-Sleep -Seconds 3
    $second = Window-Showing 'c.pdf'
    $first = Window-Showing 'b.pdf'
    foreach ($case in @(@('p_second', $second, 2), @('p_first', $first, 1))) {
        $name = $case[0]; $h = $case[1]; $n = $case[2]
        Chord $h @($CTRL, $KEY_O)
        $dialog = [IntPtr]::Zero
        for ($i = 0; $i -lt 30 -and $dialog -eq [IntPtr]::Zero; $i++) {
            Start-Sleep -Milliseconds 500
            $dialog = @([W2e]::Of([uint32]$script:Proc.Id) | Where-Object { [W2e]::Class($_) -eq '#32770' }) | Select-Object -First 1
            if ($null -eq $dialog) { $dialog = [IntPtr]::Zero }
        }
        $owner = if ($dialog -ne [IntPtr]::Zero) { [W2e]::Owner($dialog) } else { [IntPtr]::Zero }
        $last = (Ui @('state')).last_file_dialog
        $ok = ($dialog -ne [IntPtr]::Zero) -and ($owner -eq $h) -and ($last.window -eq $n) -and ($last.parented -eq $true)
        Row ($(if ($ok) { 'PASS' } else { 'FAIL' })) $name "dialog=$dialog '$(if ($dialog -ne [IntPtr]::Zero) { [W2e]::Text($dialog) })' owner=$owner window=$h; state window=$($last.window) parented=$($last.parented)"
        if ($dialog -ne [IntPtr]::Zero) {
            [void][W2e]::PostMessage($dialog, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            Start-Sleep -Seconds 2
            # Cancel by Escape if the dialog ignored WM_CLOSE.
            if ([W2e]::IsWindow($dialog)) { Chord $dialog @($ESC) }
        }
        Start-Sleep -Seconds 3
    }
    Stop-App

    # --- H: closing the first window with another open keeps the app; the first native window
    # takes the other's place (H1).
    Start-App 'h'
    [void](Ui @('window.new', 'window=1', 'tab=2')); Start-Sleep -Seconds 3
    $first = Window-Showing 'b.pdf'
    [void][W2e]::PostMessage($first, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Seconds 4
    $left = @(App-Windows)
    $alive = -not $script:Proc.HasExited
    $ok = $alive -and $left.Count -eq 1 -and $left[0] -eq $first -and ([W2e]::Text($left[0]) -like '*c.pdf*')
    Row ($(if ($ok) { 'PASS' } else { 'FAIL' })) 'h1' "alive=$alive windows=$($left.Count) left=$(if ($left.Count) { [W2e]::Text($left[0]) }) same native window=$(if ($left.Count) { $left[0] -eq $first })"
    Stop-App
} catch {
    Row 'FAIL' 'script' $_.ToString()
    Stop-App
}
$fails = @(Get-Content $results | Where-Object { $_ -like 'FAIL *' }).Count
Write-Host "W2e Windows native checks: $fails failing row(s); results in $results"
if ($fails -gt 0) { exit 1 }

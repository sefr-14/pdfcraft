# TAB.WINDOW.1 W0 spike: zip windows\w0-results plus system information for return.
# Usage:  powershell -ExecutionPolicy Bypass -File <w0-spike>\windows\collect.ps1
# Writes windows\w0-results-<computer>-<timestamp>.zip. Nothing is uploaded anywhere.
# The zip holds the spike logs, results.csv, build-info.txt and system.txt (Windows version,
# GPU and driver, monitors with their resolution and scale). It contains the computer name in
# the file name only; delete or rename before sharing if that matters.
$ErrorActionPreference = 'Stop'
$results = Join-Path $PSScriptRoot 'w0-results'
if (-not (Test-Path $results)) { throw "no results folder at $results - run run.ps1 first" }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class W0Dpi {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    public delegate bool MonitorEnumProc(IntPtr hMonitor, IntPtr hdc, IntPtr rect, IntPtr data);
    [DllImport("user32.dll")] public static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc proc, IntPtr data);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MONITORINFOEX { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string szDevice; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool GetMonitorInfo(IntPtr h, ref MONITORINFOEX mi);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr h, int type, out uint dpiX, out uint dpiY);
    public static string Describe() {
        SetProcessDPIAware();
        var sb = new System.Text.StringBuilder();
        EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, delegate (IntPtr h, IntPtr hdc, IntPtr r, IntPtr d) {
            var mi = new MONITORINFOEX(); mi.cbSize = Marshal.SizeOf(typeof(MONITORINFOEX));
            GetMonitorInfo(h, ref mi);
            uint x = 0, y = 0; GetDpiForMonitor(h, 0, out x, out y);
            sb.AppendFormat("{0}: {1},{2} {3}x{4} px, dpi {5} (scale {6}%){7}\r\n", mi.szDevice, mi.rcMonitor.Left, mi.rcMonitor.Top,
                mi.rcMonitor.Right - mi.rcMonitor.Left, mi.rcMonitor.Bottom - mi.rcMonitor.Top, x, x * 100 / 96,
                (mi.dwFlags & 1) != 0 ? " primary" : "");
            return true;
        }, IntPtr.Zero);
        return sb.ToString();
    }
}
'@

$sys = Join-Path $results 'system.txt'
$o = @()
$os = Get-CimInstance Win32_OperatingSystem
$o += "date: $(Get-Date -Format o)"
$o += "windows: $($os.Caption) $($os.Version) build $($os.BuildNumber) $($os.OSArchitecture)"
$o += "powershell: $($PSVersionTable.PSVersion)"
foreach ($g in (Get-CimInstance Win32_VideoController)) {
    $o += "gpu: $($g.Name); driver $($g.DriverVersion) ($($g.DriverDate)); mode $($g.CurrentHorizontalResolution)x$($g.CurrentVerticalResolution)"
}
$o += 'monitors (physical pixels, per-monitor DPI):'
$o += [W0Dpi]::Describe()
if (Get-Command rustc -ErrorAction SilentlyContinue) { $o += "rustc: $(& rustc --version)" }
$o | Set-Content -Encoding UTF8 $sys
Write-Host ($o -join "`n")

$zip = Join-Path $PSScriptRoot ("w0-results-{0}-{1}.zip" -f $env:COMPUTERNAME, (Get-Date -Format 'yyyyMMdd-HHmmss'))
Compress-Archive -Path (Join-Path $results '*') -DestinationPath $zip -Force
Write-Host "Wrote $zip ($([math]::Round((Get-Item $zip).Length / 1KB)) KB). Send this file back; nothing was uploaded."

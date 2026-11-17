# TAB.WINDOW.1 W0 spike: build for Windows (disposable test tool, not PDF Overlay).
# Usage, from any folder:  powershell -ExecutionPolicy Bypass -File <w0-spike>\windows\build.ps1
# Builds target\release\w0-spike.exe (and xclose.exe, a no-op on Windows). Both renderers are
# compiled into the one binary (eframe features "wgpu" and "glow"); run.ps1 picks one per run
# with renderer=wgpu or renderer=glow. Takes 5-15 minutes the first time.
param([switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
$Spike = Split-Path -Parent $PSScriptRoot
Set-Location $Spike

$results = Join-Path $PSScriptRoot 'w0-results'
New-Item -ItemType Directory -Force -Path $results | Out-Null
$info = Join-Path $results 'build-info.txt'

$lines = @()
$lines += "date: $(Get-Date -Format o)"
$lines += "rustc: $(& rustc --version)"
$lines += "cargo: $(& cargo --version)"
$lines += "host: $((& rustc -vV | Select-String '^host:').ToString())"
$commit = 'unknown (not a git checkout)'
if (Get-Command git -ErrorAction SilentlyContinue) {
    $c = & git -C $Spike rev-parse HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $c) { $commit = $c }
}
$lines += "commit: $commit"
$lines | ForEach-Object { Write-Host $_ }

$env:CARGO_INCREMENTAL = '0'
$profileArgs = @('--release')
if ($DebugBuild) { $profileArgs = @() }
# --locked: build exactly the dependency versions in w0-spike\Cargo.lock (the app's pins).
& cargo build --locked --bins @profileArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

$dir = 'release'
if ($DebugBuild) { $dir = 'debug' }
$exe = Join-Path $Spike "target\$dir\w0-spike.exe"
if (-not (Test-Path $exe)) { throw "build finished but $exe is missing" }
$lines += "exe: $exe"
$lines += "exe sha256: $((Get-FileHash -Algorithm SHA256 $exe).Hash.ToLowerInvariant())"
$lines | Set-Content -Encoding UTF8 $info
Write-Host "Built $exe"
Write-Host "Build info written to $info"

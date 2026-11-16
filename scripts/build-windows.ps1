# Build Caption Studio for Windows (single self-contained .exe).
# Run from the caption-studio directory in PowerShell:
#   .\scripts\build-windows.ps1
$ErrorActionPreference = "Stop"
cargo build --release --locked
$exe = "target\release\caption-studio.exe"
if (-not (Test-Path $exe)) { throw "build failed: $exe not found" }
Write-Host "Built: $exe ($((Get-Item $exe).Length) bytes)"
$dlls = Get-ChildItem "target\release" -Filter *.dll -ErrorAction SilentlyContinue
if ($dlls) { Write-Warning "DLLs beside the exe (should be none from this app): $($dlls.Name -join ', ')" }
else { Write-Host "No app DLLs beside the exe. Ready to ship." }

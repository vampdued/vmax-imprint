<#
.SYNOPSIS
    Runs Caption Studio screenshot test, converts PNGs to compressed JPEGs, and packages into a ZIP archive.

.DESCRIPTION
    1. Drives Caption Studio (--shot-test) through every reviewable UI state at 1920x1080.
    2. Converts captured PNGs to compressed JPEGs (~85% quality) to drastically reduce file size (~35MB -> ~1.7MB).
    3. Replaces .png references with .jpg in shots.txt.
    4. Bundles the output folder into a clean, cross-platform ZIP archive ready for sharing/review.

.PARAMETER InputFolder
    Folder of images to scan. Defaults to C:\Users\VampDued\Downloads\!!!!!!!

.PARAMETER OutputFolder
    Destination folder for screenshots. Defaults to .\shot-test-output

.PARAMETER TranslatedJson
    Optional translated JSON to import. Defaults to Translated texts.json in InputFolder if present.

.PARAMETER Quality
    JPEG quality setting (1-100). Defaults to 85.

.PARAMETER Exe
    Optional path to caption-studio.exe. Defaults to standard cargo target locations.
#>

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [string]$InputFolder = "C:\Users\VampDued\Downloads\!!!!!!!",

    [Parameter(Position = 1)]
    [string]$OutputFolder = "",

    [Parameter(Position = 2)]
    [string]$TranslatedJson = "",

    [Parameter(Position = 3)]
    [int]$Quality = 85,

    [string]$Exe = ""
)

$ErrorActionPreference = "Stop"

# Determine repository root (one level up from scripts/)
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir
if (-not $RepoRoot) { $RepoRoot = (Get-Location).Path }

# Default OutputFolder
if (-not $OutputFolder) {
    $OutputFolder = Join-Path $RepoRoot "shot-test-output"
}
$OutputFolder = [System.IO.Path]::GetFullPath($OutputFolder)

# Locate executable
if (-not $Exe) {
    $envTarget = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "release\caption-studio.exe" } else { $null }
    $candidates = @(
        "D:\cargo-target\release\caption-studio.exe",
        $envTarget,
        (Join-Path $RepoRoot "target\release\caption-studio.exe")
    ) | Where-Object { $_ -and (Test-Path $_) }

    if ($candidates.Count -gt 0) {
        $Exe = $candidates[0]
    } else {
        Write-Error "Could not find caption-studio.exe. Looked at:`n  - D:\cargo-target\release\caption-studio.exe`n  - target\release\caption-studio.exe`nPlease run 'cargo build --release --offline' first."
        exit 2
    }
}

# Default TranslatedJson
if (-not $TranslatedJson) {
    $candidateJson1 = Join-Path $InputFolder "Translated texts.json"
    $candidateJson2 = "C:\Users\VampDued\Downloads\!!!!!!!\Translated texts.json"
    if (Test-Path $candidateJson1) {
        $TranslatedJson = $candidateJson1
    } elseif (Test-Path $candidateJson2) {
        $TranslatedJson = $candidateJson2
    }
}

# Quality bounds
$Quality = [math]::Max(10, [math]::Min(100, $Quality))

Write-Host "========================================================" -ForegroundColor Cyan
Write-Host "Caption Studio Shot Test & JPEG Packager" -ForegroundColor Cyan
Write-Host "  EXE:      $Exe"
Write-Host "  INPUT:    $InputFolder"
Write-Host "  OUTPUT:   $OutputFolder"
if ($TranslatedJson) {
    Write-Host "  JSON:     $TranslatedJson"
} else {
    Write-Host "  JSON:     (none)"
}
Write-Host "  QUALITY:  $Quality%"
Write-Host "========================================================" -ForegroundColor Cyan
Write-Host ""

if (-not (Test-Path $InputFolder)) {
    Write-Error "Input folder does not exist: $InputFolder"
    exit 1
}

# Ensure output directory exists
if (-not (Test-Path $OutputFolder)) {
    New-Item -ItemType Directory -Path $OutputFolder -Force | Out-Null
}

# 1. Run caption-studio --shot-test
Write-Host "[1/3] Running screenshot test in caption-studio..." -ForegroundColor Yellow

$argList = @("--shot-test", "`"$InputFolder`"", "`"$OutputFolder`"")
if ($TranslatedJson -and (Test-Path $TranslatedJson)) {
    $argList += @("--json", "`"$TranslatedJson`"")
}
$argString = $argList -join " "

$proc = Start-Process -FilePath $Exe -ArgumentList $argString -Wait -PassThru -NoNewWindow
if ($proc.ExitCode -ne 0) {
    Write-Error "Shot test exited with error code: $($proc.ExitCode)"
    exit $proc.ExitCode
}

# 2. Convert PNGs to JPEG
Write-Host ""
Write-Host "[2/3] Converting PNG screenshots to JPEG ($Quality% quality)..." -ForegroundColor Yellow

Add-Type -AssemblyName System.Drawing

$pngFiles = Get-ChildItem -Path $OutputFolder -Filter "*.png" -File
if ($pngFiles.Count -eq 0) {
    Write-Warning "No PNG screenshots found in $OutputFolder."
} else {
    $origTotalBytes = ($pngFiles | Measure-Object -Property Length -Sum).Sum

    $codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object { $_.FormatDescription -eq "JPEG" }
    $encoderParams = New-Object System.Drawing.Imaging.EncoderParameters(1)
    $encoderParams.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter([System.Drawing.Imaging.Encoder]::Quality, [long]$Quality)

    foreach ($png in $pngFiles) {
        $jpgPath = [System.IO.Path]::ChangeExtension($png.FullName, ".jpg")
        try {
            $srcBmp = [System.Drawing.Bitmap]::FromFile($png.FullName)
            # Create a 24bpp RGB surface with solid white background to guarantee zero alpha/black artifacts
            $rgbBmp = New-Object System.Drawing.Bitmap($srcBmp.Width, $srcBmp.Height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
            $gfx = [System.Drawing.Graphics]::FromImage($rgbBmp)
            $gfx.Clear([System.Drawing.Color]::White)
            $gfx.DrawImage($srcBmp, 0, 0, $srcBmp.Width, $srcBmp.Height)
            $gfx.Dispose()
            $srcBmp.Dispose()

            $rgbBmp.Save($jpgPath, $codec, $encoderParams)
            $rgbBmp.Dispose()

            Remove-Item -Force $png.FullName
            Write-Host "  Converted: $($png.Name) -> $([System.IO.Path]::GetFileName($jpgPath))" -ForegroundColor Gray
        } catch {
            Write-Warning "Failed to convert $($png.FullName): $_"
        }
    }

    # Update shots.txt to refer to .jpg
    $shotsTxt = Join-Path $OutputFolder "shots.txt"
    if (Test-Path $shotsTxt) {
        $txtContent = Get-Content -Path $shotsTxt -Raw
        $txtContent = $txtContent.Replace(".png", ".jpg")
        Set-Content -Path $shotsTxt -Value $txtContent -NoNewline
    }
}

# 3. Create ZIP Archive
Write-Host ""
Write-Host "[3/3] Creating ZIP archive..." -ForegroundColor Yellow

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$zipPath = $OutputFolder.TrimEnd('\/') + ".zip"
if (Test-Path $zipPath) {
    Remove-Item -Force $zipPath
}

$baseName = Split-Path -Leaf $OutputFolder
$zipArchive = [System.IO.Compression.ZipFile]::Open($zipPath, [System.IO.Compression.ZipArchiveMode]::Create)
$outputFiles = Get-ChildItem -Path $OutputFolder -Recurse -File

foreach ($file in $outputFiles) {
    $rel = $file.FullName.Substring($OutputFolder.Length).TrimStart('\/')
    $entryName = $baseName + "/" + ($rel.Replace('\', '/'))
    [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zipArchive, $file.FullName, $entryName, [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
}
$zipArchive.Dispose()

# Summary
$jpgFiles = Get-ChildItem -Path $OutputFolder -Filter "*.jpg" -File
$newTotalBytes = ($jpgFiles | Measure-Object -Property Length -Sum).Sum
$zipBytes = (Get-Item $zipPath).Length
$mb = 1048576.0

$pctSaved = 0
if ($origTotalBytes -and $origTotalBytes -gt 0) {
    $pctSaved = [math]::Round((1.0 - ($newTotalBytes / $origTotalBytes)) * 100.0, 1)
}

Write-Host ""
Write-Host "========================================================" -ForegroundColor Green
Write-Host "Shot Test & Compression Complete!" -ForegroundColor Green
Write-Host ("  Screenshots:  {0} images" -f $jpgFiles.Count)
if ($origTotalBytes) {
    Write-Host ("  Original PNG: {0:N2} MB" -f ($origTotalBytes / $mb))
    Write-Host ("  Converted JPG:{0:N2} MB  ({1}% size reduction)" -f ($newTotalBytes / $mb), $pctSaved)
}
Write-Host ("  Output Dir:   {0}" -f $OutputFolder)
Write-Host ("  ZIP Archive:  {0} ({1:N2} MB)" -f $zipPath, ($zipBytes / $mb)) -ForegroundColor Cyan
Write-Host "========================================================" -ForegroundColor Green

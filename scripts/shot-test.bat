@echo off
REM VMAX Imprint shot-test (requires: cargo build --release --offline --features shot-test): drives the app through every reviewable
REM state and saves a PNG per state into the output folder.
REM
REM Usage: scripts\shot-test.bat [input-folder] [output-folder] [translated-json]
REM   input-folder    Folder of images to scan (default: C:\Users\VampDued\Downloads\!!!!!!!).
REM   output-folder   Where the PNGs go (default: .\shot-test-output).
REM   translated-json Optional translated JSON to import (default: C:\Users\VampDued\Downloads\!!!!!!!\Translated texts.json).
setlocal
cd /d "%~dp0.."

set "EXE=D:\cargo-target\release\vmax-imprint.exe"
if not exist "%EXE%" if exist "%CARGO_TARGET_DIR%\release\vmax-imprint.exe" set "EXE=%CARGO_TARGET_DIR%\release\vmax-imprint.exe"
if not exist "%EXE%" if exist "target\release\vmax-imprint.exe" set "EXE=target\release\vmax-imprint.exe"
if not exist "%EXE%" if exist "D:\cargo-target\release\caption-studio.exe" set "EXE=D:\cargo-target\release\caption-studio.exe"
if not exist "%EXE%" if exist "%CARGO_TARGET_DIR%\release\caption-studio.exe" set "EXE=%CARGO_TARGET_DIR%\release\caption-studio.exe"
if not exist "%EXE%" if exist "target\release\caption-studio.exe" set "EXE=target\release\caption-studio.exe"

if not exist "%EXE%" (
  echo Could not find vmax-imprint.exe at:
  echo   D:\cargo-target\release\vmax-imprint.exe
  echo   or target\release\vmax-imprint.exe
  echo Please run 'cargo build --release --offline --features shot-test' first.
  pause
  exit /b 2
)

set "INPUT=%~1"
if "%INPUT%"=="" set "INPUT=C:\Users\VampDued\Downloads\!!!!!!!"

set "OUTPUT=%~2"
if "%OUTPUT%"=="" set "OUTPUT=%~dp0..\shot-test-output"

set "JSON=%~3"
if "%JSON%"=="" (
  if exist "%INPUT%\Translated texts.json" (
    set "JSON=%INPUT%\Translated texts.json"
  ) else if exist "C:\Users\VampDued\Downloads\!!!!!!!\Translated texts.json" (
    set "JSON=C:\Users\VampDued\Downloads\!!!!!!!\Translated texts.json"
  )
)

echo ========================================================
echo VMAX Imprint Shot Test
echo   EXE:    %EXE%
echo   INPUT:  %INPUT%
echo   OUTPUT: %OUTPUT%
echo   JSON:   %JSON%
echo ========================================================
echo.

if "%JSON%"=="" (
  "%EXE%" --shot-test "%INPUT%" "%OUTPUT%"
) else (
  "%EXE%" --shot-test "%INPUT%" "%OUTPUT%" --json "%JSON%"
)

echo.
echo Done. Screenshots are in %OUTPUT%
echo Upload that folder (or the PNGs) to review.
pause

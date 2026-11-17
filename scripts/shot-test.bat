@echo off
REM Caption Studio shot-test: drives the app through every reviewable
REM state and saves a PNG per state into the output folder.
REM
REM Usage: scripts\shot-test.bat <input-folder> [output-folder] [translated-json]
REM   input-folder    Folder of images to scan (your real folder).
REM   output-folder   Where the PNGs go (default: .\shot-test-output).
REM   translated-json Optional translated JSON to import first.
REM
REM Example:
REM   scripts\shot-test.bat "C:\Users\VampDued\Pictures\krithika-memes"
REM   scripts\shot-test.bat "C:\Users\VampDued\Pictures\krithika-memes" "C:\Users\VampDued\Desktop\shots" "C:\Users\VampDued\Downloads\Translated texts.json"
setlocal
cd /d "%~dp0.."

if not exist "target\release\caption-studio.exe" (
  echo Build the exe first:  .\scripts\build-windows.ps1
  pause
  exit /b 2
)

set "INPUT=%~1"
if "%INPUT%"=="" (
  echo Usage: shot-test.bat ^<input-folder^> [output-folder] [translated-json]
  echo   Example: scripts\shot-test.bat "C:\Users\VampDued\Pictures\krithika-memes"
  pause
  exit /b 2
)
set "OUTPUT=%~2"
if "%OUTPUT%"=="" set "OUTPUT=%~dp0..\shot-test-output"
set "JSON=%~3"

if "%JSON%"=="" (
  target\release\caption-studio.exe --shot-test "%INPUT%" "%OUTPUT%"
) else (
  target\release\caption-studio.exe --shot-test "%INPUT%" "%OUTPUT%" --json "%JSON%"
)

echo.
echo Done. Screenshots are in %OUTPUT%
echo Upload that folder (or the PNGs) to review.
pause

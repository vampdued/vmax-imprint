@echo off
REM =========================================================================
REM VMAX Imprint Shot Test & JPEG Packager
REM
REM Drives the app through all reviewable states, converts PNG screenshots
REM to compressed JPEGs (~85% quality), and packages them into a clean ZIP.
REM
REM Usage:
REM   scripts\shot-test-jpeg.bat [input-folder] [output-folder] [translated-json] [jpeg-quality]
REM =========================================================================
setlocal
cd /d "%~dp0.."

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0shot-test-jpeg.ps1" %*
set "EXIT_CODE=%errorlevel%"

echo.
if %EXIT_CODE% equ 0 (
  echo Done. You can upload or share the generated .zip file.
) else (
  echo [ERROR] Process exited with code %EXIT_CODE%.
)
pause
exit /b %EXIT_CODE%

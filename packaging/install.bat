@echo off
rem -----------------------------------------------------------------------
rem  tex-suite installer launcher (Windows)
rem
rem  Double-clickable wrapper around install-windows.ps1. Runs PowerShell
rem  with the execution policy bypassed for this process only (no system
rem  policy change), passing through any arguments you supply on the
rem  command line, e.g.:
rem
rem      install.bat -System
rem      install.bat -Uninstall
rem      install.bat -Help
rem
rem  The window stays open at the end so double-click launches from File
rem  Explorer do not vanish before you can read the result.
rem -----------------------------------------------------------------------
setlocal
set "PS1=%~dp0install-windows.ps1"

if not exist "%PS1%" (
    echo [tex-suite] ERROR: install-windows.ps1 not found next to this file.
    echo [tex-suite]        Expected: %PS1%
    echo.
    pause
    exit /b 1
)

echo [tex-suite] Running installer: "%PS1%" %*
echo.

rem Prefer Windows PowerShell (present on every supported Windows version);
rem ExecutionPolicy Bypass applies to this one process, nothing persistent.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%PS1%" %*
set "RC=%ERRORLEVEL%"

if not "%RC%"=="0" (
    echo.
    echo [tex-suite] Installer exited with code %RC% - see messages above.
    echo.
)

echo Press any key to close this window...
pause >nul
exit /b %RC%

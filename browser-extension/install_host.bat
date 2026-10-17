@echo off
rem ============================================================
rem WinOCR Native Messaging Host - installer launcher
rem Launches the PowerShell installer with bypass policy.
rem ============================================================
setlocal
set SCRIPT_DIR=%~dp0
powershell -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%install_host.ps1"
if errorlevel 1 (
    echo.
    echo [X] Install failed. See messages above.
    pause
    exit /b 1
)
echo.
pause
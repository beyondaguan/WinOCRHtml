@echo off
setlocal
cd /d "%~dp0"

set "EXE_DEBUG=%~dp0target\debug\winocr.exe"
set "EXE_REL=%~dp0target\release\winocr.exe"

echo ==========================================
echo   WinOCR build helper  -  Rust + Slint
echo ==========================================
echo.

where cargo >nul 2>&1
if errorlevel 1 (
  echo [-] cargo not found in PATH.
  echo     Install Rust first: https://rustup.rs
  goto :end
)

echo [*] ensuring MSVC toolchain is default ...
rustup default stable-x86_64-pc-windows-msvc >nul 2>&1
if errorlevel 1 echo [!] rustup failed or missing - continuing anyway

echo.
echo   [1] build debug + run
echo   [2] build debug only
echo   [3] build release + run
echo   [4] exit
echo.
set "CHOICE="
set /p CHOICE=Choose 1-4 [default 1]:
if "%CHOICE%"=="" set "CHOICE=1"

if "%CHOICE%"=="1" goto :dbg_run
if "%CHOICE%"=="2" goto :dbg
if "%CHOICE%"=="3" goto :rel
if "%CHOICE%"=="4" goto :end
echo [-] invalid choice.
goto :end

:dbg
echo.
echo [*] cargo build -p winocr ...
cargo build -p winocr
if errorlevel 1 goto :buildfail
echo [+] build OK: %EXE_DEBUG%
goto :end

:dbg_run
echo.
echo [*] cargo build -p winocr ...
cargo build -p winocr
if errorlevel 1 goto :buildfail
echo [+] build OK: %EXE_DEBUG%
echo [*] launching debug build ...
echo     If OpenGL error appears, run:  set SLINT_BACKEND=winit-software
"%EXE_DEBUG%"
goto :end

:rel
echo.
echo [*] cargo build -p winocr --release ...
cargo build -p winocr --release
if errorlevel 1 goto :buildfail
echo [+] build OK: %EXE_REL%
echo [*] launching release build ...
"%EXE_REL%"
goto :end

:buildfail
echo.
echo [-] build FAILED - see the errors above.
goto :end

:end
echo.
pause
endlocal

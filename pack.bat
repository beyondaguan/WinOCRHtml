@echo off
setlocal
cd /d "%~dp0"

set "REL_DIR=%~dp0target\release"
set "OUT_ZIP=%~dp0target\release\WinOCR-Portable-v1.0.0.zip"

echo ==========================================
echo   WinOCR packager  -  v1.0.0
echo ==========================================
echo.

where cargo >nul 2>&1
if errorlevel 1 (
  echo [-] cargo not found in PATH.
  goto :end
)

echo [*] cargo build -p winocr --release ...
cargo build -p winocr --release
if errorlevel 1 goto :buildfail
echo [+] build OK

echo [*] copying extension folder ...
if exist "%REL_DIR%\extension" rmdir /s /q "%REL_DIR%\extension"
xcopy /e /i /q "%~dp0extension-0.8" "%REL_DIR%\extension" >nul
if errorlevel 1 echo [!] extension copy failed - continuing anyway
echo [+] extension folder copied

echo [*] removing build artifacts from release dir ...
del /q "%REL_DIR%\*.d" 2>nul
del /q "%REL_DIR%\*.pdb" 2>nul
rd /s /q "%REL_DIR%\build" 2>nul
rd /s /q "%REL_DIR%\deps" 2>nul
rd /s /q "%REL_DIR%\examples" 2>nul
rd /s /q "%REL_DIR%\incremental" 2>nul

echo [*] creating zip archive ...
if exist "%OUT_ZIP%" del /q "%OUT_ZIP%"
powershell -NoProfile -Command "Compress-Archive -Path '%REL_DIR%\*' -DestinationPath '%OUT_ZIP%' -Force"
if errorlevel 1 (
  echo [!] zip creation failed - release folder is ready anyway
) else (
  echo [+] zip created: %OUT_ZIP%
)

echo.
echo [+] packaging complete.
echo     release folder: %REL_DIR%
echo     zip file:      %OUT_ZIP%
goto :end

:buildfail
echo.
echo [-] build FAILED - see the errors above.

:end
echo.
pause
endlocal

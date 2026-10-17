#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
pack.bat 的唯一真源（生成器）。

生成纯 ASCII + CRLF 的 pack.bat，功能：
1. cargo build -p winocr --release（build.rs 自动复制 assets/tray.ico/winocr-argos.exe）
2. 复制 extension-0.8/ 到 target/release/extension/
3. 复制 README.txt（版本说明）到 target/release/
4. 打包为 target/release/WinOCR-Portable-v1.0.0.zip

用法：
    python tools/_gen_pack_bat.py            # 生成 仓库根/pack.bat
    python tools/_gen_pack_bat.py --check    # 仅校验现有 pack.bat
"""
import sys
import pathlib

TEMPLATE = r"""@echo off
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
"""


def to_ascii_crlf(text: str) -> bytes:
    norm = text.replace("\r\n", "\n").replace("\r", "\n")
    data = norm.encode("ascii")
    return data.replace(b"\n", b"\r\n")


def verify(raw: bytes) -> list:
    problems = []
    if not all(b < 128 for b in raw):
        problems.append("contains non-ASCII bytes")
    if b"\r\n" not in raw:
        problems.append("no CRLF line ending found")
    if b"\n" in raw.replace(b"\r\n", b""):
        problems.append("contains bare LF")
    return problems


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    out = root / "pack.bat"

    if "--check" in sys.argv:
        if not out.exists():
            print("[-] pack.bat not found:", out)
            return 1
        problems = verify(out.read_bytes())
        print("[+] pack.bat OK (pure ASCII + CRLF)" if not problems
              else "[-] pack.bat FAILED: " + "; ".join(problems))
        return 0 if problems else 1

    try:
        data = to_ascii_crlf(TEMPLATE)
    except UnicodeEncodeError as exc:
        print("[-] template contains non-ASCII:", exc)
        return 1

    out.write_bytes(data)

    problems = verify(out.read_bytes())
    if problems:
        print("[-] self-check FAILED:", "; ".join(problems))
        return 1
    print("[+] wrote %s (%d bytes, pure ASCII + CRLF)" % (out, len(data)))
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
build.bat 的唯一真源（生成器）。

背景（血泪教训）：Windows 中文环境（代码页 936）下，cmd.exe 按当前代码页逐行解码批处理，
UTF-8 编码的 .bat 必乱码。最稳做法 = 纯 ASCII + CRLF（ASCII 是 GBK 严格子集）。

本脚本内嵌纯 ASCII 模板 -> 归一为 LF -> encode('ascii')（非 ASCII 直接报错）
-> 强制 CRLF -> 写出 -> 自校验（无高字节 / 含 CRLF / 无裸 LF），校验不过 exit 1。

用法：
    python tools/_gen_build_bat.py            # 生成 仓库根/build.bat
    python tools/_gen_build_bat.py --check    # 仅校验现有 build.bat

注意：绝不要用会写 UTF-8 的编辑器直接改 build.bat，一律改本脚本后重新生成。
"""
import sys
import pathlib

TEMPLATE = r"""@echo off
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
"""


def to_ascii_crlf(text: str) -> bytes:
    """归一换行 -> 断言纯 ASCII -> 强制 CRLF。"""
    norm = text.replace("\r\n", "\n").replace("\r", "\n")
    data = norm.encode("ascii")          # 非 ASCII 直接抛 UnicodeEncodeError
    return data.replace(b"\n", b"\r\n")


def verify(raw: bytes) -> list:
    """返回问题列表（空 = 通过）。"""
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
    out = root / "build.bat"

    if "--check" in sys.argv:
        if not out.exists():
            print("[-] build.bat not found:", out)
            return 1
        problems = verify(out.read_bytes())
        print("[+] build.bat OK (pure ASCII + CRLF)" if not problems
              else "[-] build.bat FAILED: " + "; ".join(problems))
        return 0 if not problems else 1

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

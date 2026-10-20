# -*- coding: utf-8 -*-
"""
WinOCR v1.1.0 发布打包：组装 staging 目录供 Inno Setup 打安装器。

用法：
    python tools/pack_release.py            # 组装 staging（构建需先完成）
    python tools/pack_release.py --check    # 仅校验各产物是否存在

staging 布局（target/installer/staging/）：
    winocr.exe                  主程序（app crate, release）
    winocr-host.exe             浏览器扩展 Native Messaging Host
    winocr-argos.exe            Argos 离线翻译子进程（argos-bridge crate）
    assets/dict/                词典数据（open-dictionary/ipa/etymology/stage, ~94MB）
    assets/models/ocr/          PaddleOCR 蒸馏模型（~6MB）
    assets/argos_packages/      Argos en<->zh 翻译包（~164MB）
    assets/onnxruntime.dll      OCR 推理运行时（~17MB）
    assets/tray.ico             托盘图标
    browser-extension/          MV3 划词采集扩展（含 vendor/models, ~35MB）

Inno 脚本：installer/winocr.iss → WinOCR-Setup-v1.1.0.exe
"""
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STAGING = ROOT / "target" / "installer" / "staging"

PLAN = [
    # (源, staging 相对路径, 是否目录)
    (ROOT / "target" / "release" / "winocr.exe", "winocr.exe", False),
    (ROOT / "target" / "release" / "winocr-host.exe", "winocr-host.exe", False),  # 浏览器扩展 Native Messaging Host
    (ROOT / "argos-bridge" / "target" / "release" / "winocr-argos.exe", "winocr-argos.exe", False),
    (ROOT / "app" / "assets" / "dict", "assets/dict", True),
    (ROOT / "assets" / "models" / "ocr", "assets/models/ocr", True),
    (ROOT / "assets" / "argos_packages", "assets/argos_packages", True),
    (ROOT / "assets" / "onnxruntime.dll", "assets/onnxruntime.dll", False),
    (ROOT / "assets" / "tray.ico", "assets/tray.ico", False),
    (ROOT / "browser-extension", "browser-extension", True),
]


def check() -> int:
    ok = True
    for src, rel, is_dir in PLAN:
        tag = "[+] " if src.exists() else "[-] "
        if not src.exists():
            ok = False
        kind = "dir " if is_dir else "file"
        print(f"{tag}{kind} {src}  ->  {rel}")
    return 0 if ok else 1


def main() -> int:
    if "--check" in sys.argv:
        return check()

    if check() != 0:
        print("\n[-] 存在缺失产物：请先 cargo build --release（app 与 argos-bridge 两个 crate）")
        return 1

    if STAGING.exists():
        print("[*] 清理旧 staging ...")
        shutil.rmtree(STAGING)
    STAGING.mkdir(parents=True)
    print(f"[+] staging: {STAGING}")

    for src, rel, is_dir in PLAN:
        dst = STAGING / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        if is_dir:
            shutil.copytree(src, dst)
        else:
            shutil.copy2(src, dst)
        print(f"[+] {rel}")

    total = sum(f.stat().st_size for f in STAGING.rglob("*") if f.is_file())
    print(f"\n[+] 组装完成，共 {total / 1048576:.0f} MB")
    print("[*] 下一步：iscc installer/winocr.iss")
    return 0


if __name__ == "__main__":
    sys.exit(main())

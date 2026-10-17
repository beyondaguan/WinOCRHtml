# -*- coding: utf-8 -*-
"""WinOCR Argos 离线翻译包下载器（官方源 argos-net.com，纯 ASCII 输出）。

用法：
    python tools/download_argos.py              # 下载 en<->zh 两个语言包

说明：
  - 两个语言包（英→中、中→英）均下载自 Argos 官方源（argos-net.com，版本 1.9）；
  - 下载后自动解压到 vendor/argos_packages/（本地分发版已内置则跳过）；
  - 任一步失败都不中断安装流程（缺包时离线翻译自动降级回退链）。
  - 本脚本纯 ASCII 输出，任意控制台编码下都不会因打印报错。
"""
import os
import shutil
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from winocr.core.paths import argos_install_dir  # noqa: E402

# (code, from_code, to_code, url, 期望解压目录名)
# 版本 1.9，来自官方索引 argospm-index（argos-net.com/v1/*.argosmodel）
_PKGS = [
    ("translate-en_zh", "en", "zh",
     "https://argos-net.com/v1/translate-en_zh-1_9.argosmodel",
     "translate-en_zh-1_9"),
    ("translate-zh_en", "zh", "en",
     "https://argos-net.com/v1/translate-zh_en-1_9.argosmodel",
     "translate-zh_en-1_9"),
]

_MIN_VALID_SIZE = 10 * 1024 * 1024   # 翻译包小于 10MB 视为残缺，拒收


def _download(url: str, dest: Path) -> bool:
    try:
        print(f"  <- {url.split('/')[-1]}")
        req = urllib.request.Request(url, headers={"User-Agent": "winocr"})
        with urllib.request.urlopen(req, timeout=600) as r:
            with open(dest, "wb") as f:
                shutil.copyfileobj(r, f, 1 << 20)
        if dest.stat().st_size < _MIN_VALID_SIZE:
            print(f"     [拒绝] 文件过小 {dest.stat().st_size}B，疑似占位")
            dest.unlink(missing_ok=True)
            return False
        print(f"     OK {dest.stat().st_size / (1024 * 1024):.1f} MiB 下载完成")
        return True
    except Exception as e:
        print(f"     FAIL: {type(e).__name__}: {e}")
        dest.unlink(missing_ok=True)
        return False


def _is_zip_ok(path: Path) -> bool:
    try:
        with zipfile.ZipFile(path) as z:
            bad = z.testzip()
        return bad is None
    except Exception as e:
        print(f"     [拒绝] 压缩包损坏: {type(e).__name__}: {e}")
        return False


def _valid_install_dir(dest: Path) -> bool:
    return (dest / "metadata.json").is_file() and (dest / "model" / "model.bin").is_file()


def _safe_extract(zf: zipfile.ZipFile, target: Path) -> None:
    """安全解压（防 zip-slip）；归一化嵌套：保证 target 顶层含 metadata.json。"""
    members = zf.infolist()
    for m in members:
        name = m.filename.replace("\\", "/")
        if name.startswith("/") or ".." in name.split("/"):
            raise ValueError(f"非安全条目: {name}")
    root = None                      # 含 metadata.json 的最短顶层前缀
    for m in members:
        if not m.is_dir() and m.filename.endswith("metadata.json"):
            prefix = m.filename.replace("\\", "/").rsplit("/", 1)[0]
            if root is None or prefix.count("/") < root.count("/"):
                root = prefix
    if root is None:
        raise ValueError("压缩包中未找到 metadata.json")

    for m in members:
        name = m.filename.replace("\\", "/")
        if name == root:
            continue
        rel = name[len(root):].lstrip("/") if name.startswith(root + "/") else name
        if not rel:
            continue
        out = target / rel
        if m.is_dir() or name.endswith("/"):
            out.mkdir(parents=True, exist_ok=True)
            continue
        out.parent.mkdir(parents=True, exist_ok=True)
        with zf.open(m) as src, open(out, "wb") as dst:
            shutil.copyfileobj(src, dst)


def _install_one(spec, install_dir: Path) -> bool:
    code, frm, to, url, dirname = spec
    dest = install_dir / dirname
    print(f"[{code}] ({frm} -> {to})")
    if _valid_install_dir(dest):
        print("  已存在且结构完整，跳过。")
        return True

    fd, tmpzip = tempfile.mkstemp(suffix=".zip")
    os.close(fd)
    tmpdir = tempfile.mkdtemp(prefix="winocr_argos_")
    try:
        if not _download(url, Path(tmpzip)):
            return False
        if not _is_zip_ok(Path(tmpzip)):
            return False
        with zipfile.ZipFile(tmpzip) as zf:
            _safe_extract(zf, Path(tmpdir))

        # tmpdir 里应有顶层目录（含 metadata.json）; 归一化到目标名
        contents = [p for p in Path(tmpdir).iterdir()]
        if len(contents) == 1 and contents[0].is_dir():
            src_dir = contents[0]
        else:
            src_dir = Path(tmpdir)
        if not _valid_install_dir(src_dir):
            print(f"  解压后结构不完整，跳过 {dirname}")
            return False
        install_dir.mkdir(parents=True, exist_ok=True)
        shutil.move(str(src_dir), str(dest))
        print(f"  OK 已安装到 vendor/argos_packages/{dirname}")
        return True
    except Exception as e:
        print(f"  FAIL: {type(e).__name__}: {e}")
        return False
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)
        try:
            Path(tmpzip).unlink(missing_ok=True)
        except OSError:
            pass


def main() -> int:
    install_dir = argos_install_dir()
    print(f"下载 Argos 中文翻译包 -> {install_dir}")
    ok = 0
    for spec in _PKGS:
        if _install_one(spec, install_dir):
            ok += 1
    print(f"\n结果: {ok}/{len(_PKGS)} 个语言包就绪")
    if ok == len(_PKGS):
        print("中英互译离线可用；可运行 main.py doctor 确认。")
        return 0
    print("部分下载失败。离线翻译将自动降级；重跑本脚本会跳过已成功的包。")
    return 1


if __name__ == "__main__":
    sys.exit(main())
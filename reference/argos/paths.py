# -*- coding: utf-8 -*-
"""路径解析 — 全应用唯一的「文件在哪」真相来源。

WinOCR2.0 的问题：每个模块各自用 os.path.dirname(os.path.abspath(__file__)) 往上拼，
换个目录结构就全线崩。3.0 把所有路径集中在这里，其余模块只调用函数。

支持两种部署形态：
  - 便携模式：项目根存在 config.toml → 配置与数据都放项目内（U 盘可带走）
  - 常规模式：配置放 ~/.winocr/（跨版本升级不丢设置）
环境变量 WINOCR_HOME 可强制指定数据目录。
"""

from __future__ import annotations

import logging

logger = logging.getLogger(__name__)

import os
import sys
from pathlib import Path
from typing import List

if getattr(sys, "frozen", False):
    # 打包形态（PyInstaller onedir）：项目根 = 可执行文件所在目录。
    # models/、vendor/、plugins/、config.toml 全部与 exe 同级放置，
    # 便携模式（根目录放 config.toml）在打包后天然成立，路径逻辑与源码一致。
    _PROJECT_ROOT = Path(sys.executable).resolve().parent  # <app>（exe 所在目录）
    _PKG_DIR = _PROJECT_ROOT / "winocr"  # 仅兼容调用，打包后极少使用
else:
    _PKG_DIR = Path(__file__).resolve().parent.parent  # <root>/winocr
    _PROJECT_ROOT = _PKG_DIR.parent  # <root>


def package_dir() -> Path:
    """winocr 包所在目录。"""
    return _PKG_DIR


def project_root() -> Path:
    """项目根目录（含 main.py / models / vendor）。"""
    return _PROJECT_ROOT


def is_portable() -> bool:
    """项目根存在 config.toml 即视为便携模式。"""
    return (_PROJECT_ROOT / "config.toml").is_file()


def user_dir() -> Path:
    """用户数据目录（配置、历史、下载的模型）。"""
    env = os.environ.get("WINOCR_HOME")
    p = (
        Path(env)
        if env
        else (_PROJECT_ROOT if is_portable() else Path.home() / ".winocr")
    )
    try:
        p.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        logger.debug("无法创建数据目录 %s：%s", p, e)
    return p


def config_path() -> Path:
    """配置文件路径（便携优先）。"""
    portable = _PROJECT_ROOT / "config.toml"
    if portable.is_file():
        return portable
    return user_dir() / "config.toml"


def history_path() -> Path:
    return user_dir() / "history.json"


def chat_history_path() -> Path:
    """AI 对话历史（重开程序后保留上下文）。"""
    return user_dir() / "chat_history.json"


def project_history_path(project_id: str) -> Path:
    """按项目隔离的翻译/划词历史（真·项目栏 MVP）。

    放在 history/ 子目录，文件名即项目 id；与全局 history.json 互不干扰。
    """
    safe = _safe_id(project_id)
    return user_dir() / "history" / f"{safe}.json"


def project_chat_history_path(project_id: str) -> Path:
    """按项目隔离的 AI 对话历史。"""
    safe = _safe_id(project_id)
    return user_dir() / "chat_history" / f"{safe}.json"


def _safe_id(project_id: str) -> str:
    """把项目 id 规整成文件名安全的短串（防目录穿越/特殊字符）。"""
    s = (project_id or "default").strip().replace("\\", "_").replace("/", "_")
    s = "".join(ch for ch in s if ch.isalnum() or ch in "-_")
    return s or "default"


def knowledge_path() -> Path:
    """知识库数据库（sqlite3，纯本地、离线）。"""
    return user_dir() / "knowledge.db"


def import_templates_dir() -> Path:
    """导入模板目录（系统自动生成「知识库导入模板.csv / .json + 说明」）。

    固定路径，首次导入时自动生成；幂等：文件已存在则不覆盖。
    """
    d = user_dir() / "templates"
    try:
        d.mkdir(parents=True, exist_ok=True)
    except OSError:
        pass
    return d


_OCR_TIERS = ("tiny", "small", "medium")


def ocr_model_dir(tier: str = "tiny") -> Path:
    """内置 PP-OCRv6 模型目录（按档位分目录，档位名即目录名）。

    tier: tiny / small / medium；非法值回落 tiny。
    目录结构：models/v6_tiny、models/v6_small、models/v6_medium。
    """
    if tier not in _OCR_TIERS:
        tier = "tiny"
    return _PROJECT_ROOT / "models" / f"v6_{tier}"


def argos_search_dirs() -> List[Path]:
    """Argos 离线翻译模型包的搜索顺序（先找到先用）。

    顺序设计：环境变量 > 用户目录 > 项目内 vendor > argos-translate 官方默认位置。
    这样「下载一次，多版本共用」与「项目自带、拷走即用」两种诉求都能满足。
    """
    dirs: List[Path] = []
    env = os.environ.get("ARGOS_PACKAGES_DIR")
    if env:
        dirs.append(Path(env))
    dirs.append(user_dir() / "models" / "argos")
    dirs.append(_PROJECT_ROOT / "vendor" / "argos_packages")
    dirs.append(Path.home() / ".local" / "share" / "argos-translate" / "packages")
    return dirs


def argos_install_dir() -> Path:
    """下载器写入位置（项目内 vendor 优先，保证「拷走即用」）。"""
    d = _PROJECT_ROOT / "vendor" / "argos_packages"
    try:
        d.mkdir(parents=True, exist_ok=True)
    except OSError:
        d = user_dir() / "models" / "argos"
        d.mkdir(parents=True, exist_ok=True)
    return d


def llama_model_search_dirs() -> List[Path]:
    """llama.cpp GGUF 模型搜索目录（先找到先用）。

    顺序：环境变量 > 用户目录 > 项目内 models。
    这样「下载一次，多版本共用」与「项目自带、拷走即用」两种诉求都能满足。
    """
    dirs: List[Path] = []
    env = os.environ.get("WINOCR_LLAMA_MODEL_DIR")
    if env:
        dirs.append(Path(env))
    dirs.append(user_dir() / "models" / "llama")
    dirs.append(_PROJECT_ROOT / "models" / "llama")
    return dirs


def llama_model_dir() -> Path:
    """GGUF 模型默认存放目录（下载器写入位置）。"""
    d = _PROJECT_ROOT / "models" / "llama"
    try:
        d.mkdir(parents=True, exist_ok=True)
    except OSError:
        d = user_dir() / "models" / "llama"
        d.mkdir(parents=True, exist_ok=True)
    return d


def find_llama_gguf(model_name: str = "") -> str:
    """在搜索目录中查找 GGUF 模型文件，返回绝对路径（找不到返回空串）。

    model_name 非空时优先精确匹配文件名（含/不含 .gguf 后缀均可）；
    否则返回找到的第一个 .gguf 文件。
    """
    for d in llama_model_search_dirs():
        if not d.is_dir():
            continue
        try:
            entries = sorted(os.listdir(d))
        except OSError:
            continue
        if model_name:
            for f in entries:
                full = os.path.join(str(d), f)
                if f == model_name and os.path.isfile(full):
                    return full
                if f == model_name + ".gguf" and os.path.isfile(full):
                    return full
        for f in entries:
            if f.lower().endswith(".gguf"):
                full = os.path.join(str(d), f)
                if os.path.isfile(full):
                    return full
    return ""


def plugins_dir() -> Path:
    """第三方插件目录（用户可丢 .py 进去扩展任意一轴）。"""
    d = _PROJECT_ROOT / "plugins"
    return d

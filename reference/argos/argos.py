# -*- coding: utf-8 -*-
"""
Argos 离线翻译引擎 — 无 torch / 无 stanza 独立实现（移植自 WinOCR2.0/engines/argos.py）

关键改进（相对旧版）：
  - ctranslate2 / sentencepiece 改为方法内惰性导入，
    因此「发现插件」这一步不需要安装这两个重型库，注册表扫描更轻。
  - 其余推理逻辑保持与 argostranslate 1.11 一致，翻译质量零损失。

模型包结构兼容 argos：<pkg>/model/model.bin + <pkg>/sentencepiece.model + metadata.json
搜索目录： $ARGOS_PACKAGES_DIR → ~/.local/share/argos-translate/packages
        → <项目>/vendor/argos_packages → <项目>/models/argos
"""
from __future__ import annotations

import json
import logging
import os
import re
import threading

from .base import TranslateEngine

logger = logging.getLogger(__name__)


# ----------------------------------------------------------------------------
# 推理超参（与 argostranslate 1.11 默认一致，确保输出逐字一致）
# ----------------------------------------------------------------------------
from ...core.hardware import suggest_cpu_threads

_BEAM_SIZE = 4
_LENGTH_PENALTY = 0.2
_BATCH_SIZE = 32
_COMPUTE_TYPE = "auto"
_INTER_THREADS = 1
# intra_threads 负责算子内并行（矩阵乘），封顶 8 是 ctranslate2 的收益上限
_INTRA_THREADS = suggest_cpu_threads(max_threads=8)

_SENT_CJK = re.compile(r"(?<=[。！？；])")
_SENT_LATIN = re.compile(r"(?<=[.!?;])\s+")


def _split_sentences(text: str) -> list:
    text = text.strip()
    if not text:
        return []
    sentences = []
    for piece in _SENT_CJK.split(text):
        piece = piece.strip()
        if not piece:
            continue
        for seg in _SENT_LATIN.split(piece):
            seg = seg.strip()
            if seg:
                sentences.append(seg)
    return sentences


class _ArgosPackage:
    def __init__(self, path: str):
        self.path = path
        with open(os.path.join(path, "metadata.json"), "r", encoding="utf-8") as f:
            meta = json.load(f)
        self.from_code = meta["from_code"]
        self.to_code = meta["to_code"]
        self.target_prefix = meta.get("target_prefix", "") or ""
        # 惰性：sentencepiece 在首次使用时导入
        self._sp = None
        self._translator = None
        self._lock = threading.Lock()

    # 惰性导入重型依赖
    def _get_sp(self):
        if self._sp is None:
            import sentencepiece as spm
            self._sp = spm.SentencePieceProcessor(
                model_file=os.path.join(self.path, "sentencepiece.model"))
        return self._sp

    def _get_translator(self):
        if self._translator is None:
            with self._lock:
                if self._translator is None:
                    import ctranslate2
                    self._translator = ctranslate2.Translator(
                        os.path.join(self.path, "model"),
                        device="cpu",
                        inter_threads=_INTER_THREADS,
                        intra_threads=_INTRA_THREADS,
                        compute_type=_COMPUTE_TYPE,
                    )
        return self._translator

    def encode(self, sentence: str) -> list:
        return self._get_sp().encode(sentence, out_type=str)

    def decode(self, tokens: list) -> str:
        return (self._get_sp().decode_pieces(tokens)
                .replace("▁", " ").replace("_", " "))

    def translate(self, text: str) -> str:
        translator = self._get_translator()
        sentences = _split_sentences(text)
        if not sentences:
            return text
        tokenized = [self.encode(s) for s in sentences]
        target_prefix = ([[self.target_prefix]] * len(tokenized)
                         if self.target_prefix else None)
        results = translator.translate_batch(
            tokenized, target_prefix=target_prefix, replace_unknowns=True,
            max_batch_size=_BATCH_SIZE, batch_type="tokens",
            beam_size=_BEAM_SIZE, num_hypotheses=1, length_penalty=_LENGTH_PENALTY,
            return_scores=True,
        )
        translated_tokens = []
        for r in results:
            translated_tokens.extend(r.hypotheses[0])
        value = self.decode(translated_tokens)
        if self.target_prefix and value.startswith(self.target_prefix):
            value = value[len(self.target_prefix):]
        if value and value[0] == " ":
            value = value[1:]
        return value


def _package_search_dirs() -> list:
    """模型包搜索目录，顺序与去重由 core.paths 统一定义（不再各处硬拼路径）。"""
    from ...core.paths import argos_search_dirs

    seen, out = set(), []
    for p in argos_search_dirs():
        d = str(p)
        if d and d not in seen and os.path.isdir(d):
            seen.add(d)
            out.append(d)
    return out


class ArgosEngine(TranslateEngine):
    name = "argos"
    display_name = "Argos 离线"
    online = False

    def __init__(self):
        self._lang_map = {"zh-CN": "zh", "zh": "zh", "en": "en",
                          "ja": "ja", "ko": "ko", "fr": "fr", "de": "de",
                          "es": "es", "ru": "ru"}
        self._packages = {}
        self._loaded = False
        self._lock = threading.Lock()

    def _load_packages(self):
        if self._loaded:
            return
        with self._lock:
            if self._loaded:
                return
            for d in _package_search_dirs():
                try:
                    names = os.listdir(d)
                except OSError:
                    continue
                for name in names:
                    pdir = os.path.join(d, name)
                    if not os.path.isdir(pdir) or not os.path.isfile(
                            os.path.join(pdir, "metadata.json")):
                        continue
                    try:
                        pkg = _ArgosPackage(pdir)
                        self._packages[(pkg.from_code, pkg.to_code)] = pkg
                    except Exception as e:
                        logger.warning("[Argos] 跳过无效模型包 %s: %s", name, e)
            self._loaded = True

    def available(self) -> bool:
        self._load_packages()
        return len(self._packages) > 0

    def warmup(self):
        try:
            self._load_packages()
            for pkg in self._packages.values():
                try:
                    pkg.translate(" ")
                except Exception:
                    pass
        except Exception as e:
            logger.warning("[预热] Argos 预热失败: %s", e)

    def translate(self, text: str, source: str, target: str) -> str:
        self._load_packages()
        if not self._packages:
            raise RuntimeError("Argos 离线翻译未安装语言包")
        src = self._lang_map.get(source, source)
        tgt = self._lang_map.get(target, target)
        pkg = self._packages.get((src, tgt))
        if pkg is None:
            raise RuntimeError(f"Argos 未安装 {src}->{tgt} 语言包")
        out = []
        for line in text.split("\n"):
            if not line.strip():
                out.append(line)
                continue
            try:
                out.append(pkg.translate(line) or line)
            except Exception as e:
                logger.warning("[Argos 单行翻译失败] %s", e)
                out.append(line)
        return "\n".join(out)

# -*- coding: utf-8 -*-
"""翻译调度器 — 插件化的自动回退链（移植并泛化自 WinOCR2.0/translator.py）。

旧版把回退顺序硬编码在 _do_translate 里，加引擎要改调度代码。
3.0 把「引擎实例表」和「回退顺序」都注入进来：调度器不认识任何具体引擎，
新增引擎 = 往 services/translate/ 丢一个文件 + 在配置的 fallback_order 里写个名字。
"""
from __future__ import annotations

import logging
import time
from typing import Dict, List, Optional

from .base import TranslateEngine
from ...core.types import Lang, TranslateResult

logger = logging.getLogger(__name__)


class TranslateDispatcher:
    def __init__(self, engines: Dict[str, TranslateEngine], config) -> None:
        self.engines = engines
        self.config = config              # TranslateConfig
        self.last_engine = ""

    # ---- 语言方向自动检测（移植自 translator._detect_direction） ----
    @staticmethod
    def _has_chinese(text: str) -> bool:
        return any("\u4e00" <= ch <= "\u9fff" for ch in text)

    @staticmethod
    def _lang_ratio(text: str) -> float:
        """中文占比 = 中文字数 / (中文字数 + 拉丁字母数)。

        空白、数字、标点一律忽略，专用于「混杂中英」时按*主方向*判定源语种：
        中文占比过半才当作中文源（译外），否则当作英文源（译中）。
        这样划到「少量中文 + 大段英文」时不会再被误判成中文而翻译成英文。
        """
        zh = 0
        latin = 0
        for ch in text:
            if "\u4e00" <= ch <= "\u9fff":
                zh += 1
            elif ("a" <= ch <= "z") or ("A" <= ch <= "Z"):
                latin += 1
        total = zh + latin
        return (zh / total) if total else 0.0

    # ---- langid 轻量语言识别（懒加载；失败自动回落占比法）----
    _langid = None
    _langid_tried = False

    @classmethod
    def _langid_engine(cls):
        """懒加载 langid 并限定只分中英；导入失败返回 None（绝不抛给调用方）。

        set_languages(["zh","en"]) 让 langid 只在中英之间做选择——
        本项目只做中英互译，其余语言一律按「非中文」归入英文源（外译中）。
        """
        if cls._langid_tried:
            return cls._langid
        cls._langid_tried = True
        try:
            import langid
            langid.set_languages(["zh", "en"])
            cls._langid = langid
        except Exception:
            cls._langid = None
        return cls._langid

    @classmethod
    def _langid_detect(cls, text: str) -> Optional[str]:
        """langid 判源，只分中英：中文 → zh-CN，其余一律 en（外译中）。

        不可用（未安装 / 异常）返回 None，由调用方回落字符占比法。
        含日文假名（平/片假名）时直接按非中文处理——langid 只分中英时
        会把假名误并进中文，这里提前拦住。
        """
        if not text:
            return None
        # 日文假名预检：出现任一平/片假名即视为非中文（外译中）
        if any("\u3040" <= ch <= "\u30ff" for ch in text):
            return "en"
        lid = cls._langid_engine()
        if lid is None:
            return None
        try:
            lang, _conf = lid.classify(text[:500])
        except Exception:
            return None
        return "zh-CN" if lang == "zh" else "en"

    @staticmethod
    def _has_letter(text: str) -> bool:
        """是否含有实质文字（中文字符或拉丁字母）。纯数字/标点/空白返回 False。"""
        return any("\u4e00" <= ch <= "\u9fff" or "a" <= ch <= "z"
                   or "A" <= ch <= "Z" for ch in text)

    def detect(self, text: str, target: str, explicit: bool = False):
        """推断源语言；仅在「未显式指定目标」时才纠正 target。

        explicit=False（一键翻译 / 热键）：
        - 空文本 / 纯符号数字（无翻译意义）→ 源语言「unknown」，跳过翻译
        - langid 可用：中文 → 中译外；其余语言一律按英文源（外译中，只做中英互译）
        - langid 不可用：回落「中文占比 > 0.5」判定（原逻辑）
        自动躲开「中文翻中文」「大段英文里夹几个汉字却被误译英」的空转。

        explicit=True（用户点了「译中」「译英」）：**只推断 source，绝不动 target**。
        这里曾是个真 bug —— 早期无条件纠正，导致中文原文点「译中」被偷偷改成
        译英，译文区内容跟上一次一模一样，看起来像「按钮没反应」。
        用户明确表达的意图，任何时候都不该被程序悄悄改写。
        """
        target = Lang.normalize(target)
        text = (text or "").strip()
        # 空文本 / 纯符号：无翻译意义 → 未知源语言（上层直接返回原文不翻译）
        if not text or not self._has_letter(text):
            source = "unknown"
        else:
            src = self._langid_detect(text)
            source = src if src is not None else (
                "zh-CN" if self._lang_ratio(text) > 0.5 else "en")
        if explicit:
            return source, target
        if source == "zh-CN" and target == "zh-CN":
            target = "en"
        elif source == "en" and target == "en":
            target = "zh-CN"
        return source, target

    @classmethod
    def is_same_language(cls, text: str, lang: str) -> bool:
        """文本是否已经就是 lang 语种（只对中/英给准话，其余一律返回 False）。

        UI 用它决定「该翻原文区还是译文区」。语言知识留在翻译轴里，
        不外泄到界面层 —— 否则将来加语种就得两处改。
        """
        if not text or not text.strip():
            return False
        lang = Lang.normalize(lang)
        zh = cls._has_chinese(text)
        if lang == "zh-CN":
            return zh
        if lang == "en":
            return not zh
        return False

    # ---- 引擎清单 ----
    def available_engines(self) -> List[str]:
        offline = getattr(self.config, "offline_mode", False)
        out = []
        for name, eng in self.engines.items():
            if offline and getattr(eng, "online", False):
                continue
            try:
                if eng.available():
                    out.append(name)
            except Exception:
                pass
        return out

    def engine_display(self, name: str) -> str:
        eng = self.engines.get(name)
        return getattr(eng, "display_name", name) if eng else name

    def cycle_engine(self) -> str:
        """在「auto + 各可用引擎」间轮转（对应 2.0 的 Ctrl+Shift+E）。"""
        options = ["auto"] + self.available_engines()
        try:
            idx = options.index(self.config.engine)
        except ValueError:
            idx = -1
        self.config.engine = options[(idx + 1) % len(options)]
        return self.config.engine

    # ---- 翻译 ----
    def translate(self, text: str, target: Optional[str] = None,
                  explicit: bool = False) -> str:
        return self.translate_detailed(text, target, explicit).text

    def translate_detailed(self, text: str, target: Optional[str] = None,
                           explicit: bool = False) -> TranslateResult:
        if not text or not text.strip():
            return TranslateResult(text=text or "")
        source, tgt = self.detect(text, target or self.config.target, explicit)
        t0 = time.time()

        # 纯符号 / 数字文本（无可翻译内容）：直接返回原文，标注无需翻译
        if source == "unknown":
            return TranslateResult(text=text, source_lang="unknown", target_lang=tgt,
                                   engine="(无需翻译)", elapsed=0.0)

        # 指定单引擎：失败就报错，不静默回退（用户明确选了它，掩盖失败反而更糟）
        if self.config.engine and self.config.engine != "auto":
            eng = self.engines.get(self.config.engine)
            if eng is None:
                raise RuntimeError(f"翻译引擎 '{self.config.engine}' 未找到")
            if not eng.available():
                raise RuntimeError(f"翻译引擎 '{eng.display_name}' 不可用（缺模型或未配置 Key）")
            out = eng.translate(text, source, tgt)
            # 空结果与 auto 模式同判为失败：静默返回空译文会让用户以为引擎坏了
            if not out or not out.strip():
                raise RuntimeError(f"引擎 '{eng.display_name}' 返回空结果")
            self.last_engine = eng.name
            return TranslateResult(text=out, source_lang=source, target_lang=tgt,
                                   engine=eng.name, elapsed=time.time() - t0)

        # auto：按配置顺序逐个尝试，首个成功即返回
        errors = []
        offline = getattr(self.config, "offline_mode", False)
        for name in self.config.fallback_order:
            eng = self.engines.get(name)
            if eng is None:
                continue
            # 离线模式：跳过需联网的引擎（避免偷偷回退到在线引擎击穿离线承诺）
            if offline and getattr(eng, "online", False):
                continue
            try:
                if not eng.available():
                    continue
                out = eng.translate(text, source, tgt)
                if out and out.strip():
                    self.last_engine = eng.name
                    return TranslateResult(text=out, source_lang=source, target_lang=tgt,
                                           engine=eng.name, elapsed=time.time() - t0)
            except Exception as e:
                errors.append(f"{eng.display_name}: {e}")
                logger.warning("[翻译回退] %s 失败：%s", eng.display_name, e)

        # 全线失败：返回原文并把原因带出去，UI 可以告诉用户到底怎么了
        return TranslateResult(text=text, source_lang=source, target_lang=tgt,
                               engine="(全部失败) " + "; ".join(errors[:2]),
                               elapsed=time.time() - t0)

    def warmup(self) -> None:
        for eng in self.engines.values():
            try:
                if eng.available():
                    eng.warmup()
            except Exception:
                pass

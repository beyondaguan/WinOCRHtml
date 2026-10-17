# -*- coding: utf-8 -*-
"""翻译引擎基类 — 统一契约（插件契约）。

任何翻译引擎只需继承并实现 translate() / available()，放在
winocr/services/translate/ 下即被注册表自动发现，无需改动核心代码。
相比 WinOCR2.0 的 EngineBase，这里把「在线/离线」「优先级」等元数据上提为类属性，
便于调度器排序与回退。
"""
from __future__ import annotations

from abc import ABC, abstractmethod


class TranslateEngine(ABC):
    # 子类必须设置（也作为插件注册键）
    name: str = ""
    display_name: str = ""
    online: bool = False          # 是否需联网（离线优先排序用）

    @abstractmethod
    def translate(self, text: str, source: str, target: str) -> str:
        """翻译文本；失败抛异常（由调度器捕获并回退）。"""
        ...

    @abstractmethod
    def available(self) -> bool:
        """引擎是否可用（依赖/配置/网络就绪）。"""
        ...

    def set_config(self, **kwargs) -> None:
        """接收注入的配置（API Key 等）。默认忽略。"""

    def warmup(self) -> None:
        """可选预热（如加载模型）。默认无操作。"""

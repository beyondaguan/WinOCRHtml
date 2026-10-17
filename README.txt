WinOCR v1.0.0 - 便携版
==============================================

【安装】
解压即用，无需安装。将 zip 内容解压到任意目录。

【运行】
双击 winocr.exe 启动。首次启动需 1-2 秒预热 OCR 模型。

【依赖】
无需额外依赖。所有 OCR 模型、翻译引擎资源已内置。
- OCR: PP-OCRv6 (ONNX, 内置 det_tiny/rec_tiny 模型)
- 翻译: Argos 离线 (CTranslate2 + SentencePiece, en≈zh 164MB 内置)
- TTS: edge-tts 在线 / OneCore 离线回退

【主功能】
- Alt+Q: 截图翻译（全局热键）
- Alt+W: 唤起主窗口
- Alt+C: 复制译文
- Alt+E: 重译
- Alt+R: 朗读
- Alt+F: 收藏生词
- Ctrl+H: 历史记录
- Ctrl+K: 生词本

【数据目录】
%APPDATA%/WinOCR/config.json - 配置（密钥已 DPAPI 加密）
%APPDATA%/WinOCR/history.jsonl - 历史记录
%APPDATA%/WinOCR/vocab.db - 生词本（SQLite）

【扩展】
将 extension/ 目录加载到 Chrome/Edge 的扩展程序页面（开发者模式 → 加载已解压的扩展程序）

【版本】
v1.0.0 - 2026-09-30

【协议】
MIT License - 详见源代码

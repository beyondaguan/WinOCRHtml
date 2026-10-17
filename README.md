# WinOCR-Html 重构仓库（2026-09-27 起）

> 本目录为 **全新重构起点**：按 `docs/GUI设计文档.md` 的技术路线（**Rust + WebView2**）与 `docs/WinOCR3.4_Technical_Document.html` 的架构/样式蓝本，重写 WinOCR-Html。
> 由 `D:\AI\WinOCR-Html0.3fb`（0.3fb，旧版）与 `D:\AI\WinOCR-Html0.8`、`D:\Documents\getwinOCR\winOCR3.4-master` 汇集而来。

## 目录结构

```
D:\AI\WinOCRHtml20060927\
├─ docs\                     设计文档（单一事实来源）
│   ├─ GUI设计文档.md          ★ 界面/技术路线/样式/架构（本仓库纲领）
│   ├─ WinOCR界面地图.md       交互裁决（S1–S12 surface 分层）
│   ├─ WinOCR3.4_Technical_Document.html  ★ 架构与样式蓝本
│   ├─ Rust技术路线-Argos.md   ★ Argos 本地翻译的 Rust 实现路线（ct2rs / ort+ONNX）
│   ├─ Manggo参考分析.md       竞品功能对照（13 项）
│   ├─ 交付.md                 历史交付看板（旧版记录）
│   ├─ 全场景OCR翻译系统详细开发计划_Rust版_v1.4_含开发策略.md
│   ├─ main-window-design.html 主窗口设计稿
│   └─ README.md               旧版说明
├─ design\                   Hi-Fi 设计参考（winocr-desktop.html / hifi.css 等）
├─ extension-0.8\            0.8 版扩展（参考基线：划词气泡/替换/采集/生词/导出）
├─ extension-0.3fb\          0.3fb 版扩展（更完整：+悬停预览/SM-2复习/文档翻译/插件占位）
├─ rust-reference\           旧版 Rust 宿主源码（native_host_rust/src，排除 target/）
│   ├─ src\  Cargo.toml  Cargo.lock  build.rs
├─ reference\argos\          ★ Argos 离线翻译「参考实现」（从 winOCR3.4 摘取）
│   ├─ argos.py              CTranslate2+SentencePiece 引擎（无 torch）
│   ├─ download_argos.py     官方源下载器（argos-net.com v1.9）
│   ├─ dispatcher.py         回退链调度
│   ├─ translate_base.py     TranslateEngine 契约
│   └─ paths.py              argos 搜索目录解析
├─ assets\                   随包资源（开箱即用）
│   ├─ argos_packages\       ★ Argos 语言包 en↔zh（≈164MB）
│   │   ├─ translate-en_zh-1_9\   (metadata.json + model/model.bin + sentencepiece.model)
│   │   └─ translate-zh_en-1_9\
│   ├─ models\ocr\           本地 OCR 模型（det_tiny / rec_tiny / ppocr_dict.txt）
│   ├─ onnxruntime.dll
│   └─ tray.ico              彩色托盘图标
└─ tools\make_tray_icon.py   生成彩色托盘 ICO
```

## 关键决策（见 `docs/GUI设计文档.md`）

- **GUI 技术路线**：Rust + **WebView2**（代表 Tauri v2；轻量替代 wry）；界面归 Web（HTML/CSS/JS），能力归 Rust。
- **架构**：三层严格分层（UI → Services → Core，不反向依赖）+ **六轴插件**（Capture / Ocr / Translate / Ai / Attach / Persist + Hotkey/Tts/OpenAiCompat）；EventBus 数据流。
- **设计样式**：WinOCR 3.4 的 **Indigo/Slate 令牌**（`--brand:#6366f0` 等）。
- **主窗口定位**：**工作台（一等公民）**，非"按需展开"；布局 = 左导航轨 + 顶部工具栏 + 中央工作区（双栏 ⇄ AI 对话互斥）+ 单行状态条。
- **翻译引擎**：**Argos 本地离线优先**（默认），回退链 `argos → ollama → mymemory → sf`，含离线模式开关。
- **截图热键**：`Alt+Q`；快捷键体系按 6 类分组（见 GUI 文档 §S12）。

## 下一步（迁移计划草案）

1. **脚手架**：初始化 Rust + WebView2（Tauri v2 或 wry）工程；落地三层目录（`core/ services/ ui/`）。
2. **Core**：Config / EventBus / Pipeline / Registry / Types。
3. **Services（六轴）**：
   - Capture：DXGI/GDI 截图 + 剪贴板 + 文件 + 在线图片；
   - Ocr：本地 PP-OCRv6（复用 `assets/models/ocr` + `onnxruntime.dll`）；
   - **Translate：Argos 本地离线**（`reference/argos/argos.py` 的 Rust 移植，用 `ct2rs` + SentencePiece，复用 `assets/argos_packages`；随包分发 `ctranslate2.dll`）；
   - Ai / Attach / Persist（历史 JSONL、知识库 SQLite FTS5）。
4. **UI（WebView2 前端）**：主工作台 + 蒙版浮层 + 设置 + 历史/知识库；采用 `docs/GUI设计文档.md` 的 tokens 与组件。
5. **扩展**：以 `extension-0.3fb` 为基线（它已含 0.8 的全部能力 + 更多），按新设计对齐。
6. **打包**：`build.rs` 自动随包 `assets/`（模型 + Argos 包 + dll + ico）。

## 来源与备注

- `docs/`、`rust-reference/`、`design/`、`tools/`、`assets/models|dll|ico` 复制自 `D:\AI\WinOCR-Html0.3fb`。
- `extension-0.8/` 复制自 `D:\AI\WinOCR-Html0.8\extension`。
- `reference/argos/`、`assets/argos_packages/` 摘自 `D:\Documents\getwinOCR\winOCR3.4-master`（Argos 官方语言包 + 参考实现）。
- 本目录是**起点**，尚无可用构建；重构工作未开始。

## 数据与许可（2026-10-17 公开仓库声明）

仓库内随附的数据资产来源：

| 资产 | 来源 | 许可 |
|---|---|---|
| `app/assets/dict/open-dictionary/` | [mhollingshead/open-dictionary](https://github.com/mhollingshead/open-dictionary)（Wiktionary 英英释义解析） | 遵循上游许可（Wiktionary 内容 CC BY-SA） |
| `app/assets/dict/etymology_en_subset.json` | [droher/etymology-db](https://github.com/droher/etymology-db) 英语子集 | CC BY-SA 3.0 |
| `app/assets/dict/ipa_en.json` | [open-dict-data/ipa-dict](https://github.com/open-dict-data/ipa-dict)（word2ipa broad.tsv）+ CMUdict（ARPAbet→IPA 转换） | 上游分别为 MIT-ish / BSD 类许可 |
| `app/assets/terms/cefr_full.txt` | CEFR-J Wordlist v1.5（东京外国语大学）+ Octanove C1/C2 词表 | CEFR-J 商用免费需引用；Octanove CC BY-SA 4.0 |
| `assets/models/ocr/` | PaddleOCR det_tiny / rec_tiny 蒸馏模型 + 字典 | Apache-2.0（PaddleOCR） |

未进仓库的运行时大资产（首次运行需自行获取）：

- **Argos 翻译包**（en↔zh，约 164MB）：`python reference/argos/download_argos.py` 官方源下载，或 [Argos Translate](https://www.argos-translate.com/) releases
- **onnxruntime.dll**（17MB）：[Microsoft ONNX Runtime](https://onnxruntime.ai/) 官方 release
- **ECDICT 英汉词典**（stardict.db，812MB）：[skywind3000/ECDICT](https://github.com/skywind3000/ECDICT) release 下载后放 `%APPDATA%/WinOCR/dict/ecdict/`，或 `python tools/install_ecdict.py`

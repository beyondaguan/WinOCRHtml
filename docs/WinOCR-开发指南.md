# WinOCR v2.0 开发指南

> **唯一权威文档** — 编码前必读 · 所有其他文档已归档至 `docs-archive-2026-10-05.zip`
> **版本**：v2.0 · **日期**：2026-10-06 · **定位**：阅读 + 采集工具
> **GUI PRD**：`docs/GUI-PRD.md` (PM 产出，方案A 工作台为主+抽屉/模态)

---

## 一、项目快照

### 1.1 定位（一句话）

> **WinOCR 是专业人士的阅读采集管道：截图 → OCR → 翻译 → 采集 → 朗读 → 导出到外部学习工具。**

### 1.2 核心数据流

```
阅读 → Alt+Q截图 → OCR原文 → 翻译(Term保护→翻译→还原)
                            ↓
                    自动入历史流水(SQLite)
                            ↓
                    用户手动标记采集(collected=true)
                            ↓
                    导出(Anki/MD/欧路) + 朗读(TTS)
                            ↓
                    外部学习(Anki/欧路/Obsidian)
```

### 1.3 三层架构

```
┌─────────────────────────────────────────────┐
│ UI 层（Slint 1.18 软件渲染）                  │
│ MainWindow · History · Collect · Export · AI │
├─────────────────────────────────────────────┤
│ Services 层                                  │
│ Capture · Ocr · Translate · Ai · Tts ·      │
│ Persist · Collect · Export · Hotkey · Tray  │
├─────────────────────────────────────────────┤
│ Core 层                                      │
│ Config · Types · EventBus · Lang             │
└─────────────────────────────────────────────┘
```

### 1.4 线程模型

```
主线程(UI + Slint 事件循环)
热键线程(RegisterGetMessage → UI)
托盘线程(Shell_NotifyIconW)
截图工作线程(每次 capture 新建)
划词钩子线程(WH_MOUSE_LL)
AI 对话线程(每次 send 新建)
```

---

## 二、术语与命名统一

| 旧名(v1) | 新名(v2) | 说明 |
|---|---|---|
| `vocab.rs` | `collect.rs` | 采集服务(无 SM-2) |
| `VocabItem` | `CollectItem` | 采集条目 |
| `vocab.jsonl` | — | 废除,采集=历史 collected=true 子集 |
| `vocab-lines` | `collect-rows` | UI 表格行 |
| `vocab-ids` | `collect-ids` | UI 行 ID 列表 |
| `vocab-projects` | `collect-tags` | UI 项目标签列表 |
| `vocab-current-project` | `collect-filter-tag` | 当前筛选标签 |
| `vocab-add-word` | `collect-add-src` | 手动添加：原文输入框 |
| `vocab-add-translation` | `collect-add-tgt` | 手动添加：释义输入框 |
| `vocab-total` | `collect-total` | 采集总数 |
| `vocab-candidates` | `collect-candidates` | 候选采集词 |
| `load-vocab()` | `load-collect()` | 加载采集队列回调 |
| `add-vocab()` | `add-collect()` | 手动添加采集 |
| `remove-vocab()` | `remove-collect()` | 删除采集条目 |
| `grade-vocab()` | — | 移除（v2.0 无 SM-2） |
| `refresh-vocab-projects()` | `refresh-collect-tags()` | 刷新标签列表 |
| `生词本` | `采集队列` | 页面/视图名 |
| `加入生词` | `采集` | 按钮文案 |
| `收藏生词` | `采集` | 气泡按钮文案 |
| `favorited` | `collected` | 历史表字段 |
| `history-lines` | `history-rows` | UI 模型 |

### 导航轨

| 页键 | 中文名 | 图标 |
|---|---|---|
| `capture` | 工作台 | ▣ |
| `ai` | AI对话 | ✦ |
| `history` | 历史记录 | ↻ |
| `collect` | 采集队列 | 📌 |
| `export` | 导出 | 📤 |
| `settings` | 设置 | ⚙ |

---

## 三、引擎职责矩阵

| 引擎 | 文件 | 负责 | 不做 |
|---|---|---|---|
| **PP-OCR** | `ocr.rs` | 图片→文字 | 不翻译 |
| **Argos** | `translate.rs→ArgosBridge` | en↔zh 离线翻译 | 不联网、不解释 |
| **Ollama** | `translate.rs+ai.rs` | 翻译回退+AI对话 | 不联网 |
| **SiliconFlow** | `translate.rs+ai.rs` | 翻译+AI对话 | 不吃本地 GPU |
| **MyMemory** | `translate.rs` | 兜底翻译 | 仅兜底 |
| **有道智云** | `translate.rs` | 中日/中韩专业翻译 | 需 AppKey |
| **术语库** | `terms.rs` | 占位保护/还原 | 不翻译 |
| **AI 对话** | `ai.rs` | 术语解释/段落理解/问答 | 不背单词/不生成例句 |
| **TTS** | `tts.rs` | 文本→语音 | 不评测发音/不做字幕 |

### 翻译引擎回退链

```
默认: Argos(离线) → Ollama → MyMemory → SiliconFlow
用户可配置固定引擎: argos/ollama/sf/mymemory/youdao
```

### AI 对话范围

**做**: 术语解释、段落理解、上下文问答、翻译润色、关键信息提取
**不做**: 生成例句、闪卡复习、间隔重复、掌握度追踪、发音训练

### AI 对话上下文（v2.0 新增）

```rust
ai.chat_stream_with_context(
    user_msg,
    ChatContext { current_src, current_tgt, selection },
    on_chunk
).await
```

### TTS 引擎回退链

**桌面端**: `OneCore(离线, Win10 Home 标准语音) → edge-tts(在线, 音质更好)`
**浏览器端**: `Web Speech API(浏览器内置)`

### TTS 调用点

| 桌面端 | 浏览器端 |
|---|---|
| 译文栏"🔊朗读" | 气泡"朗读" |
| 气泡"朗读" | popup 采集队列"听发音" |
| 采集队列"听发音" | — |
| AI 回复"朗读" | — |

---

## 四、数据模型

### 4.1 历史记录(唯一真源)

```rust
pub struct HistoryItem {
    pub id: String,           // hex-ts-counter
    pub ts: i64,              // Unix 秒
    pub src: String,          // 原文
    pub tgt: String,          // 译文
    pub src_lang: String,
    pub tgt_lang: String,
    pub engine: String,       // argos/ollama/sf/...
    pub screenshot: String,   // 截图路径
    pub project: String,      // 项目/来源名
    pub collected: bool,      // 是否已采集
    pub note: String,         // 自定义释义
    pub tags: String,         // 逗号分隔
    pub exported: bool,       // 是否已导出
}
```

### 4.2 SQLite 表结构

```sql
CREATE TABLE IF NOT EXISTS history (
    id          TEXT PRIMARY KEY,
    ts          INTEGER NOT NULL,
    src         TEXT NOT NULL,
    tgt         TEXT NOT NULL,
    src_lang    TEXT NOT NULL,
    tgt_lang    TEXT NOT NULL,
    engine      TEXT NOT NULL,
    screenshot  TEXT DEFAULT '',
    project     TEXT DEFAULT '',
    collected   INTEGER DEFAULT 0,
    note        TEXT DEFAULT '',
    tags        TEXT DEFAULT '',
    exported    INTEGER DEFAULT 0
);
CREATE INDEX idx_history_ts ON history(ts DESC);
CREATE INDEX idx_history_collected ON history(collected);
CREATE INDEX idx_history_project ON history(project);

CREATE TABLE IF NOT EXISTS ocr_cache (
    image_hash TEXT PRIMARY KEY,
    result_json TEXT NOT NULL,
    ts INTEGER NOT NULL
);
```

### 4.3 UI 模型

```rust
pub struct HistoryRow {
    pub id: String,
    pub ts_display: String,    // "15:30:05"
    pub src: String,
    pub tgt: String,
    pub engine: String,
    pub project: String,
    pub collected: bool,
}

pub struct CollectRow {
    pub id: String,
    pub src: String,
    pub tgt: String,
    pub tags: String,
    pub note: String,
    pub exported: bool,
}
```

---

## 五、导出格式规格

### Anki TSV

```
prostate	前列腺	The prostate is a gland...\n前列腺是产生精液的腺体。	泌尿 病理学
```

### 欧路 XML

```xml
<?xml version="1.0" encoding="UTF-8"?>
<wordbook>
  <item>
    <word>prostate</word>
    <trans><![CDATA[前列腺]]></trans>
    <tags>泌尿 病理学</tags>
  </item>
</wordbook>
```

### Markdown

```markdown
# WinOCR 采集导出
> 2026-10-05 · 项目：Robbins Pathology
## prostate
- **释义**：前列腺
- **标签**：#泌尿 #病理学
```

---

## 六、浏览器扩展设计

### 6.1 技术方案

```
浏览器扩展(MV3)
  ├── content script(选区气泡, Shadow DOM)
  ├── background worker(Native Messaging)
  ├── tts.js(Web Speech API)
  └── popup.html(快速入口 + 统计)
```

### 6.2 Native Messaging 协议

```json
// 浏览器 → 桌面端
{"id":"req-001","action":"translate","src":"prostate","srcLang":"en","tgtLang":"zh"}
{"id":"req-002","action":"collect","src":"prostate","tgt":"前列腺","tags":["泌尿"]}
// 桌面端 → 浏览器
{"id":"req-001","ok":true,"tgt":"前列腺腺癌","engine":"argos"}
```

### 6.3 核心原则

- 浏览器端**无存储、无推理**
- 所有数据经 Native Messaging 回写桌面端
- 优雅降级: 桌面端未运行时显示断开标记

---

## 七、代码到文件映射表

> 编码时按此表找文件，确认要改什么

| 任务 | 要改的文件 | 摘要 |
|---|---|---|
| 导航轨改名 | `main_window.slint` | `☆生词`→`📌采集`, `⇩导出`→`📤导出` |
| 历史页表格化 | `main_window.slint` | ScrollView→TableView + 搜索/筛选 |
| | `core/types.rs` | 新增 `HistoryRow` |
| | `mod.rs` | `on_load_history` 填充 `Vec<HistoryRow>` |
| 采集队列页 | `main_window.slint` | `vocab`→`collect`: 移除 SM-2/忽略词 |
| | `mod.rs` | `load-vocab`→`load-collect`, 移除 `grade-vocab` |
| 新建 collect.rs | `services/collect.rs` | toggle/list/update/mark_exported |
| 导出格式扩展 | `main_window.slint` | 格式加 Anki/欧路, 加字段映射 |
| | `services/export.rs` | AnkiTsv + EudicXml |
| SQLite 持久化 | `persist.rs` | JSONL→SQLite 全量重写 |
| | `core/types.rs` | HistoryItem 加 6 字段 |
| 旧数据迁移 | `services/migrate.rs` | 新建, 一次性迁移 |
| 翻译预热 | `mod.rs` | spawn_blocking 预热 Argos |
| OCR 缓存 | `ocr.rs` | ocr_cache 表, 入口查缓存 |
| 采集按钮 | `main_window.slint`+`mod.rs` | "加入生词"→"采集" |
| 气泡采集 | `bubble.slint`+`mod.rs` | "收藏生词"→"采集" |
| TTS 服务 | `services/tts.rs` | 新建: OneCore+edge-tts |
| AI 对话上下文 | `ai.rs` | chat_stream_with_context |
| 浏览器扩展 | `browser-extension/*` | 新建 MV3 扩展 |
| Native Messaging | `src/bin/winocr-host.rs` | 新建: stdin/stdout 行协议 |
| 清理 vocab.rs | `services/vocab.rs` + `mod.rs` | 删除 |

---

## 八、实施计划与进度

> `☐` = 待办 · `☑` = 完成 · `⚡` = 进行中

### Phase 1: GUI 重构 (P0)

> 基于 PM 决策：工作台为主 + 抽屉/模态（方案 A）· 参考 `docs/GUI-PRD.md`

| # | 任务 | 状态 | 踩坑记录 |
|---|---|---|---|
| 1.1 | 导航轨改名(vocab→collect) | ☑ | 踩坑：Python批量替换后仍有旧名残留，需grep二次排查 |
| P0-1 | 工作台双栏重构(空状态+有内容+候选词面板) | ☑ | v2.0 重写 main_window.slint（1070行，Python脚本生成） |
| P0-2 | 导航轨改4项+抽屉入口(语义从切页→开抽屉) | ☑ | 导航轨 60px 4项（工作台/采集/历史/AI），点击触发 open-drawer/close-drawer |
| P0-3 | 采集抽屉(表格+搜索+筛选+批量操作) | ☑ | 480px 右侧抽屉，含搜索栏+标签筛选+总数+添加表单 |
| P0-4 | 历史抽屉(表格+高级筛选+重新采集) | ☑ | 480px 右侧抽屉，含刷新/清空工具+卡片列表 |
| P0-5 | 导出模态(Anki/欧路/MD/TXT+字段映射+预览) | ☑ | 560×420 居中模态，含 Anki TSV/Markdown/欧路XML/纯文本 4 格式 |
| P0-6 | 设置模态(翻译/AI/术语/划词/主题) | ☑ | 640×480 居中模态，含翻译引擎/AI后端/主题/术语 |
| P0-7 | AI面板(上下文+流式+术语采集) | ☑ | 480px 抽屉，含状态条+上下文条+对话区+输入区 |
| P0-8 | 候选词面板(截图后自动出现) | ☑ | ocr-src 变化时 recompute-candidates，非空则显示 |
| P0-d | 抽屉/模态组件(≥480px抽屉+模态对话框) | ☑ | 踩坑：抽屉覆盖层不能在 HorizontalLayout 内设 x（布局管理 x），须提到外壳 Rectangle 顶层；border-left-width/color 不存在，用 border-width/color；Close shell 只能闭合 VerticalLayout+Rectangle（2个}），多闭合会让后续元素变顶层 |
| P0-m | mod.rs回调适配(open/close drawer+modal) | ☑ | 删 on_navigate/on_open_settings/set_current_page；新增 on_open_drawer/on_close_drawer/on_open_modal/on_close_modal；set_history_lines→set_history_rows、set_setting_test_status→set_setting_save_status、border_soft 属性已删 |

### Phase 2: 数据层重构

| # | 任务 | 状态 | 踩坑记录 |
|---|---|---|---|
| 2.1 | HistoryItem 增加 6 字段 | ☑ | 保留 favorited 兼容旧 JSONL；13 字段带 #[serde(default)] 保证旧数据可反序列化 |
| 2.2 | persist.rs SQLite 重写 | ☑ | **get_or_try_init 是不稳定特性**（E0658），改 OnceLock<Result<Mutex<Connection>,String>> + get_or_init；`&dyn ToSql` 引用不能指向 if-let 块内绑定，需提升到块外 |
| 2.3 | collect.rs 新建 | ☑ | 复用 persist 的 `&'static Mutex<Connection>`；Collect::new 调用 ensure_table 幂等建表 |
| 2.4 | export.rs AnkiTsv/EudicXml | ☑ | **UI 导出回调需对齐 slint 的 export-format 取值**：0=Anki,1=MD,2=欧路,3=纯文本（旧代码把 0 当 MD，导致选 Anki 实际导出 MD）；format.extension() 统一扩展名 |
| 2.5 | migrate.rs 旧数据迁移 | ☑ | 迁移完成改名 .bak；两个迁移函数在 rename 前显式 drop 连接锁 |
| 2.6 | vocab.rs 删除 + mod.rs 清理 | ☑ | 实际无 vocab.rs 文件、mod.rs 无引用，仅确认清理 |
| 2.7 | tts.rs 新建 | ☑ | edge-tts/onecore 预留接口返回「未实现」，SAPI PowerShell 兜底可用；TtsEngine 枚举预留 |
| 2.8 | AI 对话上下文(chat_stream_with_context) | ☑ | **Weak 不 Copy**：w_final 被 async move 捕获后不能在流式完成闭包再用，需额外 clone w_ctx 读 OCR 上下文 |

### Phase 3: 性能优化

| # | 任务 | 状态 | 踩坑记录 |
|---|---|---|---|
| 3.1 | 翻译预热(启动异步 Argos) | ☑ | `Translate::prewarm()` 只调 argos（不走回退链，避免预热失败触发 ollama/mymemory/sf 网络请求）；run() 中 spawn 线程 + rt.block_on，不阻塞启动。**Argos 子进程是懒加载模型**（收到请求才 load_engine），故预热必须发真实短文本请求，光 spawn 子进程无效 |
| 3.2 | OCR 缓存(dhash) | ☑ | 拆 `recognize_image`(缓存包装) + `recognize_image_uncached`(原逻辑)；**缓存查在 ensure_ort_init 之前**（命中连 ORT 都不初始化）；dHash 9x8 灰度→64bit，只缓存非空结果（不缓存「识别不出」的坏结果）；开关走 Config::cache_enabled + OnceLock\<AtomicBool\>，避免每次 OCR 读配置文件；`set_ocr_cache_enabled` 加 #[allow(dead_code)] 待设置页接线 |
| 3.3 | 骨架屏 + 错误态 UI | ☑ | 加 `busy` / `ocr-loading` 两个 in-out property；原文骨架条件 `ocr-loading && ocr-src==""`（**不能只判 ocr-loading**，否则重新识别旧图时骨架与双栏重叠）；空状态引导加 `&& !root.ocr-loading`；译文栏 `if busy && ocr-out=="" : 骨架 else : TextEdit`；状态条加 busy Pill。5 条 OCR 入口（capture/open_file/url/paste/re-ocr）全部接线 busy 复位，**取消截图(Ok(None))与各失败分支也必须复位**，否则永久卡在骨架 |
| 3.4 | 对照区左右/上下切换 | ☑ | 抽出 `component Pane`（参数化标题/正文绑定/只读/骨架/动作条按钮），左右用 HorizontalLayout、上下用 VerticalLayout 各放两个 Pane，**避免两套布局重复维护同一套 UI**（重复定义从 112 行降到 51 行）；`split-vertical` property + `toggle-split` 回调，持久化到 `Config::split_vertical`，启动时回填 |

### Phase 4: 浏览器扩展

| # | 任务 | 状态 | 踩坑记录 |
|---|---|---|---|
| 4.1 | MV3 扩展骨架 | ☑ | `browser-extension/`：manifest.json（MV3，nativeMessaging/contextMenus/storage/activeTab 权限）、content.js（Shadow DOM 划词气泡）、background.js（NM 桥接+ping）、popup、tts.js、icons（纯 Python 生成 PNG） |
| 4.2 | tts.js Web Speech API | ☑ | 浏览器端 Web Speech 朗读；content.js 动态加载 tts.js（需在 manifest 声明 web_accessible_resources） |
| 4.3 | winocr-host.exe Native Messaging | ☑ | `app/src/bin/winocr-host.rs`：stdin/stdout 4 字节小端长度前缀 + JSON；复用 Translate/Collect/Config；需新增 `app/src/lib.rs` 导出 core+services 供 host 引用 |
| 4.4 | 消息协议实现 | ☑ | translate/collect/ping 三动作；请求带 id 关联响应；错误回写 `{"ok":false,"error":"..."}` |
| 4.5 | 连接检测 + 状态标记 | ☑ | background 每 10s ping；popup/content 红绿点状态；install_host.ps1 注册 HKCU（Chrome+Edge） |

---

## 九、关键约束(编码铁律)

| 规则 | 说明 |
|---|---|
| AI 不生成例句 | 句子来自真实截图 |
| 翻译引擎不解释 | 解释转 AI 对话 |
| OCR 不翻译 | OCR 只输出原文 |
| 浏览器端无存储 | 数据回写桌面端 |
| 采集=历史子集 | 不建独立表 |
| 术语库不翻译 | 只做占位/还原 |
| 托盘不处理业务 | 只发事件 |
| TTS 不翻译 | 只朗读已有译文 |
| TTS 不评测 | 仅朗读 |
| vocab.rs 彻底删除 | 不保留, 不重构 |

---

## 十、踩坑记录区

> 每次踩坑立即记录，避免后人重复

| 日期 | 阶段 | 问题 | 根因 | 解法 |
|---|---|---|---|---|
| | | | | |

---

## 十一、关键决策

| # | 决策 | 原因 |
|---|---|---|
| D1 | 生词本→历史子集 | 避免数据重复 |
| D2 | SM-2 移除 | Anki/欧路更专业 |
| D3 | SQLite 存储 | 查询快, 可扩展 |
| D4 | Argos 旁路子进程 | 规避 CRT 风险 |
| D5 | 三种导出格式全做 | 覆盖主要学习工具 |
| D6 | 浏览器端无独立存储 | 避免同步冲突 |
| D7 | TTS 两端各自调用 | 简单可靠 |
| D8 | 实施顺序: GUI→数据→性能→扩展 | 先见效果 |

---

## 十二、技术栈确认

| 层 | 技术 |
|---|---|
| UI | Slint 1.18 (软件渲染) |
| 推理 | ort (ONNX Runtime) |
| 翻译 | Argos 旁路子进程 |
| 存储 | SQLite (rusqlite) |
| 导出 | 自写序列化 |
| 热键 | windows-sys RegisterHotKey |
| 托盘 | windows-sys Shell_NotifyIconW |
| TTS | OneCore + edge-tts / Web Speech |
| 浏览器 | MV3 + Native Messaging |

---

> **编码前必读本文档 · 进度实时更新 · 踩坑即记录 · 其他文档见 `docs-archive-2026-10-05.zip`**

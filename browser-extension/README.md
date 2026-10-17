# WinOCR 浏览器扩展（Phase 4）

划词翻译 + 一键采集生词 + **本地 OCR（PP-OCRv6，无需主程序）**。
- 划词翻译/采集：默认经 **Native Messaging** 回写桌面端 WinOCR（离线 Argos 翻译 + 生词本）。
- 截图 OCR：**完全在浏览器内运行**（onnxruntime-web + 内置 PP-OCRv6 tiny 模型），**不经过主程序**。
- 也可选在线/本地翻译引擎（SiliconFlow / MyMemory / Ollama / 浏览器内置），**主程序未运行时扩展也能独立划词翻译**。

## 架构

```
浏览器页面
  └─ content.js（划词气泡，Shadow DOM 隔离）
       └─ background.js（service worker）
            ├─ Native Messaging 桥接 → winocr-host.exe → 桌面端 WinOCR（翻译 + 生词本）
            ├─ Offscreen Document（本地 OCR：PP-OCRv6 + onnxruntime-web）
            └─ 在线翻译引擎（sf / mymemory / ollama / browser）→ 独立翻译，无需桌面端
```

## 本地 OCR（内置，不依赖主程序）

- 模型：**PP-OCRv6 tiny**（`models/ocr/det_tiny.onnx` 1.8MB + `rec_tiny.onnx` 4.5MB + `ppocr_dict.txt` 6904 字符）
- 运行时：**onnxruntime-web 1.30**（`vendor/ort.all.min.js` + `vendor/ort-wasm-simd-threaded.jsep.wasm`）
- 架构：background 启动时创建 Offscreen Document（`offscreen.html`），常驻内存加载模型；
  截图 OCR 请求发送到 offscreen，由 `ocr.js` 完成 文本检测（DB）→ 文本行裁剪 → CTC 识别 全流程。
- 若本地 OCR 不可用（如浏览器版本过低不支持 offscreen），自动回退桌面端 host。

## 目录结构

| 文件 | 作用 |
|---|---|
| `manifest.json` | MV3 清单（含 offscreen 权限、CSP `'wasm-unsafe-eval'`） |
| `content.js` / `content.css` | 划词气泡（Shadow DOM），采集结果显示原文/译文/过滤生词列表 |
| `background.js` | Native Messaging 桥接、ping、上下文菜单、本地 OCR offscreen 管理 |
| `ocr.js` | 本地 OCR 推理管线（PP-OCRv6：det 预处理/DB 后处理、rec 预处理/CTC 解码） |
| `offscreen.html` / `offscreen.js` | Offscreen Document：承载 onnxruntime-web，消息路由 |
| `models/ocr/` | PP-OCRv6 tiny ONNX 模型 + 字典 |
| `vendor/` | onnxruntime-web（ort.all.min.js + wasm） |
| `popup.html` / `popup.js` | 快速翻译/采集入口 + 连接状态 |
| `options.html` / `options.js` | 设置页（语言、气泡、右键菜单、TTS） |
| `tts.js` | Web Speech API 朗读（浏览器本地） |
| `install_host.ps1` / `uninstall_host.ps1` | 注册/卸载 Native Messaging host |
| `icons/` | 扩展图标 |

## 设置页

- 入口：右键扩展图标 →「选项」，或 `chrome://extensions` → 详情 →「扩展程序选项」
- 功能：
  - **翻译引擎**：选择翻译引擎，主程序未运行时扩展也能独立翻译
  - **划词翻译**：默认源/目标语言、气泡开关、**气泡常显示**（默认开启，点「关闭」或页面其它处才消失；也可设为 N 秒自动隐藏）
  - **右键菜单**：翻译 / 采集生词入口开关
  - **朗读**：Web Speech 语速 / 音调
  - **关于**：桌面端连接状态、host 名、数据去向
- 设置存于 `chrome.storage.sync`，跨设备同步；修改后实时生效（无需刷新页面）

## 独立采集缓存同步（方案 A）

主程序未运行时，**采集生词也能用**：

- **划词翻译 / 截图 OCR**：完全在浏览器内完成（在线引擎 / 本地 PP-OCRv6）。
- **采集生词**：桌面端未连接时，生词先暂存到浏览器本地（`chrome.storage.local`），气泡显示「**已缓存到本地（待同步）**」；桌面端下次启动并建立连接后，**自动补传**到生词本（`%APPDATA%/WinOCR/winocr.db`）。

流程：

```
划词 → 点「采集生词」
   ├─ host 已连接 → 直接写入桌面端生词本（气泡：已采集到生词本 ✓）
   └─ host 未连接 → 存 chrome.storage.local（气泡：已缓存到本地 ✓）
         └─ 桌面端启动 → connect()/ping 成功 → flushPendingCollects() 自动补传
```

特性：

- 缓存按 `word` 去重（重复采集同一词只保留最新一条）
- 缓存上限 500 条，防止本地存储无限增长
- 补传成功自动从缓存移除；失败保留，等下次连接再试
- 补传时机：`connect()` 成功时、每次 `ping` 成功时（每 10 秒检测）
- 右键菜单「采集生词」同样支持缓存

## 翻译引擎（独立工作）

浏览器扩展支持 5 种翻译引擎，**不依赖桌面端 WinOCR 也能翻译**：

| 引擎 | 说明 | 依赖 |
|---|---|---|
| `host`（默认） | 桌面端 WinOCR（Argos 离线翻译） | 桌面端运行；未运行自动回退在线引擎 |
| `sf` | SiliconFlow（OpenAI 兼容） | 需 API Key（`https://cloud.siliconflow.cn` 免费申请） |
| `mymemory` | MyMemory（免费） | 无需 Key；填邮箱可提升额度 |
| `ollama` | 本地 Ollama 模型 | 需本机运行 Ollama |
| `browser` | 浏览器内置 Translator API | 需 Chrome/Edge 138+ 且语言包已下载 |

- 选择 `host` 时，若桌面端未连接，会自动回退：`sf → mymemory → ollama → browser`。
- 选择其他引擎时，**完全独立于桌面端**，主程序未运行也能划词翻译。
- 设置页提供「测试翻译引擎」按钮，可实时验证当前引擎是否可用。

### 气泡内翻译引擎状态

划词气泡头部下方会显示**本次翻译实际使用的引擎**：

- `⚡ 引擎：桌面端 WinOCR · 桌面端已连接` —— host 成功
- `⚡ 引擎：SiliconFlow · 在线引擎` / `MyMemory` / `Ollama` / `浏览器内置` —— 在线引擎
- `⚡ 引擎：本地 OCR（PP-OCRv6）· 本地模型` —— 截图 OCR 走浏览器内置模型
- 翻译失败时不显示引擎条（保留错误信息）

设置页「测试翻译引擎」与 popup 快速翻译结果区也会显示引擎名。

## 桌面端 host

`app/src/bin/winocr-host.rs` → `winocr-host.exe`

- 复用 `services::translate::Translate`（Argos + 回退链）
- 复用 `services::collect::Collect`（写 `%APPDATA%/WinOCR/winocr.db`）
- 协议：stdin/stdout 4 字节小端长度前缀 + JSON

### 构建

```bat
cd /d D:\AI\WinOCRHtml20060927
cargo build -p winocr --bin winocr-host
```

> 本工程是 cargo workspace（根在仓库根），target 统一输出到
> `D:\AI\WinOCRHtml20060927\target\debug\winocr-host.exe`，
> 在 `app\` 目录下执行 `cargo build --bin winocr-host` 结果一样。
> 依赖 `lib.rs`（新增），导出 `core` + `services` 供 host 复用。
> 主程序 `main.rs` 不受影响。

## 安装步骤

1. **构建 host**（见上）
2. **注册 host**：右键 `browser-extension/install_host.ps1` → 「使用 PowerShell 运行」
   - 生成 `com.winocr.native_host.json` 并写入 `HKCU\Software\Google\Chrome\NativeMessagingHosts\` 与 Edge 对应键
3. **加载扩展**：
   - Chrome：`chrome://extensions` → 开启「开发者模式」→「加载已解压的扩展程序」→ 选 `browser-extension/` 目录
   - Edge：`edge://extensions` 同理
   - 每次修改代码后，在扩展管理页点击 **「刷新」** 重新加载
4. **启用划词**：选中网页文本 → 气泡出现 → 点「翻译 / 采集生词 / 复制 / 朗读」

## 连接检测

- `background.js` 每 10 秒向 host 发 `ping`，桌面端未运行 / host 未注册时显示**未连接**（气泡红点、popup 红点）
- 桌面端运行后自动恢复，无需刷新页面

## 注册表安全收紧（已完成）

`install_host.ps1` 生成的 manifest 中 `allowed_origins` 已收紧为**真实扩展 ID**：

- 扩展 ID：`ahbpjindiehpcfpmjcfkpaigjfhnibji`
- `allowed_origins`：`["chrome-extension://ahbpjindiehpcfpmjcfkpaigjfhnibji/"]`

如需换用其他扩展 ID（例如测试另一台机器），先设置环境变量再运行安装脚本：

```powershell
$env:WINOCR_EXT_ID = "你的扩展ID"
powershell -ExecutionPolicy Bypass -File browser-extension\install_host.ps1
```

重新运行 `install_host.ps1` 会用当前 ID 覆盖 manifest 并刷新注册表。

## 消息协议

### 浏览器 → host

```json
{"id":"req-001","action":"translate","src":"prostate","srcLang":"en","tgtLang":"zh"}
{"id":"req-002","action":"collect","src":"prostate","tgt":"前列腺","tags":["泌尿"],"srcLang":"en","tgtLang":"zh"}
{"id":"req-004","action":"list_collect","limit":500}
{"id":"req-003","action":"ping"}
```

### host → 浏览器

```json
{"id":"req-001","ok":true,"tgt":"前列腺腺癌","engine":"argos"}
{"id":"req-002","ok":true,"data":{"count":1,"items":[{"word":"prostate","project":"320.2740 泌尿外科学","level":"专业"}]}}
{"id":"req-004","ok":true,"data":{"count":2,"items":[{"id":1,"ts":1728880000,"word":"prostate","translation":"前列腺","project":"320.2740 泌尿外科学","level":"专业","context":"prostate cancer"},{"id":2,"ts":1728880100,"word":"nephritis","translation":"肾炎","project":"","level":"","context":"…"}]}}
{"id":"req-003","ok":true}
```

> `list_collect`（2026-10-14，B 方案）：只读拉取桌面端生词本。桌面端为**唯一真源**，扩展侧只做镜像不做写入。`limit` 可省略（0/缺省 = 500 条，最大 2000）。popup 底部「生词本」链接打开 `words.html` 面板（搜索 / 按级别 / 按项目筛选）。

> `collect` 响应中的 `data.items` 为过滤后实际入库的生词明细，气泡内会展示「原文 / 译文 / 过滤生词列表」。
> 划词气泡（`content.js`）与截图 OCR 结果气泡（`background.js` 注入的 `regionSelectFunc`）均已支持该列表；采集缓存时同样显示词条（标记「待同步」）。
>
> **采集范围（2026-10-14）**：host 将 `src`（原文）与 `tgt`（译文）**合并为采集池**，只从中提取 ASCII 英文词（中文/数字/符号不收；撇号词 don't、patient's 保持完整；同段内去重）。无论原文是中是英，只要原文或译文里有英文词就会入库；`context` 优先记录原文（学习时回忆出处）。

## 已知限制

- `content.js` 的 TTS 用浏览器 Web Speech API，音色取决于系统/浏览器，与桌面端 OneCore 无关
- **左键划词即自动翻译**：气泡出现后自动调用「翻译」按钮（无需再点），若桌面端未连接则显示「翻译失败：桌面端未连接？」
- 划词检测：`mouseup` + `selectionchange`；气泡内 `mousedown` 已 `preventDefault`，点按钮不会清空页面选区
- **扩展被重新加载/更新后**，旧页面已注入的 content script 会上下文失效。已做防御：
  - 所有 `chrome.runtime.sendMessage` 调用经 `safeSendMessage` 包装（检查上下文 + try-catch + lastError 处理）
  - 检测到 `Extension context invalidated` 时自动清理页面上残留的气泡/标记/遮罩，静默失败不崩溃
- **本地 OCR 首次运行需加载模型**（约 6.3MB ONNX + 28MB wasm），Offscreen Document 预加载后常驻；低端机器首次可能需 2~5 秒
- 本地 OCR 需要 Chrome/Edge 109+（Offscreen Document API）；浏览器不支持时自动回退桌面端 host
- **采集缓存的同步有窗口期**：生词在桌面端启动前只存在于浏览器本地，若清除浏览器数据会丢失缓存；换设备不同步（缓存是本地的）
- `allowed_origins` 已收紧为真实扩展 ID（见上）

## 卸载

```powershell
powershell -ExecutionPolicy Bypass -File browser-extension/uninstall_host.ps1
```

然后在 `chrome://extensions` 移除扩展。
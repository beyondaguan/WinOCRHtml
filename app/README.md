# WinOCR · Rust + Slint 原生脚手架

原生 UI（**Slint** + Rust），不使用 HTML / WebView2。主窗口 UI 复刻
`.workbuddy/outputs/main-window-v1` 的工作台布局。

> **配套文档**：`架构.md`（分层/模块状态/数据流/接口契约）·
> `技术.md`（技术栈/Slint 铁律/Win32 集成/踩坑记录）。
> **维护约定**：每接入一个服务、每个 Phase 收尾，验证并回补这两份文档。

## 运行

> **工具链**：本项目使用 **MSVC** 工具链（`stable-x86_64-pc-windows-msvc`，已在本机设为默认），
> 依赖 VS Build Tools 2022 的 C++ 工作负载（cl.exe / link.exe / Windows SDK）。
> 无需额外 PATH 配置，直接构建即可。
>
> ```bash
> # 若默认工具链被改动过，可显式指定：
> rustup default stable-x86_64-pc-windows-msvc
> ```

```bash
# 方式一：从仓库根（workspace）
cargo run -p winocr

# 方式二：进入 app
cd app && cargo run
```

> **双 bin 说明**：本包含两个 bin——`winocr`（主程序，来自 `src/main.rs`）和
> `winocr-host`（Native Messaging host，来自 `src/bin/winocr-host.rs`）。
> `app/Cargo.toml` 已设 `default-run = "winocr"`，`cargo run -p winocr` 默认跑主程序；
> host 需显式指定：`cargo run --bin winocr-host` 或重编 `cargo build --release --bin winocr-host`。
> host 被浏览器占用导致编译 `os error 5` 时，用仓库根的 `stop_host.ps1`（杀进程 + 摘注册表 + 重建 + 恢复）。

> 首次 `cargo run` 会编译 Slint（winit 后端 + **软件渲染器**）及依赖，耗时较长（首次约 5 分钟）。
>
> **dev profile 已开优化（2026-10-17）**：根 `Cargo.toml` 设了 `[profile.dev] opt-level = 1` +
> `[profile.dev.package."*"] opt-level = 2`。OCR 热路径全是逐像素循环（resize/CTC 解码/连通域），
> 纯 O0 的 debug 构建慢 20-50 倍，大图 OCR 会假死数分钟（曾被误报"程序锁死"）。
> 代价：改依赖后的首次编译更慢；本 crate 代码仍是秒级增量。
>
> **渲染后端**：默认**软件渲染**（无 OpenGL 依赖，远程桌面 / 虚拟机 / 无显卡环境均可跑）。
> 本 UI 很简单，软件渲染性能足够；实测本机 OpenGL 上下文创建失败，故弃用 femtovg。
> 如需 GPU 加速，可在 `app/Cargo.toml` 给 `slint` 加回 `renderer-femtovg`（需 OpenGL 3.3+）
> 或 `renderer-skia`（画质最好但编译很慢）。

## 手动构建（逐步命令）

> 在**你自己的终端**里执行（PowerShell / cmd / Git Bash 均可）。

**步骤 0 · 打开终端并进入项目**

```powershell
cd D:\AI\WinOCRHtml20060927
```

**步骤 1 · 确认工具链（首次或换机时）**

```powershell
rustup show active-toolchain
# 期望输出：stable-x86_64-pc-windows-msvc (default)
```

> 若显示 `gnu` 而非 `msvc`，执行 `rustup default stable-x86_64-pc-windows-msvc` 切换。
>
> **注意**：`where.exe cl` 找不到 cl.exe 是**正常的**——VS Build Tools 默认不把 cl.exe
> 加入 PATH。rustc 会自己（经注册表 / vswhere）定位 MSVC 的 `link.exe`，**无需
> vcvarsall 或 Developer Command Prompt**。判断工具链是否可用，直接跑步骤 2 构建即可。

**步骤 2 · 构建（debug）**

```powershell
cargo build -p winocr
# 首次约 5-20 分钟（Slint 编译慢），之后增量构建为秒级
# 成功标志：Finished `dev` profile ... ，产物 target\debug\winocr.exe
```

**步骤 3 · 运行**

```powershell
.\target\debug\winocr.exe
```

若报 `Cannot create OpenGL context`（远程桌面 / 虚拟机 / 无显卡），改用软件渲染：

```powershell
$env:SLINT_BACKEND = "winit-software"; .\target\debug\winocr.exe
```

**步骤 4 · 发布版（可选）**

```powershell
cargo build -p winocr --release
.\target\release\winocr.exe
```

**常见问题排查**

| 现象 | 处理 |
|---|---|
| 构建“卡住不动” | 其实在编译 Slint，首次 5-20 分钟属正常；看任务管理器 CPU 是否在跑，别杀进程 |
| 报 `.cargo-build-lock` 拒绝访问 | 删 `target\debug\.cargo-*lock` 后重试；或换目标目录：`$env:CARGO_TARGET_DIR="D:\AI\WinOCRHtml20060927\target_v2"; cargo build -p winocr` |
| Git Bash 下换目录报错 | `CARGO_TARGET_DIR` 必须写 Windows 原生路径 `D:\...`；写 `/d/...` 会被拼成 `d:/d/...` |

## 架构（Core / Services / UI 严格分层）

```
UI (slint + ui/mod.rs)
  │  调用
  ▼
Services (services/*)   ── 依赖 ──▶  Core (core/*)
                                       │ 不反向依赖 UI / Services
```

- **Core**（`core/`）：`types`（Lang / WorkMode / AppState / AppEvent）、
  `event_bus`（基于 `std::sync::mpsc` 的 publish/subscribe）、`config`（stub 读写 JSON）。
  **不依赖** UI / Services。
- **Services**（`services/`）：`capture` / `ocr` / `translate` / `ai` / `attach` /
  `persist` / `hotkey` / `tray`。接线进度：
  - `hotkey` — **✅ 真实实现**：`RegisterHotKey` + 独立线程消息循环（移植自
    rust-reference）。**P0-1（2026-10-14）扩展为三组热键**：`Alt+Q`/`Alt+T` → 截图翻译、
    `Alt+W` → 开/关划词取词（实时生效 + 持久化）、`Alt+D` → 显示/置前主窗口；
    注册结果反馈到状态条。Alt+Q 失败视为致命（多实例），其余失败仅告警不阻塞。
    配置驱动热键表（Ctrl+Shift+V 等）待 config 服务接入。
  - `capture` — **✅ 真实实现**：全屏框选蒙版（冻结背景 + 拖选 + 尺寸提示 + Esc 取消）
    → GDI BitBlt 抓选区 → PNG 存 `%TEMP%`（移植自 screenshot.rs + selection_window.rs
    核心路径）。MVP 限主显示器；DXGI/放大镜/8 手柄后置。结果存 `last_capture()` 供 OCR。
  - `ocr` — **✅ 真实实现**：Windows.Media.Ocr 系统引擎（zh-Hans → 用户语言 → 兜底），
    截图后自动识别，文本写入左栏 `ocr-src`。PP-OCR 可选引擎后补。
  - 其余 5 个仍为 **占位 stub**（struct + new() + 一个异步/普通 fn 返回 `Ok(())`
    或空集合，**不会**启动即 panic）。每个文件含 `// TODO:` 注明将来从
    `rust-reference/` 移植什么。
- **UI**（`ui/`）：`main_window.slint`（声明式组件，复刻工作台视觉与结构）+
  `ui/mod.rs`（`include_modules!` + `run()` 绑定回调到 Services 占位）。

依赖方向：**UI → Services → Core**，单向，无反向。

## 主窗口结构（复刻要点）

- **顶栏**：logo 渐变块 + 标题/版本 + 状态 mini + 截图/粘贴按钮 + 语言对（双
  select + 互换）+ AI 模式切换 + 最小化/关闭。
- **左导航轨**（分组）：视图[截图 | AI] / 工具[生词 | 历史 | 知识] / 系统[诊断 |
  设置]。
- **中央工作区**：`if ai-mode` 互斥切换 ——
  - 双栏：`Hero 截图翻译`（渐变大按钮 → `capture()`）+ `OCR 原文 TextEdit` ⇄
    `翻译 TextEdit`。
  - AI 对话：消息列表 + 输入框 + 发送（`send-ai`）。
- **状态条**：OCR/翻译/缓存/代理 状态 pill + 额度 + 版本。

## 当前为 Stub 的部分

- 截图、OCR、翻译、AI 对话、划词、历史/生词、热键、托盘 **均只打日志**，未接真实逻辑。
- 图标用字符占位（▣ ✦ ☆ 等），真实 SVG 图标后续通过 `slint-build` 嵌入 assets。
- 语言对互换（`swap-lang`）当前仅记录日志，未真正交换选中项。

## 后续接入真实逻辑

对照 `rust-reference/`（**只读参考，不复制**）：

| stub | 来源 |
|------|------|
| `services/capture.rs` | `rust-reference/screenshot.rs` |
| `services/ocr.rs` | `rust-reference/ocr.rs` |
| `services/translate.rs` | `rust-reference/translate.rs` |
| `services/ai.rs` | `rust-reference` 中 AI 对话模块 |
| `services/attach.rs` | `rust-reference/selection_window.rs` |
| `services/persist.rs` | `rust-reference/history.rs` + `terminology.rs` |
| `services/hotkey.rs` | `rust-reference/hotkey.rs` |
| `services/tray.rs` | `rust-reference/tray.rs` |
| `core/config.rs` | `rust-reference/config.rs`（路径 + DPAPI） |

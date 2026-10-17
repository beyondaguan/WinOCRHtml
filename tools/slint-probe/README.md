# slint-probe — 离屏布局量测探针

## 为什么有这个工具

`.slint` 的**语法检查通过 ≠ 布局正确**。Slint 的裁切（父容器 `clip: true`）、
stretch 分配、preferred-width 撑破等问题**不会报任何编译错误**，只能靠真实像素定位。

本项目已两次因「读代码推理布局」而误判（以为左右对照坏了、以为要加 `wrap`），
故建此工具：**离屏渲染出 PNG，直接看真实布局**。

## 组成

| 文件 | 作用 |
|---|---|
| `main.rs` | 离屏渲染主程序（`MinimalSoftwareWindow` + `OffscreenPlatform`） |
| `build.rs` | 调 `slint_build::compile` 编译目标 `.slint` |
| `probe_px.py` | 像素级量测 PNG 的内容边界（判断是否被窗口裁切） |

配套的 `tools/slint-check` 只做语法/类型检查（秒级），两者互补：
**先 slint-check 过语法 → 再 slint-probe 看真实布局**。

## 用法

```bash
cd /d/AI/WinOCRHtml20060927

# 基本渲染：<slint> <输出png> <宽> <高>
cargo run --quiet --manifest-path tools/slint-probe/Cargo.toml -- \
    app/src/ui/main_window.slint .workbuddy/outputs/probe.png 1080 600
```

### 环境变量（复现不同 UI 状态）

| 变量 | 作用 |
|---|---|
| `PROBE_WITH_TEXT=1` | 给 `ocr-out` 填样例译文（有内容态，默认只填 `ocr-src`） |
| `PROBE_SPLIT_VERTICAL=1` | `split-vertical=true` → 上下对照 |
| `PROBE_CANDIDATES=N` | 造 N 个候选词（复现候选词面板溢出/换行） |
| `PROBE_DRAWER=collect` | 打开指定抽屉（`collect` / `history` / `ai`） |
| `PROBE_MODAL=settings` | 打开指定模态（`settings` / `export`） |
| `PROBE_UI_SCALE=150` | 设界面缩放百分比（验证字体缩放） |
| `PROBE_UI_FONT=SimSun` | 设界面字体族 |
| `PROBE_BARE=1` | 跳过所有属性设置（用于渲染最小复现组件） |
| `PROBE_SLINT=../../app/src/ui/main_window.slint` | 指定要编译的 `.slint`（默认即主窗口） |

### ⚠️ 渲染不同窗口尺寸（验证「窗口能否缩放」）

```bash
PROBE_WITH_TEXT=1 cargo run --quiet --manifest-path tools/slint-probe/Cargo.toml -- \
    x out-1130.png 1130 640
PROBE_WITH_TEXT=1 cargo run --quiet --manifest-path tools/slint-probe/Cargo.toml -- \
    x out-1600.png 1600 900
python tools/slint-probe/probe_px.py out-1600.png --bg=255,255,255
```
若两张图的内容边界**相同**（例如都止于 x=1129），说明内容宽度被写死、
没有跟随窗口 —— 这正是「窗口能拉大但内容不跟着变」的根因。

⚠️ **必须把尺寸设到组件自己的窗口**（`window.window().set_size(...)`），
只设 `MinimalSoftwareWindow` 不会改变组件根的 width/height，会得到「窗口变了
但布局没变」的伪影。

### 像素量测

```bash
python tools/slint-probe/probe_px.py .workbuddy/outputs/probe.png --rows=575,570,290
```
输出「内容最右/最下坐标」，触边（= 宽-2）即提示可能被裁切。

## ⚠️ 三个必须知道的坑

1. **build script 缓存**：改 `build.rs` 后 cargo 不一定重编 build script 本体，
   会静默复用旧的生成代码（表现为「改了 slint 但渲染没变」）。
   排查用：`ls -la target/debug/build/slint-probe-*/out/main_window.rs` 看时间戳，
   必要时 `rm -rf target/debug/build/slint-probe-* target/debug/.fingerprint/slint-probe-*`。

2. **`rerun-if-env-changed=PROBE_SLINT` 必须声明**：cargo 默认不跟踪环境变量，
   切换目标 `.slint` 时不加这句会渲染旧文件。

3. **半透明（alpha）渲染不出来**：探针用 `PremultipliedRgbaColor` 作目标缓冲，
   `Rectangle { background: #RRGGBBAA }` 的**半透明色渲染后与不设等价**，
   而全不透明色（如 `#ff0000ff`）正常。所以**不要用本探针验证遮罩/半透明效果**，
   需真机跑 `cargo run` 看。（z 序、全不透明色、stretch、裁切、换行都能可靠验证。）

## 关键结论（Slint 布局语义，实测确认）

- **chip 类元素不设 `horizontal-stretch: 0` 会被按 stretch 均分/撑开**，
  使所在行的 preferred-width 远超容器 → 父容器 `clip: true` 把右侧内容推出窗口
  （本项目候选词面板就踩过：表现为「词条被裁 + 翻译栏按钮消失」）。
- **Slint 没有自动换行容器，也没有 `string.split` / `trim`**
  （字符串内建只有 `is-empty` / `character-count` / `to-lowercase` / `to-uppercase` /
  `starts-with` / `ends-with` / `replace-all`）。要换行只能在 Rust 侧算好，
  用 `<[[string]]>` 二维模型 + 双层 `for` 渲染。
  Rust 侧 setter 签名是 `ModelRc<ModelRc<SharedString>>`（内层是 `ModelRc`，不是 `VecModel`）。
- **8 位 hex 的 alpha 在后两位**：`#RRGGBBAA`，20% 黑 = `#00000033`（不是 `#33000000`）。
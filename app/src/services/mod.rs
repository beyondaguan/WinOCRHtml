// ============================================================
// Services 层
// 依赖 Core（类型 / 事件总线 / 配置），被 UI 调用。不反向依赖 UI。
// 本目录当前为占位 stub：struct + new() + 一个 async/pub fn 占位，
// 真实逻辑后续从 rust-reference 对应模块移植。
// ============================================================
pub use crate::core::types::Lang;
pub mod capture;
pub mod ocr;
pub mod translate;
pub use translate::Translate;
pub mod ai;
pub mod attach;
pub mod clipboard;
pub mod cancel;
pub mod persist;
pub mod hotkey;
pub mod single_instance;
pub mod tray;
pub mod terms;
pub mod classify;
pub mod collect;
pub mod export;
pub mod migrate;
pub mod tts;
pub mod dict;
pub mod enrich;
pub mod study;
pub mod stage;

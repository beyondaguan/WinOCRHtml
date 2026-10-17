// ============================================================
// WinOCR 库入口
// 供同 crate 内的多个 binary（主程序 main.rs、Native Messaging host
// winocr-host.rs）共享 core 与 services 层。
// UI 层不放入 lib：Slint 生成代码 + 事件循环仅主程序需要，
// host 不需要 UI，避免把 GUI 依赖链带进 host。
// ============================================================
pub mod core;
pub mod services;
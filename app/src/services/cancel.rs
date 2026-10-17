// ============================================================
// Services · 取消标志（跨线程共享）
// UI 层点击"取消"→ 设置 flag；OCR/翻译线程检查此 flag 提前退出。
// ============================================================
use std::sync::atomic::{AtomicBool, Ordering};

/// 全局取消标志
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

/// 请求取消当前操作
pub fn request_cancel() {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
}

/// 检查是否已请求取消
pub fn is_cancelled() -> bool {
    CANCEL_FLAG.load(Ordering::SeqCst)
}

/// 重置取消标志（新操作开始前调用）
pub fn reset() {
    CANCEL_FLAG.store(false, Ordering::SeqCst);
}

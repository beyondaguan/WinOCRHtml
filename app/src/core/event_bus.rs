// ============================================================
// Core · 极简事件总线
// 基于 std::sync::mpsc，支持 publish / subscribe。
// Services 发布事件，UI 或其他 Services 订阅消费。
// ============================================================
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::core::types::AppEvent;

#[derive(Clone)]
#[allow(dead_code)] // v1.1: 事件总线接入（UI/Services 订阅）
pub struct EventBus {
    senders: Arc<Mutex<Vec<Sender<AppEvent>>>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    #[allow(dead_code)] // v1.1: 事件总线接入
    pub fn new() -> Self {
        Self {
            senders: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 订阅事件，返回 Receiver 供调用方 recv()
    #[allow(dead_code)] // v1.1: 事件总线接入
    pub fn subscribe(&self) -> Receiver<AppEvent> {
        let (tx, rx) = channel();
        self.senders.lock().unwrap().push(tx);
        rx
    }

    /// 广播事件给所有订阅者（断开的订阅者自动清理）
    #[allow(dead_code)] // v1.1: 事件总线接入
    pub fn publish(&self, ev: AppEvent) {
        let mut guard = self.senders.lock().unwrap();
        guard.retain(|tx| tx.send(ev.clone()).is_ok());
    }
}

//! OCR 结果缓存（§14.5 / P2-5）
//!
//! 设计：以「图片内容哈希」为 key 缓存 OCR 文本，避免同一区域重复推理。
//! - 哈希：FNV-1a 64 位（无第三方依赖，跨进程/跨版本确定）。
//! - 淘汰：内存 LRU，容量由 `ocr_cache_max` 控制（默认 500）。
//! - 持久化：`data_local_dir/WinOCR-Html/ocr_cache.json`（含命中统计），
//!   进程重启后命中率不归零；写入走 tmp + rename 原子替换。
//! - 统计：hit / miss 与命中率，供 `--bench` 与日志观测（目标 ≥ 80%）。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// 缓存文件路径
fn cache_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("WinOCR-Html").join("ocr_cache.json"))
}

/// FNV-1a 64 位：offset basis = 0xcbf29ce484222325，prime = 0x100000001b3
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// 图片内容键（十六进制，便于落盘与排查）
pub fn content_key(img_data: &[u8]) -> String {
    format!("{:016x}", fnv1a64(img_data))
}

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    entries: HashMap<String, String>,
    #[serde(default)]
    hits: u64,
    #[serde(default)]
    misses: u64,
}

struct Cache {
    map: HashMap<String, String>,
    /// LRU 顺序：队首最旧，队尾最新
    order: VecDeque<String>,
    max: usize,
    hits: u64,
    misses: u64,
    /// 距上次落盘累计写入次数
    dirty: usize,
    enabled: bool,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| {
        let persisted = load().unwrap_or_default();
        let mut order: VecDeque<String> = VecDeque::new();
        for k in persisted.entries.keys() {
            order.push_back(k.clone());
        }
        Mutex::new(Cache {
            map: persisted.entries,
            order,
            max: 500,
            hits: persisted.hits,
            misses: persisted.misses,
            dirty: 0,
            enabled: true,
        })
    })
}

/// 启动后用配置初始化（开关 / 容量）
pub fn init(enabled: bool, max: usize) {
    let mut c = cache().lock().unwrap();
    c.enabled = enabled;
    if max > 0 {
        c.max = max;
        while c.map.len() > c.max {
            if let Some(old) = c.order.pop_front() {
                c.map.remove(&old);
            } else {
                break;
            }
        }
    }
}

fn load() -> Option<Persisted> {
    let p = cache_path()?;
    let s = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str::<Persisted>(&s).ok()
}

/// 落盘（原子写：tmp + rename）
fn flush_locked(c: &mut Cache) {
    c.dirty = 0;
    let Some(p) = cache_path() else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let payload = Persisted {
        entries: c.map.clone(),
        hits: c.hits,
        misses: c.misses,
    };
    let Ok(json) = serde_json::to_string(&payload) else {
        return;
    };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &p);
}

/// 查询缓存；命中返回文本（并刷新 LRU 顺序）
pub fn get(key: &str) -> Option<String> {
    let mut c = cache().lock().unwrap();
    if !c.enabled {
        return None;
    }
    if let Some(v) = c.map.get(key).cloned() {
        c.hits += 1;
        // 移到队尾（最近使用）
        c.order.retain(|k| k != key);
        c.order.push_back(key.to_string());
        return Some(v);
    }
    c.misses += 1;
    None
}

/// 写入缓存（仅缓存非空结果，避免模型缺失时把空结果写死）
pub fn put(key: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let mut c = cache().lock().unwrap();
    if !c.enabled {
        return;
    }
    if c.map.contains_key(key) {
        c.order.retain(|k| k != key);
        c.order.push_back(key.to_string());
        return;
    }
    while c.map.len() >= c.max {
        match c.order.pop_front() {
            Some(old) => {
                c.map.remove(&old);
            }
            None => break,
        }
    }
    c.map.insert(key.to_string(), text.to_string());
    c.order.push_back(key.to_string());
    c.dirty += 1;
    if c.dirty >= 8 {
        flush_locked(&mut c);
    }
}

/// 命中统计快照
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub enabled: bool,
}

impl CacheStats {
    /// 命中率 0.0 ~ 1.0（无样本时为 0.0）
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

pub fn stats() -> CacheStats {
    let c = cache().lock().unwrap();
    CacheStats {
        hits: c.hits,
        misses: c.misses,
        entries: c.map.len(),
        enabled: c.enabled,
    }
}

/// 手动落盘（退出前 / bench 结束时调用）
pub fn flush() {
    let mut c = cache().lock().unwrap();
    flush_locked(&mut c);
}

/// 清空缓存（设置/排障用）
pub fn clear() {
    let mut c = cache().lock().unwrap();
    c.map.clear();
    c.order.clear();
    c.hits = 0;
    c.misses = 0;
    flush_locked(&mut c);
}

/// 便捷：给定图片字节取 key（供调用方复用）
pub fn key_for(img_data: &[u8]) -> String {
    content_key(img_data)
}

/// 供 bench 使用：重置统计（不动缓存内容）
pub fn reset_stats() {
    let mut c = cache().lock().unwrap();
    c.hits = 0;
    c.misses = 0;
}

/// 单元测试用：不落盘的纯函数验证
#[allow(dead_code)]
fn _assert_deterministic() -> Result<()> {
    let a = fnv1a64(b"hello");
    let b = fnv1a64(b"hello");
    assert_eq!(a, b);
    assert_ne!(fnv1a64(b"hello"), fnv1a64(b"hellp"));
    Ok(())
}

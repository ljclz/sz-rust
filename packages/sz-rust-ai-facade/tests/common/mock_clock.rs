//! Mock 时钟 — 确定性时间控制
//!
//! 替代裸 `tokio::time::sleep`（spec 5.1.1.8），通过 `advance` 推进时间。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Mock 时钟（毫秒精度）
#[derive(Debug, Clone)]
pub struct MockClock {
    now_ms: Arc<AtomicU64>,
}

impl MockClock {
    /// 创建 mock 时钟，初始时间戳为 0
    pub fn new() -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 创建 mock 时钟，指定初始时间戳
    pub fn at(initial_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(initial_ms)),
        }
    }

    /// 获取当前时间（毫秒）
    pub fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    /// 推进时间（毫秒）
    pub fn advance(&self, ms: u64) {
        self.now_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

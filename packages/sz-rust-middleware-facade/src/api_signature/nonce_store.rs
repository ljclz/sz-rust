// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Nonce 防重放存储（spec 5.12.4）

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 本地 nonce 存储
///
/// Redis 不可用时降级为本地缓存（spec 5.12.3 异常1）。
pub struct NonceStore {
    seen: Mutex<HashSet<String>>,
    window: Duration,
    start: Mutex<Instant>,
}

impl NonceStore {
    /// 创建 nonce 存储
    pub fn new(window: Duration) -> Self {
        Self {
            seen: Mutex::new(HashSet::new()),
            window,
            start: Mutex::new(Instant::now()),
        }
    }

    /// 检查并存储 nonce
    ///
    /// 返回 true 表示 nonce 可用（未重复），false 表示重复。
    pub fn check_and_store(&self, nonce: &str) -> bool {
        let mut seen = self.seen.lock();
        let mut start = self.start.lock();

        // 窗口过期 → 清空
        if start.elapsed() >= self.window {
            seen.clear();
            *start = Instant::now();
        }

        if seen.contains(nonce) {
            return false;
        }

        seen.insert(nonce.to_string());
        true
    }

    /// 当前已存储 nonce 数
    pub fn count(&self) -> usize {
        self.seen.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_first_use_ok() {
        let store = NonceStore::new(Duration::from_secs(60));
        assert!(store.check_and_store("nonce1"));
    }

    #[test]
    fn test_nonce_replay_rejected() {
        let store = NonceStore::new(Duration::from_secs(60));
        assert!(store.check_and_store("nonce1"));
        assert!(!store.check_and_store("nonce1"), "重复 nonce 应拒绝");
    }

    #[test]
    fn test_nonce_different_ok() {
        let store = NonceStore::new(Duration::from_secs(60));
        assert!(store.check_and_store("nonce1"));
        assert!(store.check_and_store("nonce2"));
    }
}

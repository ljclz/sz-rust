// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 滑动窗口限流算法（P3-3）

use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 滑动窗口限流器
///
/// 在 `window` 时间窗口内最多允许 `quota` 次请求。
/// 通过记录请求时间戳，淘汰窗口外的旧记录。
pub struct SlidingWindow {
    window: Duration,
    quota: u32,
    timestamps: Mutex<Vec<Instant>>,
}

impl SlidingWindow {
    /// 创建滑动窗口限流器
    pub fn new(quota: u32, window: Duration) -> Self {
        Self {
            window,
            quota,
            timestamps: Mutex::new(Vec::with_capacity(quota as usize)),
        }
    }

    /// 尝试获取一个请求配额
    ///
    /// 返回 `true` 表示允许，`false` 表示限流。
    pub fn try_acquire(&self) -> bool {
        let now = Instant::now();
        let mut timestamps = self.timestamps.lock();

        // 淘汰窗口外的旧时间戳
        timestamps.retain(|t| now.duration_since(*t) < self.window);

        if timestamps.len() < self.quota as usize {
            timestamps.push(now);
            true
        } else {
            false
        }
    }

    /// 当前窗口内请求数
    pub fn current_count(&self) -> usize {
        let now = Instant::now();
        let mut timestamps = self.timestamps.lock();
        timestamps.retain(|t| now.duration_since(*t) < self.window);
        timestamps.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_up_to_quota() {
        let sw = SlidingWindow::new(3, Duration::from_secs(10));
        assert!(sw.try_acquire());
        assert!(sw.try_acquire());
        assert!(sw.try_acquire());
        assert!(!sw.try_acquire(), "4th request should be rejected");
    }

    #[test]
    fn test_window_expiry() {
        let sw = SlidingWindow::new(1, Duration::from_millis(50));
        assert!(sw.try_acquire());
        std::thread::sleep(Duration::from_millis(60));
        assert!(sw.try_acquire(), "after window expiry, should allow again");
    }

    #[test]
    fn test_current_count() {
        let sw = SlidingWindow::new(5, Duration::from_secs(10));
        sw.try_acquire();
        sw.try_acquire();
        assert_eq!(sw.current_count(), 2);
    }

    #[test]
    fn test_zero_quota_rejects_all() {
        let sw = SlidingWindow::new(0, Duration::from_secs(10));
        assert!(!sw.try_acquire());
    }

    #[test]
    fn test_current_count_expires_to_zero() {
        let sw = SlidingWindow::new(3, Duration::from_millis(50));
        assert!(sw.try_acquire());
        assert!(sw.try_acquire());
        assert_eq!(sw.current_count(), 2);

        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            sw.current_count(),
            0,
            "all timestamps should be purged after window expiry"
        );
        assert!(
            sw.try_acquire(),
            "quota should be fully reclaimed after expiry"
        );
    }

    #[test]
    fn test_partial_window_keeps_active_records() {
        let sw = SlidingWindow::new(3, Duration::from_millis(100));
        assert!(sw.try_acquire());
        assert!(sw.try_acquire());

        // 未到窗口边界，记录应保留
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(sw.current_count(), 2);

        // 过半后首条记录已过期、第二条仍在窗口内
        std::thread::sleep(Duration::from_millis(40));
        let count = sw.current_count();
        assert!(
            (1..=2).contains(&count),
            "expected 1-2 active records after partial expiry, got {count}"
        );
    }
}

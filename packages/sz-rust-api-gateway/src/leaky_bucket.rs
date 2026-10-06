// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 漏桶限流算法（P3-3）

use std::time::Instant;

use parking_lot::Mutex;

/// 漏桶状态
struct LeakyBucketState {
    /// 当前水量
    water: f64,
    /// 上次漏水时间
    last_leak: Instant,
}

/// 漏桶限流器
///
/// 以恒定速率 `leak_rate`（滴/秒）漏水，容量为 `capacity`。
/// 请求到达时加一滴水，溢出则拒绝。
pub struct LeakyBucket {
    capacity: f64,
    leak_rate: f64,
    state: Mutex<LeakyBucketState>,
}

impl LeakyBucket {
    /// 创建漏桶限流器
    ///
    /// - `capacity`：桶容量
    /// - `leak_rate`：漏水速率（滴/秒）
    pub fn new(capacity: u32, leak_rate: f64) -> Self {
        Self {
            capacity: capacity as f64,
            leak_rate,
            state: Mutex::new(LeakyBucketState {
                water: 0.0,
                last_leak: Instant::now(),
            }),
        }
    }

    /// 尝试获取一个请求配额
    ///
    /// 返回 `true` 表示允许，`false` 表示限流。
    pub fn try_acquire(&self) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock();

        // 计算漏水量
        let elapsed = now.duration_since(state.last_leak).as_secs_f64();
        let leaked = elapsed * self.leak_rate;
        state.water = (state.water - leaked).max(0.0);
        state.last_leak = now;

        if state.water + 1.0 <= self.capacity {
            state.water += 1.0;
            true
        } else {
            false
        }
    }

    /// 当前水量
    pub fn current_water(&self) -> f64 {
        let now = Instant::now();
        let mut state = self.state.lock();
        let elapsed = now.duration_since(state.last_leak).as_secs_f64();
        let leaked = elapsed * self.leak_rate;
        state.water = (state.water - leaked).max(0.0);
        state.last_leak = now;
        state.water
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_allows_burst_up_to_capacity() {
        let lb = LeakyBucket::new(3, 1.0);
        assert!(lb.try_acquire());
        assert!(lb.try_acquire());
        assert!(lb.try_acquire());
        assert!(!lb.try_acquire(), "4th should be rejected");
    }

    #[test]
    fn test_leaks_over_time() {
        let lb = LeakyBucket::new(1, 10.0); // 10 drops/sec
        assert!(lb.try_acquire());
        assert!(!lb.try_acquire(), "bucket full");
        std::thread::sleep(Duration::from_millis(150));
        assert!(lb.try_acquire(), "after leak, should allow");
    }

    #[test]
    fn test_current_water_decreases() {
        let lb = LeakyBucket::new(5, 100.0);
        lb.try_acquire();
        lb.try_acquire();
        let w1 = lb.current_water();
        std::thread::sleep(Duration::from_millis(10));
        let w2 = lb.current_water();
        assert!(w2 < w1, "water should decrease over time");
    }

    #[test]
    fn test_current_water_reaches_zero_after_long_sleep() {
        // 容量 5、速率 100 滴/秒：注水 2 滴后等 50ms，应漏完 5 滴 → 水量 0。
        // `elapsed * leak_rate` 的 `*`→`/` 与 `water - leaked` 的 `-`→`/`
        // 变异体都会产生非零水量，从而被杀死。
        let lb = LeakyBucket::new(5, 100.0);
        lb.try_acquire();
        lb.try_acquire();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(lb.current_water(), 0.0, "50ms 后水量应完全漏完为 0");
    }
}

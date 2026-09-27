// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 慢调用熔断器（P3-3）
//!
//! 慢调用率超阈值触发熔断 + 半开探测限量 + 降级响应。

use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::error::GatewayError;

/// 慢调用熔断配置
#[derive(Debug, Clone)]
pub struct SlowCallBreakerConfig {
    /// 慢调用阈值（毫秒）
    pub slow_call_threshold_ms: u64,
    /// 慢调用率阈值（0~1）
    pub slow_call_rate_threshold: f32,
    /// 统计窗口大小
    pub window: Duration,
    /// 最少请求数才计算慢调用率
    pub min_requests: u32,
    /// 半开探测限量（≤10）
    pub max_half_open_probes: u8,
    /// 熔断恢复时间
    pub recovery_timeout: Duration,
}

impl Default for SlowCallBreakerConfig {
    fn default() -> Self {
        Self {
            slow_call_threshold_ms: 500,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 5,
            max_half_open_probes: 5,
            recovery_timeout: Duration::from_secs(30),
        }
    }
}

impl SlowCallBreakerConfig {
    /// 创建配置，自动钳制参数
    pub fn new(slow_call_threshold_ms: u64, slow_call_rate_threshold: f32) -> Self {
        Self {
            slow_call_threshold_ms,
            slow_call_rate_threshold: slow_call_rate_threshold.clamp(0.0, 1.0),
            ..Self::default()
        }
    }
}

/// 半开探测限量
#[derive(Debug, Clone)]
pub struct HalfOpenProbeLimit {
    /// 最大探测请求数（≤10）
    pub max_probes: u8,
}

impl HalfOpenProbeLimit {
    /// 创建半开探测限量，钳制到 ≤10
    pub fn new(max_probes: u8) -> Self {
        Self {
            max_probes: max_probes.min(10),
        }
    }
}

/// 熔断状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// 关闭（正常放行）
    Closed,
    /// 打开（拒绝所有请求）
    Open,
    /// 半开（限量探测）
    HalfOpen,
}

struct BreakerInner {
    state: BreakerState,
    /// 窗口内的请求记录：(时间, 是否慢调用)
    records: Vec<(Instant, bool)>,
    /// 熔断打开时间
    opened_at: Option<Instant>,
    /// 半开探测计数
    half_open_probes: u8,
    /// 半开探测成功数
    half_open_successes: u8,
}

/// 慢调用熔断器
///
/// 基于慢调用率触发熔断。当窗口内慢调用率超过阈值时打开熔断，
/// 经过恢复时间后进入半开状态，限量探测请求。
pub struct SlowCallBreaker {
    config: SlowCallBreakerConfig,
    inner: Mutex<BreakerInner>,
}

impl SlowCallBreaker {
    /// 创建慢调用熔断器
    pub fn new(config: SlowCallBreakerConfig) -> Self {
        Self {
            config,
            inner: Mutex::new(BreakerInner {
                state: BreakerState::Closed,
                records: Vec::new(),
                opened_at: None,
                half_open_probes: 0,
                half_open_successes: 0,
            }),
        }
    }

    /// 获取当前熔断状态
    pub fn state(&self) -> BreakerState {
        self.inner.lock().state
    }

    /// 检查是否允许请求
    ///
    /// - `Closed` → 放行
    /// - `Open` → 检查恢复时间，到期转 `HalfOpen`
    /// - `HalfOpen` → 限量探测（≤ `max_half_open_probes`）
    pub fn can_request(&self) -> Result<(), GatewayError> {
        let now = Instant::now();
        let mut inner = self.inner.lock();

        match inner.state {
            BreakerState::Closed => Ok(()),
            BreakerState::Open => {
                if let Some(opened_at) = inner.opened_at {
                    if now.duration_since(opened_at) >= self.config.recovery_timeout {
                        inner.state = BreakerState::HalfOpen;
                        inner.half_open_probes = 1;
                        inner.half_open_successes = 0;
                        return Ok(());
                    }
                }
                Err(GatewayError::CircuitBroken("circuit open".to_string()))
            }
            BreakerState::HalfOpen => {
                if inner.half_open_probes < self.config.max_half_open_probes {
                    inner.half_open_probes += 1;
                    Ok(())
                } else {
                    Err(GatewayError::CircuitBroken(
                        "half-open probe limit reached".to_string(),
                    ))
                }
            }
        }
    }

    /// 记算慢调用率
    fn slow_call_rate(records: &[(Instant, bool)]) -> f32 {
        let total = records.len();
        if total == 0 {
            return 0.0;
        }
        let slow = records.iter().filter(|(_, is_slow)| *is_slow).count();
        slow as f32 / total as f32
    }

    /// 计算总请求数
    pub fn total_requests(&self) -> usize {
        self.inner.lock().records.len()
    }

    /// 计算慢请求数
    pub fn slow_requests(&self) -> usize {
        self.inner
            .lock()
            .records
            .iter()
            .filter(|(_, is_slow)| *is_slow)
            .count()
    }

    /// 记录请求延迟并更新熔断状态
    ///
    /// - `Closed`：记录延迟，若慢调用率超阈值且请求数 ≥ min_requests → 打开熔断
    /// - `HalfOpen`：快速调用 → 探测成功；慢调用 → 探测失败重新打开
    /// - `Open`：忽略
    pub fn record_latency(&self, latency: Duration) {
        let now = Instant::now();
        let is_slow = latency.as_millis() > self.config.slow_call_threshold_ms as u128;
        let mut inner = self.inner.lock();

        match inner.state {
            BreakerState::Closed => {
                inner.records.push((now, is_slow));
                inner
                    .records
                    .retain(|(t, _)| now.duration_since(*t) < self.config.window);

                if inner.records.len() >= self.config.min_requests as usize {
                    let rate = Self::slow_call_rate(&inner.records);
                    if rate > self.config.slow_call_rate_threshold {
                        inner.state = BreakerState::Open;
                        inner.opened_at = Some(now);
                        inner.records.clear();
                    }
                }
            }
            BreakerState::HalfOpen => {
                if is_slow {
                    inner.state = BreakerState::Open;
                    inner.opened_at = Some(now);
                    inner.half_open_probes = 0;
                    inner.half_open_successes = 0;
                } else {
                    inner.half_open_successes += 1;
                    if inner.half_open_successes >= self.config.max_half_open_probes {
                        inner.state = BreakerState::Closed;
                        inner.records.clear();
                        inner.opened_at = None;
                        inner.half_open_probes = 0;
                        inner.half_open_successes = 0;
                    }
                }
            }
            BreakerState::Open => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_clamp() {
        let cfg = SlowCallBreakerConfig::new(100, 1.5);
        assert!((cfg.slow_call_rate_threshold - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_half_open_probe_limit_max_10() {
        let limit = HalfOpenProbeLimit::new(20);
        assert_eq!(limit.max_probes, 10);
    }

    #[test]
    fn test_breaker_starts_closed() {
        let breaker = SlowCallBreaker::new(SlowCallBreakerConfig::default());
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.can_request().is_ok());
    }

    #[test]
    fn test_breaker_opens_on_high_slow_rate() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 4,
            max_half_open_probes: 3,
            recovery_timeout: Duration::from_millis(50),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(50));

        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(breaker.can_request().is_err());
    }

    #[test]
    fn test_breaker_stays_closed_below_threshold() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 4,
            max_half_open_probes: 3,
            recovery_timeout: Duration::from_millis(50),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(50));
        breaker.record_latency(Duration::from_millis(50));
        breaker.record_latency(Duration::from_millis(50));

        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.can_request().is_ok());
    }

    #[test]
    fn test_breaker_recovery_to_half_open() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 2,
            max_half_open_probes: 3,
            recovery_timeout: Duration::from_millis(20),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(200));
        assert_eq!(breaker.state(), BreakerState::Open);

        std::thread::sleep(Duration::from_millis(30));
        assert!(
            breaker.can_request().is_ok(),
            "should transition to half-open"
        );
        assert_eq!(breaker.state(), BreakerState::HalfOpen);
    }

    #[test]
    fn test_half_open_probe_limit() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 2,
            max_half_open_probes: 2,
            recovery_timeout: Duration::from_millis(10),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(200));
        assert_eq!(breaker.state(), BreakerState::Open);

        std::thread::sleep(Duration::from_millis(15));
        assert!(breaker.can_request().is_ok(), "probe 1");
        assert!(breaker.can_request().is_ok(), "probe 2");
        assert!(breaker.can_request().is_err(), "probe 3 should be rejected");
    }

    #[test]
    fn test_half_open_success_closes_breaker() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 2,
            max_half_open_probes: 3,
            recovery_timeout: Duration::from_millis(10),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(200));
        assert_eq!(breaker.state(), BreakerState::Open);

        std::thread::sleep(Duration::from_millis(15));
        breaker.can_request().unwrap();
        breaker.record_latency(Duration::from_millis(50));
        breaker.can_request().unwrap();
        breaker.record_latency(Duration::from_millis(50));
        breaker.can_request().unwrap();
        breaker.record_latency(Duration::from_millis(50));

        assert_eq!(
            breaker.state(),
            BreakerState::Closed,
            "all probes fast → close"
        );
    }

    #[test]
    fn test_half_open_failure_reopens_breaker() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 2,
            max_half_open_probes: 3,
            recovery_timeout: Duration::from_millis(10),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(200));
        assert_eq!(breaker.state(), BreakerState::Open);

        std::thread::sleep(Duration::from_millis(15));
        breaker.can_request().unwrap();
        breaker.record_latency(Duration::from_millis(500));

        assert_eq!(breaker.state(), BreakerState::Open, "slow probe → reopen");
    }

    #[test]
    fn test_min_requests_guard() {
        let cfg = SlowCallBreakerConfig {
            slow_call_threshold_ms: 100,
            slow_call_rate_threshold: 0.5,
            window: Duration::from_secs(10),
            min_requests: 10,
            max_half_open_probes: 3,
            recovery_timeout: Duration::from_millis(50),
        };
        let breaker = SlowCallBreaker::new(cfg);

        breaker.record_latency(Duration::from_millis(500));
        breaker.record_latency(Duration::from_millis(500));
        assert_eq!(
            breaker.state(),
            BreakerState::Closed,
            "should not open with < min_requests"
        );
    }

    #[test]
    fn test_slow_request_count() {
        let cfg = SlowCallBreakerConfig::new(100, 0.5);
        let breaker = SlowCallBreaker::new(cfg);
        breaker.record_latency(Duration::from_millis(200));
        breaker.record_latency(Duration::from_millis(50));
        breaker.record_latency(Duration::from_millis(200));
        assert_eq!(breaker.total_requests(), 3);
        assert_eq!(breaker.slow_requests(), 2);
    }
}

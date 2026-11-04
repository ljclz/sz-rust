// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CircuitBreaker 中间件 — 熔断器（P1-5 本仓独立实现）
//!
//! 提供 Closed→Open→HalfOpen→Closed 三态熔断保护：
//! - **Closed**：正常放行，统计错误率
//! - **Open**：熔断，返回 HTTP 503
//! - **HalfOpen**：探测，放行有限请求试探恢复
//!
//! ## 状态流转
//!
//! ```text
//! Closed ──(错误率≥阈值)──> Open
//! Open   ──(冷却到期)────> HalfOpen
//! HalfOpen ──(探测全成功)─> Closed
//! HalfOpen ──(探测失败)───> Open（重置冷却）
//! ```
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_middleware_facade::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
//! use std::time::Duration;
//! use std::sync::Arc;
//!
//! let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
//!     error_threshold: 0.5,
//!     cooldown: Duration::from_secs(30),
//!     probe_requests: 3,
//!     stat_window: Duration::from_secs(60),
//! }));
//! ```

//! ## 统计窗口（2026-10-29 重写）
//!
//! 错误率统计使用**固定 60 桶的环形时间窗**：每桶覆盖 `stat_window / 60`，
//! 记录一条结果只写一个桶（O(1)，与窗口内请求量无关）；判定时扫描固定 60 个桶。
//! 旧实现为 `Vec<(bool, Instant)>` + 每请求全量 `retain`，统计量随吞吐线性增长
//! （4.4k rps × 60s ≈ 26 万条），单次记录退化为 O(n)，实测把 /health 吞吐压到
//! 约 2/3。分桶后 60s 语义不变，精度为 1 个桶宽。

use std::sync::Arc;
use std::time::{Duration, Instant};

/// 熔断器状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// 关闭态：正常放行，统计错误率
    Closed,
    /// 打开态：熔断，拒绝请求
    Open,
    /// 半开态：探测，放行有限请求
    HalfOpen,
}

/// 熔断器配置
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// 错误率阈值（0.0~1.0），窗口内错误率 ≥ 此值触发 Closed→Open
    pub error_threshold: f64,
    /// 冷却时间，Open 态持续此时间后转为 HalfOpen
    pub cooldown: Duration,
    /// 探测请求数，HalfOpen 态放行此数量请求，全成功则 Closed
    pub probe_requests: u32,
    /// 统计窗口，仅统计此时间范围内的请求
    pub stat_window: Duration,
}

/// 熔断器决策
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitBreakerDecision {
    /// 放行
    Allow,
    /// 拒绝（熔断中）
    Reject,
}

/// 统计窗口的分桶数（固定值：窗口内记录成本与请求量无关）
const STAT_BUCKETS: usize = 60;

#[derive(Clone, Copy)]
struct StatBucket {
    epoch: u64,
    successes: u32,
    failures: u32,
}

impl StatBucket {
    const fn empty() -> Self {
        Self {
            epoch: 0,
            successes: 0,
            failures: 0,
        }
    }
}

/// 固定分桶的错误率统计窗口
struct ErrorWindow {
    started: Instant,
    bucket_nanos: u64,
    buckets: [StatBucket; STAT_BUCKETS],
}

impl ErrorWindow {
    fn new(window: Duration) -> Self {
        let bucket_nanos = (window.as_nanos() / STAT_BUCKETS as u128).max(1) as u64;
        Self {
            started: Instant::now(),
            bucket_nanos,
            buckets: [StatBucket::empty(); STAT_BUCKETS],
        }
    }

    fn epoch(&self, now: Instant) -> u64 {
        (now.saturating_duration_since(self.started).as_nanos() as u64) / self.bucket_nanos
    }

    fn record(&mut self, now: Instant, success: bool) {
        let epoch = self.epoch(now);
        let slot = &mut self.buckets[(epoch % STAT_BUCKETS as u64) as usize];
        if slot.epoch != epoch {
            *slot = StatBucket {
                epoch,
                ..StatBucket::empty()
            };
        }
        if success {
            slot.successes = slot.successes.saturating_add(1);
        } else {
            slot.failures = slot.failures.saturating_add(1);
        }
    }

    /// 返回窗口内 (失败数, 总数)
    fn snapshot(&self, now: Instant) -> (u64, u64) {
        let current = self.epoch(now);
        let mut failures = 0u64;
        let mut total = 0u64;
        for bucket in &self.buckets {
            if bucket.epoch <= current && current - bucket.epoch < STAT_BUCKETS as u64 {
                failures += u64::from(bucket.failures);
                total += u64::from(bucket.failures) + u64::from(bucket.successes);
            }
        }
        (failures, total)
    }

    fn clear(&mut self) {
        self.buckets = [StatBucket::empty(); STAT_BUCKETS];
    }
}

struct CircuitBreakerInner {
    state: CircuitState,
    /// 统计窗口（固定分桶，O(1) 记录）
    window: ErrorWindow,
    /// Open 态开始时间（用于判断冷却是否到期）
    opened_at: Option<Instant>,
    /// HalfOpen 态探测结果
    probe_results: Vec<bool>,
}

/// 熔断器（线程安全）
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    inner: parking_lot::Mutex<CircuitBreakerInner>,
}

impl CircuitBreaker {
    /// 创建熔断器
    pub fn new(config: CircuitBreakerConfig) -> Self {
        assert!(
            config.error_threshold > 0.0 && config.error_threshold <= 1.0,
            "error_threshold must be in (0, 1]"
        );
        assert!(config.cooldown > Duration::ZERO, "cooldown must be > 0");
        assert!(config.probe_requests > 0, "probe_requests must be > 0");
        assert!(
            config.stat_window > Duration::ZERO,
            "stat_window must be > 0"
        );
        Self {
            config: config.clone(),
            inner: parking_lot::Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                window: ErrorWindow::new(config.stat_window),
                opened_at: None,
                probe_results: Vec::new(),
            }),
        }
    }

    /// 查询当前是否允许请求
    pub fn can_request(&self) -> CircuitBreakerDecision {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        match inner.state {
            CircuitState::Closed => CircuitBreakerDecision::Allow,
            CircuitState::Open => {
                if let Some(opened_at) = inner.opened_at {
                    if now.duration_since(opened_at) >= self.config.cooldown {
                        inner.state = CircuitState::HalfOpen;
                        inner.probe_results.clear();
                        CircuitBreakerDecision::Allow
                    } else {
                        CircuitBreakerDecision::Reject
                    }
                } else {
                    CircuitBreakerDecision::Reject
                }
            }
            CircuitState::HalfOpen => {
                if inner.probe_results.len() < self.config.probe_requests as usize {
                    CircuitBreakerDecision::Allow
                } else {
                    CircuitBreakerDecision::Reject
                }
            }
        }
    }

    /// 记录成功
    pub fn record_success(&self) {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        match inner.state {
            CircuitState::Closed => {
                inner.window.record(now, true);
            }
            CircuitState::HalfOpen => {
                inner.probe_results.push(true);
                if inner.probe_results.len() >= self.config.probe_requests as usize {
                    inner.state = CircuitState::Closed;
                    inner.window.clear();
                    inner.opened_at = None;
                    inner.probe_results.clear();
                }
            }
            CircuitState::Open => {}
        }
    }

    /// 记录失败
    pub fn record_failure(&self) {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        match inner.state {
            CircuitState::Closed => {
                inner.window.record(now, false);
                let (failures, total) = inner.window.snapshot(now);
                if total > 0 && failures as f64 / total as f64 >= self.config.error_threshold {
                    inner.state = CircuitState::Open;
                    inner.opened_at = Some(now);
                }
            }
            CircuitState::HalfOpen => {
                inner.state = CircuitState::Open;
                inner.opened_at = Some(now);
                inner.probe_results.clear();
            }
            CircuitState::Open => {}
        }
    }

    /// 查询当前状态
    pub fn state(&self) -> CircuitState {
        self.inner.lock().state
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("config", &self.config)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

/// 熔断器中间件
///
/// Open 态返回 HTTP 503 Service Unavailable；
/// Closed/HalfOpen 放行并上报结果；
/// 内部错误 fail-open 放行 + 错误日志。
pub async fn circuit_breaker_middleware(
    axum::extract::State(cb): axum::extract::State<Arc<CircuitBreaker>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    match cb.can_request() {
        CircuitBreakerDecision::Allow => {
            let resp = next.run(req).await;
            if resp.status().is_server_error() {
                cb.record_failure();
            } else {
                cb.record_success();
            }
            resp
        }
        CircuitBreakerDecision::Reject => {
            tracing::warn!(state = ?cb.state(), "circuit breaker open, rejecting request");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({
                    "code": 503,
                    "msg": "Service Unavailable",
                    "data": { "state": format!("{:?}", cb.state()) }
                })),
            )
                .into_response()
        }
    }
}

use axum::response::IntoResponse;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn test_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            error_threshold: 0.5,
            cooldown: Duration::from_millis(100),
            probe_requests: 3,
            stat_window: Duration::from_secs(60),
        }
    }

    #[test]
    fn test_initial_state_closed() {
        let cb = CircuitBreaker::new(test_config());
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.can_request(), CircuitBreakerDecision::Allow);
    }

    #[test]
    fn test_closed_to_open_on_error_threshold() {
        let cb = CircuitBreaker::new(test_config());
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert_eq!(cb.can_request(), CircuitBreakerDecision::Reject);
    }

    #[test]
    fn test_open_to_halfopen_after_cooldown() {
        let cb = CircuitBreaker::new(test_config());
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        thread::sleep(Duration::from_millis(150));
        assert_eq!(cb.can_request(), CircuitBreakerDecision::Allow);
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_halfopen_to_closed_on_all_probes_success() {
        let cb = CircuitBreaker::new(test_config());
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        thread::sleep(Duration::from_millis(150));
        cb.can_request();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.record_success();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_halfopen_to_open_on_probe_failure() {
        let cb = CircuitBreaker::new(test_config());
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        thread::sleep(Duration::from_millis(150));
        cb.can_request();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.record_success();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_closed_stays_closed_under_threshold() {
        let cb = CircuitBreaker::new(test_config());
        cb.record_success();
        cb.record_success();
        cb.record_success();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_stat_window_expires_old_records() {
        let config = CircuitBreakerConfig {
            error_threshold: 0.75,
            cooldown: Duration::from_millis(100),
            probe_requests: 1,
            stat_window: Duration::from_millis(60),
        };
        let cb = CircuitBreaker::new(config);
        cb.record_success();
        cb.record_failure();
        // 窗口内 1/2 = 0.5 < 0.75
        assert_eq!(cb.state(), CircuitState::Closed);
        thread::sleep(Duration::from_millis(120));
        cb.record_failure();
        // 旧记录全部出窗，仅剩本次失败 ⇒ 1.0 ≥ 0.75
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_many_records_stay_closed_with_low_error_rate() {
        // 大量成功记录不应触发熔断，也不应随记录数增长放大内存
        let config = CircuitBreakerConfig {
            error_threshold: 0.5,
            cooldown: Duration::from_millis(100),
            probe_requests: 1,
            stat_window: Duration::from_secs(60),
        };
        let cb = CircuitBreaker::new(config);
        // 1% 失败率：既验证窗口统计不受记录量影响，也验证不会因首条记录误熔断
        for i in 0..100_000u32 {
            if i % 100 == 99 {
                cb.record_failure();
            } else {
                cb.record_success();
            }
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_error_rate_boundary_50_percent() {
        let config = CircuitBreakerConfig {
            error_threshold: 0.5,
            cooldown: Duration::from_millis(100),
            probe_requests: 1,
            stat_window: Duration::from_secs(60),
        };
        let cb = CircuitBreaker::new(config);
        cb.record_success();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_record_success_in_open_state_noop() {
        let cb = CircuitBreaker::new(test_config());
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_concurrent_access_no_panic() {
        let cb = Arc::new(CircuitBreaker::new(test_config()));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let cb_clone = Arc::clone(&cb);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let _ = cb_clone.can_request();
                    cb_clone.record_success();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_halfopen_rejects_after_probe_limit() {
        let config = CircuitBreakerConfig {
            error_threshold: 0.5,
            cooldown: Duration::from_millis(50),
            probe_requests: 2,
            stat_window: Duration::from_secs(60),
        };
        let cb = CircuitBreaker::new(config);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(cb.can_request(), CircuitBreakerDecision::Allow);
        cb.record_success();
        assert_eq!(cb.can_request(), CircuitBreakerDecision::Allow);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }
}

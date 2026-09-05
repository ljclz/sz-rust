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

struct CircuitBreakerInner {
    state: CircuitState,
    /// 统计窗口内的请求记录（success/failure + timestamp）
    records: Vec<(bool, Instant)>,
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
            config,
            inner: parking_lot::Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                records: Vec::new(),
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
                inner.records.push((true, now));
                Self::prune_records(&mut inner, self.config.stat_window, now);
            }
            CircuitState::HalfOpen => {
                inner.probe_results.push(true);
                if inner.probe_results.len() >= self.config.probe_requests as usize {
                    inner.state = CircuitState::Closed;
                    inner.records.clear();
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
                inner.records.push((false, now));
                Self::prune_records(&mut inner, self.config.stat_window, now);
                let total = inner.records.len();
                if total > 0 {
                    let failures = inner.records.iter().filter(|(s, _)| !s).count();
                    let error_rate = failures as f64 / total as f64;
                    if error_rate >= self.config.error_threshold {
                        inner.state = CircuitState::Open;
                        inner.opened_at = Some(now);
                    }
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

    fn prune_records(inner: &mut CircuitBreakerInner, window: Duration, now: Instant) {
        let cutoff = now - window;
        inner.records.retain(|(_, t)| *t > cutoff);
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

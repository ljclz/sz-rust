// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 网关中间件（T028）
//!
//! 鉴权/限流/熔断检查。可桥接到 `middleware-facade` 的对应中间件。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::config::{AuthConfig, CircuitBreakerConfig, RateLimitConfig};
use crate::error::GatewayError;

/// 鉴权检查结果
#[derive(Debug, Clone)]
pub struct AuthResult {
    pub authorized: bool,
    pub user_id: Option<String>,
    pub user_context: HashMap<String, String>,
}

/// 鉴权中间件
pub struct AuthMiddleware {
    config: AuthConfig,
}

impl AuthMiddleware {
    /// 创建鉴权中间件
    pub fn new(config: AuthConfig) -> Self {
        Self { config }
    }

    /// 检查请求是否需要鉴权
    pub fn requires_auth(&self, path: &str) -> bool {
        if !self.config.enabled {
            return false;
        }
        !self.config.public_paths.iter().any(|p| path.starts_with(p))
    }

    /// 执行鉴权检查
    pub fn check(
        &self,
        path: &str,
        headers: &HashMap<String, String>,
    ) -> Result<AuthResult, GatewayError> {
        if !self.requires_auth(path) {
            return Ok(AuthResult {
                authorized: true,
                user_id: None,
                user_context: HashMap::new(),
            });
        }

        let token = headers
            .get(&self.config.token_header)
            .ok_or_else(|| GatewayError::Unauthorized("missing token".into()))?;

        let user_id = Self::extract_user_id(token)
            .ok_or_else(|| GatewayError::Unauthorized("invalid token".into()))?;

        let mut user_context = HashMap::new();
        user_context.insert("user_id".to_string(), user_id.clone());
        user_context.insert("X-User-Id".to_string(), user_id.clone());

        Ok(AuthResult {
            authorized: true,
            user_id: Some(user_id),
            user_context,
        })
    }

    /// 从 Token 提取用户 ID（简化版，生产环境应桥接到 middleware-facade auth）
    fn extract_user_id(token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        if let Some(bearer) = token.strip_prefix("Bearer ") {
            if !bearer.is_empty() {
                return Some(format!("user-{}", &bearer[..bearer.len().min(8)]));
            }
        }
        None
    }
}

/// 限流中间件（令牌桶）
pub struct RateLimitMiddleware {
    config: RateLimitConfig,
    buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
}

impl RateLimitMiddleware {
    /// 创建限流中间件
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 检查请求是否允许通过
    pub fn check(&self, key: &str) -> Result<(), GatewayError> {
        if !self.config.enabled {
            return Ok(());
        }

        let mut buckets = self.buckets.write();
        let bucket = buckets.entry(key.to_string()).or_insert_with(|| {
            TokenBucket::new(self.config.burst, self.config.requests_per_second)
        });

        if bucket.try_consume() {
            Ok(())
        } else {
            Err(GatewayError::RateLimited(format!("key: {key}")))
        }
    }
}

/// 令牌桶
struct TokenBucket {
    capacity: u32,
    tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: u32, refill_per_sec: u32) -> Self {
        Self {
            capacity,
            tokens: capacity as f64,
            refill_rate: refill_per_sec as f64,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity as f64);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// 熔断状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// 熔断中间件
pub struct CircuitBreakerMiddleware {
    config: CircuitBreakerConfig,
    failure_count: AtomicU64,
    state: RwLock<CircuitState>,
    opened_at: RwLock<Option<Instant>>,
}

impl CircuitBreakerMiddleware {
    /// 创建熔断中间件
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            failure_count: AtomicU64::new(0),
            state: RwLock::new(CircuitState::Closed),
            opened_at: RwLock::new(None),
        }
    }

    /// 检查是否允许请求
    pub fn can_request(&self) -> Result<(), GatewayError> {
        if !self.config.enabled {
            return Ok(());
        }

        let state = *self.state.read();
        match state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                let opened_at = *self.opened_at.read();
                if let Some(t) = opened_at {
                    if t.elapsed() > Duration::from_secs(self.config.reset_timeout_secs) {
                        *self.state.write() = CircuitState::HalfOpen;
                        return Ok(());
                    }
                }
                Err(GatewayError::CircuitBroken("circuit is open".into()))
            }
            CircuitState::HalfOpen => Ok(()),
        }
    }

    /// 记录成功
    pub fn record_success(&self) {
        if !self.config.enabled {
            return;
        }
        self.failure_count.store(0, Ordering::Relaxed);
        *self.state.write() = CircuitState::Closed;
        *self.opened_at.write() = None;
    }

    /// 记录失败
    pub fn record_failure(&self) {
        if !self.config.enabled {
            return;
        }
        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count >= self.config.failure_threshold as u64 {
            *self.state.write() = CircuitState::Open;
            *self.opened_at.write() = Some(Instant::now());
        }
    }

    /// 获取当前状态
    pub fn state(&self) -> CircuitState {
        *self.state.read()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_public_path() {
        let auth = AuthMiddleware::new(AuthConfig::default());
        assert!(!auth.requires_auth("/health"));
        assert!(!auth.requires_auth("/metrics"));
        assert!(auth.requires_auth("/api/users"));
    }

    #[test]
    fn test_auth_disabled() {
        let config = AuthConfig {
            enabled: false,
            ..Default::default()
        };
        let auth = AuthMiddleware::new(config);
        assert!(!auth.requires_auth("/api/secure"));
    }

    #[test]
    fn test_auth_missing_token() {
        let auth = AuthMiddleware::new(AuthConfig::default());
        let headers = HashMap::new();
        let result = auth.check("/api/users", &headers);
        assert!(matches!(result, Err(GatewayError::Unauthorized(_))));
    }

    #[test]
    fn test_auth_valid_token() {
        let auth = AuthMiddleware::new(AuthConfig::default());
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer abc12345".to_string());
        let result = auth.check("/api/users", &headers).unwrap();
        assert!(result.authorized);
        assert!(result.user_id.is_some());
        assert!(result.user_context.contains_key("X-User-Id"));
    }

    #[test]
    fn test_auth_invalid_token() {
        let auth = AuthMiddleware::new(AuthConfig::default());
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "InvalidToken".to_string());
        let result = auth.check("/api/users", &headers);
        assert!(matches!(result, Err(GatewayError::Unauthorized(_))));
    }

    #[test]
    fn test_auth_empty_bearer() {
        let auth = AuthMiddleware::new(AuthConfig::default());
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer ".to_string());
        let result = auth.check("/api/users", &headers);
        assert!(matches!(result, Err(GatewayError::Unauthorized(_))));
    }

    #[test]
    fn test_rate_limit_allows_within_capacity() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 100,
            burst: 5,
        };
        let rl = RateLimitMiddleware::new(config);
        for _ in 0..5 {
            assert!(rl.check("client-1").is_ok());
        }
    }

    #[test]
    fn test_rate_limit_blocks_over_capacity() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 1,
            burst: 2,
        };
        let rl = RateLimitMiddleware::new(config);
        assert!(rl.check("c1").is_ok());
        assert!(rl.check("c1").is_ok());
        assert!(matches!(rl.check("c1"), Err(GatewayError::RateLimited(_))));
    }

    #[test]
    fn test_rate_limit_disabled() {
        let config = RateLimitConfig {
            enabled: false,
            requests_per_second: 0,
            burst: 0,
        };
        let rl = RateLimitMiddleware::new(config);
        for _ in 0..100 {
            assert!(rl.check("c1").is_ok());
        }
    }

    #[test]
    fn test_rate_limit_different_keys_independent() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 1,
            burst: 1,
        };
        let rl = RateLimitMiddleware::new(config);
        assert!(rl.check("c1").is_ok());
        assert!(rl.check("c2").is_ok());
        assert!(matches!(rl.check("c1"), Err(GatewayError::RateLimited(_))));
    }

    #[test]
    fn test_circuit_breaker_closed_allows() {
        let cb = CircuitBreakerMiddleware::new(CircuitBreakerConfig::default());
        assert!(cb.can_request().is_ok());
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let config = CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 3,
            reset_timeout_secs: 30,
        };
        let cb = CircuitBreakerMiddleware::new(config);
        cb.record_failure();
        cb.record_failure();
        assert!(cb.can_request().is_ok());
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(matches!(
            cb.can_request(),
            Err(GatewayError::CircuitBroken(_))
        ));
    }

    #[test]
    fn test_circuit_breaker_success_resets() {
        let config = CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 3,
            reset_timeout_secs: 30,
        };
        let cb = CircuitBreakerMiddleware::new(config);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert!(cb.can_request().is_ok(), "failure count should be reset");
    }

    #[test]
    fn test_circuit_breaker_disabled() {
        let config = CircuitBreakerConfig {
            enabled: false,
            failure_threshold: 0,
            reset_timeout_secs: 0,
        };
        let cb = CircuitBreakerMiddleware::new(config);
        for _ in 0..100 {
            cb.record_failure();
            assert!(cb.can_request().is_ok());
        }
    }

    #[test]
    fn test_circuit_breaker_half_open_after_timeout() {
        let config = CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 1,
            reset_timeout_secs: 0,
        };
        let cb = CircuitBreakerMiddleware::new(config);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(10));
        assert!(cb.can_request().is_ok());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }
}

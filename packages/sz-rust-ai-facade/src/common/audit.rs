// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub rps: u32,
    pub burst: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self { rps: 10, burst: 20 }
    }
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    rps: f64,
    capacity: f64,
}

impl TokenBucket {
    fn new(rps: u32, burst: u32) -> Self {
        Self {
            tokens: burst as f64,
            last_refill: Instant::now(),
            rps: rps as f64,
            capacity: burst as f64,
        }
    }

    fn try_acquire(&mut self) -> Option<u64> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rps).min(self.capacity);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None
        } else {
            let needed = 1.0 - self.tokens;
            let retry_after_ms = (needed / self.rps * 1000.0).ceil() as u64;
            Some(retry_after_ms)
        }
    }

    fn update_config(&mut self, rps: u32, burst: u32) {
        self.rps = rps as f64;
        self.capacity = burst as f64;
        self.tokens = self.tokens.min(self.capacity);
    }
}

pub struct AuditHttpClient {
    client: reqwest::Client,
    rate_limit: Arc<RwLock<RateLimitConfig>>,
    buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
}

impl AuditHttpClient {
    pub fn new(client: reqwest::Client, rate_limit: RateLimitConfig) -> Self {
        Self {
            client,
            rate_limit: Arc::new(RwLock::new(rate_limit)),
            buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn rate_limit_config(&self) -> RateLimitConfig {
        self.rate_limit.read().clone()
    }

    pub fn update_rate_limit(&self, config: RateLimitConfig) {
        {
            let mut rl = self.rate_limit.write();
            *rl = config.clone();
        }
        let mut buckets = self.buckets.write();
        for bucket in buckets.values_mut() {
            bucket.update_config(config.rps, config.burst);
        }
    }

    pub fn check_rate_limit(&self, provider: &str) -> Result<(), AiError> {
        let config = self.rate_limit.read().clone();
        let mut buckets = self.buckets.write();
        let bucket = buckets
            .entry(provider.to_string())
            .or_insert_with(|| TokenBucket::new(config.rps, config.burst));

        if let Some(retry_after_ms) = bucket.try_acquire() {
            return Err(AiError::RateLimited { retry_after_ms });
        }
        Ok(())
    }

    pub async fn send(&self, req: reqwest::Request) -> Result<reqwest::Response, AiError> {
        let host = req.url().host_str().unwrap_or("unknown").to_string();
        let start = std::time::Instant::now();
        let resp = self.client.execute(req).await.map_err(AiError::from)?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let status = resp.status().as_u16();
        tracing::info!(
            target: "ai_audit",
            host = %host,
            status_code = status,
            duration_ms = duration_ms,
            "AI provider request completed"
        );
        Ok(resp)
    }

    pub async fn send_with_audit(
        &self,
        req: reqwest::Request,
        provider: &str,
        model: &str,
    ) -> Result<reqwest::Response, AiError> {
        self.check_rate_limit(provider)?;
        let host = req.url().host_str().unwrap_or("unknown").to_string();
        let start = std::time::Instant::now();
        let resp = self.client.execute(req).await.map_err(AiError::from)?;
        let duration_ms = start.elapsed().as_millis() as u64;
        let status = resp.status().as_u16();
        tracing::info!(
            target: "ai_audit",
            host = %host,
            provider = %provider,
            model = %model,
            status_code = status,
            duration_ms = duration_ms,
            "AI provider request completed"
        );
        Ok(resp)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_config_default() {
        let config = RateLimitConfig::default();
        assert_eq!(config.rps, 10);
        assert_eq!(config.burst, 20);
    }

    #[test]
    fn token_bucket_acquire_within_burst() {
        let mut bucket = TokenBucket::new(10, 5);
        for _ in 0..5 {
            assert!(bucket.try_acquire().is_none(), "should allow within burst");
        }
    }

    #[test]
    fn token_bucket_rate_limit_when_exhausted() {
        let mut bucket = TokenBucket::new(10, 1);
        assert!(
            bucket.try_acquire().is_none(),
            "first acquire should succeed"
        );
        let result = bucket.try_acquire();
        assert!(result.is_some(), "should be rate limited");
        assert!(result.unwrap() > 0, "retry_after_ms should be positive");
    }

    #[test]
    fn token_bucket_update_config_reduces_capacity() {
        let mut bucket = TokenBucket::new(1, 100);
        // tokens = 100 initially
        bucket.update_config(1, 5);
        // tokens truncated to min(100, 5) = 5
        for _ in 0..5 {
            assert!(
                bucket.try_acquire().is_none(),
                "should allow within new capacity"
            );
        }
        assert!(
            bucket.try_acquire().is_some(),
            "should be rate limited after exhausting new capacity"
        );
    }

    #[test]
    fn audit_http_client_new() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig::default());
        assert_eq!(audit.rate_limit_config().rps, 10);
        assert_eq!(audit.rate_limit_config().burst, 20);
    }

    #[test]
    fn audit_http_client_client_ref_accessible() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig::default());
        let _ref = audit.client();
    }

    #[test]
    fn update_rate_limit_changes_config() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig::default());
        audit.update_rate_limit(RateLimitConfig {
            rps: 50,
            burst: 100,
        });
        let config = audit.rate_limit_config();
        assert_eq!(config.rps, 50);
        assert_eq!(config.burst, 100);
    }

    #[test]
    fn check_rate_limit_allows_within_burst() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig { rps: 10, burst: 5 });
        for _ in 0..5 {
            assert!(audit.check_rate_limit("provider-a").is_ok());
        }
    }

    #[test]
    fn check_rate_limit_denies_when_exhausted() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig { rps: 1, burst: 1 });
        assert!(audit.check_rate_limit("provider-b").is_ok());
        let err = audit.check_rate_limit("provider-b").unwrap_err();
        assert_eq!(err.error_code(), "AI_RATE_LIMITED");
    }

    #[test]
    fn check_rate_limit_independent_per_provider() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig { rps: 1, burst: 1 });
        assert!(audit.check_rate_limit("provider-a").is_ok());
        assert!(audit.check_rate_limit("provider-a").is_err());
        assert!(audit.check_rate_limit("provider-b").is_ok());
    }

    #[test]
    fn update_rate_limit_affects_existing_buckets() {
        let client = reqwest::Client::new();
        let audit = AuditHttpClient::new(client, RateLimitConfig { rps: 1, burst: 100 });
        // 消耗 50 个令牌
        for _ in 0..50 {
            assert!(audit.check_rate_limit("provider-a").is_ok());
        }
        // 更新为更小的 capacity
        audit.update_rate_limit(RateLimitConfig { rps: 1, burst: 2 });
        // tokens 被截断到 min(~50, 2) = 2
        assert!(audit.check_rate_limit("provider-a").is_ok());
        assert!(audit.check_rate_limit("provider-a").is_ok());
        // 第 3 次应被限流
        assert!(audit.check_rate_limit("provider-a").is_err());
    }
}

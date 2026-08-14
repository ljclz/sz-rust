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

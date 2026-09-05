// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
enum ProviderState {
    Available,
    Degraded { fail_count: u32 },
    Cooldown { until: Instant },
}

pub struct ProviderFailover {
    threshold: u32,
    cooldown: Duration,
    states: Arc<Mutex<HashMap<String, ProviderState>>>,
}

impl ProviderFailover {
    pub fn new(threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            threshold,
            cooldown: Duration::from_millis(cooldown_ms),
            states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn is_available(&self, provider: &str) -> bool {
        let states = self.states.lock();
        match states.get(provider) {
            Some(ProviderState::Cooldown { until }) => Instant::now() >= *until,
            _ => true,
        }
    }

    pub fn record_success(&self, provider: &str) {
        let mut states = self.states.lock();
        states.insert(provider.to_string(), ProviderState::Available);
    }

    pub fn record_failure(&self, provider: &str) {
        let mut states = self.states.lock();
        let current = states
            .get(provider)
            .cloned()
            .unwrap_or(ProviderState::Available);
        let new_state = match current {
            ProviderState::Available => ProviderState::Degraded { fail_count: 1 },
            ProviderState::Degraded { fail_count } => {
                if fail_count + 1 >= self.threshold {
                    ProviderState::Cooldown {
                        until: Instant::now() + self.cooldown,
                    }
                } else {
                    ProviderState::Degraded {
                        fail_count: fail_count + 1,
                    }
                }
            }
            ProviderState::Cooldown { until } => {
                if Instant::now() >= until {
                    ProviderState::Degraded { fail_count: 1 }
                } else {
                    ProviderState::Cooldown { until }
                }
            }
        };
        states.insert(provider.to_string(), new_state);
    }

    pub async fn call_with_failover<F, Fut, R>(
        &self,
        primary: &str,
        fallback: Option<&str>,
        f: F,
    ) -> Result<R, crate::common::AiError>
    where
        F: Fn(&str) -> Fut,
        Fut: std::future::Future<Output = Result<R, crate::common::AiError>>,
    {
        if self.is_available(primary) {
            match f(primary).await {
                Ok(r) => {
                    self.record_success(primary);
                    return Ok(r);
                }
                Err(e) => {
                    self.record_failure(primary);
                    if !e.is_retryable() {
                        return Err(e);
                    }
                }
            }
        }

        if let Some(fb) = fallback {
            if self.is_available(fb) {
                match f(fb).await {
                    Ok(r) => {
                        self.record_success(fb);
                        return Ok(r);
                    }
                    Err(e) => {
                        self.record_failure(fb);
                        return Err(e);
                    }
                }
            }
        }

        Err(crate::common::AiError::ProviderUnavailable(format!(
            "all providers unavailable (primary: {}, fallback: {:?})",
            primary, fallback
        )))
    }

    /// 链式故障切换：按 providers 顺序遍历，每个 Provider 失败达 threshold 进入 Cooldown 切到下一个
    ///
    /// 非重试错误立即返回；全部不可用返回 `AiError::ProviderUnavailable`。
    pub async fn call_with_failover_chain<F, Fut, R>(
        &self,
        providers: &[&str],
        f: F,
    ) -> Result<R, crate::common::AiError>
    where
        F: Fn(&str) -> Fut,
        Fut: std::future::Future<Output = Result<R, crate::common::AiError>>,
    {
        for provider in providers {
            if self.is_available(provider) {
                match f(provider).await {
                    Ok(r) => {
                        self.record_success(provider);
                        return Ok(r);
                    }
                    Err(e) => {
                        self.record_failure(provider);
                        if !e.is_retryable() {
                            return Err(e);
                        }
                    }
                }
            }
        }

        Err(crate::common::AiError::ProviderUnavailable(
            "all providers unavailable".to_string(),
        ))
    }

    /// 获取 Provider 当前状态（用于测试断言）
    pub fn state(&self, provider: &str) -> &'static str {
        let states = self.states.lock();
        match states.get(provider) {
            Some(ProviderState::Available) => "available",
            Some(ProviderState::Degraded { .. }) => "degraded",
            Some(ProviderState::Cooldown { .. }) => "cooldown",
            None => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::AiError;

    #[test]
    fn is_available_initially_true() {
        let fo = ProviderFailover::new(3, 5000);
        assert!(fo.is_available("openai"));
    }

    #[test]
    fn record_success_resets_state() {
        let fo = ProviderFailover::new(3, 5000);
        fo.record_failure("openai");
        fo.record_failure("openai");
        fo.record_success("openai");
        assert!(fo.is_available("openai"));
    }

    #[test]
    fn degraded_below_threshold_still_available() {
        let fo = ProviderFailover::new(3, 5000);
        fo.record_failure("openai");
        fo.record_failure("openai");
        assert!(fo.is_available("openai"));
    }

    #[test]
    fn cooldown_after_threshold_makes_unavailable() {
        let fo = ProviderFailover::new(3, 5000);
        fo.record_failure("openai");
        fo.record_failure("openai");
        fo.record_failure("openai");
        assert!(!fo.is_available("openai"));
    }

    #[tokio::test]
    async fn call_with_failover_primary_success() {
        let fo = ProviderFailover::new(3, 5000);
        let result: Result<i32, AiError> = fo
            .call_with_failover("openai", Some("claude"), |_| async { Ok(42) })
            .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn call_with_failover_fallback_on_retryable_error() {
        let fo = ProviderFailover::new(3, 5000);
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();
        let result: Result<i32, AiError> = fo
            .call_with_failover("openai", Some("claude"), move |provider| {
                let cc = cc.clone();
                let p = provider.to_string();
                async move {
                    cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if p == "openai" {
                        Err(AiError::ProviderUnavailable("down".into()))
                    } else {
                        Ok(99)
                    }
                }
            })
            .await;
        assert_eq!(result.unwrap(), 99);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn call_with_failover_non_retryable_returns_immediately() {
        let fo = ProviderFailover::new(3, 5000);
        let result: Result<i32, AiError> = fo
            .call_with_failover("openai", Some("claude"), |_| async {
                Err(AiError::ProviderAuthFailed("bad key".into()))
            })
            .await;
        assert_eq!(result.unwrap_err().error_code(), "AI_PROVIDER_AUTH_FAILED");
    }

    #[tokio::test]
    async fn call_with_failover_all_fail() {
        let fo = ProviderFailover::new(3, 5000);
        let result: Result<i32, AiError> = fo
            .call_with_failover("openai", Some("claude"), |_| async {
                Err(AiError::ProviderUnavailable("down".into()))
            })
            .await;
        assert_eq!(result.unwrap_err().error_code(), "AI_PROVIDER_UNAVAILABLE");
    }

    #[tokio::test]
    async fn call_with_failover_no_fallback() {
        let fo = ProviderFailover::new(3, 5000);
        let result: Result<i32, AiError> = fo
            .call_with_failover("openai", None, |_| async {
                Err(AiError::ProviderUnavailable("down".into()))
            })
            .await;
        assert_eq!(result.unwrap_err().error_code(), "AI_PROVIDER_UNAVAILABLE");
    }
}

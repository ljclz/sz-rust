// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! OTLP 批量导出 + 重试（spec 5.9）
//!
//! 支持多后端（Jaeger/Tempo/Zipkin）+ 批量聚合 + 失败重试 + 运行时切换。

#![forbid(unsafe_code)]

use std::time::Duration;

/// OTLP 后端类型（spec 5.9.1）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtlpBackend {
    /// Jaeger 后端（endpoint URL）
    Jaeger(String),
    /// Tempo 后端（endpoint URL）
    Tempo(String),
    /// Zipkin 后端（endpoint URL）
    Zipkin(String),
}

impl OtlpBackend {
    /// 获取端点 URL
    pub fn endpoint(&self) -> &str {
        match self {
            OtlpBackend::Jaeger(url) => url,
            OtlpBackend::Tempo(url) => url,
            OtlpBackend::Zipkin(url) => url,
        }
    }

    /// 后端名称
    pub fn name(&self) -> &str {
        match self {
            OtlpBackend::Jaeger(_) => "jaeger",
            OtlpBackend::Tempo(_) => "tempo",
            OtlpBackend::Zipkin(_) => "zipkin",
        }
    }
}

/// 批量配置（spec 6.6.2/6.6.3）
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// 批量大小（默认 256，spec 6.6.2）
    pub batch_size: usize,
    /// 时间窗口（默认 5000ms，spec 6.6.3）
    pub time_window: Duration,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 256,
            time_window: Duration::from_millis(5000),
        }
    }
}

/// 重试配置（spec 6.6.5）
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// 最大重试次数（默认 5，spec 6.6.5）
    pub max_retries: u32,
    /// 缓冲上限
    pub buffer_limit: usize,
    /// 退避策略
    pub backoff: BackoffStrategy,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            buffer_limit: 10000,
            backoff: BackoffStrategy::Exponential {
                initial: Duration::from_millis(100),
                max: Duration::from_secs(30),
            },
        }
    }
}

/// 退避策略
#[derive(Debug, Clone)]
pub enum BackoffStrategy {
    /// 固定间隔
    Fixed(Duration),
    /// 指数退避
    Exponential {
        /// 初始延迟
        initial: Duration,
        /// 最大延迟
        max: Duration,
    },
}

impl BackoffStrategy {
    /// 计算第 n 次重试的延迟
    pub fn delay(&self, retry_count: u32) -> Duration {
        match self {
            BackoffStrategy::Fixed(d) => *d,
            BackoffStrategy::Exponential { initial, max } => {
                let multiplier = 2u64.saturating_pow(retry_count);
                let delay = initial.saturating_mul(multiplier as u32);
                delay.min(*max)
            }
        }
    }
}

/// TLS 配置（spec 5.9.4 + 6.6.4）
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// 是否启用 TLS（默认 true）
    pub enabled: bool,
    /// 证书路径
    pub cert_path: Option<String>,
    /// 密钥路径
    pub key_path: Option<String>,
    /// CA 证书路径
    pub ca_path: Option<String>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cert_path: None,
            key_path: None,
            ca_path: None,
        }
    }
}

/// OTLP 后端管理器（spec 5.9.5）
///
/// 使用 ArcSwap 实现运行时切换后端无需重启。
pub struct OtlpBackendManager {
    current: parking_lot::RwLock<OtlpBackend>,
    tls_config: TlsConfig,
}

impl OtlpBackendManager {
    /// 创建后端管理器
    pub fn new(backend: OtlpBackend, tls_config: TlsConfig) -> Self {
        Self {
            current: parking_lot::RwLock::new(backend),
            tls_config,
        }
    }

    /// 切换后端（spec 5.9.5）
    pub fn switch(&self, backend: OtlpBackend) {
        let mut current = self.current.write();
        tracing::info!(from = current.name(), to = backend.name(), "OTLP 后端切换");
        *current = backend;
    }

    /// 获取当前后端
    pub fn current(&self) -> OtlpBackend {
        self.current.read().clone()
    }

    /// 获取 TLS 配置
    pub fn tls_config(&self) -> &TlsConfig {
        &self.tls_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_endpoint() {
        let jaeger = OtlpBackend::Jaeger("http://localhost:14268".to_string());
        assert_eq!(jaeger.endpoint(), "http://localhost:14268");
        assert_eq!(jaeger.name(), "jaeger");
    }

    #[test]
    fn test_batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 256, "默认批量大小 256");
        assert_eq!(
            config.time_window,
            Duration::from_millis(5000),
            "默认时间窗口 5000ms"
        );
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 5, "默认最大重试 5");
    }

    #[test]
    fn test_backoff_exponential() {
        let strategy = BackoffStrategy::Exponential {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(30),
        };
        assert_eq!(strategy.delay(0), Duration::from_millis(100));
        assert_eq!(strategy.delay(1), Duration::from_millis(200));
        assert_eq!(strategy.delay(2), Duration::from_millis(400));
        // 应受 max 限制
        assert!(strategy.delay(10) <= Duration::from_secs(30));
    }

    #[test]
    fn test_tls_config_default_enabled() {
        let config = TlsConfig::default();
        assert!(config.enabled, "TLS 默认应启用（spec 5.9.4）");
    }

    #[test]
    fn test_backend_manager_switch() {
        let manager = OtlpBackendManager::new(
            OtlpBackend::Jaeger("http://jaeger:14268".to_string()),
            TlsConfig::default(),
        );
        assert_eq!(manager.current().name(), "jaeger");

        manager.switch(OtlpBackend::Tempo("http://tempo:4318".to_string()));
        assert_eq!(manager.current().name(), "tempo");
        assert_eq!(manager.current().endpoint(), "http://tempo:4318");
    }
}

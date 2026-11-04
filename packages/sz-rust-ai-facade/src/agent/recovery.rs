// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 错误恢复策略（T036）
//!
//! 步骤失败时按配置策略处理：重试/跳过/终止。

use serde_json::Value;

use crate::common::AiError;

/// 恢复策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RecoveryStrategy {
    /// 重试（最多 N 次）
    Retry,
    /// 跳过（继续下一步）
    Skip,
    /// 终止（停止执行）
    #[default]
    Abort,
}

/// 恢复策略配置
#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    /// 策略
    pub strategy: RecoveryStrategy,
    /// 最大重试次数
    pub max_retries: u32,
    /// 重试间隔（毫秒）
    pub retry_interval_ms: u64,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            strategy: RecoveryStrategy::Abort,
            max_retries: 3,
            retry_interval_ms: 100,
        }
    }
}

impl RecoveryConfig {
    /// 创建重试策略
    pub fn retry(max_retries: u32) -> Self {
        Self {
            strategy: RecoveryStrategy::Retry,
            max_retries,
            retry_interval_ms: 100,
        }
    }

    /// 创建跳过策略
    pub fn skip() -> Self {
        Self {
            strategy: RecoveryStrategy::Skip,
            max_retries: 0,
            retry_interval_ms: 0,
        }
    }

    /// 创建终止策略
    pub fn abort() -> Self {
        Self {
            strategy: RecoveryStrategy::Abort,
            max_retries: 0,
            retry_interval_ms: 0,
        }
    }
}

/// 恢复决策结果
#[derive(Debug, Clone)]
pub enum RecoveryDecision {
    /// 重试
    Retry { attempt: u32 },
    /// 跳过
    Skip,
    /// 终止
    Abort { error: String },
    /// 成功
    Proceed { result: Value },
}

/// 错误恢复处理器
pub struct RecoveryHandler {
    config: RecoveryConfig,
}

impl RecoveryHandler {
    /// 创建恢复处理器
    pub fn new(config: RecoveryConfig) -> Self {
        Self { config }
    }

    /// 根据策略处理错误
    pub fn handle_error(&self, error: &AiError, attempt: u32) -> RecoveryDecision {
        match self.config.strategy {
            RecoveryStrategy::Retry => {
                if attempt < self.config.max_retries {
                    RecoveryDecision::Retry {
                        attempt: attempt + 1,
                    }
                } else {
                    RecoveryDecision::Abort {
                        error: format!(
                            "retry exhausted after {} attempts: {error}",
                            self.config.max_retries
                        ),
                    }
                }
            }
            RecoveryStrategy::Skip => RecoveryDecision::Skip,
            RecoveryStrategy::Abort => RecoveryDecision::Abort {
                error: error.to_string(),
            },
        }
    }

    /// 获取重试间隔
    pub fn retry_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.retry_interval_ms)
    }

    /// 获取配置
    pub fn config(&self) -> &RecoveryConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_error() -> AiError {
        AiError::Internal("test error".into())
    }

    #[test]
    fn test_recovery_strategy_default() {
        assert_eq!(RecoveryStrategy::default(), RecoveryStrategy::Abort);
    }

    #[test]
    fn test_retry_strategy() {
        let handler = RecoveryHandler::new(RecoveryConfig::retry(3));
        let error = make_error();

        let decision = handler.handle_error(&error, 0);
        assert!(matches!(decision, RecoveryDecision::Retry { attempt: 1 }));

        let decision = handler.handle_error(&error, 2);
        assert!(matches!(decision, RecoveryDecision::Retry { attempt: 3 }));

        let decision = handler.handle_error(&error, 3);
        assert!(matches!(decision, RecoveryDecision::Abort { .. }));
    }

    #[test]
    fn test_skip_strategy() {
        let handler = RecoveryHandler::new(RecoveryConfig::skip());
        let error = make_error();
        let decision = handler.handle_error(&error, 0);
        assert!(matches!(decision, RecoveryDecision::Skip));
    }

    #[test]
    fn test_abort_strategy() {
        let handler = RecoveryHandler::new(RecoveryConfig::abort());
        let error = make_error();
        let decision = handler.handle_error(&error, 0);
        assert!(matches!(decision, RecoveryDecision::Abort { .. }));
    }

    #[test]
    fn test_recovery_config_defaults() {
        let config = RecoveryConfig::default();
        assert_eq!(config.strategy, RecoveryStrategy::Abort);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_recovery_strategy_serde() {
        let s = serde_json::to_string(&RecoveryStrategy::Retry).unwrap();
        assert_eq!(s, "\"retry\"");
        let v: RecoveryStrategy = serde_json::from_str("\"skip\"").unwrap();
        assert_eq!(v, RecoveryStrategy::Skip);
    }

    #[test]
    fn test_retry_interval() {
        let handler = RecoveryHandler::new(RecoveryConfig::retry(3));
        assert_eq!(
            handler.retry_interval(),
            std::time::Duration::from_millis(100)
        );
    }
}

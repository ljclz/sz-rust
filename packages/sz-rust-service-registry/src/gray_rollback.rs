// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 灰度回滚决策（P3-2）
//!
//! 失败率超阈值 + 连续失败达阈值时触发自动回滚。

/// 健康统计快照
#[derive(Debug, Clone)]
pub struct HealthStats {
    /// 当前失败率（0~1）
    pub failure_rate: f32,
    /// 连续失败次数
    pub consecutive_failures: u32,
}

/// 回滚决策
#[derive(Debug, Clone, PartialEq)]
pub enum RollbackDecision {
    /// 继续灰度
    Continue,
    /// 触发回滚
    Rollback {
        /// 触发时的失败率
        failure_rate: f32,
        /// 触发时的连续失败次数
        consecutive_failures: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_stats_construction() {
        let stats = HealthStats {
            failure_rate: 0.5,
            consecutive_failures: 3,
        };
        assert!((stats.failure_rate - 0.5).abs() < f32::EPSILON);
        assert_eq!(stats.consecutive_failures, 3);
    }

    #[test]
    fn test_rollback_decision_equality() {
        assert_eq!(RollbackDecision::Continue, RollbackDecision::Continue);
        assert_ne!(
            RollbackDecision::Continue,
            RollbackDecision::Rollback {
                failure_rate: 0.5,
                consecutive_failures: 3
            }
        );
    }
}

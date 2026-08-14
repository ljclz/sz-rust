use std::time::Duration;

use crate::definition::FaultStrategy;
use crate::error::{WorkflowError, WorkflowResult};

/// 插件节点执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginNodeOutcome {
    Completed,
    Skipped,
    InstanceTerminated,
}

/// 容错策略决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultDecision {
    /// 终止实例
    Terminate,
    /// 跳过节点
    Skip,
    /// 重试（返回剩余重试次数与退避时间）
    Retry { remaining: u32, backoff: Duration },
}

/// 容错策略处理 trait。
pub trait FaultStrategyHandler: Send + Sync + 'static {
    /// 根据策略与错误决定下一步动作。
    fn decide(&self, strategy: FaultStrategy, error: &WorkflowError, attempt: u32)
        -> FaultDecision;
}

/// 默认容错策略处理器。
pub struct DefaultFaultStrategyHandler {
    retry_max: u32,
    retry_backoff: Duration,
}

impl DefaultFaultStrategyHandler {
    pub fn new(retry_max: u32, retry_backoff: Duration) -> Self {
        Self {
            retry_max,
            retry_backoff,
        }
    }
}

impl Default for DefaultFaultStrategyHandler {
    fn default() -> Self {
        Self::new(3, Duration::from_millis(100))
    }
}

impl FaultStrategyHandler for DefaultFaultStrategyHandler {
    fn decide(
        &self,
        strategy: FaultStrategy,
        error: &WorkflowError,
        attempt: u32,
    ) -> FaultDecision {
        match strategy {
            FaultStrategy::Fail => {
                tracing::error!(error = %error, "插件节点失败，终止实例");
                FaultDecision::Terminate
            }
            FaultStrategy::Skip => {
                tracing::warn!(error = %error, "插件节点失败，跳过");
                FaultDecision::Skip
            }
            FaultStrategy::Retry => {
                if attempt < self.retry_max {
                    let backoff = self.retry_backoff * 2u32.pow(attempt);
                    tracing::warn!(
                        attempt = attempt + 1,
                        backoff_ms = backoff.as_millis(),
                        "重试"
                    );
                    FaultDecision::Retry {
                        remaining: self.retry_max - attempt,
                        backoff,
                    }
                } else {
                    tracing::error!(error = %error, "重试超限，终止实例");
                    FaultDecision::Terminate
                }
            }
        }
    }
}

/// 便捷函数：根据决策返回 PluginNodeOutcome。
pub fn outcome_from_decision(decision: FaultDecision) -> WorkflowResult<PluginNodeOutcome> {
    match decision {
        FaultDecision::Terminate => Ok(PluginNodeOutcome::InstanceTerminated),
        FaultDecision::Skip => Ok(PluginNodeOutcome::Skipped),
        FaultDecision::Retry { remaining: 0, .. } => Ok(PluginNodeOutcome::InstanceTerminated),
        FaultDecision::Retry { .. } => Ok(PluginNodeOutcome::Completed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WorkflowErrorCode;

    #[test]
    fn fail_strategy() {
        let handler = DefaultFaultStrategyHandler::default();
        let error = WorkflowError::new(WorkflowErrorCode::CapabilityNotFound, "能力不存在");
        let decision = handler.decide(FaultStrategy::Fail, &error, 0);
        assert_eq!(decision, FaultDecision::Terminate);
    }

    #[test]
    fn skip_strategy() {
        let handler = DefaultFaultStrategyHandler::default();
        let error = WorkflowError::new(WorkflowErrorCode::CapabilityNotFound, "能力不存在");
        let decision = handler.decide(FaultStrategy::Skip, &error, 0);
        assert_eq!(decision, FaultDecision::Skip);
    }

    #[test]
    fn retry_first_attempt() {
        let handler = DefaultFaultStrategyHandler::new(3, Duration::from_millis(10));
        let error = WorkflowError::new(WorkflowErrorCode::CapabilityNotFound, "能力不存在");
        let decision = handler.decide(FaultStrategy::Retry, &error, 0);
        match decision {
            FaultDecision::Retry { remaining, backoff } => {
                assert_eq!(remaining, 3);
                assert_eq!(backoff, Duration::from_millis(10));
            }
            _ => panic!("期望 Retry"),
        }
    }

    #[test]
    fn retry_second_attempt() {
        let handler = DefaultFaultStrategyHandler::new(3, Duration::from_millis(10));
        let error = WorkflowError::new(WorkflowErrorCode::CapabilityNotFound, "能力不存在");
        let decision = handler.decide(FaultStrategy::Retry, &error, 1);
        match decision {
            FaultDecision::Retry { remaining, backoff } => {
                assert_eq!(remaining, 2);
                assert_eq!(backoff, Duration::from_millis(20));
            }
            _ => panic!("期望 Retry"),
        }
    }

    #[test]
    fn retry_exhausted() {
        let handler = DefaultFaultStrategyHandler::new(2, Duration::from_millis(1));
        let error = WorkflowError::new(WorkflowErrorCode::CapabilityNotFound, "能力不存在");
        let decision = handler.decide(FaultStrategy::Retry, &error, 2);
        assert_eq!(decision, FaultDecision::Terminate);
    }
}

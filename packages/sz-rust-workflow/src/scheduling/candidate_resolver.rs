// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use async_trait::async_trait;

use crate::definition::CandidateStrategy;
use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::guard::GuardEvaluator;
use sz_rust_capability::CapabilityRegistry;

/// 候选人解析 trait，对齐 design 2.2.2.6。
#[async_trait]
pub trait CandidateResolver: Send + Sync + 'static {
    async fn resolve(
        &self,
        strategy: &CandidateStrategy,
        context: &serde_json::Value,
    ) -> WorkflowResult<Vec<String>>;
}

/// 默认候选人解析器。
pub struct DefaultCandidateResolver {
    guard_evaluator: Arc<dyn GuardEvaluator>,
    capability_registry: Arc<CapabilityRegistry>,
}

impl DefaultCandidateResolver {
    pub fn new(
        guard_evaluator: Arc<dyn GuardEvaluator>,
        capability_registry: Arc<CapabilityRegistry>,
    ) -> Self {
        Self {
            guard_evaluator,
            capability_registry,
        }
    }
}

#[async_trait]
impl CandidateResolver for DefaultCandidateResolver {
    async fn resolve(
        &self,
        strategy: &CandidateStrategy,
        context: &serde_json::Value,
    ) -> WorkflowResult<Vec<String>> {
        let candidates = match strategy {
            CandidateStrategy::Static { users, roles } => {
                let mut result: Vec<String> = users.clone();
                let _ = roles;
                result.sort();
                result.dedup();
                result
            }
            CandidateStrategy::Dynamic { expr } => {
                let val = self.guard_evaluator.evaluate(expr, context).await?;
                if let serde_json::Value::Array(arr) =
                    serde_json::to_value(val).unwrap_or(serde_json::Value::Null)
                {
                    arr.into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                } else {
                    return Err(WorkflowError::new(
                        WorkflowErrorCode::CandidateFormatError,
                        "Dynamic 表达式求值结果非数组",
                    ));
                }
            }
            CandidateStrategy::Capability {
                capability_name,
                args,
            } => {
                let result = self
                    .capability_registry
                    .call(capability_name, args.clone())
                    .await
                    .map_err(|e| {
                        WorkflowError::new(
                            WorkflowErrorCode::CandidateFormatError,
                            format!("能力调用失败：{e}"),
                        )
                    })?;
                match result {
                    serde_json::Value::Array(arr) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => {
                        return Err(WorkflowError::with_field(
                            WorkflowErrorCode::CandidateFormatError,
                            "能力返回值非数组",
                            "capability",
                            capability_name,
                        ))
                    }
                }
            }
        };
        if candidates.is_empty() {
            return Err(WorkflowError::new(
                WorkflowErrorCode::NoCandidates,
                "候选人为空集合",
            ));
        }
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::DefaultGuardEvaluator;

    #[tokio::test]
    async fn static_strategy() {
        let resolver = DefaultCandidateResolver::new(
            Arc::new(DefaultGuardEvaluator::default()),
            Arc::new(CapabilityRegistry::new()),
        );
        let strategy = CandidateStrategy::Static {
            users: vec!["u1".into(), "u2".into()],
            roles: vec![],
        };
        let result = resolver
            .resolve(&strategy, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result, vec!["u1", "u2"]);
    }

    #[tokio::test]
    async fn static_strategy_empty() {
        let resolver = DefaultCandidateResolver::new(
            Arc::new(DefaultGuardEvaluator::default()),
            Arc::new(CapabilityRegistry::new()),
        );
        let strategy = CandidateStrategy::Static {
            users: vec![],
            roles: vec![],
        };
        let result = resolver.resolve(&strategy, &serde_json::json!({})).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, WorkflowErrorCode::NoCandidates);
    }
}

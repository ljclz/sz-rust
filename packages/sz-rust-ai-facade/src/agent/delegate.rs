// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 子 Agent 委托（T035）
//!
//! 主 Agent 委托子 Agent 执行子任务，结果返回主 Agent。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::agent::multi_step::{FnStep, StepAction, StepContext};
use crate::common::AiError;

/// 子 Agent trait
#[async_trait]
pub trait SubAgent: Send + Sync + 'static {
    /// 执行子任务
    async fn execute_subtask(&self, input: &Value) -> Result<Value, AiError>;

    /// 子 Agent 名称
    fn name(&self) -> &str;
}

/// 子 Agent 委托器
pub struct AgentDelegator {
    sub_agents: parking_lot::RwLock<std::collections::HashMap<String, Arc<dyn SubAgent>>>,
}

impl AgentDelegator {
    /// 创建委托器
    pub fn new() -> Self {
        Self {
            sub_agents: parking_lot::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// 注册子 Agent
    pub fn register(&self, agent: Arc<dyn SubAgent>) {
        self.sub_agents
            .write()
            .insert(agent.name().to_string(), agent);
    }

    /// 委托子 Agent 执行
    pub async fn delegate(&self, agent_name: &str, input: &Value) -> Result<Value, AiError> {
        let agent = {
            let agents = self.sub_agents.read();
            agents
                .get(agent_name)
                .cloned()
                .ok_or_else(|| AiError::Internal(format!("sub agent not found: {agent_name}")))?
        };
        agent.execute_subtask(input).await
    }

    /// 创建委托步骤（用于 MultiStepOrchestrator）
    /// 使用 `create_delegate_step` 函数代替
    pub fn delegate_step(&self, _agent_name: &str) -> Arc<dyn StepAction> {
        Arc::new(FnStep::new(|input: Value, _ctx: StepContext| async move {
            Ok(input)
        }))
    }

    /// 列出已注册的子 Agent
    pub fn list_agents(&self) -> Vec<String> {
        self.sub_agents.read().keys().cloned().collect()
    }
}

impl Default for AgentDelegator {
    fn default() -> Self {
        Self::new()
    }
}

/// 为 AgentDelegator 创建委托步骤的便捷方法
pub fn create_delegate_step(
    delegator: Arc<AgentDelegator>,
    agent_name: &str,
) -> Arc<dyn StepAction> {
    let name = agent_name.to_string();
    Arc::new(FnStep::new(move |input: Value, _ctx: StepContext| {
        let name = name.clone();
        let delegator = delegator.clone();
        async move { delegator.delegate(&name, &input).await }
    }))
}

/// 闭包包装为 SubAgent
pub struct FnSubAgent<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, AiError>> + Send + 'static,
{
    name: String,
    func: F,
}

impl<F, Fut> FnSubAgent<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, AiError>> + Send + 'static,
{
    /// 创建闭包子 Agent
    pub fn new(name: &str, func: F) -> Self {
        Self {
            name: name.to_string(),
            func,
        }
    }
}

#[async_trait]
impl<F, Fut> SubAgent for FnSubAgent<F, Fut>
where
    F: Fn(Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, AiError>> + Send + 'static,
{
    async fn execute_subtask(&self, input: &Value) -> Result<Value, AiError> {
        (self.func)(input.clone()).await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::multi_step::{MultiStepOrchestrator, OrchestrationStep};

    #[tokio::test]
    async fn test_delegate_sub_agent() {
        let delegator = AgentDelegator::new();
        delegator.register(Arc::new(FnSubAgent::new(
            "translator",
            |input: Value| async move { Ok(serde_json::json!({"translated": input})) },
        )));

        let result = delegator
            .delegate("translator", &serde_json::json!("hello"))
            .await
            .unwrap();
        assert_eq!(result["translated"], "hello");
    }

    #[tokio::test]
    async fn test_delegate_unknown_agent() {
        let delegator = AgentDelegator::new();
        let result = delegator.delegate("unknown", &Value::Null).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delegate_in_orchestrator() {
        let delegator = Arc::new(AgentDelegator::new());
        delegator.register(Arc::new(FnSubAgent::new(
            "processor",
            |input: Value| async move { Ok(serde_json::json!({"processed": input})) },
        )));

        let step = OrchestrationStep::new(
            "delegate-step",
            create_delegate_step(delegator.clone(), "processor"),
        );
        let orchestrator = MultiStepOrchestrator::new(vec![step]);
        let result = orchestrator
            .execute(serde_json::json!("data"))
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.final_output.unwrap()["processed"], "data");
    }

    #[test]
    fn test_list_agents() {
        let delegator = AgentDelegator::new();
        delegator.register(Arc::new(FnSubAgent::new("a", |_| async {
            Ok(Value::Null)
        })));
        delegator.register(Arc::new(FnSubAgent::new("b", |_| async {
            Ok(Value::Null)
        })));
        let agents = delegator.list_agents();
        assert_eq!(agents.len(), 2);
        assert!(agents.contains(&"a".to_string()));
        assert!(agents.contains(&"b".to_string()));
    }
}

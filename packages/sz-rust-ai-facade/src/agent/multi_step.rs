// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 多步骤 Agent 编排器（T035）
//!
//! 支持多步骤任务编排，步骤间传递上下文。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::common::AiError;

/// 步骤执行 trait
#[async_trait]
pub trait StepAction: Send + Sync + 'static {
    /// 执行步骤
    async fn execute(&self, input: &Value, context: &StepContext) -> Result<Value, AiError>;
}

/// 步骤上下文（步骤间传递数据）
#[derive(Debug, Clone, Default)]
pub struct StepContext {
    /// 命名数据槽
    slots: HashMap<String, Value>,
}

impl StepContext {
    /// 创建空上下文
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置槽值
    pub fn set(&mut self, key: &str, value: Value) {
        self.slots.insert(key.to_string(), value);
    }

    /// 获取槽值
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.slots.get(key)
    }

    /// 获取槽值（克隆）
    pub fn get_cloned(&self, key: &str) -> Option<Value> {
        self.slots.get(key).cloned()
    }

    /// 是否包含槽
    pub fn contains(&self, key: &str) -> bool {
        self.slots.contains_key(key)
    }

    /// 合并其他上下文
    pub fn merge(&mut self, other: StepContext) {
        for (k, v) in other.slots {
            self.slots.insert(k, v);
        }
    }
}

/// 编排步骤
pub struct OrchestrationStep {
    /// 步骤 ID
    pub step_id: String,
    /// 步骤动作
    pub action: Arc<dyn StepAction>,
    /// 输入槽名（从上下文读取作为输入，None 则用初始输入）
    pub input_slot: Option<String>,
    /// 输出槽名（将输出写入上下文）
    pub output_slot: Option<String>,
}

impl OrchestrationStep {
    /// 构造步骤
    pub fn new(step_id: impl Into<String>, action: Arc<dyn StepAction>) -> Self {
        Self {
            step_id: step_id.into(),
            action,
            input_slot: None,
            output_slot: None,
        }
    }

    /// 设置输入槽
    pub fn with_input_slot(mut self, slot: &str) -> Self {
        self.input_slot = Some(slot.to_string());
        self
    }

    /// 设置输出槽
    pub fn with_output_slot(mut self, slot: &str) -> Self {
        self.output_slot = Some(slot.to_string());
        self
    }
}

/// 步骤执行结果
#[derive(Debug, Clone)]
pub struct StepExecutionResult {
    pub step_id: String,
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
}

/// 多步骤编排结果
#[derive(Debug, Clone)]
pub struct OrchestrationResult {
    pub success: bool,
    pub steps: Vec<StepExecutionResult>,
    pub context: StepContext,
    pub final_output: Option<Value>,
    pub failed_step: Option<String>,
}

/// 多步骤编排器
pub struct MultiStepOrchestrator {
    steps: Vec<OrchestrationStep>,
}

impl MultiStepOrchestrator {
    /// 创建编排器
    pub fn new(steps: Vec<OrchestrationStep>) -> Self {
        Self { steps }
    }

    /// 执行编排
    pub async fn execute(&self, initial_input: Value) -> Result<OrchestrationResult, AiError> {
        let mut context = StepContext::new();
        context.set("initial", initial_input.clone());

        let mut results: Vec<StepExecutionResult> = Vec::with_capacity(self.steps.len());
        let mut current_input = initial_input;
        let mut final_output: Option<Value> = None;

        for step in &self.steps {
            let input = if let Some(ref slot) = step.input_slot {
                context.get_cloned(slot).unwrap_or(Value::Null)
            } else {
                current_input.clone()
            };

            match step.action.execute(&input, &context).await {
                Ok(output) => {
                    if let Some(ref slot) = step.output_slot {
                        context.set(slot, output.clone());
                    }
                    current_input = output.clone();
                    final_output = Some(output.clone());
                    results.push(StepExecutionResult {
                        step_id: step.step_id.clone(),
                        success: true,
                        output: Some(output),
                        error: None,
                    });
                }
                Err(e) => {
                    results.push(StepExecutionResult {
                        step_id: step.step_id.clone(),
                        success: false,
                        output: None,
                        error: Some(e.to_string()),
                    });
                    return Ok(OrchestrationResult {
                        success: false,
                        steps: results,
                        context,
                        final_output,
                        failed_step: Some(step.step_id.clone()),
                    });
                }
            }
        }

        Ok(OrchestrationResult {
            success: true,
            steps: results,
            context,
            final_output,
            failed_step: None,
        })
    }
}

/// 闭包包装为 StepAction
pub struct FnStep<F, Fut>
where
    F: Fn(Value, StepContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, AiError>> + Send + 'static,
{
    func: F,
}

impl<F, Fut> FnStep<F, Fut>
where
    F: Fn(Value, StepContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, AiError>> + Send + 'static,
{
    /// 创建闭包步骤
    pub fn new(func: F) -> Self {
        Self { func }
    }
}

#[async_trait]
impl<F, Fut> StepAction for FnStep<F, Fut>
where
    F: Fn(Value, StepContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, AiError>> + Send + 'static,
{
    async fn execute(&self, input: &Value, context: &StepContext) -> Result<Value, AiError> {
        (self.func)(input.clone(), context.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_step(id: &str, should_fail: bool) -> OrchestrationStep {
        OrchestrationStep::new(
            id,
            Arc::new(FnStep::new(
                move |input: Value, _ctx: StepContext| async move {
                    if should_fail {
                        return Err(AiError::Internal("step failed".into()));
                    }
                    Ok(input)
                },
            )),
        )
    }

    #[tokio::test]
    async fn test_multi_step_all_success() {
        let steps = vec![
            make_step("s1", false),
            make_step("s2", false),
            make_step("s3", false),
        ];
        let orchestrator = MultiStepOrchestrator::new(steps);
        let result = orchestrator.execute(Value::Null).await.unwrap();

        assert!(result.success);
        assert_eq!(result.steps.len(), 3);
        assert!(result.failed_step.is_none());
    }

    #[tokio::test]
    async fn test_multi_step_fail_at_second() {
        let steps = vec![
            make_step("s1", false),
            make_step("s2", true),
            make_step("s3", false),
        ];
        let orchestrator = MultiStepOrchestrator::new(steps);
        let result = orchestrator.execute(Value::Null).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_step.as_deref(), Some("s2"));
        assert_eq!(result.steps.len(), 2, "only 2 steps should have run");
    }

    #[tokio::test]
    async fn test_context_passing() {
        let s1 = OrchestrationStep::new(
            "s1",
            Arc::new(FnStep::new(|_input: Value, _ctx: StepContext| async move {
                Ok(serde_json::json!({"user": "admin"}))
            })),
        )
        .with_output_slot("user_data");

        let s2 = OrchestrationStep::new(
            "s2",
            Arc::new(FnStep::new(|_input: Value, ctx: StepContext| async move {
                let user = ctx.get_cloned("user_data").unwrap_or(Value::Null);
                Ok(serde_json::json!({"processed": user}))
            })),
        )
        .with_input_slot("user_data");

        let orchestrator = MultiStepOrchestrator::new(vec![s1, s2]);
        let result = orchestrator.execute(Value::Null).await.unwrap();

        assert!(result.success);
        assert!(result.context.contains("user_data"));
        assert_eq!(result.context.get("user_data").unwrap()["user"], "admin");
    }

    #[tokio::test]
    async fn test_empty_steps() {
        let orchestrator = MultiStepOrchestrator::new(vec![]);
        let result = orchestrator.execute(Value::Null).await.unwrap();
        assert!(result.success);
        assert!(result.steps.is_empty());
    }

    #[test]
    fn test_step_context() {
        let mut ctx = StepContext::new();
        ctx.set("key", Value::String("value".into()));
        assert!(ctx.contains("key"));
        assert_eq!(ctx.get("key").unwrap(), &Value::String("value".into()));

        let mut other = StepContext::new();
        other.set("other", Value::Null);
        ctx.merge(other);
        assert!(ctx.contains("other"));
    }
}

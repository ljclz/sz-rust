// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Agent 状态持久化（T037）
//!
//! Agent 执行状态持久化到 sz_agent_executions 表，支持崩溃后恢复。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::AiError;

/// 执行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    /// 待执行
    Pending,
    /// 进行中
    Running,
    /// 已完成
    Completed,
    /// 已失败
    Failed,
    /// 已暂停（HITL）
    Paused,
    /// 已取消
    Cancelled,
}

/// Agent 执行记录（对应 sz_agent_executions 表）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecution {
    /// 执行 ID
    pub execution_id: String,
    /// Agent 名称
    pub agent_name: String,
    /// 执行状态
    pub state: ExecutionState,
    /// 当前步骤索引
    pub current_step: u32,
    /// 总步骤数
    pub total_steps: u32,
    /// 初始输入
    pub input: Value,
    /// 最终输出
    pub output: Option<Value>,
    /// 步骤追踪
    pub step_traces: Vec<StepTrace>,
    /// Token 消耗
    pub total_tokens: u64,
    /// 创建时间（Unix 毫秒）
    pub created_at: i64,
    /// 更新时间（Unix 毫秒）
    pub updated_at: i64,
}

/// 单步追踪
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTrace {
    /// 步骤 ID
    pub step_id: String,
    /// 步骤索引
    pub step_index: u32,
    /// 输入
    pub input: Value,
    /// 输出
    pub output: Option<Value>,
    /// 耗时（毫秒）
    pub elapsed_ms: f64,
    /// Token 消耗
    pub tokens: u64,
    /// 是否成功
    pub success: bool,
    /// 错误信息
    pub error: Option<String>,
}

/// 执行存储 trait
#[async_trait]
pub trait ExecutionStore: Send + Sync + 'static {
    /// 保存执行记录
    async fn save(&self, execution: &AgentExecution) -> Result<(), AiError>;

    /// 更新状态
    async fn update_state(
        &self,
        execution_id: &str,
        state: ExecutionState,
        current_step: u32,
    ) -> Result<(), AiError>;

    /// 获取执行记录
    async fn get(&self, execution_id: &str) -> Result<Option<AgentExecution>, AiError>;

    /// 获取进行中的执行
    async fn list_running(&self) -> Result<Vec<AgentExecution>, AiError>;

    /// 添加步骤追踪
    async fn add_step_trace(&self, execution_id: &str, trace: StepTrace) -> Result<(), AiError>;
}

/// 内存执行存储（测试/降级用）
#[derive(Debug, Default)]
pub struct InMemoryExecutionStore {
    inner: Arc<RwLock<HashMap<String, AgentExecution>>>,
}

impl InMemoryExecutionStore {
    /// 创建内存存储
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录数
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl ExecutionStore for InMemoryExecutionStore {
    async fn save(&self, execution: &AgentExecution) -> Result<(), AiError> {
        self.inner
            .write()
            .insert(execution.execution_id.clone(), execution.clone());
        Ok(())
    }

    async fn update_state(
        &self,
        execution_id: &str,
        state: ExecutionState,
        current_step: u32,
    ) -> Result<(), AiError> {
        let mut map = self.inner.write();
        let exec = map
            .get_mut(execution_id)
            .ok_or_else(|| AiError::Internal(format!("execution not found: {execution_id}")))?;
        exec.state = state;
        exec.current_step = current_step;
        exec.updated_at = chrono::Utc::now().timestamp_millis();
        Ok(())
    }

    async fn get(&self, execution_id: &str) -> Result<Option<AgentExecution>, AiError> {
        Ok(self.inner.read().get(execution_id).cloned())
    }

    async fn list_running(&self) -> Result<Vec<AgentExecution>, AiError> {
        Ok(self
            .inner
            .read()
            .values()
            .filter(|e| e.state == ExecutionState::Running || e.state == ExecutionState::Paused)
            .cloned()
            .collect())
    }

    async fn add_step_trace(&self, execution_id: &str, trace: StepTrace) -> Result<(), AiError> {
        let mut map = self.inner.write();
        let exec = map
            .get_mut(execution_id)
            .ok_or_else(|| AiError::Internal(format!("execution not found: {execution_id}")))?;
        exec.step_traces.push(trace);
        exec.updated_at = chrono::Utc::now().timestamp_millis();
        Ok(())
    }
}

/// 执行追踪器
pub struct ExecutionTracker {
    store: Arc<dyn ExecutionStore>,
}

impl ExecutionTracker {
    /// 创建追踪器
    pub fn new(store: Arc<dyn ExecutionStore>) -> Self {
        Self { store }
    }

    /// 开始执行
    pub async fn start(
        &self,
        execution_id: &str,
        agent_name: &str,
        total_steps: u32,
        input: Value,
    ) -> Result<(), AiError> {
        let now = chrono::Utc::now().timestamp_millis();
        let execution = AgentExecution {
            execution_id: execution_id.to_string(),
            agent_name: agent_name.to_string(),
            state: ExecutionState::Running,
            current_step: 0,
            total_steps,
            input,
            output: None,
            step_traces: vec![],
            total_tokens: 0,
            created_at: now,
            updated_at: now,
        };
        self.store.save(&execution).await
    }

    /// 记录步骤完成
    pub async fn record_step(&self, execution_id: &str, trace: StepTrace) -> Result<(), AiError> {
        self.store.add_step_trace(execution_id, trace).await
    }

    /// 完成执行
    pub async fn complete(
        &self,
        execution_id: &str,
        output: Value,
        total_tokens: u64,
    ) -> Result<(), AiError> {
        let mut map = self
            .store
            .get(execution_id)
            .await?
            .ok_or_else(|| AiError::Internal(format!("execution not found: {execution_id}")))?;
        map.state = ExecutionState::Completed;
        map.output = Some(output);
        map.total_tokens = total_tokens;
        self.store.save(&map).await
    }

    /// 恢复进行中的执行
    pub async fn recover(&self) -> Result<Vec<AgentExecution>, AiError> {
        self.store.list_running().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_execution(id: &str, state: ExecutionState) -> AgentExecution {
        let now = chrono::Utc::now().timestamp_millis();
        AgentExecution {
            execution_id: id.to_string(),
            agent_name: "test-agent".to_string(),
            state,
            current_step: 0,
            total_steps: 3,
            input: Value::Null,
            output: None,
            step_traces: vec![],
            total_tokens: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_save_and_get() {
        let store = InMemoryExecutionStore::new();
        let exec = make_execution("exec-1", ExecutionState::Running);
        store.save(&exec).await.unwrap();
        let got = store.get("exec-1").await.unwrap().unwrap();
        assert_eq!(got.execution_id, "exec-1");
        assert_eq!(got.state, ExecutionState::Running);
    }

    #[tokio::test]
    async fn test_update_state() {
        let store = InMemoryExecutionStore::new();
        store
            .save(&make_execution("exec-1", ExecutionState::Running))
            .await
            .unwrap();
        store
            .update_state("exec-1", ExecutionState::Completed, 3)
            .await
            .unwrap();
        let got = store.get("exec-1").await.unwrap().unwrap();
        assert_eq!(got.state, ExecutionState::Completed);
        assert_eq!(got.current_step, 3);
    }

    #[tokio::test]
    async fn test_list_running() {
        let store = InMemoryExecutionStore::new();
        store
            .save(&make_execution("e1", ExecutionState::Running))
            .await
            .unwrap();
        store
            .save(&make_execution("e2", ExecutionState::Completed))
            .await
            .unwrap();
        store
            .save(&make_execution("e3", ExecutionState::Paused))
            .await
            .unwrap();
        let running = store.list_running().await.unwrap();
        assert_eq!(running.len(), 2, "Running + Paused");
    }

    #[tokio::test]
    async fn test_add_step_trace() {
        let store = InMemoryExecutionStore::new();
        store
            .save(&make_execution("e1", ExecutionState::Running))
            .await
            .unwrap();
        let trace = StepTrace {
            step_id: "s1".into(),
            step_index: 0,
            input: Value::Null,
            output: Some(Value::Null),
            elapsed_ms: 10.0,
            tokens: 100,
            success: true,
            error: None,
        };
        store.add_step_trace("e1", trace).await.unwrap();
        let got = store.get("e1").await.unwrap().unwrap();
        assert_eq!(got.step_traces.len(), 1);
    }

    #[tokio::test]
    async fn test_tracker_lifecycle() {
        let store = Arc::new(InMemoryExecutionStore::new());
        let tracker = ExecutionTracker::new(store.clone());

        tracker
            .start("exec-1", "agent", 3, Value::Null)
            .await
            .unwrap();
        assert_eq!(store.len(), 1);

        tracker
            .complete("exec-1", serde_json::json!({"result": "ok"}), 500)
            .await
            .unwrap();
        let got = store.get("exec-1").await.unwrap().unwrap();
        assert_eq!(got.state, ExecutionState::Completed);
        assert_eq!(got.total_tokens, 500);
    }

    #[tokio::test]
    async fn test_tracker_recover() {
        let store = Arc::new(InMemoryExecutionStore::new());
        let tracker = ExecutionTracker::new(store.clone());

        tracker.start("e1", "agent", 3, Value::Null).await.unwrap();
        tracker.start("e2", "agent", 2, Value::Null).await.unwrap();
        tracker.complete("e1", Value::Null, 0).await.unwrap();

        let running = tracker.recover().await.unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].execution_id, "e2");
    }

    #[test]
    fn test_execution_state_serde() {
        assert_eq!(
            serde_json::to_string(&ExecutionState::Running).unwrap(),
            "\"running\""
        );
        let v: ExecutionState = serde_json::from_str("\"paused\"").unwrap();
        assert_eq!(v, ExecutionState::Paused);
    }
}

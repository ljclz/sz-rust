// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 事务状态持久化（T025）
//!
//! 事务状态持久化到 sz_dtx_log 表，支持崩溃后恢复。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::DtxError;

/// 事务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxType {
    /// Saga 事务
    Saga,
    /// TCC 事务
    Tcc,
}

/// 事务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxState {
    /// 进行中
    Running,
    /// 已完成
    Completed,
    /// 已补偿
    Compensated,
    /// 已取消
    Cancelled,
    /// 失败（待人工处理）
    Failed,
    /// 超时
    Timeout,
}

/// 事务日志条目（对应 sz_dtx_log 表）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxLogEntry {
    /// 事务 ID
    pub tx_id: String,
    /// 事务类型
    pub tx_type: TxType,
    /// 事务状态
    pub state: TxState,
    /// 步骤列表（JSON）
    pub steps: Value,
    /// 当前 payload
    pub payload: Value,
    /// 创建时间（Unix 毫秒）
    pub created_at: i64,
    /// 更新时间（Unix 毫秒）
    pub updated_at: i64,
}

/// 事务日志存储 trait
#[async_trait::async_trait]
pub trait TxLogStore: Send + Sync + 'static {
    /// 保存事务日志
    async fn save(&self, entry: &TxLogEntry) -> Result<(), DtxError>;

    /// 更新事务状态
    async fn update_state(
        &self,
        tx_id: &str,
        state: TxState,
        payload: &Value,
    ) -> Result<(), DtxError>;

    /// 查询事务日志
    async fn get(&self, tx_id: &str) -> Result<Option<TxLogEntry>, DtxError>;

    /// 查询所有进行中的事务
    async fn list_running(&self) -> Result<Vec<TxLogEntry>, DtxError>;

    /// 删除事务日志
    async fn delete(&self, tx_id: &str) -> Result<(), DtxError>;
}

/// 内存事务日志存储（测试/降级用）
#[derive(Debug, Default)]
pub struct InMemoryTxLogStore {
    inner: Arc<RwLock<HashMap<String, TxLogEntry>>>,
}

impl InMemoryTxLogStore {
    /// 创建内存存储
    pub fn new() -> Self {
        Self::default()
    }

    /// 条目数
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl TxLogStore for InMemoryTxLogStore {
    async fn save(&self, entry: &TxLogEntry) -> Result<(), DtxError> {
        self.inner
            .write()
            .insert(entry.tx_id.clone(), entry.clone());
        Ok(())
    }

    async fn update_state(
        &self,
        tx_id: &str,
        state: TxState,
        payload: &Value,
    ) -> Result<(), DtxError> {
        let mut map = self.inner.write();
        let entry = map
            .get_mut(tx_id)
            .ok_or_else(|| DtxError::NotFound(tx_id.to_string()))?;
        entry.state = state;
        entry.payload = payload.clone();
        entry.updated_at = chrono::Utc::now().timestamp_millis();
        Ok(())
    }

    async fn get(&self, tx_id: &str) -> Result<Option<TxLogEntry>, DtxError> {
        Ok(self.inner.read().get(tx_id).cloned())
    }

    async fn list_running(&self) -> Result<Vec<TxLogEntry>, DtxError> {
        Ok(self
            .inner
            .read()
            .values()
            .filter(|e| e.state == TxState::Running)
            .cloned()
            .collect())
    }

    async fn delete(&self, tx_id: &str) -> Result<(), DtxError> {
        self.inner.write().remove(tx_id);
        Ok(())
    }
}

/// 事务日志记录器（封装存储 + 自动时间戳）
pub struct TxLogger {
    store: Arc<dyn TxLogStore>,
}

impl TxLogger {
    /// 创建事务日志记录器
    pub fn new(store: Arc<dyn TxLogStore>) -> Self {
        Self { store }
    }

    /// 记录事务开始
    pub async fn log_start(
        &self,
        tx_id: &str,
        tx_type: TxType,
        steps: Value,
        payload: Value,
    ) -> Result<(), DtxError> {
        let now = chrono::Utc::now().timestamp_millis();
        let entry = TxLogEntry {
            tx_id: tx_id.to_string(),
            tx_type,
            state: TxState::Running,
            steps,
            payload,
            created_at: now,
            updated_at: now,
        };
        self.store.save(&entry).await
    }

    /// 记录事务完成
    pub async fn log_complete(&self, tx_id: &str, payload: &Value) -> Result<(), DtxError> {
        self.store
            .update_state(tx_id, TxState::Completed, payload)
            .await
    }

    /// 记录事务补偿
    pub async fn log_compensate(&self, tx_id: &str, payload: &Value) -> Result<(), DtxError> {
        self.store
            .update_state(tx_id, TxState::Compensated, payload)
            .await
    }

    /// 记录事务失败
    pub async fn log_fail(&self, tx_id: &str, payload: &Value) -> Result<(), DtxError> {
        self.store
            .update_state(tx_id, TxState::Failed, payload)
            .await
    }

    /// 恢复进行中的事务
    pub async fn recover(&self) -> Result<Vec<TxLogEntry>, DtxError> {
        self.store.list_running().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(tx_id: &str, state: TxState) -> TxLogEntry {
        let now = chrono::Utc::now().timestamp_millis();
        TxLogEntry {
            tx_id: tx_id.to_string(),
            tx_type: TxType::Saga,
            state,
            steps: Value::Array(vec![]),
            payload: Value::Null,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_in_memory_save_and_get() {
        let store = InMemoryTxLogStore::new();
        let entry = make_entry("tx-1", TxState::Running);
        store.save(&entry).await.unwrap();
        let got = store.get("tx-1").await.unwrap().unwrap();
        assert_eq!(got.tx_id, "tx-1");
        assert_eq!(got.state, TxState::Running);
    }

    #[tokio::test]
    async fn test_in_memory_get_missing() {
        let store = InMemoryTxLogStore::new();
        let got = store.get("missing").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_update_state() {
        let store = InMemoryTxLogStore::new();
        let entry = make_entry("tx-2", TxState::Running);
        store.save(&entry).await.unwrap();
        store
            .update_state("tx-2", TxState::Completed, &Value::Null)
            .await
            .unwrap();
        let got = store.get("tx-2").await.unwrap().unwrap();
        assert_eq!(got.state, TxState::Completed);
    }

    #[tokio::test]
    async fn test_in_memory_update_missing_fails() {
        let store = InMemoryTxLogStore::new();
        let result = store
            .update_state("missing", TxState::Completed, &Value::Null)
            .await;
        assert!(matches!(result, Err(DtxError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_in_memory_list_running() {
        let store = InMemoryTxLogStore::new();
        store
            .save(&make_entry("tx-1", TxState::Running))
            .await
            .unwrap();
        store
            .save(&make_entry("tx-2", TxState::Completed))
            .await
            .unwrap();
        store
            .save(&make_entry("tx-3", TxState::Running))
            .await
            .unwrap();

        let running = store.list_running().await.unwrap();
        assert_eq!(running.len(), 2);
    }

    #[tokio::test]
    async fn test_in_memory_delete() {
        let store = InMemoryTxLogStore::new();
        store
            .save(&make_entry("tx-1", TxState::Running))
            .await
            .unwrap();
        assert_eq!(store.len(), 1);
        store.delete("tx-1").await.unwrap();
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn test_tx_logger_lifecycle() {
        let store = Arc::new(InMemoryTxLogStore::new());
        let logger = TxLogger::new(store.clone());

        logger
            .log_start("tx-1", TxType::Saga, Value::Null, Value::Null)
            .await
            .unwrap();
        assert_eq!(store.len(), 1);

        logger.log_complete("tx-1", &Value::Null).await.unwrap();
        let got = store.get("tx-1").await.unwrap().unwrap();
        assert_eq!(got.state, TxState::Completed);
    }

    #[tokio::test]
    async fn test_tx_logger_recover() {
        let store = Arc::new(InMemoryTxLogStore::new());
        let logger = TxLogger::new(store.clone());

        logger
            .log_start("tx-1", TxType::Saga, Value::Null, Value::Null)
            .await
            .unwrap();
        logger
            .log_start("tx-2", TxType::Tcc, Value::Null, Value::Null)
            .await
            .unwrap();
        logger.log_complete("tx-1", &Value::Null).await.unwrap();

        let running = logger.recover().await.unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].tx_id, "tx-2");
    }

    #[test]
    fn test_tx_type_serde() {
        assert_eq!(serde_json::to_string(&TxType::Saga).unwrap(), "\"saga\"");
        assert_eq!(serde_json::to_string(&TxType::Tcc).unwrap(), "\"tcc\"");
    }

    #[test]
    fn test_tx_state_serde() {
        let s = serde_json::to_string(&TxState::Running).unwrap();
        assert_eq!(s, "\"running\"");
        let v: TxState = serde_json::from_str("\"timeout\"").unwrap();
        assert_eq!(v, TxState::Timeout);
    }
}

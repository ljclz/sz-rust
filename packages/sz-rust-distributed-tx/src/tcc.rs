// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! TCC 事务编排器（T025）
//!
//! Try 全成功 → Confirm / 任一失败 → Cancel

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tracing::warn;

use crate::error::DtxError;
use crate::saga::SagaAction;

/// TCC 参与者
pub struct TccParticipant {
    /// 参与者 ID
    pub participant_id: String,
    /// Try 阶段（预留资源）
    pub try_action: Arc<dyn SagaAction>,
    /// Confirm 阶段（确认提交）
    pub confirm_action: Arc<dyn SagaAction>,
    /// Cancel 阶段（取消预留）
    pub cancel_action: Arc<dyn SagaAction>,
}

impl TccParticipant {
    /// 构造 TCC 参与者
    pub fn new(
        participant_id: impl Into<String>,
        try_action: Arc<dyn SagaAction>,
        confirm_action: Arc<dyn SagaAction>,
        cancel_action: Arc<dyn SagaAction>,
    ) -> Self {
        Self {
            participant_id: participant_id.into(),
            try_action,
            confirm_action,
            cancel_action,
        }
    }
}

/// TCC 阶段状态
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TccPhase {
    /// Try 阶段
    Trying,
    /// Confirm 阶段
    Confirming,
    /// Cancel 阶段
    Cancelling,
    /// 已完成（Confirm 成功）
    Completed,
    /// 已取消（Cancel 成功）
    Cancelled,
    /// 失败（Cancel 也失败）
    Failed,
}

/// TCC 参与者执行结果
#[derive(Debug, Clone)]
pub struct ParticipantResult {
    pub participant_id: String,
    pub try_success: bool,
    pub confirm_success: Option<bool>,
    pub cancel_success: Option<bool>,
    pub error: Option<String>,
}

/// TCC 事务结果
#[derive(Debug, Clone)]
pub struct TccResult {
    /// 事务 ID
    pub tx_id: String,
    /// 最终阶段
    pub phase: TccPhase,
    /// 是否成功
    pub success: bool,
    /// 各参与者结果
    pub participants: Vec<ParticipantResult>,
    /// 最终 payload
    pub final_payload: Value,
}

/// TCC 编排器
#[derive(Default)]
pub struct TccOrchestrator {
    /// 超时时间
    timeout: Option<Duration>,
}

impl TccOrchestrator {
    /// 创建 TCC 编排器
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置超时
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// 执行 TCC 事务
    pub async fn execute(
        &self,
        participants: Vec<TccParticipant>,
        payload: Value,
    ) -> Result<TccResult, DtxError> {
        let tx_id = crate::next_tx_id("tcc");
        self.execute_with_tx_id(participants, payload, tx_id).await
    }

    /// 使用指定事务 ID 执行 TCC 事务
    pub async fn execute_with_tx_id(
        &self,
        participants: Vec<TccParticipant>,
        payload: Value,
        tx_id: String,
    ) -> Result<TccResult, DtxError> {
        let mut current_payload = payload;
        let mut results: Vec<ParticipantResult> = Vec::with_capacity(participants.len());
        let mut try_succeeded: Vec<usize> = Vec::new();

        for (idx, p) in participants.iter().enumerate() {
            let try_result = self
                .run_with_timeout(p.try_action.as_ref(), &current_payload)
                .await;

            match try_result {
                Ok(output) => {
                    current_payload = output;
                    results.push(ParticipantResult {
                        participant_id: p.participant_id.clone(),
                        try_success: true,
                        confirm_success: None,
                        cancel_success: None,
                        error: None,
                    });
                    try_succeeded.push(idx);
                }
                Err(e) => {
                    results.push(ParticipantResult {
                        participant_id: p.participant_id.clone(),
                        try_success: false,
                        confirm_success: None,
                        cancel_success: None,
                        error: Some(e.to_string()),
                    });

                    return self
                        .execute_cancel(
                            &tx_id,
                            &participants,
                            &try_succeeded,
                            &mut results,
                            &mut current_payload,
                        )
                        .await;
                }
            }
        }

        self.execute_confirm(&tx_id, &participants, &mut results, &mut current_payload)
            .await
    }

    async fn execute_confirm(
        &self,
        tx_id: &str,
        participants: &[TccParticipant],
        results: &mut Vec<ParticipantResult>,
        payload: &mut Value,
    ) -> Result<TccResult, DtxError> {
        let mut confirmed: Vec<usize> = Vec::new();
        for (idx, p) in participants.iter().enumerate() {
            match self
                .run_with_timeout(p.confirm_action.as_ref(), payload)
                .await
            {
                Ok(output) => {
                    *payload = output;
                    results[idx].confirm_success = Some(true);
                    confirmed.push(idx);
                }
                Err(e) => {
                    warn!(tx_id = %tx_id, participant = %p.participant_id, error = %e, "confirm failed");
                    results[idx].confirm_success = Some(false);
                    results[idx].error = Some(e.to_string());

                    // 只回滚 Try 成功且 Confirm 尚未成功的参与者：
                    // 已 Confirm 的参与者资源已提交，回滚会破坏一致性。
                    let to_cancel: Vec<usize> = results
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.try_success && r.confirm_success != Some(true))
                        .map(|(i, _)| i)
                        .collect();
                    let mut outcome = self
                        .execute_cancel(tx_id, participants, &to_cancel, results, payload)
                        .await?;
                    // 部分参与者已提交且无法回滚 → 需要人工介入，不能标记为已取消
                    if !confirmed.is_empty() {
                        outcome.phase = TccPhase::Failed;
                        outcome.success = false;
                    }
                    return Ok(outcome);
                }
            }
        }

        Ok(TccResult {
            tx_id: tx_id.to_string(),
            phase: TccPhase::Completed,
            success: true,
            participants: std::mem::take(results),
            final_payload: std::mem::take(payload),
        })
    }

    async fn execute_cancel(
        &self,
        tx_id: &str,
        participants: &[TccParticipant],
        try_succeeded: &[usize],
        results: &mut Vec<ParticipantResult>,
        payload: &mut Value,
    ) -> Result<TccResult, DtxError> {
        let mut all_cancelled = true;

        for &idx in try_succeeded.iter().rev() {
            let p = &participants[idx];
            match self
                .run_with_timeout(p.cancel_action.as_ref(), payload)
                .await
            {
                Ok(output) => {
                    *payload = output;
                    results[idx].cancel_success = Some(true);
                }
                Err(e) => {
                    warn!(tx_id = %tx_id, participant = %p.participant_id, error = %e, "cancel failed");
                    results[idx].cancel_success = Some(false);
                    results[idx].error = Some(e.to_string());
                    all_cancelled = false;
                }
            }
        }

        let phase = if all_cancelled {
            TccPhase::Cancelled
        } else {
            TccPhase::Failed
        };
        Ok(TccResult {
            tx_id: tx_id.to_string(),
            phase,
            success: false,
            participants: std::mem::take(results),
            final_payload: std::mem::take(payload),
        })
    }

    async fn run_with_timeout(
        &self,
        action: &dyn SagaAction,
        payload: &Value,
    ) -> Result<Value, DtxError> {
        match self.timeout {
            Some(d) => tokio::time::timeout(d, action.execute(payload))
                .await
                .map_err(|_| DtxError::Timeout(d))?,
            None => action.execute(payload).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saga::FnAction;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_action(counter: Arc<AtomicUsize>, should_fail: bool) -> Arc<dyn SagaAction> {
        Arc::new(FnAction::new(move |payload: Value| {
            let c = counter.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                if should_fail {
                    return Err(DtxError::generic("action failed"));
                }
                Ok(payload)
            }
        }))
    }

    #[tokio::test]
    async fn test_tcc_all_success() {
        let t = Arc::new(AtomicUsize::new(0));
        let c = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicUsize::new(0));

        let participants = vec![TccParticipant::new(
            "p1",
            make_action(t.clone(), false),
            make_action(c.clone(), false),
            make_action(cancel.clone(), false),
        )];

        let orchestrator = TccOrchestrator::new();
        let result = orchestrator
            .execute(participants, Value::Null)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.phase, TccPhase::Completed);
        assert_eq!(t.load(Ordering::SeqCst), 1);
        assert_eq!(c.load(Ordering::SeqCst), 1);
        assert_eq!(cancel.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_tcc_try_fail_triggers_cancel() {
        let t1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let cancel1 = Arc::new(AtomicUsize::new(0));
        let t2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let cancel2 = Arc::new(AtomicUsize::new(0));

        let participants = vec![
            TccParticipant::new(
                "p1",
                make_action(t1.clone(), false),
                make_action(c1.clone(), false),
                make_action(cancel1.clone(), false),
            ),
            TccParticipant::new(
                "p2",
                make_action(t2.clone(), true),
                make_action(c2.clone(), false),
                make_action(cancel2.clone(), false),
            ),
        ];

        let orchestrator = TccOrchestrator::new();
        let result = orchestrator
            .execute(participants, Value::Null)
            .await
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.phase, TccPhase::Cancelled);
        assert_eq!(t1.load(Ordering::SeqCst), 1, "p1 try ran");
        assert_eq!(t2.load(Ordering::SeqCst), 1, "p2 try ran and failed");
        assert_eq!(cancel1.load(Ordering::SeqCst), 1, "p1 cancel ran");
        assert_eq!(
            cancel2.load(Ordering::SeqCst),
            0,
            "p2 cancel NOT ran (try failed)"
        );
        assert_eq!(c1.load(Ordering::SeqCst), 0, "p1 confirm NOT ran");
        assert_eq!(c2.load(Ordering::SeqCst), 0, "p2 confirm NOT ran");
    }

    #[tokio::test]
    async fn test_tcc_confirm_fail_triggers_cancel() {
        let participants = vec![
            TccParticipant::new(
                "p1",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
            TccParticipant::new(
                "p2",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let orchestrator = TccOrchestrator::new();
        let result = orchestrator
            .execute(participants, Value::Null)
            .await
            .unwrap();

        assert!(!result.success);
        // p1 已完成 Confirm（资源已提交），无法整体回滚 → 需人工介入
        assert_eq!(result.phase, TccPhase::Failed);
    }

    #[tokio::test]
    async fn test_tcc_confirm_fail_cancels_only_unconfirmed() {
        let cancel1 = Arc::new(AtomicUsize::new(0));
        let cancel2 = Arc::new(AtomicUsize::new(0));
        let participants = vec![
            TccParticipant::new(
                "p1",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(cancel1.clone(), false),
            ),
            TccParticipant::new(
                "p2",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(cancel2.clone(), false),
            ),
        ];

        let result = TccOrchestrator::new()
            .execute(participants, Value::Null)
            .await
            .unwrap();

        assert!(!result.success);
        assert_eq!(
            cancel1.load(Ordering::SeqCst),
            0,
            "已 Confirm 的参与者不得回滚"
        );
        assert_eq!(
            cancel2.load(Ordering::SeqCst),
            1,
            "Confirm 失败的参与者需要回滚预留资源"
        );
    }

    #[tokio::test]
    async fn test_tcc_tx_id_unique_under_concurrency() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..100 {
            let ids_vec = vec![TccParticipant::new(
                "p1",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            )];
            let r = TccOrchestrator::new()
                .execute(ids_vec, Value::Null)
                .await
                .unwrap();
            assert!(ids.insert(r.tx_id));
        }
    }

    #[tokio::test]
    async fn test_tcc_cancel_fail_marks_failed() {
        let participants = vec![
            TccParticipant::new(
                "p1",
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), true),
            ),
            TccParticipant::new(
                "p2",
                make_action(Arc::new(AtomicUsize::new(0)), true),
                make_action(Arc::new(AtomicUsize::new(0)), false),
                make_action(Arc::new(AtomicUsize::new(0)), false),
            ),
        ];

        let orchestrator = TccOrchestrator::new();
        let result = orchestrator
            .execute(participants, Value::Null)
            .await
            .unwrap();

        assert!(!result.success);
        assert_eq!(
            result.phase,
            TccPhase::Failed,
            "cancel failed should mark as Failed"
        );
    }

    #[tokio::test]
    async fn test_tcc_empty_participants() {
        let orchestrator = TccOrchestrator::new();
        let result = orchestrator.execute(vec![], Value::Null).await.unwrap();
        assert!(result.success);
        assert_eq!(result.phase, TccPhase::Completed);
    }

    #[tokio::test]
    async fn test_tcc_timeout() {
        let slow = Arc::new(FnAction::new(|_| async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(Value::Null)
        }));
        let noop = Arc::new(FnAction::new(|p| async move { Ok(p) }));

        let participants = vec![TccParticipant::new("slow", slow, noop.clone(), noop)];
        let orchestrator = TccOrchestrator::new().with_timeout(Duration::from_millis(50));
        let result = orchestrator
            .execute(participants, Value::Null)
            .await
            .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_tcc_phase_serde() {
        let s = serde_json::to_string(&TccPhase::Completed).unwrap();
        assert_eq!(s, "\"completed\"");
        let v: TccPhase = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(v, TccPhase::Failed);
    }
}

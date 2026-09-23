// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Saga 集成测试（T026）
//!
//! 验证部分失败后数据一致性、超时处理、补偿失败重试

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sz_rust_distributed_tx::{
    DtxError, FnAction, InMemoryTxLogStore, SagaAction, SagaOrchestrator, SagaStep, StepStatus,
    TxLogStore, TxLogger, TxState, TxType,
};

/// 模拟账户余额操作
fn deposit(account: &'static str, counter: Arc<AtomicUsize>) -> Arc<dyn SagaAction> {
    Arc::new(FnAction::new(move |mut payload: Value| {
        let c = counter.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            let amount = payload["amount"].as_i64().unwrap_or(0);
            let current = payload[account].as_i64().unwrap_or(0);
            payload[account] = json!(current + amount);
            Ok(payload)
        }
    }))
}

fn withdraw(account: &'static str, counter: Arc<AtomicUsize>) -> Arc<dyn SagaAction> {
    Arc::new(FnAction::new(move |mut payload: Value| {
        let c = counter.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            let amount = payload["amount"].as_i64().unwrap_or(0);
            let current = payload[account].as_i64().unwrap_or(0);
            payload[account] = json!(current - amount);
            Ok(payload)
        }
    }))
}

fn fail_action(counter: Arc<AtomicUsize>) -> Arc<dyn SagaAction> {
    Arc::new(FnAction::new(move |_payload: Value| {
        let c = counter.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err(DtxError::generic("simulated failure"))
        }
    }))
}

/// 转账 Saga：扣款 → 加款 → 记账
#[tokio::test]
async fn test_transfer_saga_success() {
    let f1 = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::new(AtomicUsize::new(0));
    let f2 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));
    let f3 = Arc::new(AtomicUsize::new(0));
    let c3 = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "withdraw-from",
            withdraw("from", f1.clone()),
            deposit("from", c1.clone()),
        ),
        SagaStep::new(
            "deposit-to",
            deposit("to", f2.clone()),
            withdraw("to", c2.clone()),
        ),
        SagaStep::new(
            "record-ledger",
            deposit("ledger", f3.clone()),
            withdraw("ledger", c3.clone()),
        ),
    ];

    let initial = json!({"from": 100, "to": 50, "ledger": 0, "amount": 30});
    let orchestrator = SagaOrchestrator::new();
    let result = orchestrator.execute(steps, initial).await.unwrap();

    assert!(result.success);
    assert_eq!(f1.load(Ordering::SeqCst), 1);
    assert_eq!(f2.load(Ordering::SeqCst), 1);
    assert_eq!(f3.load(Ordering::SeqCst), 1);
    assert_eq!(c1.load(Ordering::SeqCst), 0, "no compensate on success");
    assert_eq!(c2.load(Ordering::SeqCst), 0);
    assert_eq!(c3.load(Ordering::SeqCst), 0);

    assert_eq!(result.final_payload["from"], json!(70));
    assert_eq!(result.final_payload["to"], json!(80));
    assert_eq!(result.final_payload["ledger"], json!(30));
}

/// 转账 Saga：第三步失败 → 前两步补偿 → 余额恢复
#[tokio::test]
async fn test_transfer_saga_third_step_fail_compensates() {
    let f1 = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::new(AtomicUsize::new(0));
    let f2 = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "withdraw-from",
            withdraw("from", f1.clone()),
            deposit("from", c1.clone()),
        ),
        SagaStep::new(
            "deposit-to",
            deposit("to", f2.clone()),
            withdraw("to", c2.clone()),
        ),
        SagaStep::new(
            "fail-step",
            fail_action(Arc::new(AtomicUsize::new(0))),
            fail_action(Arc::new(AtomicUsize::new(0))),
        ),
    ];

    let initial = json!({"from": 100, "to": 50, "amount": 30});
    let orchestrator = SagaOrchestrator::new();
    let result = orchestrator.execute(steps, initial).await.unwrap();

    assert!(!result.success);
    assert_eq!(result.failed_step.as_deref(), Some("fail-step"));

    assert_eq!(f1.load(Ordering::SeqCst), 1, "step1 forward ran");
    assert_eq!(f2.load(Ordering::SeqCst), 1, "step2 forward ran");
    assert_eq!(c1.load(Ordering::SeqCst), 1, "step1 compensate ran");
    assert_eq!(c2.load(Ordering::SeqCst), 1, "step2 compensate ran");

    assert_eq!(result.final_payload["from"], json!(100), "from restored");
    assert_eq!(result.final_payload["to"], json!(50), "to restored");
}

/// 转账 Saga：第二步失败 → 第一步补偿
#[tokio::test]
async fn test_transfer_saga_second_step_fail() {
    let f1 = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::new(AtomicUsize::new(0));

    let steps = vec![
        SagaStep::new(
            "withdraw-from",
            withdraw("from", f1.clone()),
            deposit("from", c1.clone()),
        ),
        SagaStep::new(
            "fail-step",
            fail_action(Arc::new(AtomicUsize::new(0))),
            fail_action(Arc::new(AtomicUsize::new(0))),
        ),
    ];

    let initial = json!({"from": 100, "to": 50, "amount": 30});
    let orchestrator = SagaOrchestrator::new();
    let result = orchestrator.execute(steps, initial).await.unwrap();

    assert!(!result.success);
    assert_eq!(
        result.final_payload["from"],
        json!(100),
        "from restored after compensate"
    );
    assert_eq!(result.final_payload["to"], json!(50), "to unchanged");
}

/// 超时触发补偿
#[tokio::test]
async fn test_timeout_triggers_compensate() {
    let slow = Arc::new(FnAction::new(|payload: Value| async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(payload)
    }));
    let noop = Arc::new(FnAction::new(|p: Value| async move { Ok(p) }));
    let compensate_counter = Arc::new(AtomicUsize::new(0));
    let compensate = Arc::new(FnAction::new({
        let c = compensate_counter.clone();
        move |p: Value| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(p)
            }
        }
    }));

    let steps = vec![
        SagaStep::new("quick", noop.clone(), noop.clone()),
        SagaStep::new("slow", slow, compensate),
    ];

    let orchestrator = SagaOrchestrator::new().with_timeout(Duration::from_millis(50));
    let result = orchestrator.execute(steps, Value::Null).await.unwrap();

    assert!(!result.success);
    assert_eq!(result.failed_step.as_deref(), Some("slow"));
    assert_eq!(
        compensate_counter.load(Ordering::SeqCst),
        0,
        "slow step compensate NOT called (forward failed/timeout)"
    );
}

/// 补偿失败重试后成功
#[tokio::test]
async fn test_compensate_retry_then_success() {
    let retry_count = Arc::new(AtomicUsize::new(0));

    let fail_forward = fail_action(Arc::new(AtomicUsize::new(0)));
    let compensate = Arc::new(FnAction::new({
        let rc = retry_count.clone();
        move |p: Value| {
            let rc = rc.clone();
            async move {
                if rc.fetch_add(1, Ordering::SeqCst) < 2 {
                    return Err(DtxError::generic("compensate retry"));
                }
                Ok(p)
            }
        }
    }));

    let success_action = Arc::new(FnAction::new(|p: Value| async move { Ok(p) }));

    let steps = vec![
        SagaStep::new("step-1", success_action.clone(), compensate),
        SagaStep::new("step-2", fail_forward, success_action.clone()),
    ];

    let orchestrator = SagaOrchestrator::new()
        .with_compensate_retries(3)
        .with_retry_interval(Duration::from_millis(1));
    let result = orchestrator.execute(steps, Value::Null).await.unwrap();

    assert!(!result.success);
    assert_eq!(
        retry_count.load(Ordering::SeqCst),
        3,
        "compensate retried 3 times, 3rd succeeded"
    );

    let compensated = result
        .steps
        .iter()
        .find(|s| s.status == StepStatus::Compensated);
    assert!(compensated.is_some(), "step-1 should be compensated");
}

/// 补偿重试耗尽后标记待人工处理
#[tokio::test]
async fn test_compensate_retry_exhausted() {
    let success_action = Arc::new(FnAction::new(|p: Value| async move { Ok(p) }));
    let fail_compensate = fail_action(Arc::new(AtomicUsize::new(0)));
    let fail_forward = fail_action(Arc::new(AtomicUsize::new(0)));

    let steps = vec![
        SagaStep::new("step-1", success_action.clone(), fail_compensate),
        SagaStep::new("step-2", fail_forward, success_action.clone()),
    ];

    let orchestrator = SagaOrchestrator::new()
        .with_compensate_retries(2)
        .with_retry_interval(Duration::from_millis(1));
    let result = orchestrator.execute(steps, Value::Null).await.unwrap();

    assert!(!result.success);
    let failed = result
        .steps
        .iter()
        .find(|s| s.status == StepStatus::CompensateFailed);
    assert!(failed.is_some(), "should have CompensateFailed status");
    assert_eq!(failed.unwrap().step_id, "step-1");
}

/// 事务日志记录 Saga 生命周期
#[tokio::test]
async fn test_saga_with_tx_logging() {
    let store = Arc::new(InMemoryTxLogStore::new());
    let logger = TxLogger::new(store.clone());

    let success_action = Arc::new(FnAction::new(|p: Value| async move { Ok(p) }));

    let steps = vec![SagaStep::new(
        "step-1",
        success_action.clone(),
        success_action.clone(),
    )];

    let tx_id = "test-tx-001";
    logger
        .log_start(tx_id, TxType::Saga, json!(["step-1"]), Value::Null)
        .await
        .unwrap();

    let orchestrator = SagaOrchestrator::new();
    let result = orchestrator.execute(steps, Value::Null).await.unwrap();

    if result.success {
        logger
            .log_complete(tx_id, &result.final_payload)
            .await
            .unwrap();
    } else {
        logger.log_fail(tx_id, &result.final_payload).await.unwrap();
    }

    let entry = store.get(tx_id).await.unwrap().unwrap();
    assert_eq!(entry.state, TxState::Completed);
    assert_eq!(entry.tx_type, TxType::Saga);
}

/// 多步骤 Saga 部分成功后补偿顺序验证
#[tokio::test]
async fn test_compensate_order_reverse() {
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    let make_forward = |id: &'static str, order: Arc<std::sync::Mutex<Vec<String>>>| {
        Arc::new(FnAction::new(move |p: Value| {
            let o = order.clone();
            async move {
                o.lock().unwrap().push(format!("{id}-fwd"));
                Ok(p)
            }
        }))
    };
    let make_compensate = |id: &'static str, order: Arc<std::sync::Mutex<Vec<String>>>| {
        Arc::new(FnAction::new(move |p: Value| {
            let o = order.clone();
            async move {
                o.lock().unwrap().push(format!("{id}-comp"));
                Ok(p)
            }
        }))
    };

    let steps = vec![
        SagaStep::new(
            "s1",
            make_forward("s1", order.clone()),
            make_compensate("s1", order.clone()),
        ),
        SagaStep::new(
            "s2",
            make_forward("s2", order.clone()),
            make_compensate("s2", order.clone()),
        ),
        SagaStep::new(
            "s3",
            fail_action(Arc::new(AtomicUsize::new(0))),
            make_compensate("s3", order.clone()),
        ),
    ];

    let orchestrator = SagaOrchestrator::new();
    let result = orchestrator.execute(steps, Value::Null).await.unwrap();

    assert!(!result.success);

    let order = order.lock().unwrap();
    assert_eq!(order[0], "s1-fwd");
    assert_eq!(order[1], "s2-fwd");
    assert_eq!(order[2], "s2-comp", "s2 compensated first (reverse)");
    assert_eq!(order[3], "s1-comp", "s1 compensated second (reverse)");
}

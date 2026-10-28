// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 分布式事务 — Saga/TCC 编排 + 补偿 + 状态持久化

#![forbid(unsafe_code)]

pub mod error;
pub mod persistence;
pub mod saga;
pub mod tcc;

#[cfg(feature = "dtx-parallel")]
pub mod parallel_saga;

pub use error::DtxError;
pub use persistence::{InMemoryTxLogStore, TxLogEntry, TxLogStore, TxLogger, TxState, TxType};
pub use saga::{
    FnAction, SagaAction, SagaOrchestrator, SagaResult, SagaStep, StepResult, StepStatus,
};
pub use tcc::{ParticipantResult, TccOrchestrator, TccParticipant, TccPhase, TccResult};

#[cfg(feature = "dtx-parallel")]
pub use parallel_saga::{BackoffRetry, DependencyGraph, IdempotentCompensate, ParallelSaga};

/// 生成全局唯一的事务 ID
///
/// 组成：`{prefix}-{毫秒时间戳:013x}{进程内序号:04x}{进程号:04x}`。
/// 时间戳保证可排序，序号保证同毫秒内不冲突，进程号保证多实例不冲突。
pub fn next_tx_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff;
    let pid = std::process::id() & 0xffff;
    format!("{prefix}-{millis:013x}{seq:04x}{pid:04x}")
}

#[cfg(test)]
mod tests {
    use super::next_tx_id;

    #[test]
    fn next_tx_id_is_unique_and_prefixed() {
        let a = next_tx_id("saga");
        let b = next_tx_id("saga");
        assert!(a.starts_with("saga-"));
        assert_ne!(a, b);
    }
}

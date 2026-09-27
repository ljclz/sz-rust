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
pub use parallel_saga::{
    BackoffRetry, DependencyGraph, IdempotentCompensate, ParallelSaga,
};

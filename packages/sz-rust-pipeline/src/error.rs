// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! sz-rust-pipeline 错误类型
//!
//! 错误码段: 17050-17059

use thiserror::Error;

use crate::stage::StageId;

/// 流水线错误
#[derive(Debug, Error)]
pub enum PipelineError {
    /// 阶段超时（spec §5.8 异常 1，code: 17050）
    #[error("阶段 `{0}` 超时 (code: 17050)")]
    StageTimeout(StageId),
    /// 阶段失败（code: 17051）
    #[error("阶段 `{0}` 失败: {1} (code: 17051)")]
    StageFailed(StageId, String),
    /// 背压持续触发（spec §5.8 异常 2，code: 17052）
    #[error("背压持续触发 (code: 17052)")]
    BackpressureSustained,
    /// 阶段间共享可变状态（spec §5.8 禁止项，code: 17053）
    #[error("阶段间共享可变状态 (code: 17053)")]
    SharedMutableState,
    /// 依赖图有环（code: 17054）
    #[error("依赖图有环: {0} (code: 17054)")]
    CyclicDependency(String),
    /// 配置错误（code: 17055）
    #[error("配置错误: {0} (code: 17055)")]
    Config(String),
    /// 内部错误（code: 17056）
    #[error("内部错误: {0} (code: 17056)")]
    Internal(String),
}

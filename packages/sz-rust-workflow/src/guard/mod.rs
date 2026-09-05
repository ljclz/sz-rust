// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 守卫条件求值

pub mod default;

use async_trait::async_trait;
pub use default::DefaultGuardEvaluator;

use crate::error::WorkflowResult;

/// 守卫条件求值 trait，对齐 design 2.2.2.5。
#[async_trait]
pub trait GuardEvaluator: Send + Sync + 'static {
    /// 求值守卫表达式，返回布尔结果。
    async fn evaluate(&self, expr: &str, context: &serde_json::Value) -> WorkflowResult<bool>;
}

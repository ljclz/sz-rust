// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! sz-rust-benchmark-suite 错误类型
//!
//! 错误码段: 17070-17079

use thiserror::Error;

/// 基准压测错误
#[derive(Debug, Clone, Error)]
pub enum BenchmarkError {
    /// 基线缺失（spec §5.10 异常 1，code: 17070）
    #[error("基线缺失，请先建立基线 (code: 17070)")]
    BaselineMissing,
    /// 基准运行不稳定（spec §5.10 异常 2，code: 17071）
    #[error("基准运行不稳定，波动过大: {0}% (code: 17071)")]
    Unstable(f64),
    /// 回归超门禁（code: 17072）
    #[error("性能回归 {0}% 超过门禁 {1}% (code: 17072)")]
    RegressionExceeded(f64, f64),
    /// 配置错误（code: 17073）
    #[error("配置错误: {0} (code: 17073)")]
    Config(String),
    /// 内部错误（code: 17074）
    #[error("内部错误: {0} (code: 17074)")]
    Internal(String),
}

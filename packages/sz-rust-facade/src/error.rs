// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 统一门面错误类型

use thiserror::Error;

/// 门面统一错误
///
/// 聚合八个门面的错误类型，通过 `From` 自动转换。
#[derive(Debug, Error)]
pub enum FacadeError {
    /// 缓存错误
    #[error("Cache error: {0}")]
    Cache(String),

    /// 数据库错误
    #[error("Db error: {0}")]
    Db(String),

    /// 事件错误
    #[error("Event error: {0}")]
    Event(String),

    /// 队列错误
    #[error("Queue error: {0}")]
    Queue(String),

    /// 日志错误
    #[error("Log error: {0}")]
    Log(String),

    /// 配置错误
    #[error("Config error: {0}")]
    Config(String),

    /// 门面未初始化
    #[error("Facade not initialized: {0}")]
    NotInitialized(&'static str),
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! sz-rust-zerocopy 错误类型
//!
//! 错误码段: 17060-17069

use thiserror::Error;

/// 零拷贝错误
#[derive(Debug, Clone, Error)]
pub enum ZeroCopyError {
    /// 类型不匹配（spec §5.9 异常 1，code: 17060）
    #[error("类型不匹配: {0} (code: 17060)")]
    TypeMismatch(String),
    /// 字节损坏（spec §5.9 异常 2，code: 17061）
    #[error("字节损坏: {0} (code: 17061)")]
    Corrupted(String),
    /// 序列化失败（code: 17062）
    #[error("序列化失败: {0} (code: 17062)")]
    SerializeFailed(String),
    /// 反序列化失败（code: 17063）
    #[error("反序列化失败: {0} (code: 17063)")]
    DeserializeFailed(String),
    /// 不支持的格式（code: 17064）
    #[error("不支持的格式: {0:?} (code: 17064)")]
    UnsupportedFormat(crate::serializer::SerializationFormat),
    /// 内部错误（code: 17065）
    #[error("内部错误: {0} (code: 17065)")]
    Internal(String),
}

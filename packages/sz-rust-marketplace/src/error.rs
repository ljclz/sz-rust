// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件市场错误枚举

use thiserror::Error;

/// 插件市场错误
#[derive(Debug, Clone, Error)]
pub enum MarketplaceError {
    /// 清单格式无效
    #[error("清单格式无效: {0}")]
    InvalidManifest(String),

    /// 签名无效
    #[error("签名无效: {0}")]
    InvalidSignature(String),

    /// 语义化版本格式无效
    #[error("语义化版本格式无效: {0}")]
    InvalidSemVer(String),

    /// 版本冲突（非严格递增）
    #[error("版本冲突: {0}")]
    VersionConflict(String),

    /// 未授权
    #[error("未授权: {0}")]
    Unauthorized(String),

    /// 非审核员角色
    #[error("非审核员角色: {0}")]
    NotReviewer(String),

    /// 自审禁止
    #[error("自审禁止: 审核员 {reviewer_id} 与开发者 {developer_id} 相同")]
    SelfReview { reviewer_id: i64, developer_id: i64 },

    /// 版本非 pending 状态
    #[error("版本 {version_id} 非 pending 状态（当前: {current_status}）")]
    VersionNotPending {
        version_id: i64,
        current_status: String,
    },

    /// 版本已审核
    #[error("版本 {0} 已审核")]
    AlreadyReviewed(i64),

    /// 版本未批准
    #[error("版本 {0} 未批准")]
    VersionNotApproved(i64),

    /// 依赖缺失
    #[error("依赖缺失: {0}")]
    DependencyMissing(String),

    /// Capability 冲突
    #[error("Capability 冲突: {0}")]
    CapabilityConflict(String),

    /// 下载失败
    #[error("下载失败: {0}")]
    DownloadFailed(String),

    /// 内部错误
    #[error("内部错误: {0}")]
    InternalError(String),
}

/// 插件市场结果
pub type MarketplaceResult<T> = Result<T, MarketplaceError>;

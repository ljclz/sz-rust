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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_invalid_manifest() {
        let e = MarketplaceError::InvalidManifest("bad json".to_string());
        assert_eq!(e.to_string(), "清单格式无效: bad json");
    }

    #[test]
    fn test_error_display_invalid_signature() {
        let e = MarketplaceError::InvalidSignature("sig".to_string());
        assert_eq!(e.to_string(), "签名无效: sig");
    }

    #[test]
    fn test_error_display_invalid_semver() {
        let e = MarketplaceError::InvalidSemVer("x".to_string());
        assert_eq!(e.to_string(), "语义化版本格式无效: x");
    }

    #[test]
    fn test_error_display_version_conflict() {
        let e = MarketplaceError::VersionConflict("c".to_string());
        assert_eq!(e.to_string(), "版本冲突: c");
    }

    #[test]
    fn test_error_display_unauthorized() {
        let e = MarketplaceError::Unauthorized("no token".to_string());
        assert_eq!(e.to_string(), "未授权: no token");
    }

    #[test]
    fn test_error_display_not_reviewer() {
        let e = MarketplaceError::NotReviewer("user".to_string());
        assert_eq!(e.to_string(), "非审核员角色: user");
    }

    #[test]
    fn test_error_display_self_review() {
        let e = MarketplaceError::SelfReview {
            reviewer_id: 1,
            developer_id: 1,
        };
        assert_eq!(e.to_string(), "自审禁止: 审核员 1 与开发者 1 相同");
    }

    #[test]
    fn test_error_display_version_not_pending() {
        let e = MarketplaceError::VersionNotPending {
            version_id: 5,
            current_status: "approved".to_string(),
        };
        assert_eq!(e.to_string(), "版本 5 非 pending 状态（当前: approved）");
    }

    #[test]
    fn test_error_display_already_reviewed() {
        let e = MarketplaceError::AlreadyReviewed(3);
        assert_eq!(e.to_string(), "版本 3 已审核");
    }

    #[test]
    fn test_error_display_version_not_approved() {
        let e = MarketplaceError::VersionNotApproved(7);
        assert_eq!(e.to_string(), "版本 7 未批准");
    }

    #[test]
    fn test_error_display_dependency_missing() {
        let e = MarketplaceError::DependencyMissing("dep".to_string());
        assert_eq!(e.to_string(), "依赖缺失: dep");
    }

    #[test]
    fn test_error_display_capability_conflict() {
        let e = MarketplaceError::CapabilityConflict("cap".to_string());
        assert_eq!(e.to_string(), "Capability 冲突: cap");
    }

    #[test]
    fn test_error_display_download_failed() {
        let e = MarketplaceError::DownloadFailed("timeout".to_string());
        assert_eq!(e.to_string(), "下载失败: timeout");
    }

    #[test]
    fn test_error_display_internal_error() {
        let e = MarketplaceError::InternalError("boom".to_string());
        assert_eq!(e.to_string(), "内部错误: boom");
    }

    #[test]
    fn test_error_clone() {
        let e = MarketplaceError::InternalError("test".to_string());
        let e2 = e.clone();
        assert_eq!(e.to_string(), e2.to_string());
    }
}

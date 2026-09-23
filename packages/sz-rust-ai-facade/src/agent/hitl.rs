// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! HITL（Human-in-the-Loop）暂停点（T036）
//!
//! Agent 执行到需人工确认步骤时暂停并请求审核，审核后继续。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;

use crate::common::AiError;

/// HITL 审核状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// 待审核
    Pending,
    /// 已批准
    Approved,
    /// 已拒绝
    Rejected,
    /// 已修改
    Modified,
}

/// HITL 审核请求
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewRequest {
    /// 请求 ID
    pub request_id: String,
    /// 步骤 ID
    pub step_id: String,
    /// 待审核内容
    pub content: Value,
    /// 审核说明
    pub description: String,
    /// 审核状态
    pub status: ReviewStatus,
    /// 审核人（审核后填充）
    pub reviewer: Option<String>,
    /// 审核意见
    pub review_comment: Option<String>,
    /// 修改后的内容（Modified 状态时）
    pub modified_content: Option<Value>,
}

impl ReviewRequest {
    /// 创建审核请求
    pub fn new(step_id: &str, content: Value, description: &str) -> Self {
        Self {
            request_id: format!("review-{}", chrono::Utc::now().timestamp_millis()),
            step_id: step_id.to_string(),
            content,
            description: description.to_string(),
            status: ReviewStatus::Pending,
            reviewer: None,
            review_comment: None,
            modified_content: None,
        }
    }

    /// 是否已审核
    pub fn is_reviewed(&self) -> bool {
        !matches!(self.status, ReviewStatus::Pending)
    }

    /// 是否批准
    pub fn is_approved(&self) -> bool {
        matches!(self.status, ReviewStatus::Approved | ReviewStatus::Modified)
    }
}

/// 审核回调 trait
#[async_trait]
pub trait ReviewCallback: Send + Sync + 'static {
    /// 请求审核
    async fn request_review(&self, request: &ReviewRequest) -> Result<ReviewRequest, AiError>;
}

/// HITL 控制器
pub struct HitlController {
    /// 审核回调
    callback: Arc<dyn ReviewCallback>,
    /// 待审核请求
    pending: Arc<RwLock<HashMap<String, ReviewRequest>>>,
}

impl HitlController {
    /// 创建 HITL 控制器
    pub fn new(callback: Arc<dyn ReviewCallback>) -> Self {
        Self {
            callback,
            pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 暂停并请求审核
    pub async fn pause_for_review(
        &self,
        step_id: &str,
        content: Value,
        description: &str,
    ) -> Result<Value, AiError> {
        let mut request = ReviewRequest::new(step_id, content, description);
        self.pending
            .write()
            .insert(request.request_id.clone(), request.clone());

        request = self.callback.request_review(&request).await?;

        self.pending
            .write()
            .insert(request.request_id.clone(), request.clone());

        if !request.is_approved() {
            return Err(AiError::Internal(format!(
                "HITL review rejected for step {}: {}",
                request.step_id,
                request.review_comment.as_deref().unwrap_or("no comment")
            )));
        }

        Ok(request.modified_content.unwrap_or(request.content))
    }

    /// 获取待审核请求
    pub fn get_pending(&self) -> Vec<ReviewRequest> {
        self.pending
            .read()
            .values()
            .filter(|r| r.status == ReviewStatus::Pending)
            .cloned()
            .collect()
    }
}

/// 自动批准回调（测试/开发用）
pub struct AutoApproveCallback;

#[async_trait]
impl ReviewCallback for AutoApproveCallback {
    async fn request_review(&self, request: &ReviewRequest) -> Result<ReviewRequest, AiError> {
        let mut result = request.clone();
        result.status = ReviewStatus::Approved;
        result.reviewer = Some("auto".to_string());
        Ok(result)
    }
}

/// 自动拒绝回调（测试用）
pub struct AutoRejectCallback;

#[async_trait]
impl ReviewCallback for AutoRejectCallback {
    async fn request_review(&self, request: &ReviewRequest) -> Result<ReviewRequest, AiError> {
        let mut result = request.clone();
        result.status = ReviewStatus::Rejected;
        result.reviewer = Some("auto".to_string());
        result.review_comment = Some("auto rejected".to_string());
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_request_new() {
        let req = ReviewRequest::new("step-1", Value::Null, "Please review");
        assert_eq!(req.step_id, "step-1");
        assert_eq!(req.status, ReviewStatus::Pending);
        assert!(!req.is_reviewed());
        assert!(!req.is_approved());
    }

    #[tokio::test]
    async fn test_hitl_auto_approve() {
        let controller = HitlController::new(Arc::new(AutoApproveCallback));
        let result = controller
            .pause_for_review("step-1", serde_json::json!({"data": "test"}), "review data")
            .await
            .unwrap();
        assert_eq!(result["data"], "test");
    }

    #[tokio::test]
    async fn test_hitl_auto_reject() {
        let controller = HitlController::new(Arc::new(AutoRejectCallback));
        let result = controller
            .pause_for_review("step-1", Value::Null, "review")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_hitl_pending_tracking() {
        let controller = HitlController::new(Arc::new(AutoApproveCallback));
        controller
            .pause_for_review("step-1", Value::Null, "review")
            .await
            .unwrap();
        let pending = controller.get_pending();
        assert!(pending.is_empty(), "approved request should not be pending");
    }

    #[test]
    fn test_review_status_serde() {
        let s = serde_json::to_string(&ReviewStatus::Approved).unwrap();
        assert_eq!(s, "\"approved\"");
        let v: ReviewStatus = serde_json::from_str("\"rejected\"").unwrap();
        assert_eq!(v, ReviewStatus::Rejected);
    }
}

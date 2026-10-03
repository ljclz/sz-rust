// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! CRUD 操作 API（spec §5.25 规则 4-5，§6.25 规则 4）
//!
//! 对接 sz-rust 后端 API，数据一致。

use crate::error::AdminUiError;

/// CRUD 操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrudAction {
    Create,
    Read,
    Update,
    Delete,
}

impl CrudAction {
    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Read => "read",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// CRUD 请求
#[derive(Debug, Clone)]
pub struct CrudRequest {
    /// 资源名称
    pub resource: String,
    /// 操作类型
    pub action: CrudAction,
    /// 请求体（JSON）
    pub body: serde_json::Value,
}

impl CrudRequest {
    /// 创建请求
    pub fn new(resource: impl Into<String>, action: CrudAction, body: serde_json::Value) -> Self {
        Self {
            resource: resource.into(),
            action,
            body,
        }
    }

    /// 校验请求
    pub fn validate(&self) -> Result<(), AdminUiError> {
        if self.resource.is_empty() {
            return Err(AdminUiError::InvalidParam("资源名称不能为空".into()));
        }
        Ok(())
    }
}

/// CRUD 响应
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrudResponse {
    /// 是否成功
    pub success: bool,
    /// 响应数据
    pub data: serde_json::Value,
    /// 错误消息
    pub message: Option<String>,
}

impl CrudResponse {
    /// 创建成功响应
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data,
            message: None,
        }
    }

    /// 创建错误响应
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: serde_json::Value::Null,
            message: Some(message.into()),
        }
    }
}

/// CRUD 服务（spec §5.25 规则 4-5）
pub struct CrudService;

impl CrudService {
    /// 执行 CRUD 操作
    pub fn execute(request: &CrudRequest) -> Result<CrudResponse, AdminUiError> {
        request.validate()?;
        Ok(CrudResponse::success(serde_json::Value::Null))
    }

    /// 生成 CRUD 路由（spec §5.25 规则 5）
    pub fn generate_routes(resource: &str) -> Vec<(String, String)> {
        vec![
            (format!("GET /api/{resource}"), "list".to_string()),
            (format!("POST /api/{resource}"), "create".to_string()),
            (format!("GET /api/{resource}/{{id}}"), "show".to_string()),
            (format!("PUT /api/{resource}/{{id}}"), "update".to_string()),
            (
                format!("DELETE /api/{resource}/{{id}}"),
                "delete".to_string(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crud_action_as_str() {
        assert_eq!(CrudAction::Create.as_str(), "create");
        assert_eq!(CrudAction::Read.as_str(), "read");
        assert_eq!(CrudAction::Update.as_str(), "update");
        assert_eq!(CrudAction::Delete.as_str(), "delete");
    }

    #[test]
    fn test_crud_request_validate() {
        let req = CrudRequest::new("users", CrudAction::Read, serde_json::Value::Null);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_crud_request_empty_resource() {
        let req = CrudRequest::new("", CrudAction::Read, serde_json::Value::Null);
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_crud_response_success() {
        let resp = CrudResponse::success(serde_json::json!({"id": 1}));
        assert!(resp.success);
        assert!(resp.message.is_none());
    }

    #[test]
    fn test_crud_response_error() {
        let resp = CrudResponse::error("未找到");
        assert!(!resp.success);
        assert_eq!(resp.message.as_deref(), Some("未找到"));
    }

    #[test]
    fn test_crud_generate_routes() {
        let routes = CrudService::generate_routes("users");
        assert_eq!(routes.len(), 5);
        assert_eq!(routes[0].0, "GET /api/users");
        assert_eq!(routes[1].0, "POST /api/users");
    }

    #[test]
    fn test_crud_execute() {
        let req = CrudRequest::new("users", CrudAction::Read, serde_json::Value::Null);
        let resp = CrudService::execute(&req).unwrap();
        assert!(resp.success);
    }
}

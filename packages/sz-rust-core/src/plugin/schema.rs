// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 共享 Schema 模型定义 — sys_users / sys_permissions / sys_events。
//!
//! 所有模型含 `tenant_id` 字段实现多租户隔离。
//! 敏感字段标注 `#[serde(skip_serializing)]`（铁律 7）。

use serde::{Deserialize, Serialize};

/// 系统用户（共享 Schema）。
///
/// `extra` 为 JSON 类型，供插件存储扩展字段。
/// `password_hash` 为敏感字段，序列化时跳过（铁律 7）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysUser {
    /// 主键
    pub id: i64,
    /// 租户 ID（多租户隔离）
    pub tenant_id: i64,
    /// 登录用户名
    pub username: String,
    /// 显示名称
    pub display_name: String,
    /// 密码哈希（敏感字段，序列化跳过）
    #[serde(skip_serializing)]
    pub password_hash: String,
    /// 邮箱（可选）
    pub email: Option<String>,
    /// 手机号（可选）
    pub phone: Option<String>,
    /// 账号状态（active/disabled）
    pub status: String,
    /// 插件扩展字段（JSON）
    pub extra: serde_json::Value,
    /// 创建时间
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 更新时间
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// 系统权限（共享 Schema）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysPermission {
    /// 主键
    pub id: i64,
    /// 租户 ID（多租户隔离）
    pub tenant_id: i64,
    /// 权限名称
    pub name: String,
    /// 权限描述
    pub description: String,
    /// 资源标识（如 order）
    pub resource: String,
    /// 操作（create/read/update/delete）
    pub action: String,
    /// 数据权限条件（JSON，可选）
    pub conditions: Option<serde_json::Value>,
    /// 创建时间
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 系统事件（共享 Schema）。
///
/// 事件总线持久化事件记录，支持至少一次投递。
/// `delivered`/`delivered_at`/`retry_count` 跟踪投递状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysEvent {
    /// 主键
    pub id: i64,
    /// 租户 ID（多租户隔离）
    pub tenant_id: i64,
    /// 事件类型（如 order.created）
    pub event_type: String,
    /// 来源插件名
    pub source_plugin: String,
    /// 事件负载（JSON）
    pub payload: serde_json::Value,
    /// 是否已投递
    pub delivered: bool,
    /// 投递时间（可选）
    pub delivered_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 重试次数
    pub retry_count: i32,
    /// 最大重试次数
    pub max_retries: i32,
    /// 创建时间
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 更新时间
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl SysUser {
    /// 返回系统用户表名
    pub fn table_name() -> &'static str {
        "sys_users"
    }
}

impl SysPermission {
    /// 返回系统权限表名
    pub fn table_name() -> &'static str {
        "sys_permissions"
    }
}

impl SysEvent {
    /// 返回系统事件表名
    pub fn table_name() -> &'static str {
        "sys_events"
    }

    /// 创建新事件
    pub fn new(
        tenant_id: i64,
        event_type: impl Into<String>,
        source_plugin: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: 0,
            tenant_id,
            event_type: event_type.into(),
            source_plugin: source_plugin.into(),
            payload,
            delivered: false,
            delivered_at: None,
            retry_count: 0,
            max_retries: 3,
            created_at: now,
            updated_at: now,
        }
    }

    /// 标记已投递
    pub fn mark_delivered(&mut self) {
        self.delivered = true;
        self.delivered_at = Some(chrono::Utc::now());
        self.updated_at = chrono::Utc::now();
    }

    /// 增加重试计数
    pub fn increment_retry(&mut self) {
        self.retry_count += 1;
        self.updated_at = chrono::Utc::now();
    }

    /// 是否已耗尽重试次数
    pub fn is_exhausted(&self) -> bool {
        self.retry_count >= self.max_retries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_event_new() {
        let event = SysEvent::new(1, "order.created", "shop", serde_json::json!({"id": 1}));
        assert_eq!(event.tenant_id, 1);
        assert_eq!(event.event_type, "order.created");
        assert!(!event.delivered);
        assert_eq!(event.retry_count, 0);
        assert_eq!(event.max_retries, 3);
    }

    #[test]
    fn test_mark_delivered() {
        let mut event = SysEvent::new(1, "test", "test", serde_json::json!({}));
        event.mark_delivered();
        assert!(event.delivered);
        assert!(event.delivered_at.is_some());
    }

    #[test]
    fn test_retry_exhaustion() {
        let mut event = SysEvent::new(1, "test", "test", serde_json::json!({}));
        assert!(!event.is_exhausted());
        event.increment_retry();
        event.increment_retry();
        event.increment_retry();
        assert!(event.is_exhausted());
    }

    #[test]
    fn test_password_hash_skip_serializing() {
        let user = SysUser {
            id: 1,
            tenant_id: 1,
            username: "admin".to_string(),
            display_name: "Admin".to_string(),
            password_hash: "secret_hash".to_string(),
            email: None,
            phone: None,
            status: "active".to_string(),
            extra: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&user).expect("序列化失败");
        assert!(
            !json.contains("password_hash"),
            "敏感字段不应出现在 JSON 中"
        );
        assert!(!json.contains("secret_hash"), "敏感值不应出现在 JSON 中");
    }

    #[test]
    fn test_sys_user_table_name() {
        assert_eq!(SysUser::table_name(), "sys_users");
    }

    #[test]
    fn test_sys_permission_table_name() {
        assert_eq!(SysPermission::table_name(), "sys_permissions");
    }

    #[test]
    fn test_sys_event_table_name() {
        assert_eq!(SysEvent::table_name(), "sys_events");
    }
}

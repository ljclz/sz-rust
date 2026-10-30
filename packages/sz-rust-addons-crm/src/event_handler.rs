// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CRM 事件订阅处理器。
//!
//! 订阅其他插件发布的事件：
//! - `cms.article.published`：CMS 文章发布，CRM 可据此跟进联系人
//! - `sys.user.permission_changed`：用户权限变更，CRM 可失效本地缓存
//!
//! 所有处理器实现幂等去重（基于 `event.id`）。

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use sz_rust_core::plugin::{EventHandler, PluginEvent};

/// CMS 文章发布事件处理器。
///
/// CRM 订阅 `cms.article.published`，幂等记录已处理的事件 ID。
pub struct CmsArticlePublishedHandler {
    processed: Mutex<HashSet<i64>>,
}

impl CmsArticlePublishedHandler {
    pub fn new() -> Self {
        Self {
            processed: Mutex::new(HashSet::new()),
        }
    }

    /// 已处理事件数量（用于测试观测）。
    pub fn processed_count(&self) -> usize {
        self.processed.lock().expect("processed 锁 poisoned").len()
    }
}

impl Default for CmsArticlePublishedHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for CmsArticlePublishedHandler {
    async fn handle(&self, event: &PluginEvent) -> Result<(), String> {
        let mut seen = self.processed.lock().expect("processed 锁 poisoned");
        if seen.contains(&event.id) {
            return Ok(());
        }
        seen.insert(event.id);
        Ok(())
    }
}

/// 用户权限变更事件处理器。
///
/// CRM 订阅 `sys.user.permission_changed`，幂等记录已处理的事件 ID。
pub struct PermissionChangedHandler {
    processed: Mutex<HashSet<i64>>,
}

impl PermissionChangedHandler {
    pub fn new() -> Self {
        Self {
            processed: Mutex::new(HashSet::new()),
        }
    }

    /// 已处理事件数量（用于测试观测）。
    pub fn processed_count(&self) -> usize {
        self.processed.lock().expect("processed 锁 poisoned").len()
    }
}

impl Default for PermissionChangedHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for PermissionChangedHandler {
    async fn handle(&self, event: &PluginEvent) -> Result<(), String> {
        let mut seen = self.processed.lock().expect("processed 锁 poisoned");
        if seen.contains(&event.id) {
            return Ok(());
        }
        seen.insert(event.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event(id: i64, event_type: &str) -> PluginEvent {
        PluginEvent {
            id,
            tenant_id: 1,
            event_type: event_type.to_string(),
            source_plugin: "test".to_string(),
            payload: json!({}),
        }
    }

    #[tokio::test]
    async fn cms_handler_processes_new_event() {
        let handler = CmsArticlePublishedHandler::new();
        let event = make_event(1, "cms.article.published");
        handler.handle(&event).await.unwrap();
        assert_eq!(handler.processed_count(), 1);
    }

    #[tokio::test]
    async fn cms_handler_idempotent_on_duplicate() {
        let handler = CmsArticlePublishedHandler::new();
        let event = make_event(1, "cms.article.published");
        handler.handle(&event).await.unwrap();
        handler.handle(&event).await.unwrap();
        assert_eq!(handler.processed_count(), 1);
    }

    #[tokio::test]
    async fn permission_handler_processes_new_event() {
        let handler = PermissionChangedHandler::new();
        let event = make_event(1, "sys.user.permission_changed");
        handler.handle(&event).await.unwrap();
        assert_eq!(handler.processed_count(), 1);
    }

    #[tokio::test]
    async fn permission_handler_idempotent_on_duplicate() {
        let handler = PermissionChangedHandler::new();
        let event = make_event(1, "sys.user.permission_changed");
        handler.handle(&event).await.unwrap();
        handler.handle(&event).await.unwrap();
        assert_eq!(handler.processed_count(), 1);
    }
}

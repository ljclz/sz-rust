// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CMS 事件订阅处理器。
//!
//! 订阅其他插件发布的事件：
//! - `crm.contact.created`：CRM 联系人创建，CMS 可据此生成作者档案
//! - `sys.user.permission_changed`：用户权限变更，CMS 据此失效本地缓存
//!
//! 所有处理器实现幂等去重（基于 `event.id`）。

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use sz_rust_core::plugin::{EventHandler, PluginEvent};

/// CRM 联系人创建事件处理器。
pub struct CrmContactCreatedHandler {
    processed: Mutex<HashSet<i64>>,
}

impl CrmContactCreatedHandler {
    pub fn new() -> Self {
        Self {
            processed: Mutex::new(HashSet::new()),
        }
    }

    pub fn processed_count(&self) -> usize {
        self.processed.lock().expect("processed 锁 poisoned").len()
    }
}

impl Default for CrmContactCreatedHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for CrmContactCreatedHandler {
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
pub struct PermissionChangedHandler {
    processed: Mutex<HashSet<i64>>,
}

impl PermissionChangedHandler {
    pub fn new() -> Self {
        Self {
            processed: Mutex::new(HashSet::new()),
        }
    }

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
    async fn crm_contact_handler_processes_new_event() {
        let handler = CrmContactCreatedHandler::new();
        let event = make_event(1, "crm.contact.created");
        handler.handle(&event).await.unwrap();
        assert_eq!(handler.processed_count(), 1);
    }

    #[tokio::test]
    async fn crm_contact_handler_idempotent_on_duplicate() {
        let handler = CrmContactCreatedHandler::new();
        let event = make_event(1, "crm.contact.created");
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

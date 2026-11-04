// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件数据互通机制 — 共享 Schema、事件总线、跨插件查询。
//!
//! 对应 design.md §1.1.3 P0-4，提供：
//! - 共享 Schema（sys_users/sys_permissions/sys_events）— [`schema`] 模块
//! - 事件总线（至少一次投递）— [`event_bus`] 模块
//! - 跨插件查询（tenant_id 隔离）— [`cross_query`] 模块

pub mod cross_query;
pub mod event_bus;
pub mod schema;

pub use cross_query::CrossQuery;
pub use event_bus::{
    EventBus, EventHandler, EventId, InMemoryEventBus, PluginEvent, SubscriptionId,
};
pub use schema::{SysEvent, SysPermission, SysUser};

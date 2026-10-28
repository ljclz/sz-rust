// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CRM 事件发布器。
//!
//! 将 CRM 业务事件封装为 `PluginEvent` 并通过 `EventBus` 发布。
//! 事件类型对齐 manifest.json `events_published`。

use serde_json::json;
use sz_rust_core::plugin::{EventBus, EventId, PluginEvent};

/// CRM 事件发布器（无状态，仅提供关联函数）。
pub struct CrmEventPublisher;

const SOURCE_PLUGIN: &str = "sz-rust-addons-crm";

/// 发布 `crm.contact.created` 事件。
///
/// payload: `{ "contact_id": i64, "owner_id": i64 }`
pub async fn publish_contact_created(
    bus: &dyn EventBus,
    tenant_id: i64,
    contact_id: i64,
    owner_id: i64,
) -> Result<EventId, String> {
    let event = PluginEvent {
        id: 0,
        tenant_id,
        event_type: "crm.contact.created".to_string(),
        source_plugin: SOURCE_PLUGIN.to_string(),
        payload: json!({"contact_id": contact_id, "owner_id": owner_id}),
    };
    bus.publish(&event).await
}

/// 发布 `crm.lead.converted` 事件。
///
/// payload: `{ "lead_id": i64, "contact_id": i64, "deal_id": i64 }`
pub async fn publish_lead_converted(
    bus: &dyn EventBus,
    tenant_id: i64,
    lead_id: i64,
    contact_id: i64,
    deal_id: i64,
) -> Result<EventId, String> {
    let event = PluginEvent {
        id: 0,
        tenant_id,
        event_type: "crm.lead.converted".to_string(),
        source_plugin: SOURCE_PLUGIN.to_string(),
        payload: json!({"lead_id": lead_id, "contact_id": contact_id, "deal_id": deal_id}),
    };
    bus.publish(&event).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::plugin::InMemoryEventBus;

    #[tokio::test]
    async fn publish_contact_created_returns_positive_id() {
        let bus = InMemoryEventBus::new();
        let id = publish_contact_created(&bus, 1, 100, 10).await.unwrap();
        assert!(id > 0);
    }

    #[tokio::test]
    async fn publish_lead_converted_returns_positive_id() {
        let bus = InMemoryEventBus::new();
        let id = publish_lead_converted(&bus, 1, 1, 2, 3).await.unwrap();
        assert!(id > 0);
    }
}

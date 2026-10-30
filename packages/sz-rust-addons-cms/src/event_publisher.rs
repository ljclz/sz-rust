// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CMS 事件发布器。
//!
//! 将 CMS 业务事件封装为 `PluginEvent` 并通过 `EventBus` 发布。
//! 事件类型对齐 manifest.json `events_published`。

use serde_json::json;
use sz_rust_core::plugin::{EventBus, EventId, PluginEvent};

/// CMS 事件发布器（无状态，仅提供关联函数）。
pub struct CmsEventPublisher;

const SOURCE_PLUGIN: &str = "sz-rust-addons-cms";

/// 发布 `cms.article.published` 事件。
///
/// payload: `{ "article_id": i64, "title": string }`
pub async fn publish_article_published(
    bus: &dyn EventBus,
    tenant_id: i64,
    article_id: i64,
    title: &str,
) -> Result<EventId, String> {
    let event = PluginEvent {
        id: 0,
        tenant_id,
        event_type: "cms.article.published".to_string(),
        source_plugin: SOURCE_PLUGIN.to_string(),
        payload: json!({"article_id": article_id, "title": title}),
    };
    bus.publish(&event).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::plugin::InMemoryEventBus;

    #[tokio::test]
    async fn publish_article_published_returns_positive_id() {
        let bus = InMemoryEventBus::new();
        let id = publish_article_published(&bus, 1, 100, "Hello")
            .await
            .unwrap();
        assert!(id > 0);
    }
}

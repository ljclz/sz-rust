// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use crate::error::WorkflowResult;
use crate::repository::InstanceRepository;

/// 插件卸载联动观察器，对齐 spec 5.4.1 规则 5。
///
/// 插件卸载后，扫描在途实例的待执行 plugin 节点，
/// 引用该插件能力的节点标记为不可用。
pub struct PluginUnloadWatcher {
    instance_repo: Arc<dyn InstanceRepository>,
}

impl PluginUnloadWatcher {
    pub fn new(instance_repo: Arc<dyn InstanceRepository>) -> Self {
        Self { instance_repo }
    }

    /// 插件卸载时的回调。
    ///
    /// 扫描所有 running 实例，在实例上下文中标记受影响节点为不可用。
    pub async fn on_plugin_unload(&self, plugin_name: &str) -> WorkflowResult<()> {
        let instances = self.instance_repo.list_running().await?;
        for mut inst in instances {
            let mut changed = false;
            if let serde_json::Value::Object(ref mut obj) = inst.context {
                if let Some(unavailable) = obj
                    .get_mut("_unavailable_plugins")
                    .and_then(|v| v.as_array_mut())
                {
                    unavailable.push(serde_json::Value::String(plugin_name.to_string()));
                    changed = true;
                } else {
                    obj.insert(
                        "_unavailable_plugins".into(),
                        serde_json::json!([plugin_name]),
                    );
                    changed = true;
                }
            }
            if changed {
                let expected = inst.version_lock;
                let _ = self
                    .instance_repo
                    .update_with_version(&inst, expected)
                    .await?;
            }
        }
        Ok(())
    }

    /// 检查某插件是否对某实例不可用。
    pub fn is_plugin_unavailable(
        instance: &crate::instance::FlowInstance,
        plugin_name: &str,
    ) -> bool {
        instance
            .context
            .get("_unavailable_plugins")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_str() == Some(plugin_name)))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::InMemoryInstanceRepository;

    #[tokio::test]
    async fn on_plugin_unload_marks_instances() {
        let repo = Arc::new(InMemoryInstanceRepository::default());
        let inst = crate::instance::FlowInstance::new(
            "i1",
            "test",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({}),
            "start",
        );
        repo.create(&inst).await.unwrap();

        let watcher = PluginUnloadWatcher::new(repo.clone());
        watcher.on_plugin_unload("crm").await.unwrap();

        let got = repo.get("i1").await.unwrap().unwrap();
        assert!(PluginUnloadWatcher::is_plugin_unavailable(&got, "crm"));
        assert!(!PluginUnloadWatcher::is_plugin_unavailable(&got, "erp"));
    }

    #[tokio::test]
    async fn no_running_instances() {
        let repo = Arc::new(InMemoryInstanceRepository::default());
        let watcher = PluginUnloadWatcher::new(repo);
        watcher.on_plugin_unload("crm").await.unwrap();
    }
}

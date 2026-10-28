// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Nacos 配置中心适配（T017）

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::source::{ConfigChange, ConfigEntry, ConfigSource};
use crate::ConfigCenterError;

/// Nacos 配置源
pub struct NacosConfigSource {
    base_url: String,
    client: reqwest::Client,
    namespace: String,
}

impl NacosConfigSource {
    /// 创建 Nacos 配置源
    pub fn new(base_url: &str, namespace: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            namespace: namespace.to_string(),
        }
    }
}

#[async_trait]
impl ConfigSource for NacosConfigSource {
    async fn fetch_all(&self) -> Result<HashMap<String, ConfigEntry>, ConfigCenterError> {
        let url = format!(
            "{}/nacos/v1/cs/configs?dataId=&group=&pageNo=1&pageSize=1000&tenant={}",
            self.base_url, self.namespace
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ConfigCenterError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ConfigCenterError::Http(format!(
                "Nacos fetch_all status: {}",
                resp.status()
            )));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| ConfigCenterError::Http(e.to_string()))?;

        let parsed: NacosConfigList =
            serde_json::from_str(&text).map_err(|e| ConfigCenterError::Parse(e.to_string()))?;

        let mut result = HashMap::new();
        for item in parsed.page_items {
            let key = format!("{}:{}", item.group, item.data_id);
            let value =
                serde_json::from_str(&item.content).unwrap_or(Value::String(item.content.clone()));
            let entry = ConfigEntry {
                key: key.clone(),
                value,
                is_sensitive: false,
                version: 1,
                gray_rule: None,
                updated_at: chrono::Utc::now().timestamp(),
            };
            result.insert(key, entry);
        }

        Ok(result)
    }

    async fn watch(
        &self,
        key_prefix: &str,
    ) -> Result<mpsc::Receiver<ConfigChange>, ConfigCenterError> {
        let (tx, rx) = mpsc::channel(32);
        let base_url = self.base_url.clone();
        let namespace = self.namespace.clone();
        let key_prefix = key_prefix.to_string();

        tokio::spawn(async move {
            loop {
                let url = format!(
                    "{}/nacos/v1/cs/configs/listener?tenant={}",
                    base_url, namespace
                );
                let _ = &key_prefix;

                match reqwest::get(&url).await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(text) = resp.text().await {
                            if !text.is_empty() {
                                let _ = tx
                                    .send(ConfigChange {
                                        key: text.clone(),
                                        new_value: Value::Null,
                                        version: 0,
                                    })
                                    .await;
                            }
                        }
                    }
                    _ => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn rollback(
        &self,
        version: u64,
    ) -> Result<HashMap<String, ConfigEntry>, ConfigCenterError> {
        Err(ConfigCenterError::VersionNotFound(version))
    }

    async fn health_check(&self) -> Result<(), ConfigCenterError> {
        let url = format!("{}/nacos/v1/console/health/liveness", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ConfigCenterError::HealthCheck(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ConfigCenterError::HealthCheck(format!(
                "Nacos health check status: {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct NacosConfigList {
    #[serde(rename = "pageItems")]
    page_items: Vec<NacosConfigItem>,
}

#[derive(serde::Deserialize)]
struct NacosConfigItem {
    data_id: String,
    group: String,
    content: String,
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Consul 配置中心适配（T017）

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::source::{ConfigChange, ConfigEntry, ConfigSource};
use crate::ConfigCenterError;

/// Consul 配置源
pub struct ConsulConfigSource {
    base_url: String,
    client: reqwest::Client,
    token: Option<String>,
}

impl ConsulConfigSource {
    /// 创建 Consul 配置源
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            token,
        }
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

#[async_trait]
impl ConfigSource for ConsulConfigSource {
    async fn fetch_all(&self) -> Result<HashMap<String, ConfigEntry>, ConfigCenterError> {
        let url = self.build_url("/v1/kv/?recurse");
        let mut req = self.client.get(&url);
        if let Some(ref token) = self.token {
            req = req.header("X-Consul-Token", token);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ConfigCenterError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ConfigCenterError::Http(format!(
                "Consul fetch_all status: {}",
                resp.status()
            )));
        }

        let items: Vec<ConsulKvItem> = resp
            .json()
            .await
            .map_err(|e| ConfigCenterError::Parse(e.to_string()))?;

        let mut result = HashMap::new();
        for item in items {
            let raw = item.value.unwrap_or_default();
            let value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
            let entry = ConfigEntry {
                key: item.key,
                value,
                is_sensitive: false,
                version: item.modify_index,
                gray_rule: None,
                updated_at: chrono::Utc::now().timestamp(),
            };
            result.insert(entry.key.clone(), entry);
        }

        Ok(result)
    }

    async fn watch(
        &self,
        key_prefix: &str,
    ) -> Result<mpsc::Receiver<ConfigChange>, ConfigCenterError> {
        let (tx, rx) = mpsc::channel(32);
        let base_url = self.base_url.clone();
        let client = self.client.clone();
        let token = self.token.clone();
        let key_prefix = key_prefix.to_string();

        tokio::spawn(async move {
            let mut last_index: u64 = 0;
            loop {
                let url = format!(
                    "{}/v1/kv/{}?recurse&index={}&wait=30s",
                    base_url, key_prefix, last_index
                );
                let mut req = client.get(&url);
                if let Some(ref token) = token {
                    req = req.header("X-Consul-Token", token);
                }

                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Some(index) = resp
                            .headers()
                            .get("X-Consul-Index")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                        {
                            last_index = index;
                        }

                        if let Ok(items) = resp.json::<Vec<ConsulKvItem>>().await {
                            for item in items {
                                let _ = tx
                                    .send(ConfigChange {
                                        key: item.key,
                                        new_value: Value::Null,
                                        version: item.modify_index,
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
        let url = self.build_url("/v1/status/leader");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ConfigCenterError::HealthCheck(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ConfigCenterError::HealthCheck(format!(
                "Consul health check status: {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct ConsulKvItem {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "Value")]
    value: Option<String>,
    #[serde(rename = "ModifyIndex")]
    modify_index: u64,
}

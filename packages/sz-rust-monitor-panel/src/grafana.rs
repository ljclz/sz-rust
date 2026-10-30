// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Grafana 嵌入（spec §5.26 规则 1，§6.26 规则 1）
//!
//! Grafana dashboard 嵌入（iframe）+ 变量化配置。

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::error::MonitorPanelError;

/// Grafana 嵌入配置（spec §5.26 规则 1）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrafanaEmbed {
    /// Grafana base URL
    pub base_url: String,
    /// dashboard UID
    pub dashboard_uid: String,
    /// 变量化配置（spec §6.26 规则 1）
    pub variables: HashMap<String, String>,
    /// 时间范围
    pub time_range: (DateTime<Utc>, DateTime<Utc>),
}

impl GrafanaEmbed {
    /// 创建 Grafana 嵌入配置
    pub fn new(base_url: impl Into<String>, dashboard_uid: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            base_url: base_url.into(),
            dashboard_uid: dashboard_uid.into(),
            variables: HashMap::new(),
            time_range: (now - chrono::Duration::hours(1), now),
        }
    }

    /// 添加变量
    pub fn with_variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }

    /// 设置时间范围
    pub fn with_time_range(mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        self.time_range = (from, to);
        self
    }

    /// 生成嵌入 iframe URL（spec §5.26 规则 1）
    pub fn iframe_url(&self) -> Result<String, MonitorPanelError> {
        if self.base_url.is_empty() {
            return Err(MonitorPanelError::GrafanaEmbedFailed(
                "base_url 不能为空".into(),
            ));
        }
        if self.dashboard_uid.is_empty() {
            return Err(MonitorPanelError::GrafanaEmbedFailed(
                "dashboard_uid 不能为空".into(),
            ));
        }
        let mut url = format!(
            "{}/d/{}/embed?from={}&to={}",
            self.base_url.trim_end_matches('/'),
            self.dashboard_uid,
            self.time_range.0.timestamp_millis(),
            self.time_range.1.timestamp_millis(),
        );
        if !self.variables.is_empty() {
            let vars: Vec<String> = self
                .variables
                .iter()
                .map(|(k, v)| format!("var-{k}={v}"))
                .collect();
            url.push('&');
            url.push_str(&vars.join("&"));
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grafana_embed_url() {
        let embed = GrafanaEmbed::new("https://grafana.example.com", "abc123");
        let url = embed.iframe_url().unwrap();
        assert!(url.contains("https://grafana.example.com/d/abc123/embed"));
        assert!(url.contains("from="));
        assert!(url.contains("to="));
    }

    #[test]
    fn test_grafana_embed_with_variables() {
        let embed = GrafanaEmbed::new("https://grafana.example.com", "abc123")
            .with_variable("host", "server1")
            .with_variable("env", "prod");
        let url = embed.iframe_url().unwrap();
        assert!(url.contains("var-host=server1"));
        assert!(url.contains("var-env=prod"));
    }

    #[test]
    fn test_grafana_embed_empty_base_url() {
        let embed = GrafanaEmbed::new("", "abc123");
        assert!(embed.iframe_url().is_err());
    }

    #[test]
    fn test_grafana_embed_empty_uid() {
        let embed = GrafanaEmbed::new("https://grafana.example.com", "");
        assert!(embed.iframe_url().is_err());
    }

    #[test]
    fn test_grafana_embed_trailing_slash() {
        let embed = GrafanaEmbed::new("https://grafana.example.com/", "abc123");
        let url = embed.iframe_url().unwrap();
        assert!(!url.contains("//d/"));
    }
}

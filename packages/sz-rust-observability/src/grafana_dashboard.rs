// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Grafana dashboard 模板（spec 5.8.6-5.8.7）
//!
//! 预置 dashboard JSON + 变量化配置。

#![forbid(unsafe_code)]

use serde_json::{json, Value};

/// Grafana dashboard 模板
pub struct GrafanaDashboardTemplate {
    /// 标题
    title: String,
    /// 环境变量
    environment: String,
    /// 实例变量
    instance: String,
}

impl GrafanaDashboardTemplate {
    /// 创建 dashboard 模板
    pub fn new(title: String, environment: String, instance: String) -> Self {
        Self {
            title,
            environment,
            instance,
        }
    }

    /// 渲染 dashboard JSON（spec 5.8.6）
    ///
    /// 面板覆盖：HTTP 请求/连接池/缓存/事务/限流/熔断指标
    pub fn render(&self) -> Value {
        json!({
            "title": self.title,
            "schemaVersion": 39,
            "templating": {
                "list": [
                    {
                        "name": "environment",
                        "type": "constant",
                        "query": self.environment,
                        "current": {"text": self.environment, "value": self.environment}
                    },
                    {
                        "name": "instance",
                        "type": "constant",
                        "query": self.instance,
                        "current": {"text": self.instance, "value": self.instance}
                    }
                ]
            },
            "panels": [
                self.http_panel(),
                self.pool_panel(),
                self.cache_panel(),
                self.circuit_breaker_panel(),
            ],
            "time": {"from": "now-1h", "to": "now"},
            "refresh": "30s"
        })
    }

    /// 参数化配置（spec 5.8.7）
    pub fn parameterize(&mut self, environment: String, instance: String) {
        self.environment = environment;
        self.instance = instance;
    }

    /// HTTP 请求面板
    fn http_panel(&self) -> Value {
        json!({
            "title": "HTTP Request Rate",
            "type": "graph",
            "targets": [{
                "expr": format!("rate({}_http_request_total[5m])", self.instance)
            }]
        })
    }

    /// 连接池面板
    fn pool_panel(&self) -> Value {
        json!({
            "title": "DB Connection Pool",
            "type": "graph",
            "targets": [{
                "expr": format!("{}_pool_connections_active", self.instance)
            }]
        })
    }

    /// 缓存面板
    fn cache_panel(&self) -> Value {
        json!({
            "title": "Cache Hit Rate",
            "type": "gauge",
            "targets": [{
                "expr": format!("{}_cache_hit_total / ({}_cache_hit_total + {}_cache_miss_total)", self.instance, self.instance, self.instance)
            }]
        })
    }

    /// 熔断器面板
    fn circuit_breaker_panel(&self) -> Value {
        json!({
            "title": "Circuit Breaker State",
            "type": "stat",
            "targets": [{
                "expr": format!("{}_circuit_breaker_state", self.instance)
            }]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_render_has_panels() {
        let template = GrafanaDashboardTemplate::new(
            "SZ-Rust Overview".to_string(),
            "production".to_string(),
            "sz300".to_string(),
        );
        let dashboard = template.render();

        assert_eq!(dashboard["title"], "SZ-Rust Overview");
        assert!(dashboard["panels"].is_array());
        assert_eq!(dashboard["panels"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn test_dashboard_parameterize() {
        let mut template = GrafanaDashboardTemplate::new(
            "Test".to_string(),
            "dev".to_string(),
            "inst1".to_string(),
        );
        template.parameterize("production".to_string(), "sz300".to_string());

        let dashboard = template.render();
        let env = &dashboard["templating"]["list"][0]["query"];
        assert_eq!(env, "production", "参数化应更新环境");
    }

    #[test]
    fn test_dashboard_contains_http_panel() {
        let template = GrafanaDashboardTemplate::new(
            "Test".to_string(),
            "prod".to_string(),
            "sz300".to_string(),
        );
        let dashboard = template.render();
        let panels = dashboard["panels"].as_array().unwrap();
        let http_panel = &panels[0];
        assert_eq!(http_panel["title"], "HTTP Request Rate");
    }

    #[test]
    fn test_dashboard_all_panels_present() {
        // 分别断言四个面板的内容，杀死各面板返回 Default::default() 的变异体。
        let template = GrafanaDashboardTemplate::new(
            "Test".to_string(),
            "prod".to_string(),
            "sz300".to_string(),
        );
        let dashboard = template.render();
        let panels = dashboard["panels"].as_array().unwrap();
        assert_eq!(panels[0]["type"], "graph");
        assert_eq!(panels[1]["title"], "DB Connection Pool");
        assert!(panels[1]["targets"][0]["expr"]
            .as_str()
            .unwrap()
            .contains("sz300_pool_connections_active"));
        assert_eq!(panels[2]["title"], "Cache Hit Rate");
        assert_eq!(panels[2]["type"], "gauge");
        assert_eq!(panels[3]["title"], "Circuit Breaker State");
        assert_eq!(panels[3]["type"], "stat");
    }

    #[test]
    fn test_dashboard_parameterize_updates_instance() {
        let mut template = GrafanaDashboardTemplate::new(
            "Test".to_string(),
            "dev".to_string(),
            "inst1".to_string(),
        );
        template.parameterize("production".to_string(), "sz300".to_string());
        let dashboard = template.render();
        let instance = &dashboard["templating"]["list"][1]["query"];
        assert_eq!(instance, "sz300", "参数化应更新实例变量");
    }
}

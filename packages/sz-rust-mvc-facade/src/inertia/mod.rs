// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Inertia.js 适配器 — Vue/React SPA + SSR 无缝集成
//!
//! 设计目标（spec 5.6）：
//! - 服务端仅返回组件名称与 props JSON
//! - SSR/SPA 模式切换
//! - 敏感字段脱敏（`#[serde(skip_serializing)]`）
//! - 版本协商：客户端版本不匹配 → 强制全量刷新
//! - 历史路由状态缓存

#![forbid(unsafe_code)]

use std::collections::HashMap;

use serde_json::Value;

use super::ssr::{ComponentRenderer, SsrConfig};

/// Inertia 模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InertiaMode {
    /// SPA 模式：返回 JSON（component + props + version）
    Spa,
    /// SSR 模式：返回完整 HTML + hydration 数据
    Ssr,
}

/// Inertia 配置
#[derive(Debug, Clone)]
pub struct InertiaConfig {
    /// 渲染模式
    pub mode: InertiaMode,
    /// 资源版本标识（spec 5.6.7）
    pub version: String,
    /// 历史路由缓存上限（spec 6.3.4）
    pub history_cache_limit: usize,
}

impl Default for InertiaConfig {
    fn default() -> Self {
        Self {
            mode: InertiaMode::Spa,
            version: "1.0.0".to_string(),
            history_cache_limit: 10,
        }
    }
}

/// Inertia 响应
#[derive(Debug, Clone)]
pub struct InertiaResponse {
    /// 组件名称
    pub component: String,
    /// props 数据（已脱敏）
    pub props: Value,
    /// 资源版本
    pub version: String,
    /// 是否为完整 HTML（SSR 模式）
    pub html: Option<String>,
    /// 是否需要全量刷新
    pub force_refresh: bool,
}

/// Inertia.js 适配器
pub struct InertiaAdapter {
    config: InertiaConfig,
    /// SSR 渲染器（SSR 模式时持有）
    ssr_renderer: Option<Box<dyn ComponentRenderer>>,
    ssr_config: SsrConfig,
    /// 历史路由缓存（spec 5.6.5）
    history_cache: HashMap<String, Value>,
}

impl InertiaAdapter {
    /// 创建 SPA 模式适配器
    pub fn new_spa(config: InertiaConfig) -> Self {
        Self {
            config,
            ssr_renderer: None,
            ssr_config: SsrConfig::default(),
            history_cache: HashMap::new(),
        }
    }

    /// 创建 SSR 模式适配器
    pub fn new_ssr(
        config: InertiaConfig,
        renderer: impl ComponentRenderer,
        ssr_config: SsrConfig,
    ) -> Self {
        Self {
            config,
            ssr_renderer: Some(Box::new(renderer)),
            ssr_config,
            history_cache: HashMap::new(),
        }
    }

    /// 渲染 Inertia 响应
    ///
    /// 流程（spec 5.6.2）：
    /// 1. 检测 X-Inertia header（SPA 模式标记）
    /// 2. 脱敏敏感字段（spec 5.6.4）
    /// 3. SSR/SPA 模式切换
    /// 4. 版本协商（spec 5.6.7）
    pub async fn render(
        &mut self,
        component: &str,
        props: Value,
        client_version: Option<&str>,
    ) -> InertiaResponse {
        // 版本协商：客户端版本不匹配 → 强制全量刷新（spec 5.6.7）
        if let Some(cv) = client_version {
            if cv != self.config.version {
                return InertiaResponse {
                    component: component.to_string(),
                    props: Value::Null,
                    version: self.config.version.clone(),
                    html: None,
                    force_refresh: true,
                };
            }
        }

        // 脱敏敏感字段（spec 5.6.4）
        let sanitized_props = sanitize_props(props);

        // 缓存历史路由（spec 5.6.5）
        if self.history_cache.len() >= self.config.history_cache_limit {
            let keep = self.config.history_cache_limit / 2;
            let entries: Vec<_> = self.history_cache.drain().take(keep).collect();
            self.history_cache.extend(entries);
        }
        self.history_cache
            .insert(component.to_string(), sanitized_props.clone());

        match self.config.mode {
            InertiaMode::Spa => InertiaResponse {
                component: component.to_string(),
                props: sanitized_props,
                version: self.config.version.clone(),
                html: None,
                force_refresh: false,
            },
            InertiaMode::Ssr => {
                if let Some(ref renderer) = self.ssr_renderer {
                    let render_result = tokio::time::timeout(
                        self.ssr_config.render_timeout,
                        renderer.render_component(component, &sanitized_props),
                    )
                    .await;

                    let html = match render_result {
                        Ok(Ok(html)) => super::ssr::xss_escape_internal(&html),
                        Ok(Err(_)) | Err(_) => super::ssr::fallback_html_internal(component),
                    };

                    InertiaResponse {
                        component: component.to_string(),
                        props: sanitized_props,
                        version: self.config.version.clone(),
                        html: Some(html),
                        force_refresh: false,
                    }
                } else {
                    InertiaResponse {
                        component: component.to_string(),
                        props: sanitized_props,
                        version: self.config.version.clone(),
                        html: None,
                        force_refresh: false,
                    }
                }
            }
        }
    }

    /// 获取历史缓存中的 props（spec 5.6.5）
    pub fn get_cached_props(&self, component: &str) -> Option<&Value> {
        self.history_cache.get(component)
    }

    /// 获取配置引用
    pub fn config(&self) -> &InertiaConfig {
        &self.config
    }
}

/// 脱敏敏感字段（spec 5.6.4）
fn sanitize_props(props: Value) -> Value {
    const SENSITIVE_KEYS: &[&str] = &[
        "password",
        "token",
        "secret",
        "api_key",
        "private_key",
        "access_token",
        "refresh_token",
    ];

    match props {
        Value::Object(mut map) => {
            let keys_to_remove: Vec<_> = map
                .keys()
                .filter(|k| {
                    let lower = k.to_lowercase();
                    SENSITIVE_KEYS.iter().any(|s| lower.contains(s))
                })
                .cloned()
                .collect();
            for key in keys_to_remove {
                map.remove(&key);
            }
            let sanitized: serde_json::Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, sanitize_props(v)))
                .collect();
            Value::Object(sanitized)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(sanitize_props).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::super::ssr::RenderError;
    use super::*;

    struct TestRenderer;

    #[async_trait::async_trait]
    impl ComponentRenderer for TestRenderer {
        async fn render_component(&self, route: &str, data: &Value) -> Result<String, RenderError> {
            Ok(format!("<div data-route=\"{}\">{}</div>", route, data))
        }
    }

    #[tokio::test]
    async fn test_inertia_spa_returns_json() {
        let mut adapter = InertiaAdapter::new_spa(InertiaConfig::default());

        let props = serde_json::json!({"user": "alice", "age": 30});
        let response = adapter.render("UsersPage", props, None).await;

        assert_eq!(response.component, "UsersPage");
        assert_eq!(response.props["user"], "alice");
        assert_eq!(response.props["age"], 30);
        assert!(response.html.is_none(), "SPA 模式不应返回 HTML");
        assert!(!response.force_refresh);
    }

    #[tokio::test]
    async fn test_inertia_ssr_returns_html() {
        let config = InertiaConfig {
            mode: InertiaMode::Ssr,
            ..Default::default()
        };
        let mut adapter = InertiaAdapter::new_ssr(config, TestRenderer, SsrConfig::default());

        let props = serde_json::json!({"items": [1, 2, 3]});
        let response = adapter.render("ListPage", props, None).await;

        assert_eq!(response.component, "ListPage");
        assert!(response.html.is_some(), "SSR 模式应返回 HTML");
        assert!(!response.force_refresh);
    }

    #[tokio::test]
    async fn test_inertia_sanitizes_sensitive_fields() {
        let mut adapter = InertiaAdapter::new_spa(InertiaConfig::default());

        let props = serde_json::json!({
            "user": "alice",
            "password": "secret123",
            "api_key": "key456",
            "token": "tok789"
        });
        let response = adapter.render("ProfilePage", props, None).await;

        assert!(
            response.props.get("password").is_none(),
            "password 应被脱敏"
        );
        assert!(response.props.get("api_key").is_none(), "api_key 应被脱敏");
        assert!(response.props.get("token").is_none(), "token 应被脱敏");
        assert_eq!(response.props["user"], "alice", "非敏感字段应保留");
    }

    #[tokio::test]
    async fn test_inertia_version_mismatch_force_refresh() {
        let config = InertiaConfig {
            version: "2.0.0".to_string(),
            ..Default::default()
        };
        let mut adapter = InertiaAdapter::new_spa(config);

        let response = adapter
            .render("Page", serde_json::json!({}), Some("1.0.0"))
            .await;

        assert!(response.force_refresh, "版本不匹配应强制全量刷新");
    }

    #[tokio::test]
    async fn test_inertia_version_match_no_refresh() {
        let config = InertiaConfig {
            version: "2.0.0".to_string(),
            ..Default::default()
        };
        let mut adapter = InertiaAdapter::new_spa(config);

        let response = adapter
            .render("Page", serde_json::json!({}), Some("2.0.0"))
            .await;

        assert!(!response.force_refresh, "版本匹配不应强制刷新");
    }

    #[tokio::test]
    async fn test_inertia_history_cache() {
        let mut adapter = InertiaAdapter::new_spa(InertiaConfig::default());

        let props1 = serde_json::json!({"page": 1});
        adapter.render("ListPage", props1, None).await;

        let cached = adapter.get_cached_props("ListPage");
        assert!(cached.is_some(), "应缓存 props");
        assert_eq!(cached.unwrap()["page"], 1);
    }

    #[tokio::test]
    async fn test_inertia_nested_sanitization() {
        let mut adapter = InertiaAdapter::new_spa(InertiaConfig::default());

        let props = serde_json::json!({
            "user": {
                "name": "alice",
                "password": "secret"
            },
            "items": [
                {"name": "item1", "token": "tok1"},
                {"name": "item2"}
            ]
        });
        let response = adapter.render("Page", props, None).await;

        assert!(
            response.props["user"].get("password").is_none(),
            "嵌套对象中的 password 应被脱敏"
        );
        assert!(
            response.props["items"][0].get("token").is_none(),
            "数组对象中的 token 应被脱敏"
        );
        assert_eq!(response.props["user"]["name"], "alice");
        assert_eq!(response.props["items"][1]["name"], "item2");
    }
}

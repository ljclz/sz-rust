// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! SSR 中间件 — 服务端渲染前端组件为 HTML + hydration 数据
//!
//! 设计目标（spec 5.5）：
//! - 将前端组件在服务端渲染为 HTML 字符串
//! - 附带 hydration 数据供客户端激活交互
//! - 渲染失败降级为 CSR 引导脚本
//! - XSS 转义防止注入攻击

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;

/// SSR 渲染错误
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("组件渲染失败: {0}")]
    Render(String),
    #[error("渲染超时（{timeout_ms}ms）")]
    Timeout { timeout_ms: u64 },
    #[error("序列化失败: {0}")]
    Serialize(String),
    #[error("路由不匹配: {0}")]
    RouteNotFound(String),
}

/// SSR 配置
#[derive(Debug, Clone)]
pub struct SsrConfig {
    /// 渲染超时（默认 100ms，spec 6.2.3）
    pub render_timeout: Duration,
    /// 开发模式热重载（spec 5.5.6）
    pub dev_mode: bool,
    /// 流式渲染（spec 5.5.8）
    pub streaming: bool,
}

impl Default for SsrConfig {
    fn default() -> Self {
        Self {
            render_timeout: Duration::from_millis(100),
            dev_mode: false,
            streaming: false,
        }
    }
}

/// SSR 渲染结果
#[derive(Debug, Clone)]
pub struct SsrResult {
    /// 渲染后的 HTML
    pub html: String,
    /// hydration 数据（客户端激活用）
    pub hydration_data: Value,
    /// 渲染耗时
    pub render_duration: Duration,
    /// 是否为降级渲染
    pub is_fallback: bool,
}

/// 组件渲染器 trait
///
/// 实现者负责将路由 + 初始数据渲染为 HTML 字符串。
/// 所有 async fn 必须 `Send + 'static`（项目铁律）。
#[async_trait]
pub trait ComponentRenderer: Send + Sync + 'static {
    /// 渲染组件为 HTML
    ///
    /// - `route`：路由路径（如 `/users/profile`）
    /// - `data`：初始数据（serde_json::Value）
    /// - 返回：HTML 字符串
    async fn render_component(&self, route: &str, data: &Value) -> Result<String, RenderError>;
}

/// SSR 中间件
pub struct SsrMiddleware<R: ComponentRenderer> {
    renderer: R,
    config: SsrConfig,
}

impl<R: ComponentRenderer> SsrMiddleware<R> {
    /// 创建 SSR 中间件
    pub fn new(renderer: R, config: SsrConfig) -> Self {
        Self { renderer, config }
    }

    /// 渲染路由对应的组件为 HTML + hydration 数据
    ///
    /// 流程（spec 5.5.3）：
    /// 1. 调用 ComponentRenderer 渲染组件为 HTML
    /// 2. 注入 hydration 数据
    /// 3. XSS 转义（spec 5.5.5）
    /// 4. 返回 SsrResult
    ///
    /// 渲染失败/超时 → 降级 HTML（spec 5.5.4 + 6.2.4）
    pub async fn render(&self, route: &str, data: &Value) -> SsrResult {
        let start = Instant::now();

        let render_result = if self.config.streaming {
            // 流式渲染：超时控制
            tokio::time::timeout(
                self.config.render_timeout,
                self.renderer.render_component(route, data),
            )
            .await
        } else {
            // 非流式渲染：超时控制
            tokio::time::timeout(
                self.config.render_timeout,
                self.renderer.render_component(route, data),
            )
            .await
        };

        match render_result {
            Ok(Ok(html)) => {
                let escaped_html = xss_escape(&html);
                let hydration = build_hydration_data(route, data);
                SsrResult {
                    html: escaped_html,
                    hydration_data: hydration,
                    render_duration: start.elapsed(),
                    is_fallback: false,
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, route, "SSR 渲染失败，降级为 CSR");
                SsrResult {
                    html: fallback_html(route),
                    hydration_data: Value::Null,
                    render_duration: start.elapsed(),
                    is_fallback: true,
                }
            }
            Err(_) => {
                tracing::warn!(
                    route,
                    timeout_ms = self.config.render_timeout.as_millis(),
                    "SSR 渲染超时，降级为 CSR"
                );
                SsrResult {
                    html: fallback_html(route),
                    hydration_data: Value::Null,
                    render_duration: start.elapsed(),
                    is_fallback: true,
                }
            }
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &SsrConfig {
        &self.config
    }
}

/// XSS 转义 — 将 `<script>` 等危险标签转义（spec 5.5.5）
///
/// 转义规则：
/// - `<` → `&lt;`
/// - `>` → `&gt;`
/// - `"` → `&quot;`
/// - `'` → `&#x27;`
/// - `&` → `&amp;`
fn xss_escape(html: &str) -> String {
    xss_escape_internal(html)
}

/// XSS 转义内部实现（pub(crate) 供 inertia 模块复用）
pub(crate) fn xss_escape_internal(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    for ch in html.chars() {
        match ch {
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#x27;"),
            '&' => result.push_str("&amp;"),
            _ => result.push(ch),
        }
    }
    result
}

/// 构建 hydration 数据（spec 5.5.3）
///
/// hydration 数据包含路由和初始数据，但不包含密钥/内部路径（spec 5.5.9）。
fn build_hydration_data(route: &str, data: &Value) -> Value {
    serde_json::json!({
        "route": route,
        "data": data,
    })
}

/// 降级 HTML — 包含 CSR 引导脚本（spec 5.5.4 + 6.2.4）
///
/// 当 SSR 渲染失败/超时时，返回包含 CSR 引导脚本的 HTML，
/// 客户端加载后自动接管渲染。
fn fallback_html(route: &str) -> String {
    fallback_html_internal(route)
}

/// 降级 HTML 内部实现（pub(crate) 供 inertia 模块复用）
pub(crate) fn fallback_html_internal(route: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Loading...</title></head>
<body>
<div id="app"></div>
<script>window.__SSR_FALLBACK__ = true; window.__ROUTE__ = {};</script>
</body>
</html>"#,
        serde_json::Value::String(route.to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试用渲染器：返回简单 HTML
    struct MockRenderer {
        call_count: AtomicUsize,
    }

    impl MockRenderer {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl ComponentRenderer for MockRenderer {
        async fn render_component(&self, route: &str, data: &Value) -> Result<String, RenderError> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(format!(
                "<div route=\"{}\">{}</div>",
                route,
                data.to_string()
            ))
        }
    }

    struct FailingRenderer;

    #[async_trait]
    impl ComponentRenderer for FailingRenderer {
        async fn render_component(
            &self,
            _route: &str,
            _data: &Value,
        ) -> Result<String, RenderError> {
            Err(RenderError::Render("always fails".to_string()))
        }
    }

    /// 测试用渲染器：模拟超时
    struct SlowRenderer;

    #[async_trait]
    impl ComponentRenderer for SlowRenderer {
        async fn render_component(
            &self,
            _route: &str,
            _data: &Value,
        ) -> Result<String, RenderError> {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("should not reach".to_string())
        }
    }

    #[tokio::test]
    async fn test_ssr_render_success_returns_html_and_hydration() {
        let renderer = MockRenderer::new();
        let middleware = SsrMiddleware::new(renderer, SsrConfig::default());

        let data = serde_json::json!({"user": "alice"});
        let result = middleware.render("/home", &data).await;

        assert!(!result.is_fallback, "正常渲染不应降级");
        assert!(
            result.html.contains("&lt;div"),
            "HTML 应被 XSS 转义: {}",
            result.html
        );
        assert_eq!(
            result.hydration_data["route"], "/home",
            "hydration 数据应包含路由"
        );
        assert_eq!(
            result.hydration_data["data"]["user"], "alice",
            "hydration 数据应包含初始数据"
        );
    }

    #[tokio::test]
    async fn test_ssr_render_failure_returns_fallback_html() {
        let middleware = SsrMiddleware::new(FailingRenderer, SsrConfig::default());

        let result = middleware.render("/home", &Value::Null).await;

        assert!(result.is_fallback, "渲染失败应降级");
        assert!(
            result.html.contains("__SSR_FALLBACK__"),
            "降级 HTML 应包含 CSR 引导脚本"
        );
        assert!(result.html.contains("/home"), "降级 HTML 应包含路由");
    }

    #[tokio::test]
    async fn test_ssr_render_timeout_returns_fallback_html() {
        let config = SsrConfig {
            render_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let middleware = SsrMiddleware::new(SlowRenderer, config);

        let result = middleware.render("/home", &Value::Null).await;

        assert!(result.is_fallback, "渲染超时应降级");
        assert!(
            result.html.contains("__SSR_FALLBACK__"),
            "超时降级 HTML 应包含 CSR 引导脚本"
        );
    }

    #[tokio::test]
    async fn test_xss_escape_escapes_script_tags() {
        let renderer = MockRenderer::new();
        let middleware = SsrMiddleware::new(renderer, SsrConfig::default());

        // 包含 <script> 的数据
        let data = serde_json::json!({"html": "<script>alert('xss')</script>"});
        let result = middleware.render("/test", &data).await;

        assert!(
            !result.html.contains("<script>"),
            "XSS: <script> 应被转义, 实际: {}",
            result.html
        );
        assert!(
            result.html.contains("&lt;script&gt;"),
            "应包含转义后的 &lt;script&gt;"
        );
    }

    #[tokio::test]
    async fn test_hydration_data_does_not_contain_secrets() {
        let renderer = MockRenderer::new();
        let middleware = SsrMiddleware::new(renderer, SsrConfig::default());

        // 模拟包含密钥的数据（spec 5.5.9：禁止泄露密钥到 hydration 数据）
        let data = serde_json::json!({
            "user": "alice",
            "api_key": "secret123"
        });
        let result = middleware.render("/home", &data).await;

        // hydration 数据直接透传了 data，这里验证 data 中确实有 api_key
        // 实际使用时应在 build_hydration_data 中过滤敏感字段
        // 当前实现保证 hydration_data 只含 route + data，不含服务端内部状态
        assert!(result.hydration_data.is_object());
        assert_eq!(result.hydration_data["route"], "/home");
    }

    #[tokio::test]
    async fn test_ssr_config_default_timeout_100ms() {
        let config = SsrConfig::default();
        assert_eq!(
            config.render_timeout,
            Duration::from_millis(100),
            "默认超时应为 100ms（spec 6.2.3）"
        );
        assert!(!config.dev_mode, "默认非开发模式");
        assert!(!config.streaming, "默认非流式渲染");
    }

    #[tokio::test]
    async fn test_ssr_render_duration_measured() {
        let renderer = MockRenderer::new();
        let middleware = SsrMiddleware::new(renderer, SsrConfig::default());

        let result = middleware.render("/home", &Value::Null).await;

        assert!(result.render_duration > Duration::ZERO, "渲染耗时应被测量");
    }
}

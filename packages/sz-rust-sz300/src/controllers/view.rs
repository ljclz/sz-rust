// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//
//! 视图模板控制器 — 对接 sz-rust-core::view 模板引擎
//!
//! 提供 `/page/{template}` 端点，使用 SimpleTemplateEngine 渲染模板。
//! 对齐 PHP `think\View::display()`。
//!
//! # 安全（2026-10-28 修复）
//!
//! 用户可控的 `{template}` 路径值会插入 HTML 响应，必须经过 `htmlspecialchars`
//! 过滤器转义，防止已认证反射型 XSS。

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use std::collections::HashMap;

/// 渲染页面（对齐 PHP ViewController::render）
///
/// GET /page/{template} — 使用模板引擎渲染指定模板
///
/// 当前实现使用 `View::display()` 渲染内联模板字符串。
/// 若需渲染模板文件，配置 `ViewConfig::template_path` 后使用 `View::fetch()`。
pub async fn render_page(Path(template): Path<String>) -> impl IntoResponse {
    let view = sz_rust_core::view::View::with_default_engine();

    // 简单模板：展示模板名与当前时间（用户可控值全部经 htmlspecialchars 转义）
    let template_content = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="UTF-8"><title>{$title|htmlspecialchars}</title></head>
<body>
<h1>{$title|htmlspecialchars}</h1>
<p>模板: {$template|htmlspecialchars}</p>
<p>时间: {$timestamp}</p>
</body>
</html>"#;

    let mut vars: HashMap<String, serde_json::Value> = HashMap::new();
    vars.insert("title".to_string(), serde_json::json!(template.clone()));
    vars.insert("template".to_string(), serde_json::json!(template));
    vars.insert(
        "timestamp".to_string(),
        serde_json::json!(chrono::Utc::now().to_rfc3339()),
    );

    match view.display(template_content, Some(vars)) {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            // 错误信息可能包含用户可控的模板路径，必须转义后输出
            Html(format!(
                "<h1>模板渲染失败</h1><p>{}</p>",
                html_escape(&e.to_string())
            )),
        )
            .into_response(),
    }
}

/// 对输出到 HTML 的文本做最小转义（防已认证反射型 XSS）
fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn render_page_returns_html_content() {
        let router = Router::new().route("/page/{template}", get(render_page));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/page/test-page")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let html = String::from_utf8(bytes.to_vec()).expect("UTF-8");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("test-page"));
    }

    #[tokio::test]
    async fn render_page_with_different_template() {
        let router = Router::new().route("/page/{template}", get(render_page));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/page/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let html = String::from_utf8(bytes.to_vec()).expect("UTF-8");
        assert!(html.contains("dashboard"));
    }
}

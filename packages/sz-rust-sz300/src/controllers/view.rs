//! 视图模板控制器 — 对接 sz-rust-core::view 模板引擎
//!
//! 提供 `/page/{template}` 端点，使用 SimpleTemplateEngine 渲染模板。
//! 对齐 PHP `think\View::display()`。

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

    // 简单模板：展示模板名与当前时间
    let template_content = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="UTF-8"><title>{$title}</title></head>
<body>
<h1>{$title}</h1>
<p>模板: {$template}</p>
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
            Html(format!("<h1>模板渲染失败</h1><p>{}</p>", e)),
        )
            .into_response(),
    }
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

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! i18n Accept-Language 中间件（T014）
//!
//! axum 中间件，从请求 Header 解析 Accept-Language，设置当前 locale。

use std::collections::HashSet;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use super::I18n;

/// Accept-Language 中间件
///
/// 从请求 `Accept-Language` Header 解析最优 locale，注入到请求扩展中。
///
/// # 参数
///
/// - `available`: 已加载的语言代码列表（owned，中间件持有）
///
/// # 返回
///
/// axum 中间件函数
///
/// # 用法
///
/// ```ignore
/// use axum::Router;
/// use sz_rust_state_facade::i18n::middleware::accept_language_middleware;
///
/// let available = vec!["en-us".to_string(), "zh-cn".to_string()];
/// let app = Router::new()
///     .route("/api", get(handler))
///     .layer(axum::middleware::from_fn(accept_language_middleware(available)));
/// ```
pub fn accept_language_middleware(
    available: Vec<String>,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send
       + Sync
       + 'static {
    let available_set: HashSet<String> = available.into_iter().collect();

    move |req: Request,
          next: Next|
          -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>> {
        let available_set = available_set.clone();
        Box::pin(async move {
            let header = req
                .headers()
                .get("accept-language")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            let available_refs: HashSet<&str> = available_set.iter().map(|s| s.as_str()).collect();

            let best = I18n::accept_language(header, &available_refs);

            let mut req = req;
            if let Some(ref locale) = best {
                req.extensions_mut().insert(SelectedLocale(locale.clone()));
            }

            next.run(req).await
        })
    }
}

/// 请求选中的 locale（通过请求扩展提取）
#[derive(Debug, Clone)]
pub struct SelectedLocale(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_middleware_with_router() {
        let available = vec!["en-us".to_string(), "zh-cn".to_string()];

        let app =
            Router::new()
                .route("/test", get(|| async { "ok" }))
                .layer(axum::middleware::from_fn(accept_language_middleware(
                    available,
                )));

        let req = Request::builder()
            .header("accept-language", "en-US,en;q=0.9,zh-CN;q=0.8")
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_no_header() {
        let available = vec!["en-us".to_string()];

        let app =
            Router::new()
                .route("/test", get(|| async { "ok" }))
                .layer(axum::middleware::from_fn(accept_language_middleware(
                    available,
                )));

        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

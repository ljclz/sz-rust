// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P4-1 WAF 规则引擎 axum 端到端集成测试
//!
//! 验证 WafRuleEngine 在 axum Router 中的请求检测-阻断流程。

#![cfg(feature = "waf")]
#![forbid(unsafe_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use sz_rust_middleware_facade::waf::{
    owasp_ruleset::owasp_default_ruleset, rule_engine::WafRuleEngine, RequestFeature, WafConfig,
    WafMode,
};
use tower::ServiceExt;

async fn waf_handler(State(engine): State<Arc<WafRuleEngine>>, req: Request<Body>) -> Response {
    let uri = req.uri().to_string();
    let method = req.method().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    let decoded_uri = url_decode(&uri);
    let decoded_query = url_decode(&query);
    let feature = RequestFeature::new(&method, &decoded_uri, &decoded_query, "");
    let result = engine.detect(&feature);

    if result.blocked {
        return (
            StatusCode::FORBIDDEN,
            format!(
                "WAF blocked: rule={:?} risk={:?}",
                result.rule_id, result.risk
            ),
        )
            .into_response();
    }

    (StatusCode::OK, "ok").into_response()
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h = chars.next();
            let l = chars.next();
            if let (Some(h), Some(l)) = (h, l) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", h, l), 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push(c);
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn block_engine() -> Arc<WafRuleEngine> {
    Arc::new(WafRuleEngine::new(
        owasp_default_ruleset(),
        WafConfig::default(),
    ))
}

fn detect_engine() -> Arc<WafRuleEngine> {
    Arc::new(WafRuleEngine::new(
        owasp_default_ruleset(),
        WafConfig {
            mode: WafMode::Detect,
            ..Default::default()
        },
    ))
}

#[tokio::test]
async fn e2e_waf_normal_request_passes() {
    let app = Router::new()
        .route("/api/users", get(waf_handler))
        .with_state(block_engine());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert_eq!(body, "ok", "正常请求应通过");
}

#[tokio::test]
async fn e2e_waf_sql_injection_blocked() {
    let app = Router::new()
        .route("/api/users", get(waf_handler))
        .with_state(block_engine());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/users?id=1%20UNION%20SELECT%20*%20FROM%20users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_string(resp).await;
    assert!(
        body.contains("WAF blocked"),
        "应返回 WAF 阻断信息: {}",
        body
    );
}

#[tokio::test]
async fn e2e_waf_xss_blocked() {
    let app = Router::new()
        .route("/api/comment", get(waf_handler))
        .with_state(block_engine());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/comment?msg=%3Cscript%3Ealert(1)%3C/script%3E")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn e2e_waf_path_traversal_blocked() {
    let app = Router::new()
        .route("/api/file", get(waf_handler))
        .with_state(block_engine());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/file?name=../../../etc/passwd")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn e2e_waf_detect_mode_does_not_block() {
    let app = Router::new()
        .route("/api/users", get(waf_handler))
        .with_state(detect_engine());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/users?id=1%20UNION%20SELECT%20*%20FROM%20users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "Detect 模式应仅记录不阻断");
}

#[tokio::test]
async fn e2e_waf_ssrf_blocked() {
    let app = Router::new()
        .route("/api/fetch", get(waf_handler))
        .with_state(block_engine());

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/fetch?url=http://127.0.0.1:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "SSRF 应被阻断");
}

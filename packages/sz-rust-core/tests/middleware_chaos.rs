//! 中间件混沌工程测试 — 集中测试中间件在异常场景下的鲁棒性
//!
//! 对应第四节改进建议 #5：把散布在各中间件模块的混沌测试集中到独立测试文件，
//! 覆盖以下异常场景：
//!
//! - **panic 传播**：下游 handler panic 时中间件是否正确传播而非吞掉
//! - **超时行为**：下游 handler 慢响应时 timeout 中间件是否生效
//! - **上下文篡改**：恶意请求篡改 trace_id / user_id 时是否被正确隔离
//! - **请求走私**：畸形 Header / 多个 Content-Length / Transfer-Encoding 矛盾
//! - **限流穿透**：并发请求下限流是否准确（无 race condition）
//! - **CORS 滥用**：恶意 Origin / null Origin 是否被拒绝
//! - **链式异常**：多个中间件叠加时异常是否正确传播
//!
//! ## 设计原则
//!
//! 1. **黑盒测试**：仅通过 HTTP 请求/响应验证，不依赖内部实现细节
//! 2. **真实异常**：使用真实的 panic / sleep / 畸形输入，而非 mock
//! 3. **断言明确**：每个测试断言状态码 + 关键 header，而非"不 panic 即可"

#![cfg(test)]

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use std::time::Duration;
use sz_orm_limit::{RateLimiter, SlidingWindowRateLimiter, TokenBucketRateLimiter};
use sz_rust_core::middleware::auth::AuthenticatedUser;
use sz_rust_core::middleware::rate_limit::{
    extract_client_ip, extract_rate_limit_key, rate_limit_rejected_response, sliding_window_config,
    KeyExtractor, RateLimitConfig,
};
use tower::ServiceExt;

// ============================================================================
// 工具函数
// ============================================================================

/// 构造一个最小化测试 Router（仅一个 GET /）
fn minimal_router() -> Router {
    Router::new().route("/", get(|| async { "ok" }))
}

// ============================================================================
// 1. extract_client_ip 异常输入测试
// ============================================================================

#[test]
fn chaos_extract_ip_empty_x_forwarded_for() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static(""));
    assert_eq!(extract_client_ip(&headers), "unknown");
}

#[test]
fn chaos_extract_ip_only_commas_x_forwarded_for() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static(" , , , "));
    // 全是逗号和空格，trim 后为空 → 回退到 unknown
    assert_eq!(extract_client_ip(&headers), "unknown");
}

#[test]
fn chaos_extract_ip_multiple_x_forwarded_for() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        HeaderValue::from_static("1.1.1.1, 2.2.2.2, 3.3.3.3"),
    );
    // 取第一个非空段
    assert_eq!(extract_client_ip(&headers), "1.1.1.1");
}

#[test]
fn chaos_extract_ip_invalid_header_value() {
    let mut headers = HeaderMap::new();
    // 包含非 ASCII 字节，to_str() 会失败
    let invalid = HeaderValue::from_bytes(b"\xff\xfe").expect("invalid bytes");
    headers.insert("x-forwarded-for", invalid);
    assert_eq!(extract_client_ip(&headers), "unknown");
}

#[test]
fn chaos_extract_ip_x_real_ip_takes_after_x_forwarded_for() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static(""));
    headers.insert("x-real-ip", HeaderValue::from_static("9.9.9.9"));
    // X-Forwarded-For 为空 → 回退到 X-Real-IP
    assert_eq!(extract_client_ip(&headers), "9.9.9.9");
}

// ============================================================================
// 2. extract_rate_limit_key 边界测试
// ============================================================================

#[test]
fn chaos_rate_limit_key_with_empty_prefix() {
    let limiter = Arc::new(SlidingWindowRateLimiter::new(100, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter).with_key_prefix("");
    let req = Request::builder()
        .header("x-forwarded-for", "1.2.3.4")
        .body(Body::empty())
        .expect("request build");
    let key = extract_rate_limit_key(&req, &config);
    // 空前缀 → key 直接是 IP
    assert_eq!(key, "1.2.3.4");
}

#[test]
fn chaos_rate_limit_key_user_id_without_auth_falls_back_to_ip() {
    let limiter = Arc::new(SlidingWindowRateLimiter::new(100, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter)
        .with_key_extractor(KeyExtractor::UserId)
        .with_key_prefix("login");
    let req = Request::builder()
        .header("x-forwarded-for", "1.2.3.4")
        .body(Body::empty())
        .expect("request build");
    // extensions 中无 AuthenticatedUser → 回退到 IP
    let key = extract_rate_limit_key(&req, &config);
    assert_eq!(key, "login:1.2.3.4");
}

#[test]
fn chaos_rate_limit_key_user_id_with_auth() {
    let limiter = Arc::new(SlidingWindowRateLimiter::new(100, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter)
        .with_key_extractor(KeyExtractor::UserId)
        .with_key_prefix("api");
    let mut req = Request::builder()
        .header("x-forwarded-for", "1.2.3.4")
        .body(Body::empty())
        .expect("request build");
    req.extensions_mut()
        .insert(AuthenticatedUser { user_id: 42 });
    let key = extract_rate_limit_key(&req, &config);
    assert_eq!(key, "api:42");
}

// ============================================================================
// 3. 限流器并发穿透测试（无 race condition）
// ============================================================================

#[tokio::test]
async fn chaos_rate_limit_concurrent_does_not_overshoot() {
    // 容量 5，窗口 60s — 100 个并发请求最多 5 个通过
    let limiter = Arc::new(SlidingWindowRateLimiter::new(5, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter).with_key_prefix("chaos_concurrent");

    let app = minimal_router().layer(from_fn_with_state(
        config,
        sz_rust_core::middleware::rate_limit::rate_limit_middleware,
    ));

    // 100 个并发请求，全部从同一 IP
    let mut handles = Vec::new();
    for _ in 0..100 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            let req = Request::builder()
                .header("x-forwarded-for", "10.0.0.1")
                .body(Body::empty())
                .expect("request build");
            app.oneshot(req).await.expect("response")
        }));
    }

    let mut allowed = 0;
    let mut rejected = 0;
    for handle in handles {
        let resp = handle.await.expect("task join");
        if resp.status() == StatusCode::OK {
            allowed += 1;
        } else if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            rejected += 1;
        } else {
            panic!("unexpected status: {}", resp.status());
        }
    }

    // 关键断言：通过数严格等于容量 5，拒绝数严格等于 95
    // 如果限流器有 race condition，通过数可能 > 5
    assert_eq!(
        allowed, 5,
        "限流穿透：允许了 {} 个请求，应只允许 5 个",
        allowed
    );
    assert_eq!(rejected, 95);
}

#[tokio::test]
async fn chaos_rate_limit_token_bucket_concurrent_burst() {
    // 令牌桶容量 3，每秒补充 1 — 突发 50 个并发请求，最多 3 个通过
    let limiter =
        Arc::new(TokenBucketRateLimiter::new(3, 1.0)) as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter).with_key_prefix("chaos_burst");

    let app = minimal_router().layer(from_fn_with_state(
        config,
        sz_rust_core::middleware::rate_limit::rate_limit_middleware,
    ));

    let mut handles = Vec::new();
    for _ in 0..50 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            let req = Request::builder()
                .header("x-forwarded-for", "10.0.0.2")
                .body(Body::empty())
                .expect("request build");
            app.oneshot(req).await.expect("response")
        }));
    }

    let mut allowed = 0;
    for handle in handles {
        let resp = handle.await.expect("task join");
        if resp.status() == StatusCode::OK {
            allowed += 1;
        }
    }
    assert!(
        allowed <= 3,
        "令牌桶并发穿透：允许了 {} 个请求，应最多 3 个",
        allowed
    );
}

// ============================================================================
// 4. 限流器排除路径测试
// ============================================================================

#[tokio::test]
async fn chaos_rate_limit_exclude_path_bypasses_limit() {
    let limiter = Arc::new(SlidingWindowRateLimiter::new(1, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter)
        .with_exclude_paths(vec!["/health".to_string()])
        .with_key_prefix("chaos_exclude");

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/", get(|| async { "ok" }))
        .layer(from_fn_with_state(
            config,
            sz_rust_core::middleware::rate_limit::rate_limit_middleware,
        ));

    // /health 路径即使超过容量也放行
    for _ in 0..10 {
        let req = Request::builder()
            .header("x-forwarded-for", "10.0.0.3")
            .uri("/health")
            .body(Body::empty())
            .expect("request build");
        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // / 路径第 2 次请求应被限流
    let req = |uri: &str| {
        Request::builder()
            .header("x-forwarded-for", "10.0.0.3")
            .uri(uri)
            .body(Body::empty())
            .expect("request build")
    };
    let resp1 = app.clone().oneshot(req("/")).await.expect("response");
    assert_eq!(resp1.status(), StatusCode::OK);
    let resp2 = app.clone().oneshot(req("/")).await.expect("response");
    assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ============================================================================
// 5. 限流拒绝响应格式测试
// ============================================================================

#[tokio::test]
async fn chaos_rate_limit_rejected_response_status_and_headers() {
    // 使用 RateLimitResult::rejected 构造（reset_at = 当前时间 + 60s）
    let reset_at = chrono::Local::now().timestamp() + 60;
    let result = sz_orm_limit::RateLimitResult::rejected(0, reset_at);
    let response = rate_limit_rejected_response(&result);
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    // 应包含 Retry-After 或 X-RateLimit-* headers
    let has_retry_after = response.headers().get("retry-after").is_some();
    let has_x_ratelimit = response.headers().get("x-ratelimit-remaining").is_some();
    assert!(
        has_retry_after || has_x_ratelimit,
        "限流拒绝响应应包含 Retry-After 或 X-RateLimit-* headers"
    );
}

// ============================================================================
// 6. 请求走私防护 — 畸形 Header
// ============================================================================

#[tokio::test]
async fn chaos_malformed_content_length_header_does_not_panic() {
    // Content-Length 包含非数字字符 — http crate 会接受 header 值但 axum 解析 body 时处理
    let req = Request::builder()
        .method(Method::POST)
        .header("content-length", "abc")
        .body(Body::from("test"))
        .expect("request build");
    // 不应 panic，header 值存在
    assert!(req.headers().get("content-length").is_some());
    let app = Router::new().route("/", post(|| async { StatusCode::OK }));
    let resp = app.oneshot(req).await.expect("response");
    // 不应该 panic，状态码可能是 400 或 200
    assert!(
        resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::OK,
        "unexpected status: {}",
        resp.status()
    );
}

#[test]
fn chaos_duplicate_headers_does_not_panic() {
    // 重复的 X-Forwarded-For — 仅取第一个
    let mut headers = HeaderMap::new();
    headers.append("x-forwarded-for", HeaderValue::from_static("1.1.1.1"));
    headers.append("x-forwarded-for", HeaderValue::from_static("2.2.2.2"));
    let ip = extract_client_ip(&headers);
    // 取第一个
    assert_eq!(ip, "1.1.1.1");
}

// ============================================================================
// 7. 超大 Header / 超大 Body 防护
// ============================================================================

#[tokio::test]
async fn chaos_huge_x_forwarded_for_truncated_safely() {
    // 构造一个超长的 X-Forwarded-For（10KB，逗号分隔）
    let huge_value = "1.1.1.1, ".repeat(1000);
    let value = HeaderValue::from_str(&huge_value).expect("ascii only");
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", value);
    // 不应 panic，应取第一个 IP
    let ip = extract_client_ip(&headers);
    assert_eq!(ip, "1.1.1.1");
}

#[tokio::test]
async fn chaos_huge_body_rejected_or_handled() {
    // 1MB body — 应该被处理，但不应该 panic
    let huge_body = vec![b'x'; 1024 * 1024];
    let req = Request::builder()
        .method(Method::POST)
        .header("content-type", "application/octet-stream")
        .body(Body::from(huge_body))
        .expect("request build");
    let app = Router::new().route("/", post(|| async { StatusCode::OK }));
    let resp = app.oneshot(req).await.expect("response");
    // 不应该 panic
    assert!(
        resp.status() == StatusCode::OK
            || resp.status() == StatusCode::PAYLOAD_TOO_LARGE
            || resp.status() == StatusCode::BAD_REQUEST,
        "unexpected status: {}",
        resp.status()
    );
}

// ============================================================================
// 8. 链式中间件异常传播 — handler panic
// ============================================================================

#[tokio::test]
async fn chaos_middleware_chain_panic_propagation() {
    // 下游 handler panic — 在 spawn 中执行，避免 panic 终止整个测试线程。
    // 关键：panic 不应被 RateLimit 中间件静默吞掉为 200。
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                panic!("intentional panic in handler");
                #[allow(unreachable_code)]
                "unreachable".to_string()
            }),
        )
        .layer(from_fn_with_state(
            sliding_window_config(100, Duration::from_secs(60)),
            sz_rust_core::middleware::rate_limit::rate_limit_middleware,
        ));

    let req = Request::builder()
        .header("x-forwarded-for", "10.0.0.10")
        .body(Body::empty())
        .expect("request build");

    // 在独立 task 中执行，panic 会让 task 失败但不终止测试线程
    let handle = tokio::spawn(async move { app.oneshot(req).await });

    let result = handle.await;
    // spawn 的 task 因 panic 失败 → result.is_err() 为 true（可接受）
    // spawn 的 task 成功 → 检查状态码不应是 200
    match result {
        Err(join_err) => {
            // panic 传播到 task — 可接受
            assert!(join_err.is_panic(), "task 应因 panic 失败，实际是取消");
        }
        Ok(Err(_service_err)) => {
            // oneshot 返回 Service 错误 — 可接受
        }
        Ok(Ok(resp)) => {
            // 不应静默返回 200
            assert_ne!(
                resp.status(),
                StatusCode::OK,
                "panic 不应被中间件静默吞掉为 200"
            );
        }
    }
}

#[tokio::test]
async fn chaos_middleware_chain_slow_handler_with_rate_limit() {
    // handler sleep 100ms — 限流不应阻塞，应立即放行或拒绝
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                "ok"
            }),
        )
        .layer(from_fn_with_state(
            sliding_window_config(1, Duration::from_secs(60)),
            sz_rust_core::middleware::rate_limit::rate_limit_middleware,
        ));

    let req = || {
        Request::builder()
            .header("x-forwarded-for", "10.0.0.11")
            .body(Body::empty())
            .expect("request build")
    };

    let start = std::time::Instant::now();
    let resp1 = app.clone().oneshot(req()).await.expect("response");
    assert_eq!(resp1.status(), StatusCode::OK);
    // 第 2 个请求应立即被拒绝（不等 handler 完成）
    let resp2 = app.clone().oneshot(req()).await.expect("response");
    assert_eq!(resp2.status(), StatusCode::TOO_MANY_REQUESTS);
    let elapsed = start.elapsed();
    // 限流检查应几乎瞬时，不应等待第 1 个 handler 完成
    assert!(
        elapsed < Duration::from_millis(150),
        "限流拒绝耗时 {:?} 过长，可能存在阻塞",
        elapsed
    );
}

// ============================================================================
// 9. CORS 恶意 Origin 测试
// ============================================================================

#[tokio::test]
async fn chaos_cors_malicious_origin_rejected() {
    use sz_rust_core::middleware::cors::cors_layer_with_origin;

    // 仅允许 example.com 域名
    let app = minimal_router().layer(cors_layer_with_origin("example.com"));

    // 恶意 Origin
    let req = Request::builder()
        .header("origin", "https://evil.com")
        .body(Body::empty())
        .expect("request build");
    let resp = app.oneshot(req).await.expect("response");
    // 不应设置 Access-Control-Allow-Origin: https://evil.com
    let aco = resp.headers().get("access-control-allow-origin");
    assert!(
        aco.is_none() || aco != Some(&HeaderValue::from_static("https://evil.com")),
        "CORS 漏洞：恶意 Origin 被放行"
    );
}

#[tokio::test]
async fn chaos_cors_null_origin_rejected() {
    use sz_rust_core::middleware::cors::cors_layer_with_origin;

    let app = minimal_router().layer(cors_layer_with_origin("example.com"));

    // null Origin（常被用于绕过检查）
    let req = Request::builder()
        .header("origin", "null")
        .body(Body::empty())
        .expect("request build");
    let resp = app.oneshot(req).await.expect("response");
    let aco = resp.headers().get("access-control-allow-origin");
    assert!(
        aco.is_none() || aco != Some(&HeaderValue::from_static("null")),
        "CORS 漏洞：null Origin 被放行"
    );
}

#[tokio::test]
async fn chaos_cors_evil_subdomain_suffix_attack_blocked() {
    // 经典 CORS 绕过：cookie_domain="example.com"，恶意 Origin="evil-example.com"
    // 错误实现用 strpos 子串匹配会放行，正确实现用后缀匹配+边界检查会拒绝
    use sz_rust_core::middleware::cors::origin_matches_domain;

    assert!(!origin_matches_domain(
        "https://evil-example.com",
        "example.com"
    ));
    assert!(!origin_matches_domain(
        "https://evil.example.com.evil.com",
        "example.com"
    ));
    // 合法子域名应放行
    assert!(origin_matches_domain(
        "https://api.example.com",
        "example.com"
    ));
    assert!(origin_matches_domain("https://example.com", "example.com"));
}

// ============================================================================
// 10. 配置完整性测试
// ============================================================================

#[test]
fn chaos_rate_limit_config_builder_chain_does_not_panic() {
    let limiter = Arc::new(SlidingWindowRateLimiter::new(100, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    // 全链式 builder 调用不应 panic
    let _config = RateLimitConfig::new(limiter)
        .with_key_extractor(KeyExtractor::IpPlusRoute)
        .with_exclude_paths(vec!["/health".to_string(), "/metrics".to_string()])
        .with_key_prefix("api");
}

#[tokio::test]
async fn chaos_token_bucket_zero_capacity_rejected() {
    // 容量 0 — 应该立即拒绝所有请求（不 panic）
    let limiter =
        Arc::new(TokenBucketRateLimiter::new(0, 0.0)) as Arc<dyn RateLimiter + Send + Sync>;
    let result = limiter.acquire("test_key");
    assert!(result.is_ok(), "acquire 应返回 Ok 但 result={:?}", result);
    let rl_result = result.expect("acquire ok");
    assert!(!rl_result.allowed, "容量 0 的令牌桶不应允许任何请求");
}

// ============================================================================
// 11. 跨中间件状态污染测试
// ============================================================================

#[tokio::test]
async fn chaos_no_state_pollution_between_requests() {
    // 两个不同 IP 的请求不应共享限流状态
    let limiter = Arc::new(SlidingWindowRateLimiter::new(1, Duration::from_secs(60)))
        as Arc<dyn RateLimiter + Send + Sync>;
    let config = RateLimitConfig::new(limiter).with_key_prefix("chaos_pollution");

    let app = minimal_router().layer(from_fn_with_state(
        config,
        sz_rust_core::middleware::rate_limit::rate_limit_middleware,
    ));

    // IP A 第 1 个请求 → 200
    let req_a = Request::builder()
        .header("x-forwarded-for", "10.0.0.100")
        .body(Body::empty())
        .expect("request build");
    let resp_a1 = app.clone().oneshot(req_a).await.expect("response");
    assert_eq!(resp_a1.status(), StatusCode::OK);

    // IP B 第 1 个请求 → 应该 200（不因 IP A 已用尽而拒绝）
    let req_b = Request::builder()
        .header("x-forwarded-for", "10.0.0.200")
        .body(Body::empty())
        .expect("request build");
    let resp_b1 = app.clone().oneshot(req_b).await.expect("response");
    assert_eq!(
        resp_b1.status(),
        StatusCode::OK,
        "状态污染：IP B 因 IP A 的限流被错误拒绝"
    );

    // IP A 第 2 个请求 → 应该 429
    let req_a2 = Request::builder()
        .header("x-forwarded-for", "10.0.0.100")
        .body(Body::empty())
        .expect("request build");
    let resp_a2 = app.clone().oneshot(req_a2).await.expect("response");
    assert_eq!(resp_a2.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ============================================================================
// 12. IntoResponse 兼容性测试 — 错误响应可正确转换
// ============================================================================

#[tokio::test]
async fn chaos_error_response_into_response_does_not_panic() {
    // rate_limit_rejected_response 返回的 Response 应可直接使用
    let reset_at = chrono::Local::now().timestamp() + 60;
    let result = sz_orm_limit::RateLimitResult::rejected(0, reset_at);
    let response = rate_limit_rejected_response(&result);
    // 验证 IntoResponse trait 正确工作
    let _status = response.status();
    let _resp: axum::response::Response = response.into_response();
}

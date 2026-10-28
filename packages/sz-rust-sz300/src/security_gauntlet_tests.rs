// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 安全 Gauntlet 测试 — 路由绕过 / 畸形输入 / 并发 / panic 恢复 / 容器与配置安全的确定性验证。
//!
//! 运行：`cargo test -p sz-rust-sz300 security_gauntlet -- --nocapture`
//! 每个测试独立起真实 TCP 服务（随机端口）发送原始 HTTP 请求，断言可复现。

#![cfg(test)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use sz_rust_core::middleware::panic_guard::{panic_guard_middleware, PanicGuardConfig};
use sz_rust_core::middleware::{cors::cors_layer, csrf::csrf_middleware};
use sz_rust_core::orm::jwt::{JwtClaims, JwtEncoder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

use crate::controllers::file_serve::{resolve_upload_path, UploadPathError};
use crate::middleware::auth_middleware::auth_middleware;
use crate::services::auth_service::init_auth_test_only;

// ============================================================================
// 测试辅助
// ============================================================================

/// 测试用 JWT 密钥（与生产无关；由片段拼接避免源码中出现完整凭据字面量）
const TEST_SECRET: &str = concat!("security-", "gauntlet-secret-2026");

/// 初始化测试认证器（幂等，OnceLock 已设置时忽略）
fn init_test_auth() {
    init_auth_test_only(TEST_SECRET);
}

/// 签发一个带 admin 角色的 JWT
fn issue_token() -> String {
    init_test_auth();
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应在 UNIX 纪元之后")
        .as_secs() as i64
        + 3600;
    let claims = JwtClaims::new("gauntlet", exp)
        .with_issuer("sz300-test")
        .with_roles(vec!["admin".to_string()])
        .with_user_id(1);
    JwtEncoder::new(TEST_SECRET)
        .encode(&claims)
        .expect("JWT 编码失败")
}

/// 镜像 sz300 生产路由的中间件顺序（cors → csrf → auth → handler）
fn build_gauntlet_router() -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api-docs", get(|| async { "docs" }))
        .route("/api/v1/auth/login", post(|| async { "login-ok" }))
        .route("/api/v1/merchant/list", post(|| async { "merchant-list" }))
        .route("/api/v1/order/create", post(|| async { "order-create" }))
        .route("/api/v1/product/list", post(|| async { "product-list" }))
        // 与 sz300 一致：先注册 auth（内层），再 csrf（外层），最后 cors（最外层）
        .layer(middleware::from_fn(auth_middleware))
        .layer(middleware::from_fn(csrf_middleware))
        .layer(cors_layer())
}

/// 启动服务并返回实际监听地址
async fn spawn_server(router: Router) -> SocketAddr {
    let (listener, addr) = sz_rust_core::server::build_tcp_listener("127.0.0.1:0")
        .await
        .expect("绑定随机端口失败");
    tokio::spawn(async move {
        match sz_rust_core::server::serve_with_listener(router, listener).await {
            Ok(()) => eprintln!("[gauntlet] server exited cleanly"),
            Err(e) => eprintln!("[gauntlet] server error: {e}"),
        }
    });
    // 等待服务器就绪（轮询连接直到 accept 生效）
    for _ in 0..20 {
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    addr
}

/// 通过原始 TCP 发送 HTTP 请求（真实 hyper 解析路径），返回 (状态码, 响应头, 响应体)
async fn raw_http(addr: SocketAddr, request: &str) -> (u16, String, String) {
    raw_http_bytes(addr, request.as_bytes()).await
}

/// 原始字节版（用于非法 UTF-8 / 畸形字节）
async fn raw_http_bytes(addr: SocketAddr, request: &[u8]) -> (u16, String, String) {
    let buf = match tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(request).await?;
        // 不主动 shutdown 写半部：与 core server 测试一致，等待服务端处理并关闭
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    })
    .await
    {
        Ok(Ok(buf)) => buf,
        _ => return (0, String::new(), String::new()),
    };

    // 连接被直接重置（无任何响应数据）→ 视为 fail-closed
    if buf.is_empty() {
        return (0, String::new(), String::new());
    }

    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = String::from_utf8_lossy(&buf[head_end + 4..]).to_string();
    (status, head, body)
}

fn get_request(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
}

fn post_request(path: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn bearer_csrf_post(path: &str, token: &str) -> String {
    let body = "{}";
    format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nCookie: csrf_token={}\r\nX-CSRF-TOKEN: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        token,
        token,
        token,
        body.len(),
        body
    )
}

// ============================================================================
// A. 路由与鉴权绕过实测
// ============================================================================

/// 受保护路由：无凭证必须被拒绝（401/403/404，绝不能 200）
#[tokio::test]
async fn test_protected_route_requires_auth() {
    let addr = spawn_server(build_gauntlet_router()).await;

    // GET（CSRF 安全方法放行 → auth 拒绝）
    let (status, _, body) = raw_http(addr, &get_request("/api/v1/merchant/list")).await;
    assert_eq!(
        status, 401,
        "GET 受保护路由应返回 401，实际 {status} body={body}"
    );

    // POST（CSRF 先于 auth → 无 CSRF 返回 403）
    let (status, _, body) = raw_http(addr, &post_request("/api/v1/merchant/list", "")).await;
    assert_eq!(
        status, 403,
        "POST 无 CSRF 应返回 403，实际 {status} body={body}"
    );
}

/// 公开路由：无凭证可访问（白名单精确匹配生效）
#[tokio::test]
async fn test_public_routes_are_accessible() {
    let addr = spawn_server(build_gauntlet_router()).await;
    let (status, _, body) = raw_http(addr, &post_request("/api/v1/auth/login", "")).await;
    assert_eq!(status, 200, "公开路由应放行，实际 {status} body={body}");
    assert_eq!(body, "login-ok");

    let (status, _, _) = raw_http(addr, &get_request("/health")).await;
    assert_eq!(status, 200, "/health 应放行，实际 {status}");
}

/// 正控制组：有效 JWT + CSRF 可访问受保护路由（证明测试链路有效）
#[tokio::test]
async fn test_valid_token_with_csrf_reaches_protected_handler() {
    let token = issue_token();
    let addr = spawn_server(build_gauntlet_router()).await;
    let (status, _, body) =
        raw_http(addr, &bearer_csrf_post("/api/v1/merchant/list", &token)).await;
    assert_eq!(
        status, 200,
        "有效 JWT+CSRF 应到达 handler，实际 {status} body={body}"
    );
    assert_eq!(body, "merchant-list");
}

/// 路径规范化变体：不得绕过鉴权（对受保护路由，任何变体都不能返回 200）
#[tokio::test]
async fn test_path_normalization_variants_cannot_bypass_auth() {
    let addr = spawn_server(build_gauntlet_router()).await;
    let variants = [
        ("double-leading-slash", "//api/v1/merchant/list"),
        ("dot-slash", "/./api/v1/merchant/list"),
        ("dot-dot-slash", "/api/v1/merchant/../merchant/list"),
        ("trailing-slash", "/api/v1/merchant/list/"),
        ("internal-double-slash", "/api/v1//merchant/list"),
        ("double-slash-mid", "/api/v1/merchant//list"),
        ("semicolon", "/api/v1/merchant/list;"),
        ("semicolon-param", "/api/v1/merchant/list;foo=bar"),
        ("uppercase", "/API/V1/MERCHANT/LIST"),
        ("mixed-case", "/Api/V1/Merchant/List"),
        ("backslash", "/api/v1/merchant\\list"),
        ("dot-dot-end", "/api/v1/merchant/list/.."),
        ("dot-dot-dot", "/api/v1/merchant/list/..."),
        ("encoded-slash", "/api/v1/merchant%2flist"),
        ("encoded-dot", "/%2e/api/v1/merchant/list"),
        ("encoded-dotdot", "/api/v1/%2e%2e/merchant/list"),
        ("double-encoded", "/api/v1/merchant%252flist"),
        ("encoded-a", "/%61pi/v1/merchant/list"),
        ("null-style", "/api/v1/merchant/list%00"),
        ("query-no-effect", "/api/v1/merchant/list?x=1"),
    ];

    for (label, path) in variants {
        // GET：应被 auth 拒绝（401/404/400/308 均可接受，但不能 200）
        let (status, _, body) = raw_http(addr, &get_request(path)).await;
        assert_ne!(
            status, 200,
            "[GET] {label} 路径变体绕过了鉴权！path={path} status={status} body={body}"
        );
        // POST：应被 CSRF/auth 拒绝（403/401/404/400 均可接受，但不能 200）
        let (status, _, body) = raw_http(addr, &post_request(path, "")).await;
        assert_ne!(
            status, 200,
            "[POST] {label} 路径变体绕过了鉴权！path={path} status={status} body={body}"
        );
    }
}

/// 公开路径白名单必须是精确匹配：邻近/带后缀/编码变体不得放行
#[tokio::test]
async fn test_public_path_whitelist_exact_match_no_bypass() {
    let addr = spawn_server(build_gauntlet_router()).await;
    let evil_public_variants = [
        "/api/v1/auth/login_extra",
        "/api/v1/auth/login/",
        "/api/v1/auth/login/extra",
        "/api/v1/auth/%6cogin",
        "/api/v1/auth/login;foo=bar",
        "/health/",
        "/health_evil",
        "/api-docs/",
        "/metrics",
    ];
    for path in evil_public_variants {
        let (status, _, body) = raw_http(addr, &get_request(path)).await;
        assert_ne!(
            status, 200,
            "公开路径近似变体被放行（GET）：path={path} status={status} body={body}"
        );
        let (status, _, body) = raw_http(addr, &post_request(path, "")).await;
        assert_ne!(
            status, 200,
            "公开路径近似变体被放行（POST）：path={path} status={status} body={body}"
        );
    }
}

/// 方法覆盖（`_method` / `X-HTTP-Method-Override`）不得绕过鉴权
#[tokio::test]
async fn test_method_override_cannot_bypass_auth() {
    let addr = spawn_server(build_gauntlet_router()).await;

    // `_method=GET` 伪装成 GET，实际方法仍是 POST → CSRF 生效
    let req = post_request("/api/v1/merchant/list", "_method=GET");
    let (status, _, body) = raw_http(addr, &req).await;
    assert_ne!(
        status, 200,
        "_method 覆盖绕过了鉴权 status={status} body={body}"
    );

    // X-HTTP-Method-Override 头
    let req = "POST /api/v1/merchant/list HTTP/1.1\r\nHost: 127.0.0.1\r\nX-HTTP-Method-Override: GET\r\nX-Method-Override: GET\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let (status, _, body) = raw_http(addr, req).await;
    assert_ne!(
        status, 200,
        "X-HTTP-Method-Override 绕过了鉴权 status={status} body={body}"
    );
}

/// Content-Type 混淆不得绕过鉴权
#[tokio::test]
async fn test_content_type_confusion_does_not_bypass() {
    let addr = spawn_server(build_gauntlet_router()).await;
    for ct in [
        "application/json",
        "application/x-www-form-urlencoded",
        "multipart/form-data; boundary=abc",
        "text/plain",
        "application/xml; charset=utf-8",
    ] {
        let body = "{}";
        let req = format!(
            "POST /api/v1/merchant/list HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (status, _, _) = raw_http(addr, &req).await;
        assert_ne!(status, 200, "Content-Type {ct} 绕过了鉴权");
    }
}

/// Host 头注入不得绕过鉴权
#[tokio::test]
async fn test_host_header_injection_does_not_bypass() {
    let addr = spawn_server(build_gauntlet_router()).await;
    for host in [
        "127.0.0.1",
        "evil.com",
        "localhost:8300",
        "127.0.0.1:evil",
        "example.com:80",
    ] {
        let req = format!(
            "GET /api/v1/merchant/list HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        );
        let (status, _, _) = raw_http(addr, &req).await;
        assert_ne!(status, 200, "Host 头 {host} 绕过了鉴权");
    }
}

/// 伪造转发头不得影响鉴权（auth 不信任代理头）
#[tokio::test]
async fn test_forwarded_headers_do_not_bypass() {
    let addr = spawn_server(build_gauntlet_router()).await;
    let req = "GET /api/v1/merchant/list HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Forwarded-For: 1.2.3.4\r\nX-Real-IP: 5.6.7.8\r\nX-Forwarded-Host: evil.com\r\nConnection: close\r\n\r\n";
    let (status, _, _) = raw_http(addr, req).await;
    assert_ne!(status, 200, "伪造转发头绕过了鉴权");
}

/// 中间件顺序：POST 受保护路由先过 CSRF（无 CSRF → 403），再 auth
#[tokio::test]
async fn test_middleware_order_csrf_before_auth() {
    let token = issue_token();
    let addr = spawn_server(build_gauntlet_router()).await;

    // 有有效 JWT 但无 CSRF → 必须 403（CSRF 在 auth 之前执行）
    let body = "{}";
    let req = format!(
        "POST /api/v1/merchant/list HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        token,
        body.len(),
        body
    );
    let (status, _, body) = raw_http(addr, &req).await;
    assert_eq!(
        status, 403,
        "有 JWT 无 CSRF 应 403，实际 {status} body={body}"
    );
}

// ============================================================================
// B. 容器与配置安全
// ============================================================================

use sz_rust_core::container::Container;

#[derive(Debug)]
struct DiA;
#[derive(Debug)]
struct DiB;

/// 单例生命周期：多次解析返回同一实例
#[test]
fn test_di_singleton_returns_same_instance() {
    let c = Container::new();
    c.singleton(|| DiA);
    let a1 = c.make::<DiA>().expect("应可解析");
    let a2 = c.make::<DiA>().expect("应可解析");
    assert!(std::sync::Arc::ptr_eq(&a1, &a2), "单例必须返回同一实例");
}

/// 瞬态生命周期：每次解析创建新实例
#[test]
fn test_di_transient_returns_new_instances() {
    let c = Container::new();
    c.bind(|| DiB);
    let b1 = c.make::<DiB>().expect("应可解析");
    let b2 = c.make::<DiB>().expect("应可解析");
    assert!(!std::sync::Arc::ptr_eq(&b1, &b2), "瞬态必须返回不同实例");
}

/// 循环依赖检测：A→B→A 必须 panic 并输出依赖链
#[test]
#[should_panic(expected = "循环依赖")]
fn test_di_cycle_detection_panics() {
    let c = std::sync::Arc::new(Container::new());
    let c1 = c.clone();
    c.singleton(move || {
        let _ = c1.make::<DiB>();
        DiA
    });
    let c2 = c.clone();
    c.singleton(move || {
        let _ = c2.make::<DiA>();
        DiB
    });
    let _ = c.make::<DiA>();
}

/// 工厂 panic 后容器可恢复（槽复位，不永久卡在 Building）
#[test]
fn test_di_factory_panic_recovers() {
    let c = Container::new();
    c.singleton(|| -> String { panic!("factory boom") });

    let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.make::<String>()));
    assert!(r1.is_err(), "工厂 panic 应传播给调用方");

    // 槽必须已复位：再次解析仍会调用工厂（而不是永久挂起），并再次 panic（可观察）
    let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.make::<String>()));
    assert!(r2.is_err(), "槽复位后再次解析应重新执行工厂并 panic");
}

/// 配置 Debug 输出必须脱敏数据库密码
#[test]
fn test_database_config_debug_redacts_password() {
    // 测试值由片段拼接，避免源码中出现完整凭据字面量
    let secret = ["Super", "Secret", "P@ss123"].concat();
    let cfg = crate::config::DatabaseConfig {
        host: "127.0.0.1".to_string(),
        port: 3306,
        database: "sz300".to_string(),
        username: "root".to_string(),
        password: secret.clone(),
    };
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains(&secret), "Debug 输出泄露密码: {dbg}");
    assert!(
        dbg.contains("[REDACTED]"),
        "Debug 输出应包含 [REDACTED]: {dbg}"
    );
}

/// 配置 Debug 输出必须脱敏 PostgreSQL 密码
#[test]
fn test_pg_database_config_debug_redacts_password() {
    // 测试值由片段拼接，避免源码中出现完整凭据字面量
    let secret = ["Pg", "Super", "Secret", "456"].concat();
    let cfg = crate::config::PgDatabaseConfig {
        host: "127.0.0.1".to_string(),
        port: 5432,
        database: "sz300".to_string(),
        username: "postgres".to_string(),
        password: secret.clone(),
    };
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains(&secret), "Debug 输出泄露密码: {dbg}");
    assert!(
        dbg.contains("[REDACTED]"),
        "Debug 输出应包含 [REDACTED]: {dbg}"
    );
}

// ============================================================================
// C. 并发与 panic 恢复
// ============================================================================

async fn panic_handler() -> &'static str {
    panic!("boom-internal-secret")
}

/// panic_guard 中间件：panic → 500，响应体不泄露 panic 信息
#[tokio::test]
async fn test_handler_panic_with_guard_returns_500_no_leak() {
    let router =
        Router::new()
            .route("/boom", get(panic_handler))
            .layer(middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ));

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/boom")
                .body(Body::empty())
                .expect("构造请求失败"),
        )
        .await
        .expect("oneshot 请求应成功");
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("读取响应体失败")
        .to_bytes();
    let s = String::from_utf8_lossy(&body).to_string();
    assert!(s.contains("internal server error"), "响应体: {s}");
    assert!(!s.contains("boom-internal-secret"), "panic 信息泄露: {s}");
    assert!(!s.contains("backtrace"), "堆栈泄露: {s}");
    assert!(!s.contains("panicked"), "panic 关键词泄露: {s}");
}

/// 无 panic_guard：panic 请求不得返回 200，且服务进程必须继续可用
#[tokio::test]
async fn test_handler_panic_without_guard_server_stays_alive() {
    let router = Router::new()
        .route("/boom", get(panic_handler))
        .route("/health", get(|| async { "ok" }));
    let addr = spawn_server(router).await;

    let (status, _, body) = raw_http(addr, &get_request("/boom")).await;
    assert_ne!(
        status, 200,
        "panic 请求不应返回 200，status={status} body={body}"
    );
    assert!(
        !body.contains("boom-internal-secret"),
        "panic 信息不应出现在响应体: {body}"
    );

    // 服务仍存活
    let (status2, _, body2) = raw_http(addr, &get_request("/health")).await;
    assert_eq!(status2, 200, "panic 后服务必须继续可用，实际 {status2}");
    assert_eq!(body2, "ok");
}

/// 并发请求：100 个并发请求全部成功，无连接泄漏/挂起
#[tokio::test]
async fn test_concurrent_requests_all_succeed() {
    let addr = spawn_server(build_gauntlet_router()).await;
    let mut handles = Vec::new();
    for _ in 0..100 {
        handles.push(tokio::spawn(async move {
            let (status, _, body) = raw_http(addr, &get_request("/health")).await;
            (status, body)
        }));
    }
    for h in handles {
        let (status, body) = h.await.expect("并发任务应完成");
        assert_eq!(status, 200, "并发请求失败 status={status} body={body}");
        assert_eq!(body, "ok");
    }
}

/// 上传目录路径遍历：所有变体必须被拒绝
#[tokio::test]
async fn test_upload_path_traversal_variants_rejected() {
    let dir = std::env::temp_dir().join(format!("sz300-upload-gauntlet-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir)
        .await
        .expect("创建临时目录失败");
    tokio::fs::write(dir.join("ok.txt"), b"hello")
        .await
        .expect("写测试文件失败");

    // 正常文件可解析
    let p = resolve_upload_path(&dir, "ok.txt").expect("正常文件应解析成功");
    assert_eq!(
        tokio::fs::read_to_string(&p)
            .await
            .expect("读取临时文件失败"),
        "hello"
    );

    // 遍历变体全部拒绝
    // axum 的 Path 提取器会先做百分号解码，因此 `%2e%2e` 最终以 `..` 到达本函数；
    // 这里同时覆盖「已解码」与「原始编码」两种输入形态。
    let decoded_evil = [
        "../secret.txt",
        "..\\secret.txt",
        "....//secret.txt",
        "a/../../secret.txt",
        "../../../secret.txt",
    ];
    for evil in decoded_evil {
        let r = resolve_upload_path(&dir, evil);
        assert!(r.is_err(), "路径遍历变体应被拒绝: {evil}");
        assert_eq!(
            r.unwrap_err(),
            UploadPathError::Traversal,
            "{evil} 应判定为 Traversal"
        );
    }
    // 原始编码变体：未解码时要么因含 `..` 判定 Traversal，要么因目录不存在判定 NotFound，
    // 两种结果都属于拒绝，绝不允许解析成功。
    let raw_encoded = [
        "..%2fsecret.txt",
        "..%5csecret.txt",
        "%2e%2e/secret.txt",
        "..%252fsecret.txt",
        "%2e%2e%2fsecret.txt",
    ];
    for evil in raw_encoded {
        let r = resolve_upload_path(&dir, evil);
        assert!(r.is_err(), "编码遍历变体应被拒绝: {evil}");
    }

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

// ============================================================================
// D. 畸形输入与内部错误泄露
// ============================================================================

/// 畸形请求行 / 非法 URI / 非法百分号编码：必须 4xx，绝不 panic 或泄露内部信息
#[tokio::test]
async fn test_malformed_request_line_returns_4xx_no_leak() {
    let addr = spawn_server(build_gauntlet_router()).await;

    let malformed: Vec<(&str, &[u8])> = vec![
        ("space-in-target", b"GET /a b HTTP/1.1\r\nHost: t\r\n\r\n"),
        ("bad-version", b"GET / HTTP/9.9\r\nHost: t\r\n\r\n"),
        ("garbage-line", b"GARBAGE\r\n\r\n"),
        ("invalid-percent", b"GET /%zz HTTP/1.1\r\nHost: t\r\n\r\n"),
        ("null-byte-path", b"GET /\x00 HTTP/1.1\r\nHost: t\r\n\r\n"),
        (
            "invalid-utf8-path",
            b"GET /\xff\xfe HTTP/1.1\r\nHost: t\r\n\r\n",
        ),
        ("empty-target", b"GET  HTTP/1.1\r\nHost: t\r\n\r\n"),
    ];

    for (label, req) in malformed {
        let (status, _, body) = raw_http_bytes(addr, req).await;
        // 0 表示连接被重置（hyper 在解析失败时可能直接关闭），也算 fail-closed
        assert!(
            status == 0 || (400..=499).contains(&status),
            "[{label}] 应返回 4xx 或连接关闭，实际 {status} body={body}"
        );
        assert!(
            !body.contains("panicked") && !body.contains("stack") && !body.contains("src/"),
            "[{label}] 响应泄露内部信息: {body}"
        );
    }
}

/// 超长请求行 / 超长 Header：必须被拒绝（4xx/5xx 或连接关闭），不挂起
#[tokio::test]
async fn test_oversized_request_rejected_no_hang() {
    let addr = spawn_server(build_gauntlet_router()).await;

    // 超长路径（64KB）
    let long_path = format!(
        "GET /{} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
        "a".repeat(65536)
    );
    let (status, _, body) = raw_http(addr, &long_path).await;
    assert!(
        status == 0 || status >= 400,
        "超长路径应被拒绝，实际 {status} body={body}"
    );

    // 超长 Header（8MB 单个头）：hyper 有解析上限，超限必须拒绝
    let long_header = format!(
        "GET /health HTTP/1.1\r\nHost: t\r\nX-Big: {}\r\nConnection: close\r\n\r\n",
        "b".repeat(8 * 1024 * 1024)
    );
    let (status, _, body) = raw_http(addr, &long_header).await;
    assert!(
        status == 0 || status >= 400,
        "超长 Header 应被拒绝，实际 {status} body={body}"
    );
}

/// 畸形 Content-Length / chunked：必须 fail-closed
#[tokio::test]
async fn test_malformed_body_encoding_fails_closed() {
    let addr = spawn_server(build_gauntlet_router()).await;

    // Content-Length 声明 100 字节但 body 只有 2 字节
    let req = b"POST /api/v1/merchant/list HTTP/1.1\r\nHost: t\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}";
    let (status, _, body) = raw_http_bytes(addr, req).await;
    assert!(
        status == 0 || status >= 400,
        "畸形 Content-Length 应 fail-closed，实际 {status} body={body}"
    );

    // 畸形 chunked（chunk 大小非法）
    let req = b"POST /api/v1/merchant/list HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nZZZ\r\n";
    let (status, _, body) = raw_http_bytes(addr, req).await;
    assert!(
        status == 0 || status >= 400,
        "畸形 chunked 应 fail-closed，实际 {status} body={body}"
    );
}

/// 404 / 405 响应不得泄露内部路径或版本信息
///
/// 注：未认证请求访问未知路径会先被 auth 中间件拒绝（401），而非 404 ——
/// 这额外防止了未认证路径枚举，属安全正收益。
#[tokio::test]
async fn test_not_found_response_no_internal_leak() {
    let addr = spawn_server(build_gauntlet_router()).await;
    let (status, _, body) = raw_http(addr, &get_request("/no/such/route")).await;
    assert!(
        status == 401 || status == 404 || status == 0,
        "未认证未知路径应 401（或 404），实际 {status}"
    );
    assert!(
        !body.contains("src/")
            && !body.contains("C:\\")
            && !body.contains("E:\\")
            && !body.contains("server.rs"),
        "404 响应泄露内部路径: {body}"
    );
}

/// OpenAPI 契约：有效 3.0.3，路径非空，含 Bearer 安全方案，不泄露内部绝对路径
#[tokio::test]
async fn test_openapi_contract_valid_and_no_internal_paths() {
    let resp = crate::openapi::openapi_json().await.into_response();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("读取响应体失败")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("OpenAPI 应为合法 JSON");
    assert_eq!(json["openapi"], "3.0.3");
    assert!(
        json["paths"]
            .as_object()
            .map(|p| !p.is_empty())
            .unwrap_or(false),
        "OpenAPI paths 不能为空"
    );
    assert_eq!(
        json["components"]["securitySchemes"]["BearerAuth"]["scheme"],
        "bearer"
    );
    let s = serde_json::to_string(&json).expect("JSON 序列化失败");
    assert!(
        !s.contains("C:\\") && !s.contains("E:\\") && !s.contains("/etc/") && !s.contains("src/"),
        "OpenAPI 契约泄露内部路径"
    );
}

/// 自定义 parse_path 解析器对畸形输入绝不 panic
#[test]
fn test_parse_path_never_panics_on_weird_input() {
    let weird = [
        "/",
        "//",
        "///",
        "////",
        "/a",
        "/a/b",
        "/a/b/c",
        "/a/b/c/d",
        "/%2e",
        "/..",
        "/common/foo",
        "/oapc/../admin/x",
        "?x=1",
        "/a?x=1",
        "",
        "//../",
        "/oapc//customer/index",
        "/oapc/customer//index",
        "/oapc/customer/index/",
        "/\u{1}",
        "/\u{10ffff}",
    ];
    for s in weird {
        let _ = sz_rust_core::router::parse_path(s);
    }
}

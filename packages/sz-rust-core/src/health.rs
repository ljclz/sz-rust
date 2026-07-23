//! 健康检查端点 — `/health`
//!
//! 提供轻量级健康检查端点，用于容器编排（K8s liveness/readiness）和负载均衡探测。
//!
//! ## 设计原则
//!
//! - **liveness 探针**：进程存活即返回 200，不检查依赖（避免级联重启）
//! - **readiness 探针**：可附加 [`HealthCheck`] 子检查（DB/Cache 连通性），全部通过才返回 200
//! - **响应格式**：JSON，与 [`crate::response::ApiResponse`] 保持一致
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::health::{HealthRegistry, HealthCheck};
//! use axum::Router;
//!
//! let registry = HealthRegistry::new();
//! let router: Router = registry.router_at("/health");
//! ```
//!
//! ## 端点
//!
//! - `GET /health/`：liveness 探针（始终 200）
//! - `GET /health/ready`：readiness 探针（执行所有子检查）

use crate::response::ApiResponse;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 默认单个检查超时（3 秒）
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// 健康检查子项（trait）
///
/// 实现此 trait 的检查器会被注册到 [`HealthRegistry`]，在 `/health/ready` 中按序执行。
pub trait HealthCheck: Send + Sync {
    /// 检查器名称（如 "database"、"redis"）
    fn name(&self) -> &str;

    /// 执行检查，返回 `Ok(())` 表示健康，`Err(msg)` 表示异常 + 错误描述
    fn check(&self) -> Result<(), String>;
}

/// 健康检查注册表
///
/// 管理多个 [`HealthCheck`] 子检查，提供 liveness / readiness 探针。
#[derive(Clone)]
pub struct HealthRegistry {
    checks: Arc<Mutex<Vec<Arc<dyn HealthCheck>>>>,
    /// 单个检查的超时时间
    timeout: Duration,
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self {
            checks: Arc::new(Mutex::new(Vec::new())),
            timeout: DEFAULT_CHECK_TIMEOUT,
        }
    }
}

impl HealthRegistry {
    /// 创建注册表（默认 3 秒超时）
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建带自定义超时的注册表
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            checks: Arc::new(Mutex::new(Vec::new())),
            timeout,
        }
    }

    /// 注册一个健康检查子项
    pub fn register<C: HealthCheck + 'static>(&self, check: C) -> &Self {
        self.checks.lock().push(Arc::new(check));
        self
    }

    /// liveness 探针：进程存活即健康
    pub fn liveness(&self) -> Response {
        ApiResponse::success(json!({"status": "ok"}), "ok").into_response()
    }

    /// readiness 探针：执行所有子检查
    ///
    /// 全部通过返回 `code=1`，任一失败返回 `code=0` + 详细错误信息。
    /// 单个检查超过 `timeout` 视为失败。
    ///
    /// ## 异步实现
    ///
    /// 每个检查通过 `tokio::task::spawn_blocking` 丢到阻塞线程池执行，
    /// 通过 `tokio::time::timeout` 控制超时。这样不会阻塞 tokio executor，
    /// 即使被频繁探测也不会拖累主服务吞吐量。
    pub async fn readiness(&self) -> Response {
        let checks = self.checks.lock().clone();
        let mut results = serde_json::Map::new();
        let mut all_ok = true;

        for check in &checks {
            let name = check.name().to_string();
            let started = Instant::now();
            let result = self.run_with_timeout(check).await;
            let elapsed_ms = started.elapsed().as_millis();

            match result {
                Ok(()) => {
                    results.insert(name, json!({"status": "ok", "elapsed_ms": elapsed_ms}));
                }
                Err(err) => {
                    all_ok = false;
                    results.insert(
                        name,
                        json!({"status": "fail", "error": err, "elapsed_ms": elapsed_ms}),
                    );
                }
            }
        }

        let data = json!({
            "status": if all_ok { "ok" } else { "fail" },
            "checks": Value::Object(results),
        });

        if all_ok {
            ApiResponse::success(data, "ok").into_response()
        } else {
            ApiResponse::error_with_data("health check failed", data).into_response()
        }
    }

    /// 在阻塞线程池中执行单个检查，超时则返回错误
    ///
    /// 使用 `tokio::task::spawn_blocking` 将同步 `check()` 调度到阻塞线程池，
    /// 通过 `tokio::time::timeout` 控制超时。这样不会阻塞 tokio 异步 executor。
    async fn run_with_timeout(&self, check: &Arc<dyn HealthCheck>) -> Result<(), String> {
        // timeout=0 表示不超时
        if self.timeout.is_zero() {
            let check_clone = Arc::clone(check);
            return tokio::task::spawn_blocking(move || check_clone.check())
                .await
                .unwrap_or_else(|_| Err("check thread panicked".to_string()));
        }

        let check_clone = Arc::clone(check);
        let timeout = self.timeout;
        match tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || check_clone.check()),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(join_err)) => Err(format!("check thread panicked: {join_err}")),
            Err(_) => Err(format!("timeout after {}ms", timeout.as_millis())),
        }
    }

    /// 构建带路径前缀的 Router（如 `/health`）
    ///
    /// - `GET {prefix}/` → liveness
    /// - `GET {prefix}/ready` → readiness（异步执行子检查）
    pub fn router_at(&self, prefix: &str) -> Router {
        let liveness_self = self.clone();
        let readiness_self = self.clone();
        let liveness_path = format!("{prefix}/");
        let readiness_path = format!("{prefix}/ready");
        Router::new()
            .route(
                &liveness_path,
                get(move || {
                    let this = liveness_self.clone();
                    std::future::ready(this.liveness())
                }),
            )
            .route(
                &readiness_path,
                get(move || {
                    let this = readiness_self.clone();
                    async move { this.readiness().await }
                }),
            )
    }
}

/// 默认健康检查 Router（路径前缀 `/health`，无子检查）
pub fn default_health_router() -> Router {
    HealthRegistry::new().router_at("/health")
}

// ============================================================================
// 内置 HealthCheck 实现
// ============================================================================

/// 静态检查（用于测试或占位）
pub struct StaticCheck {
    name: String,
    ok: bool,
}

impl StaticCheck {
    /// 创建一个总是通过的静态检查
    pub fn ok(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok: true,
        }
    }
    /// 创建一个总是失败的静态检查
    pub fn fail(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok: false,
        }
    }
}

impl HealthCheck for StaticCheck {
    fn name(&self) -> &str {
        &self.name
    }
    fn check(&self) -> Result<(), String> {
        if self.ok {
            Ok(())
        } else {
            Err("static check failed".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn fetch_body(resp: Response) -> Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn send_get(router: Router, uri: &str) -> Response {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap()
    }

    // ====================================================================
    // liveness 探针
    // ====================================================================

    #[tokio::test]
    async fn test_liveness_returns_200() {
        let registry = HealthRegistry::new();
        let resp = registry.liveness();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_liveness_response_body() {
        let registry = HealthRegistry::new();
        let resp = registry.liveness();
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
        assert_eq!(json["msg"], "ok");
        assert_eq!(json["data"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_liveness_content_type() {
        let registry = HealthRegistry::new();
        let resp = registry.liveness();
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    // ====================================================================
    // readiness 探针 - 无子检查
    // ====================================================================

    #[tokio::test]
    async fn test_readiness_empty_registry_returns_ok() {
        let registry = HealthRegistry::new();
        let resp = registry.readiness().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
        assert_eq!(json["data"]["status"], "ok");
        assert!(json["data"]["checks"].is_object());
    }

    // ====================================================================
    // readiness 探针 - 静态检查器
    // ====================================================================

    #[tokio::test]
    async fn test_readiness_all_checks_pass() {
        let registry = HealthRegistry::new();
        registry.register(StaticCheck::ok("database"));
        registry.register(StaticCheck::ok("redis"));

        let resp = registry.readiness().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
        assert_eq!(json["data"]["status"], "ok");
        assert_eq!(json["data"]["checks"]["database"]["status"], "ok");
        assert_eq!(json["data"]["checks"]["redis"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_readiness_one_check_fails() {
        let registry = HealthRegistry::new();
        registry.register(StaticCheck::ok("database"));
        registry.register(StaticCheck::fail("redis"));

        let resp = registry.readiness().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 0);
        assert_eq!(json["data"]["status"], "fail");
        assert_eq!(json["data"]["checks"]["database"]["status"], "ok");
        assert_eq!(json["data"]["checks"]["redis"]["status"], "fail");
        assert!(json["data"]["checks"]["redis"]["error"].is_string());
    }

    #[tokio::test]
    async fn test_readiness_all_checks_fail() {
        let registry = HealthRegistry::new();
        registry.register(StaticCheck::fail("db1"));
        registry.register(StaticCheck::fail("db2"));

        let resp = registry.readiness().await;
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 0);
        assert_eq!(json["data"]["status"], "fail");
    }

    // ====================================================================
    // Router 集成
    // ====================================================================

    #[tokio::test]
    async fn test_router_liveness_endpoint() {
        let router = default_health_router();
        let resp = send_get(router, "/health/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = fetch_body(resp).await;
        assert_eq!(json["data"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_router_readiness_endpoint() {
        let registry = HealthRegistry::new();
        registry.register(StaticCheck::ok("db"));
        let router = registry.router_at("/health");

        let resp = send_get(router, "/health/ready").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
        assert_eq!(json["data"]["checks"]["db"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_router_unknown_path_returns_404() {
        let router = default_health_router();
        let resp = send_get(router, "/health/unknown").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ====================================================================
    // 超时测试
    // ====================================================================

    struct SlowCheck {
        name: String,
        delay: Duration,
    }

    impl HealthCheck for SlowCheck {
        fn name(&self) -> &str {
            &self.name
        }
        fn check(&self) -> Result<(), String> {
            std::thread::sleep(self.delay);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_readiness_timeout_handled() {
        let registry = HealthRegistry::with_timeout(Duration::from_millis(100));
        registry.register(SlowCheck {
            name: "slow".to_string(),
            delay: Duration::from_millis(500),
        });

        let resp = registry.readiness().await;
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 0);
        assert_eq!(json["data"]["status"], "fail");
        assert_eq!(json["data"]["checks"]["slow"]["status"], "fail");
        assert!(json["data"]["checks"]["slow"]["error"]
            .as_str()
            .unwrap()
            .contains("timeout"));
    }

    #[tokio::test]
    async fn test_readiness_fast_check_passes_within_timeout() {
        let registry = HealthRegistry::with_timeout(Duration::from_secs(3));
        registry.register(SlowCheck {
            name: "fast".to_string(),
            delay: Duration::from_millis(10),
        });

        let resp = registry.readiness().await;
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
        assert_eq!(json["data"]["checks"]["fast"]["status"], "ok");
    }

    #[tokio::test]
    async fn test_readiness_zero_timeout_no_limit() {
        // timeout=0 表示不超时
        let registry = HealthRegistry::with_timeout(Duration::ZERO);
        registry.register(SlowCheck {
            name: "fast".to_string(),
            delay: Duration::from_millis(10),
        });

        let resp = registry.readiness().await;
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
        assert_eq!(json["data"]["checks"]["fast"]["status"], "ok");
    }

    // ====================================================================
    // 注册表行为
    // ====================================================================

    #[test]
    fn test_registry_default_is_empty() {
        let registry = HealthRegistry::new();
        assert_eq!(registry.checks.lock().len(), 0);
    }

    #[test]
    fn test_registry_default_timeout_is_3s() {
        let registry = HealthRegistry::new();
        assert_eq!(registry.timeout, DEFAULT_CHECK_TIMEOUT);
        assert_eq!(registry.timeout, Duration::from_secs(3));
    }

    #[test]
    fn test_registry_register_increases_count() {
        let registry = HealthRegistry::new();
        registry.register(StaticCheck::ok("a"));
        registry.register(StaticCheck::ok("b"));
        registry.register(StaticCheck::ok("c"));
        assert_eq!(registry.checks.lock().len(), 3);
    }

    #[test]
    fn test_registry_clone_shares_state() {
        let registry = HealthRegistry::new();
        let cloned = registry.clone();
        cloned.register(StaticCheck::ok("shared"));
        assert_eq!(registry.checks.lock().len(), 1);
    }

    #[test]
    fn test_static_check_ok_passes() {
        let check = StaticCheck::ok("test");
        assert!(check.check().is_ok());
    }

    #[test]
    fn test_static_check_fail_fails() {
        let check = StaticCheck::fail("test");
        assert!(check.check().is_err());
    }

    #[test]
    fn test_static_check_name() {
        let check = StaticCheck::ok("my_check");
        assert_eq!(check.name(), "my_check");
    }

    // ====================================================================
    // 默认 router 集成
    // ====================================================================

    #[tokio::test]
    async fn test_default_health_router_liveness() {
        let router = default_health_router();
        let resp = send_get(router, "/health/").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_default_health_router_readiness_empty() {
        let router = default_health_router();
        let resp = send_get(router, "/health/ready").await;
        let json = fetch_body(resp).await;
        assert_eq!(json["code"], 1);
    }

    // ====================================================================
    // 自定义前缀
    // ====================================================================

    #[tokio::test]
    async fn test_router_at_custom_prefix() {
        let registry = HealthRegistry::new();
        let router = registry.router_at("/status");

        let resp = send_get(router, "/status/").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let registry2 = HealthRegistry::new();
        let router2 = registry2.router_at("/status");
        let resp2 = send_get(router2, "/status/ready").await;
        assert_eq!(resp2.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_router_at_nested_prefix() {
        let registry = HealthRegistry::new();
        let router = registry.router_at("/api/v1/health");

        let resp = send_get(router, "/api/v1/health/").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use crate::services::health_service;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde_json::{json, Value};

/// 存活检查（liveness probe）
#[tracing::instrument(skip(_state))]
pub async fn check(State(_state): State<AppState>) -> Json<Value> {
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "service": "sz300-server",
            "timestamp": chrono::Utc::now().timestamp()
        }
    }))
}

/// 就绪检查（readiness probe）：通过 service 层封装的 DB 探活验证数据库连接是否正常
///
/// 响应体仅返回聚合状态（ok/fail），不泄露 DB 内部错误细节。
/// DB 错误细节由 `health_service::ping_db` 通过 `tracing::error!` 记录到日志，便于运维排查。
///
/// 重构说明（2026-07-26 P1-5）：
/// - 移除控制器内嵌 DB 调用（`state.db_pool.acquire()` + `conn.query("SELECT 1")`）
/// - 下沉到 `health_service::ping_db`，符合分层架构
#[tracing::instrument(skip(state))]
pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    let db_ok = health_service::ping_db(&state.db_pool).await;

    if db_ok {
        (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "status": "ready",
                    "checks": { "db": "ok" }
                }
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "code": 0,
                "msg": "not ready",
                "data": {
                    "status": "unavailable",
                    "checks": { "db": "fail" }
                }
            })),
        )
    }
}

/// 启动检查（startup probe）：验证应用启动初期的依赖可用性
///
/// 与 liveness/readiness 区分：
/// - liveness：运行期间持续探活，失败触发重启
/// - readiness：运行期间持续探活，失败从负载均衡摘除
/// - startup：仅在启动初期探活，成功后 liveness/readiness 接管
///
/// 启动检查不依赖 DB（避免 DB 慢启动导致 Pod 反复重启），
/// 仅验证关键静态资源（Metrics Registry）已完成初始化。
#[tracing::instrument(skip(state))]
pub async fn startup(State(state): State<AppState>) -> impl IntoResponse {
    let metrics_rendered = state.metrics_registry.render();
    if metrics_rendered.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "code": 0,
                "msg": "not ready",
                "data": {
                    "status": "starting",
                    "checks": { "metrics": "not_initialized" }
                }
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "status": "started",
                "checks": {
                    "metrics": "ok"
                }
            }
        })),
    )
}

/// Prometheus 指标端点 — 输出 Prometheus 文本格式指标
///
/// 访问控制（2026-10-28 P1-2 修复，fail-closed）：
/// - 配置 `SZ300_METRICS_TOKEN` → 要求 `Authorization: Bearer <token>`
/// - 未配置令牌 → 仅当 `SZ300_METRICS_PUBLIC=true` 时公开；默认拒绝（401）
#[tracing::instrument(skip(state, req))]
pub async fn metrics(
    State(state): State<AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let provided = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    if !is_metrics_token_valid(
        state.metrics_token.as_deref(),
        state.metrics_public,
        provided,
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            [("content-type", "text/plain")],
            "Unauthorized".to_string(),
        );
    }

    let mut body = state.metrics_registry.render();
    body.push_str(&render_db_pool_metrics(&state.db_pool));
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        body,
    )
}

/// 渲染数据库连接池指标（抓取时读取，无后台线程）
///
/// 数据源为 `sz-orm` `Pool::pool_metrics()` 的累计计数器与 `max_size()`，
/// 用于发现「池耗尽」「连接频繁重建」两类问题（2026-10-28 数据层修复）。
fn render_db_pool_metrics(pool: &sz_rust_core::orm::Pool) -> String {
    let m = pool.pool_metrics();
    format_db_pool_metrics(
        m.acquire_count,
        m.acquire_failed_count,
        m.release_count,
        m.connection_created_count,
        m.connection_closed_count,
        m.average_acquire_wait_time().as_secs_f64(),
        m.connection_reuse_rate(),
        pool.max_size(),
    )
}

#[allow(clippy::too_many_arguments)]
fn format_db_pool_metrics(
    acquire_total: u64,
    acquire_failed_total: u64,
    release_total: u64,
    created_total: u64,
    closed_total: u64,
    avg_acquire_wait_seconds: f64,
    reuse_ratio: f64,
    max_connections: u32,
) -> String {
    format!(
        "# HELP sz300_db_pool_acquire_total Total successful connection acquisitions\n\
         # TYPE sz300_db_pool_acquire_total counter\n\
         sz300_db_pool_acquire_total {acquire_total}\n\
         # HELP sz300_db_pool_acquire_failed_total Total failed connection acquisitions\n\
         # TYPE sz300_db_pool_acquire_failed_total counter\n\
         sz300_db_pool_acquire_failed_total {acquire_failed_total}\n\
         # HELP sz300_db_pool_release_total Total connection releases\n\
         # TYPE sz300_db_pool_release_total counter\n\
         sz300_db_pool_release_total {release_total}\n\
         # HELP sz300_db_pool_connection_created_total Total connections created\n\
         # TYPE sz300_db_pool_connection_created_total counter\n\
         sz300_db_pool_connection_created_total {created_total}\n\
         # HELP sz300_db_pool_connection_closed_total Total connections closed\n\
         # TYPE sz300_db_pool_connection_closed_total counter\n\
         sz300_db_pool_connection_closed_total {closed_total}\n\
         # HELP sz300_db_pool_acquire_wait_seconds_avg Average wait per acquisition\n\
         # TYPE sz300_db_pool_acquire_wait_seconds_avg gauge\n\
         sz300_db_pool_acquire_wait_seconds_avg {avg_acquire_wait_seconds}\n\
         # HELP sz300_db_pool_reuse_ratio Connection reuse ratio (0..1)\n\
         # TYPE sz300_db_pool_reuse_ratio gauge\n\
         sz300_db_pool_reuse_ratio {reuse_ratio}\n\
         # HELP sz300_db_pool_max_connections Configured pool capacity\n\
         # TYPE sz300_db_pool_max_connections gauge\n\
         sz300_db_pool_max_connections {max_connections}\n"
    )
}

/// 校验 `/metrics` 的 Bearer 令牌（常量时间比较，防时序攻击）
///
/// - `metrics_token` 为 `Some` → 要求 `authorization` 头精确等于 `Bearer <token>`
/// - `metrics_token` 为 `None` → 返回 `metrics_public`（默认 false，显式开启才公开）
pub(crate) fn is_metrics_token_valid(
    metrics_token: Option<&str>,
    metrics_public: bool,
    authorization: Option<&str>,
) -> bool {
    let Some(expected) = metrics_token else {
        return metrics_public;
    };
    let Some(provided) = authorization else {
        return false;
    };
    let expected_bearer = format!("Bearer {expected}");
    constant_time_eq(provided.as_bytes(), expected_bearer.as_bytes())
}

/// 常量时间比较（防时序攻击）
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod metrics_auth_tests {
    use super::*;

    #[test]
    fn test_no_token_fail_closed_by_default() {
        // P1-2：未配置令牌且未显式开启公开 → 拒绝
        assert!(!is_metrics_token_valid(None, false, None));
        assert!(!is_metrics_token_valid(
            None,
            false,
            Some("Bearer anything")
        ));
    }

    #[test]
    fn test_no_token_public_only_when_opted_in() {
        assert!(is_metrics_token_valid(None, true, None));
        assert!(is_metrics_token_valid(None, true, Some("Bearer anything")));
    }

    #[test]
    fn test_missing_authorization_rejected() {
        assert!(!is_metrics_token_valid(Some("secret"), false, None));
        assert!(!is_metrics_token_valid(Some("secret"), true, None));
        assert!(!is_metrics_token_valid(Some("secret"), false, Some("")));
    }

    #[test]
    fn test_wrong_token_rejected() {
        assert!(!is_metrics_token_valid(
            Some("secret"),
            false,
            Some("Bearer wrong")
        ));
        assert!(!is_metrics_token_valid(
            Some("secret"),
            false,
            Some("Basic abc")
        ));
        // 大小写敏感
        assert!(!is_metrics_token_valid(
            Some("secret"),
            false,
            Some("bearer secret")
        ));
    }

    #[test]
    fn test_correct_token_accepted() {
        assert!(is_metrics_token_valid(
            Some("secret"),
            false,
            Some("Bearer secret")
        ));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_db_pool_metrics_prometheus_text() {
        let text = format_db_pool_metrics(120, 3, 117, 12, 2, 0.0025, 0.9, 20);
        assert!(text.contains("sz300_db_pool_acquire_total 120"));
        assert!(text.contains("sz300_db_pool_acquire_failed_total 3"));
        assert!(text.contains("sz300_db_pool_release_total 117"));
        assert!(text.contains("sz300_db_pool_connection_created_total 12"));
        assert!(text.contains("sz300_db_pool_connection_closed_total 2"));
        assert!(text.contains("sz300_db_pool_acquire_wait_seconds_avg 0.0025"));
        assert!(text.contains("sz300_db_pool_reuse_ratio 0.9"));
        assert!(text.contains("sz300_db_pool_max_connections 20"));
        // 每行指标都必须有 TYPE 声明，否则 Prometheus 抓取告警
        for line in text.lines().filter(|l| l.starts_with("sz300_db_pool_")) {
            let name = line.split_whitespace().next().unwrap_or_default();
            assert!(
                text.contains(&format!("# TYPE {name} ")),
                "缺少 TYPE 声明: {name}"
            );
        }
    }
}

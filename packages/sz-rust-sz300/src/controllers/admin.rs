// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 管理监控控制器 — `/api/admin/*`
//!
//! 提供系统信息、数据库连接池、Redis 状态的监控端点。
//! 所有端点均需 `admin` 角色（由 [`crate::middleware::role_guard`] 拦截）。
//!
//! ## 端点
//!
//! - `GET /api/admin/server/info` — 服务器系统信息（CPU/内存/磁盘/负载）
//! - `GET /api/admin/db/pool` — 数据库连接池实时状态
//! - `GET /api/admin/redis/info` — Redis 服务器状态（未配置时返回降级响应）

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::state::AppState;

/// 服务器系统信息端点
///
/// 采集当前进程所在主机的 CPU / 内存 / 磁盘 / 负载信息。
/// 调用 `sz_rust_observability::admin::collect_server_info()`，
/// 该函数内部使用 `sysinfo::System::new_all()`，首次调用约 10-50ms。
pub async fn server_info() -> impl IntoResponse {
    let info = sz_rust_observability::admin::collect_server_info();
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": info,
    }))
}

/// 数据库连接池状态端点
///
/// 查询 MySQL 主连接池的实时状态（活跃 / 空闲 / 最大连接数 / 使用率）。
/// 通过 `AppState::db_pool_stats` 适配器读取，不直接操作 `Pool` 内部字段。
pub async fn db_pool(State(state): State<AppState>) -> impl IntoResponse {
    let info = state.db_pool_stats.stats();
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": info,
    }))
}

/// Redis 状态端点
///
/// 查询 Redis 服务器实时状态（版本 / 连接数 / 内存 / 运行时长 / 角色 / 命中率 / 持久化等）。
///
/// ## 降级策略
///
/// - Redis 未配置（`state.redis_stats` 为 None）：返回 200 + `connected: false`，
///   提示 "Redis 未配置"
/// - Redis 探活失败（连接拒绝 / 超时）：返回 503 + 错误详情
#[tracing::instrument(skip_all)]
pub async fn redis_info(State(state): State<AppState>) -> Response {
    match &state.redis_stats {
        Some(stats) => match stats.info() {
            Ok(info) => Json(json!({
                "code": 1,
                "msg": "success",
                "data": info,
            }))
            .into_response(),
            Err(e) => (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "code": 0,
                    "msg": "Redis 探活失败",
                    "data": {
                        "connected": false,
                        "error": e.message,
                    }
                })),
            )
                .into_response(),
        },
        None => Json(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "connected": false,
                "uptime_in_seconds": 0,
                "uptime_in_days": 0,
                "connected_clients": 0,
                "used_memory": "",
                "variable": {
                    "used_memory": 0,
                    "used_memory_peak": 0,
                    "used_memory_rss": 0,
                    "mem_fragmentation_ratio": 0.0,
                    "keyspace_hits": 0,
                    "keyspace_misses": 0,
                    "expired_keys": 0,
                    "evicted_keys": 0,
                    "instantaneous_ops_per_sec": 0,
                    "instantaneous_input_kbps": 0.0,
                    "instantaneous_output_kbps": 0.0,
                    "total_commands_processed": 0,
                    "redis_version": "",
                    "redis_mode": "",
                    "os": "",
                    "arch_bits": 0,
                    "mem_allocator": "",
                    "role": "",
                    "tcp_port": 0,
                    "aof_enabled": 0,
                    "rdb_changes_since_last_save": 0,
                    "total_connections_received": 0,
                },
                "note": "Redis 未配置（ADMIN_REDIS_URL 环境变量未设置）"
            }
        }))
        .into_response(),
    }
}
#[cfg(test)]
mod tests {
    //! ## D3 开放决策确认结果（T6, 2026-11-06）
    //!
    //! `admin.rs` 当前是**死代码**，不参与编译，存在三重编译障碍：
    //!
    //! 1. **`controllers/mod.rs` 未声明 `pub mod admin;`**（文件未被引入，rustc 不解析）
    //! 2. **`AppState` 无 `db_pool_stats`/`redis_stats` 字段**（`state.rs` 未定义）
    //! 3. **`collect_server_info` 是 `async fn` 但 admin.rs:27 未 `.await`**，
    //!    且 `sz-rust-observability` 的 `admin` feature 未启用（函数不存在）
    //!
    //! 因此本测试模块当前**不编译**（admin.rs 未被 mod 引入）。
    //! 以下测试为"准备就绪"资产：待 admin.rs 接线后即可启用。接线需：
    //! - `mod.rs` 加 `pub mod admin;`
    //! - `state.rs` 补 `db_pool_stats: Arc<dyn DbPoolStats>` + `redis_stats: Option<RedisStats>` 字段
    //! - `Cargo.toml` 启用 `sz-rust-observability/admin` feature
    //! - `admin.rs:27` 修复 `collect_server_info().await`
    //!
    //! 在"不修改生产代码"约束下，无法让这些测试可编译。

    use super::*;
    use crate::state::mock_app_state;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// 收集 Response body 为 String。
    async fn collect_body(response: Response) -> String {
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        String::from_utf8(body_bytes.to_vec()).expect("UTF-8")
    }

    /// `GET /admin/server/info` → JSON 含 `code:1` + 系统信息（CPU/内存/磁盘/负载）。
    ///
    /// 覆盖 `server_info` handler 全路径（R4-AC8）。
    #[tokio::test]
    async fn server_info_returns_code_1_with_system_info() {
        let router = Router::new().route("/admin/server/info", get(server_info));
        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/server/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = collect_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(json["code"], 1);
        assert_eq!(json["msg"], "success");
        // data 应为 ServerInfo 对象（含 memory/env/cpu 等字段）
        assert!(json["data"].is_object(), "data 应含系统信息对象");
    }

    /// `GET /admin/db/pool` → JSON 含 `code:1` + 连接池状态（active/idle/max/usage_percent）。
    ///
    /// 覆盖 `db_pool` handler 全路径（R4-AC9）。
    #[tokio::test]
    async fn db_pool_returns_code_1_with_pool_stats() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/admin/db/pool", get(db_pool))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/db/pool")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = collect_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(json["code"], 1);
        assert_eq!(json["msg"], "success");
        assert!(json["data"].is_object(), "data 应含连接池状态");
    }

    /// `GET /admin/redis/info`（Redis 未配置）→ 降级响应 `code:1` + `connected:false`。
    ///
    /// 覆盖 `redis_info` handler 的 `None` 分支（降级路径）。
    /// `mock_app_state()` 构造的 AppState 中 `redis_stats` 为 None，
    /// 命中第 80-116 行的降级响应分支。
    #[tokio::test]
    async fn redis_info_degraded_when_unconfigured() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/admin/redis/info", get(redis_info))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/redis/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = collect_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(json["code"], 1);
        assert_eq!(json["data"]["connected"], false);
        assert_eq!(json["data"]["uptime_in_seconds"], 0);
        assert_eq!(json["data"]["connected_clients"], 0);
    }
}

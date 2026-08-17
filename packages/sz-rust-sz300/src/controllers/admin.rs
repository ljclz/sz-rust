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
#[tracing::instrument(skip_all)]
pub async fn server_info() -> impl IntoResponse {
    let info = sz_rust_observability::admin::collect_server_info().await;
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
#[tracing::instrument(skip_all)]
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

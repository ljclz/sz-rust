// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 管理后台路由 — `AdminApiState` 与 `build_admin_router`
//!
//! 注册 13 个 REST 端点，统一挂载 `admin_guard_middleware` 鉴权层。
//! axum 0.8 路径参数语法：`{name}`（非 `:name`）。

use std::sync::Arc;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;

use sz_rust_orm_facade::data_scope::ext::config_loader::ConfigLoader;
use sz_rust_orm_facade::data_scope::ext::hot_reload::HotReloadManager;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;

use super::guard::admin_guard_middleware;
use super::handlers::{
    create_policy, create_rule, delete_policy, delete_rule, get_generation, get_policy, get_rule,
    list_policies, list_rules, load_config, reload_config, update_policy, update_rule,
};

/// 管理后台 API 状态
///
/// 持有热重载管理器、配置加载器、指标与管理员角色列表，
/// 通过 `Clone` 分发给各 handler 与鉴权中间件。
#[derive(Clone)]
pub struct AdminApiState {
    /// 热重载管理器（规则/策略 CRUD + 世代号 + 通知）
    pub manager: Arc<HotReloadManager>,
    /// 配置加载器（YAML/JSON 加载与重载）
    pub config_loader: Arc<ConfigLoader>,
    /// 指标采集
    pub metrics: Arc<DataScopeMetrics>,
    /// 管理员角色名列表（鉴权用）
    pub admin_roles: Vec<String>,
}

/// 构造管理后台路由
///
/// 注册 13 个端点并挂载 `admin_guard_middleware`：
/// - 规则：POST/GET `/api/data-perm/rules`、GET/PUT/DELETE `/api/data-perm/rules/{rule_id}`
/// - 策略：POST/GET `/api/data-perm/policies`、GET/PUT/DELETE `/api/data-perm/policies/{table}/{role}`
/// - 配置：POST `/api/data-perm/load`、POST `/api/data-perm/reload`、GET `/api/data-perm/generation`
pub fn build_admin_router(state: AdminApiState) -> Router {
    Router::new()
        // 规则 CRUD
        .route("/api/data-perm/rules", post(create_rule).get(list_rules))
        .route(
            "/api/data-perm/rules/{rule_id}",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
        // 策略 CRUD
        .route(
            "/api/data-perm/policies",
            post(create_policy).get(list_policies),
        )
        .route(
            "/api/data-perm/policies/{table}/{role}",
            get(get_policy).put(update_policy).delete(delete_policy),
        )
        // 配置加载/重载/世代号
        .route("/api/data-perm/load", post(load_config))
        .route("/api/data-perm/reload", post(reload_config))
        .route("/api/data-perm/generation", get(get_generation))
        .layer(from_fn_with_state(state.clone(), admin_guard_middleware))
        .with_state(state)
}

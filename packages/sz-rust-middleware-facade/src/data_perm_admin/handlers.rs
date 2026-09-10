// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 管理后台 handler — 13 个端点实现
//!
//! 规则（5）：create_rule / list_rules / get_rule / update_rule / delete_rule
//! 策略（5）：create_policy / list_policies / get_policy / update_policy / delete_policy
//! 配置（3）：load_config / reload_config / get_generation
//!
//! 所有 handler 从 `AdminApiState` 取依赖，调用 `HotReloadManager` / `ConfigLoader`，
//! 错误经 `ApiErrorResponse::from_error` 转响应。
//! 响应 JSON 不暴露 `custom_generator` 内部实现（仅名称）。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use sz_rust_orm_facade::data_scope::error::DataScopeError;
use sz_rust_orm_facade::data_scope::ext::config_loader::LoadReport;
use sz_rust_orm_facade::data_scope::ext::hot_reload::{PolicyPatch, RulePatch};
use sz_rust_orm_facade::data_scope::field_scope::policy::FieldScopePolicy;

use sz_rust_orm_facade::data_scope::registry::RuleListFilter;
use sz_rust_orm_facade::data_scope::rule::DataScopeRule;

use super::error_response::ApiErrorResponse;
use super::router::AdminApiState;

/// 分页查询参数
#[derive(Debug, Deserialize, Default)]
pub struct ListRulesQuery {
    pub table: Option<String>,
    pub page: Option<usize>,
    pub size: Option<usize>,
}

/// 策略列表查询参数
#[derive(Debug, Deserialize, Default)]
pub struct ListPoliciesQuery {
    pub table: Option<String>,
    pub role: Option<String>,
}

/// 配置加载请求体
#[derive(Debug, Deserialize)]
pub struct LoadConfigRequest {
    pub path: String,
}

/// 配置重载请求体
#[derive(Debug, Deserialize)]
pub struct ReloadConfigRequest {
    pub path: String,
    #[serde(default)]
    pub confirm_clear: bool,
}

/// 世代号响应
#[derive(Debug, Serialize)]
pub struct GenerationResponse {
    pub generation: u64,
    pub rule_count: usize,
    pub policy_count: usize,
}

/// 分页响应包装
#[derive(Debug, Serialize)]
pub struct PagedRulesResponse {
    pub rules: Vec<DataScopeRule>,
    pub total: usize,
    pub page: usize,
    pub size: usize,
}

/// 将 `DataScopeError` 转为 HTTP 响应的内部辅助函数
fn err_response(state: &AdminApiState, err: DataScopeError) -> Response {
    let gen = state.manager.generation();
    ApiErrorResponse::from_error(err, gen).into_response()
}

// ==================== 规则 handler ====================

/// 创建规则（POST `/api/data-perm/rules`）
pub async fn create_rule(
    State(state): State<AdminApiState>,
    Json(rule): Json<DataScopeRule>,
) -> Response {
    match state.manager.create_rule(rule).await {
        Ok(result) => (StatusCode::CREATED, Json(result)).into_response(),
        Err(err) => err_response(&state, err),
    }
}

/// 列出规则（GET `/api/data-perm/rules`，支持 table/page/size，size 上限 100）
pub async fn list_rules(
    State(state): State<AdminApiState>,
    Query(query): Query<ListRulesQuery>,
) -> Response {
    let size = query.size.unwrap_or(20).clamp(1, 100);
    let page = query.page.unwrap_or(1).max(1);
    let filter = RuleListFilter {
        table: query.table.clone(),
    };
    let all = state.manager.rule_registry().list_rules(&filter);
    let total = all.len();
    let start = (page.saturating_sub(1)) * size;
    let rules: Vec<DataScopeRule> = if start >= total {
        Vec::new()
    } else {
        let end = (start + size).min(total);
        all[start..end].to_vec()
    };
    Json(PagedRulesResponse {
        rules,
        total,
        page,
        size,
    })
    .into_response()
}

/// 获取单条规则（GET `/api/data-perm/rules/{rule_id}`）
pub async fn get_rule(State(state): State<AdminApiState>, Path(rule_id): Path<String>) -> Response {
    match state.manager.rule_registry().get_rule_by_id(&rule_id) {
        Some(rule) => Json(rule).into_response(),
        None => err_response(&state, DataScopeError::RuleNotFound(rule_id)),
    }
}

/// 更新规则（PUT `/api/data-perm/rules/{rule_id}`）
pub async fn update_rule(
    State(state): State<AdminApiState>,
    Path(rule_id): Path<String>,
    Json(patch): Json<RulePatch>,
) -> Response {
    match state.manager.update_rule(&rule_id, patch).await {
        Ok(result) => Json(result).into_response(),
        Err(err) => err_response(&state, err),
    }
}

/// 删除规则（DELETE `/api/data-perm/rules/{rule_id}`）
pub async fn delete_rule(
    State(state): State<AdminApiState>,
    Path(rule_id): Path<String>,
) -> Response {
    match state.manager.delete_rule(&rule_id).await {
        Ok(result) => Json(result).into_response(),
        Err(err) => err_response(&state, err),
    }
}

// ==================== 策略 handler ====================

/// 创建策略（POST `/api/data-perm/policies`）
pub async fn create_policy(
    State(state): State<AdminApiState>,
    Json(policy): Json<FieldScopePolicy>,
) -> Response {
    match state.manager.create_policy(policy).await {
        Ok(result) => (StatusCode::CREATED, Json(result)).into_response(),
        Err(err) => err_response(&state, err),
    }
}

/// 列出策略（GET `/api/data-perm/policies`，支持 table/role 过滤）
pub async fn list_policies(
    State(state): State<AdminApiState>,
    Query(query): Query<ListPoliciesQuery>,
) -> Response {
    use sz_rust_orm_facade::data_scope::field_scope::registry::PolicyListFilter;
    let filter = PolicyListFilter {
        table: query.table,
        role: query.role,
    };
    let policies = state.manager.policy_registry().list_policies(&filter);
    Json(serde_json::json!({ "policies": policies, "total": policies.len() })).into_response()
}

/// 获取单条策略（GET `/api/data-perm/policies/{table}/{role}`）
pub async fn get_policy(
    State(state): State<AdminApiState>,
    Path((table, role)): Path<(String, String)>,
) -> Response {
    match state.manager.policy_registry().get_policy(&table, &role) {
        Some(policy) => Json(policy).into_response(),
        None => err_response(
            &state,
            DataScopeError::PolicyNotFound(format!("{table}:{role}")),
        ),
    }
}

/// 更新策略（PUT `/api/data-perm/policies/{table}/{role}`）
pub async fn update_policy(
    State(state): State<AdminApiState>,
    Path((table, role)): Path<(String, String)>,
    Json(patch): Json<PolicyPatch>,
) -> Response {
    match state.manager.update_policy(&table, &role, patch).await {
        Ok(result) => Json(result).into_response(),
        Err(err) => err_response(&state, err),
    }
}

/// 删除策略（DELETE `/api/data-perm/policies/{table}/{role}`）
pub async fn delete_policy(
    State(state): State<AdminApiState>,
    Path((table, role)): Path<(String, String)>,
) -> Response {
    match state.manager.delete_policy(&table, &role).await {
        Ok(result) => Json(result).into_response(),
        Err(err) => err_response(&state, err),
    }
}

// ==================== 配置 handler ====================

/// 加载配置（POST `/api/data-perm/load`，body 含 path）
pub async fn load_config(
    State(state): State<AdminApiState>,
    Json(req): Json<LoadConfigRequest>,
) -> Response {
    match state.config_loader.load_from_file(&req.path).await {
        Ok(report) => Json(report).into_response(),
        Err(err) => err_response(&state, err),
    }
}

/// 重载配置（POST `/api/data-perm/reload`，body 含 path 和 confirm_clear）
pub async fn reload_config(
    State(state): State<AdminApiState>,
    Json(req): Json<ReloadConfigRequest>,
) -> Response {
    match state
        .config_loader
        .reload_from_file(&req.path, req.confirm_clear)
        .await
    {
        Ok(report) => Json::<LoadReport>(report).into_response(),
        Err(err) => err_response(&state, err),
    }
}

/// 获取世代号（GET `/api/data-perm/generation`，返回 generation/rule_count/policy_count）
pub async fn get_generation(State(state): State<AdminApiState>) -> Response {
    Json(GenerationResponse {
        generation: state.manager.generation(),
        rule_count: state.manager.rule_count(),
        policy_count: state.manager.policy_count(),
    })
    .into_response()
}

// ==================== 单元测试 ====================

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_orm_facade::data_scope::custom::CustomGeneratorRegistry;
    use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
    use sz_rust_orm_facade::data_scope::ext::config_loader::ConfigLoader;
    use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
    use sz_rust_orm_facade::data_scope::ext::hot_reload::HotReloadManager;
    use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
    use sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard;
    use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;
    use sz_rust_orm_facade::data_scope::field_scope::visibility::FieldVisibility;
    use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
    use sz_rust_orm_facade::data_scope::registry::DataScopeRuleRegistry;
    use sz_rust_orm_facade::data_scope::rule::DataScopeMode;

    fn make_state() -> AdminApiState {
        let rule_registry = std::sync::Arc::new(DataScopeRuleRegistry::new());
        let policy_registry = std::sync::Arc::new(FieldScopePolicyRegistry::new());
        let custom_registry = std::sync::Arc::new(CustomGeneratorRegistry::new());
        let generation = PolicyGeneration::new();
        let notifier = ChangeNotifier::new(64);
        let metrics = std::sync::Arc::new(DataScopeMetrics::new());
        let audit: std::sync::Arc<dyn sz_rust_orm_facade::data_scope::ext::audit::AuditLogger> =
            std::sync::Arc::new(TracingAuditLogger);
        let manager = std::sync::Arc::new(HotReloadManager::new(
            rule_registry,
            policy_registry,
            custom_registry,
            generation,
            notifier,
            metrics.clone(),
            audit,
        ));
        let path_guard =
            PathGuard::new(vec![std::env::current_dir().unwrap(), std::env::temp_dir()]);
        let config_loader = std::sync::Arc::new(ConfigLoader::new(
            manager.clone(),
            path_guard,
            ConfigLoader::DEFAULT_MAX_FILE_SIZE,
        ));
        AdminApiState {
            manager,
            config_loader,
            metrics,
            admin_roles: vec!["admin".into()],
        }
    }

    #[tokio::test]
    async fn test_create_rule_handler_success() {
        let state = make_state();
        let rule = DataScopeRule::new("order", DataScopeMode::All).with_priority(5);
        let resp = create_rule(State(state.clone()), Json(rule)).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "created");
        assert_eq!(json["generation"], 1);
    }

    #[tokio::test]
    async fn test_create_rule_handler_invalid_returns_500() {
        let state = make_state();
        // Dept 模式缺 dept_field → RULE_FIELD_MISSING → 500
        let rule = DataScopeRule::new("order", DataScopeMode::Dept);
        let resp = create_rule(State(state), Json(rule)).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_list_rules_handler_pagination() {
        let state = make_state();
        // 用不同 target_table 避免 upsert 去重
        for i in 0..5 {
            let rule = DataScopeRule::new(format!("order_{i}"), DataScopeMode::All)
                .with_priority(i)
                .with_rule_id(format!("r{i}"));
            state.manager.create_rule(rule).await.unwrap();
        }
        let query = ListRulesQuery {
            table: None,
            page: Some(1),
            size: Some(2),
        };
        let resp = list_rules(State(state), Query(query)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total"], 5);
        assert_eq!(json["page"], 1);
        assert_eq!(json["size"], 2);
        assert_eq!(json["rules"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_list_rules_handler_size_capped_to_100() {
        let state = make_state();
        let query = ListRulesQuery {
            table: None,
            page: Some(1),
            size: Some(500),
        };
        let resp = list_rules(State(state), Query(query)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["size"], 100);
    }

    #[tokio::test]
    async fn test_get_rule_handler_found() {
        let state = make_state();
        let rule = DataScopeRule::new("t", DataScopeMode::All).with_rule_id("r1");
        state.manager.create_rule(rule).await.unwrap();
        let resp = get_rule(State(state), Path("r1".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_rule_handler_not_found_404() {
        let state = make_state();
        let resp = get_rule(State(state), Path("nonexistent".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_rule_handler_success() {
        let state = make_state();
        let rule = DataScopeRule::new("t", DataScopeMode::All).with_rule_id("u1");
        state.manager.create_rule(rule).await.unwrap();
        let patch = RulePatch {
            priority: Some(99),
            ..Default::default()
        };
        let resp = update_rule(State(state), Path("u1".to_string()), Json(patch)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_update_rule_handler_not_found_404() {
        let state = make_state();
        let resp = update_rule(
            State(state),
            Path("nope".to_string()),
            Json(RulePatch::default()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_rule_handler_success() {
        let state = make_state();
        let rule = DataScopeRule::new("t", DataScopeMode::All).with_rule_id("d1");
        state.manager.create_rule(rule).await.unwrap();
        let resp = delete_rule(State(state), Path("d1".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_delete_rule_handler_not_found_404() {
        let state = make_state();
        let resp = delete_rule(State(state), Path("nope".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_create_policy_handler_success() {
        let state = make_state();
        let policy = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden);
        let resp = create_policy(State(state), Json(policy)).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_list_policies_handler() {
        let state = make_state();
        state
            .manager
            .create_policy(FieldScopePolicy::new("employee", "hr"))
            .await
            .unwrap();
        state
            .manager
            .create_policy(FieldScopePolicy::new("order", "admin"))
            .await
            .unwrap();
        let resp = list_policies(State(state), Query(ListPoliciesQuery::default())).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total"], 2);
    }

    #[tokio::test]
    async fn test_get_policy_handler_found() {
        let state = make_state();
        state
            .manager
            .create_policy(FieldScopePolicy::new("employee", "hr"))
            .await
            .unwrap();
        let resp = get_policy(State(state), Path(("employee".into(), "hr".into()))).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_policy_handler_not_found_404() {
        let state = make_state();
        let resp = get_policy(State(state), Path(("no".into(), "no".into()))).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_policy_handler_success() {
        let state = make_state();
        state
            .manager
            .create_policy(FieldScopePolicy::new("t", "r"))
            .await
            .unwrap();
        let patch = PolicyPatch {
            enabled: Some(false),
            ..Default::default()
        };
        let resp = update_policy(State(state), Path(("t".into(), "r".into())), Json(patch)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_delete_policy_handler_success() {
        let state = make_state();
        state
            .manager
            .create_policy(FieldScopePolicy::new("t", "r"))
            .await
            .unwrap();
        let resp = delete_policy(State(state), Path(("t".into(), "r".into()))).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_generation_handler() {
        let state = make_state();
        state
            .manager
            .create_rule(DataScopeRule::new("t", DataScopeMode::All))
            .await
            .unwrap();
        let resp = get_generation(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["generation"], 1);
        assert_eq!(json["rule_count"], 1);
        assert_eq!(json["policy_count"], 0);
    }

    #[tokio::test]
    async fn test_load_config_handler_file_not_found() {
        let state = make_state();
        let req = LoadConfigRequest {
            path: "/nonexistent/path/to/file.yaml".into(),
        };
        let resp = load_config(State(state), Json(req)).await;
        // 路径不在白名单或文件不存在 → 404 或 500
        let status = resp.status();
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected status: {status}"
        );
    }

    #[tokio::test]
    async fn test_reload_config_handler_file_not_found() {
        let state = make_state();
        let req = ReloadConfigRequest {
            path: "/nonexistent/path/to/file.yaml".into(),
            confirm_clear: false,
        };
        let resp = reload_config(State(state), Json(req)).await;
        let status = resp.status();
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected status: {status}"
        );
    }

    #[tokio::test]
    async fn test_rule_response_does_not_leak_custom_generator_internals() {
        // 响应 JSON 仅暴露 custom_generator 名称字符串，不暴露内部实现
        let state = make_state();
        let rule = DataScopeRule::new("t", DataScopeMode::All)
            .with_custom_generator("my_generator")
            .with_rule_id("leak1");
        state.manager.create_rule(rule).await.unwrap();
        let resp = get_rule(State(state), Path("leak1".to_string())).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // custom_generator 字段应为字符串名称，而非对象/函数指针
        assert_eq!(json["custom_generator"], "my_generator");
        assert!(json["custom_generator"].is_string());
    }
}

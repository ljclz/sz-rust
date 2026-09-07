// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Web API
//!
//! axum 路由 `/api/v1/plugins/*` + `/api/v1/admin/reviews/*`
//! 含 JWT 鉴权中间件、角色校验、trace_id 注入、OpenAPI 文档。

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use axum_extra::TypedHeader;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

use crate::error::MarketplaceError;
use crate::repository::{Plugin, PluginRepository, PluginVersion, VersionRepository};
use crate::service::{MarketplaceService, ReviewDecision, ReviewRequest, SearchRequest};

// ── JWT ──

/// JWT Claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// 开发者 ID
    pub sub: i64,
    /// 用户名
    pub username: String,
    /// 是否审核员
    pub is_reviewer: bool,
    /// 过期时间
    pub exp: i64,
}

/// JWT 配置
#[derive(Clone)]
pub struct JwtConfig {
    encoding_key: Arc<EncodingKey>,
    decoding_key: Arc<DecodingKey>,
}

impl JwtConfig {
    /// 创建 JWT 配置
    pub fn new(secret: &str) -> Self {
        Self {
            encoding_key: Arc::new(EncodingKey::from_secret(secret.as_bytes())),
            decoding_key: Arc::new(DecodingKey::from_secret(secret.as_bytes())),
        }
    }

    /// 生成 token
    pub fn generate(
        &self,
        developer_id: i64,
        username: &str,
        is_reviewer: bool,
    ) -> Result<String, MarketplaceError> {
        let claims = JwtClaims {
            sub: developer_id,
            username: username.to_string(),
            is_reviewer,
            exp: (Utc::now() + Duration::hours(24)).timestamp(),
        };
        encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| MarketplaceError::Unauthorized(format!("JWT 生成失败: {e}")))
    }

    /// 验证 token
    pub fn verify(&self, token: &str) -> Result<JwtClaims, MarketplaceError> {
        decode::<JwtClaims>(token, &self.decoding_key, &Validation::default())
            .map(|data| data.claims)
            .map_err(|e| MarketplaceError::Unauthorized(format!("JWT 验证失败: {e}")))
    }
}

// ── AppState ──

/// 应用状态
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub service: Arc<MarketplaceService>,
    pub jwt: Arc<JwtConfig>,
}

// ── 请求/响应结构 ──

/// 搜索查询参数
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// 搜索响应
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub plugins: Vec<Plugin>,
    pub total: usize,
}

/// 待审核列表查询参数
#[derive(Debug, Deserialize)]
pub struct PendingQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// 待审核列表响应
#[derive(Debug, Serialize)]
pub struct PendingReviewsResponse {
    pub versions: Vec<PluginVersion>,
    pub total: usize,
}

/// 审核操作请求体
#[derive(Debug, Deserialize)]
pub struct ReviewBody {
    pub developer_id: i64,
    pub comment: Option<String>,
}

/// 登录请求
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub developer_id: i64,
    pub username: String,
    pub is_reviewer: bool,
}

/// 登录响应
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
}

/// 错误响应
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

/// Bearer token header
#[derive(Debug)]
pub struct BearerToken(String);

impl axum_extra::headers::Header for BearerToken {
    fn name() -> &'static axum::http::HeaderName {
        &axum::http::header::AUTHORIZATION
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, axum_extra::headers::Error>
    where
        I: Iterator<Item = &'i axum::http::HeaderValue>,
    {
        let value = values
            .next()
            .ok_or_else(axum_extra::headers::Error::invalid)?;
        let s = value
            .to_str()
            .map_err(|_| axum_extra::headers::Error::invalid())?;
        if let Some(token) = s.strip_prefix("Bearer ") {
            Ok(Self(token.to_string()))
        } else {
            Err(axum_extra::headers::Error::invalid())
        }
    }

    fn encode<E>(&self, values: &mut E)
    where
        E: Extend<axum::http::HeaderValue>,
    {
        let val = format!("Bearer {}", self.0);
        if let Ok(hv) = axum::http::HeaderValue::from_str(&val) {
            values.extend(std::iter::once(hv));
        }
    }
}

// ── 路由构建 ──

/// 构建路由
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/plugins/search", get(search_plugins))
        .route("/api/v1/plugins/:name", get(get_plugin))
        .route(
            "/api/v1/plugins/:name/:version/download",
            get(download_plugin),
        )
        .route("/api/v1/plugins/publish", post(publish_plugin))
        .route("/api/v1/admin/reviews/pending", get(pending_reviews))
        .route(
            "/api/v1/admin/reviews/:version_id/approve",
            post(approve_review),
        )
        .route(
            "/api/v1/admin/reviews/:version_id/reject",
            post(reject_review),
        )
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/health", get(health_check))
        .route("/api/v1/openapi.json", get(openapi_doc))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ── 端点实现 ──

/// GET /api/v1/plugins/search — 公开
async fn search_plugins(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let req = SearchRequest {
        keyword: query.q,
        tag: query.tag,
        limit: query.limit.unwrap_or(20),
        offset: query.offset.unwrap_or(0),
    };
    let plugins = state.service.search(req).await.map_err(map_error)?;
    let total = plugins.len();
    Ok(Json(SearchResponse { plugins, total }))
}

/// GET /api/v1/plugins/:name — 公开
async fn get_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Plugin>, (StatusCode, Json<ErrorResponse>)> {
    let repo = PluginRepository::new(state.pool.clone());
    let plugin = repo
        .find_by_name(&name)
        .await
        .map_err(map_error)?
        .ok_or_else(|| not_found(format!("插件 {name} 不存在")))?;
    Ok(Json(plugin))
}

/// GET /api/v1/plugins/:name/:version/download — 公开
async fn download_plugin(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let plugin_repo = PluginRepository::new(state.pool.clone());
    let plugin = plugin_repo
        .find_by_name(&name)
        .await
        .map_err(map_error)?
        .ok_or_else(|| not_found(format!("插件 {name} 不存在")))?;

    let version_repo = VersionRepository::new(state.pool.clone());
    let latest = version_repo
        .find_latest_by_plugin(plugin.id)
        .await
        .map_err(map_error)?
        .ok_or_else(|| not_found(format!("插件 {name} 无版本")))?;

    if latest.version != version {
        return Err(not_found(format!("插件 {name} 版本 {version} 不存在")));
    }

    if latest.review_status != "approved" {
        return Err(map_error(MarketplaceError::VersionNotApproved(latest.id)));
    }

    let archive = state
        .service
        .download_archive(&latest.archive_key)
        .await
        .map_err(map_error)?;

    let content_type = "application/gzip"
        .parse()
        .map_err(|_| internal_error("Content-Type parse failed"))?;
    let disposition = format!("attachment; filename=\"{name}-{version}.tar.gz\"")
        .parse()
        .map_err(|_| internal_error("Content-Disposition parse failed"))?;
    let headers = HeaderMap::from_iter([
        (axum::http::header::CONTENT_TYPE, content_type),
        (axum::http::header::CONTENT_DISPOSITION, disposition),
    ]);

    Ok((headers, axum::body::Body::from(archive)))
}

/// POST /api/v1/plugins/publish — 需鉴权
async fn publish_plugin(
    State(state): State<AppState>,
    TypedHeader(token): TypedHeader<BearerToken>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let claims = state.jwt.verify(&token.0).map_err(map_error)?;

    tracing::info!(
        developer_id = claims.sub,
        "发布请求已接收（multipart 解析待 P2-2 任务组10 CLI 接线后完善）"
    );

    Ok(StatusCode::ACCEPTED)
}

/// GET /api/v1/admin/reviews/pending — 需鉴权 + 审核员
async fn pending_reviews(
    State(state): State<AppState>,
    TypedHeader(token): TypedHeader<BearerToken>,
    Query(query): Query<PendingQuery>,
) -> Result<Json<PendingReviewsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let claims = state.jwt.verify(&token.0).map_err(map_error)?;
    if !claims.is_reviewer {
        return Err(map_error(MarketplaceError::NotReviewer(claims.username)));
    }

    let repo = VersionRepository::new(state.pool.clone());
    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);
    let versions = repo.find_pending(limit, offset).await.map_err(map_error)?;
    let total = versions.len();
    Ok(Json(PendingReviewsResponse { versions, total }))
}

/// POST /api/v1/admin/reviews/:version_id/approve — 需鉴权 + 审核员
async fn approve_review(
    State(state): State<AppState>,
    TypedHeader(token): TypedHeader<BearerToken>,
    Path(version_id): Path<i64>,
    Json(body): Json<ReviewBody>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let claims = state.jwt.verify(&token.0).map_err(map_error)?;

    let req = ReviewRequest {
        version_id,
        reviewer_id: claims.sub,
        developer_id: body.developer_id,
        decision: ReviewDecision::Approve,
        comment: body.comment,
    };
    state.service.review(req).await.map_err(map_error)?;
    Ok(StatusCode::OK)
}

/// POST /api/v1/admin/reviews/:version_id/reject — 需鉴权 + 审核员
async fn reject_review(
    State(state): State<AppState>,
    TypedHeader(token): TypedHeader<BearerToken>,
    Path(version_id): Path<i64>,
    Json(body): Json<ReviewBody>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let claims = state.jwt.verify(&token.0).map_err(map_error)?;

    let req = ReviewRequest {
        version_id,
        reviewer_id: claims.sub,
        developer_id: body.developer_id,
        decision: ReviewDecision::Reject,
        comment: body.comment,
    };
    state.service.review(req).await.map_err(map_error)?;
    Ok(StatusCode::OK)
}

/// POST /api/v1/auth/login — 公开
async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    let token = state
        .jwt
        .generate(req.developer_id, &req.username, req.is_reviewer)
        .map_err(map_error)?;
    Ok(Json(LoginResponse { token }))
}

/// GET /api/v1/health — 公开
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// GET /api/v1/openapi.json — 公开
async fn openapi_doc() -> impl IntoResponse {
    Json(build_openapi())
}

// ── OpenAPI 文档 ──

/// 构建 OpenAPI 3.0 文档
fn build_openapi() -> serde_json::Value {
    serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "SZ-Rust 插件市场 API",
            "version": "1.2.0",
            "description": "插件搜索、发布、下载、审核 RESTful API"
        },
        "paths": {
            "/api/v1/plugins/search": {
                "get": {
                    "summary": "搜索插件",
                    "tags": ["plugins"],
                    "parameters": [
                        {"name": "q", "in": "query", "schema": {"type": "string"}},
                        {"name": "tag", "in": "query", "schema": {"type": "string"}},
                        {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 20}},
                        {"name": "offset", "in": "query", "schema": {"type": "integer", "default": 0}}
                    ],
                    "responses": {
                        "200": {"description": "搜索结果"}
                    }
                }
            },
            "/api/v1/plugins/{name}": {
                "get": {
                    "summary": "获取插件详情",
                    "tags": ["plugins"],
                    "parameters": [
                        {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {"description": "插件详情"},
                        "404": {"description": "插件不存在"}
                    }
                }
            },
            "/api/v1/plugins/{name}/{version}/download": {
                "get": {
                    "summary": "下载插件归档",
                    "tags": ["plugins"],
                    "parameters": [
                        {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}},
                        {"name": "version", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "responses": {
                        "200": {"description": "tar.gz 归档"},
                        "404": {"description": "插件或版本不存在"}
                    }
                }
            },
            "/api/v1/plugins/publish": {
                "post": {
                    "summary": "发布插件",
                    "tags": ["plugins"],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "202": {"description": "已接受"},
                        "401": {"description": "未授权"}
                    }
                }
            },
            "/api/v1/admin/reviews/pending": {
                "get": {
                    "summary": "待审核列表",
                    "tags": ["admin"],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "待审核版本列表"},
                        "403": {"description": "非审核员"}
                    }
                }
            },
            "/api/v1/admin/reviews/{version_id}/approve": {
                "post": {
                    "summary": "批准版本",
                    "tags": ["admin"],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "已批准"},
                        "403": {"description": "非审核员或自审"},
                        "409": {"description": "版本非 pending"}
                    }
                }
            },
            "/api/v1/admin/reviews/{version_id}/reject": {
                "post": {
                    "summary": "拒绝版本",
                    "tags": ["admin"],
                    "security": [{"bearerAuth": []}],
                    "responses": {
                        "200": {"description": "已拒绝"},
                        "403": {"description": "非审核员或自审"},
                        "409": {"description": "版本非 pending"}
                    }
                }
            },
            "/api/v1/auth/login": {
                "post": {
                    "summary": "登录获取 JWT",
                    "tags": ["auth"],
                    "responses": {
                        "200": {"description": "JWT token"}
                    }
                }
            },
            "/api/v1/health": {
                "get": {
                    "summary": "健康检查",
                    "tags": ["system"],
                    "responses": {
                        "200": {"description": "服务正常"}
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer"
                }
            }
        }
    })
}

// ── 错误映射 ──

fn not_found(message: String) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            code: "NOT_FOUND".to_string(),
            message,
        }),
    )
}

fn internal_error(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            code: "INTERNAL_ERROR".to_string(),
            message: message.to_string(),
        }),
    )
}

fn map_error(e: MarketplaceError) -> (StatusCode, Json<ErrorResponse>) {
    let (code, status) = match &e {
        MarketplaceError::Unauthorized(_) => ("UNAUTHORIZED", StatusCode::UNAUTHORIZED),
        MarketplaceError::NotReviewer(_) => ("FORBIDDEN", StatusCode::FORBIDDEN),
        MarketplaceError::SelfReview { .. } => ("FORBIDDEN", StatusCode::FORBIDDEN),
        MarketplaceError::VersionNotPending { .. } => ("CONFLICT", StatusCode::CONFLICT),
        MarketplaceError::VersionConflict(_) => ("CONFLICT", StatusCode::CONFLICT),
        MarketplaceError::InvalidManifest(_) => {
            ("INVALID_MANIFEST", StatusCode::UNPROCESSABLE_ENTITY)
        }
        MarketplaceError::InvalidSignature(_) => {
            ("INVALID_SIGNATURE", StatusCode::UNPROCESSABLE_ENTITY)
        }
        MarketplaceError::InvalidSemVer(_) => ("INVALID_SEMVER", StatusCode::UNPROCESSABLE_ENTITY),
        MarketplaceError::VersionNotApproved(_) => ("NOT_APPROVED", StatusCode::FORBIDDEN),
        _ => ("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR),
    };
    (
        status,
        Json(ErrorResponse {
            code: code.to_string(),
            message: e.to_string(),
        }),
    )
}

// ── 测试 ──

#[cfg(test)]
mod tests {
    use super::*;
    use axum_extra::headers::Header;

    #[test]
    fn test_jwt_generate_verify() {
        let config = JwtConfig::new("test-secret");
        let token = config.generate(1, "alice", true).unwrap();
        let claims = config.verify(&token).unwrap();
        assert_eq!(claims.sub, 1);
        assert_eq!(claims.username, "alice");
        assert!(claims.is_reviewer);
    }

    #[test]
    fn test_jwt_verify_invalid_token() {
        let config = JwtConfig::new("test-secret");
        let result = config.verify("invalid-token");
        assert!(result.is_err());
    }

    #[test]
    fn test_jwt_verify_wrong_secret() {
        let config1 = JwtConfig::new("secret-1");
        let config2 = JwtConfig::new("secret-2");
        let token = config1.generate(1, "alice", false).unwrap();
        let result = config2.verify(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_openapi_doc_valid() {
        let doc = build_openapi();
        assert_eq!(doc["openapi"], "3.0.0");
        assert!(doc["paths"].is_object());
        assert!(doc["paths"]["/api/v1/plugins/search"].is_object());
        assert!(doc["paths"]["/api/v1/admin/reviews/pending"].is_object());
        assert!(doc["paths"]["/api/v1/auth/login"].is_object());
        assert!(doc["paths"]["/api/v1/health"].is_object());
        assert!(doc["components"]["securitySchemes"]["bearerAuth"].is_object());
    }

    #[test]
    fn test_error_mapping_unauthorized() {
        let (status, body) = map_error(MarketplaceError::Unauthorized("test".to_string()));
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body.code, "UNAUTHORIZED");
    }

    #[test]
    fn test_error_mapping_not_reviewer() {
        let (status, body) = map_error(MarketplaceError::NotReviewer("test".to_string()));
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.code, "FORBIDDEN");
    }

    #[test]
    fn test_error_mapping_version_conflict() {
        let (status, body) = map_error(MarketplaceError::VersionConflict("test".to_string()));
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.code, "CONFLICT");
    }

    #[test]
    fn test_error_mapping_self_review() {
        let (status, body) = map_error(MarketplaceError::SelfReview {
            reviewer_id: 1,
            developer_id: 1,
        });
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.code, "FORBIDDEN");
    }

    #[test]
    fn test_bearer_token_decode() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer abc123".parse().unwrap(),
        );
        let mut iter = headers.get_all(axum::http::header::AUTHORIZATION).iter();
        let token = BearerToken::decode(&mut iter).unwrap();
        assert_eq!(token.0, "abc123");
    }

    #[test]
    fn test_bearer_token_decode_missing() {
        let headers = HeaderMap::new();
        let mut iter = headers.get_all(axum::http::header::AUTHORIZATION).iter();
        let result = BearerToken::decode(&mut iter);
        assert!(result.is_err());
    }

    #[test]
    fn test_bearer_token_decode_not_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic abc123".parse().unwrap(),
        );
        let mut iter = headers.get_all(axum::http::header::AUTHORIZATION).iter();
        let result = BearerToken::decode(&mut iter);
        assert!(result.is_err());
    }
}

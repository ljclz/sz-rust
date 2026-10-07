// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use std::sync::Arc;
use sz_rust_core::orm::Pool;
use sz_rust_observability::MetricsRegistry;

/// 应用共享状态，在路由处理函数与中间件之间共享数据库连接池与指标注册中心
#[derive(Clone)]
pub struct AppState {
    /// MySQL 主数据库连接池
    pub db_pool: Arc<Pool>,
    /// PostgreSQL 连接池（可选，未配置时为 None）
    pub pg_pool: Option<Arc<Pool>>,
    /// Prometheus 指标注册中心
    pub metrics_registry: Arc<MetricsRegistry>,
    /// v1.8.0 RBAC 引擎（feature gate 控制）
    #[cfg(feature = "v18-rbac")]
    pub rbac_engine: Arc<sz_rust_auth_facade::RbacEngine>,
    /// v1.8.0 JWT 密钥轮换管理器（feature gate 控制）
    #[cfg(feature = "v18-key-rotation")]
    pub key_manager: Arc<sz_rust_key_rotation::KeyManager>,
    /// v1.8.0 链式哈希审计器（feature gate 控制）
    #[cfg(feature = "v18-audit-chain")]
    pub chain_auditor: Arc<sz_rust_middleware_facade::audit_chain::ChainHashAuditor>,
    /// v1.8.0 上传配置（feature gate 控制）
    #[cfg(feature = "v18-upload")]
    pub upload_config: crate::config::UploadConfig,
    /// v1.8.0 GraphQL Schema（feature gate 控制）
    #[cfg(feature = "v18-graphql")]
    pub graphql_schema: crate::graphql::GraphQLSchema,
    /// v1.9.0 DB-backed GraphQL Schema（feature gate 控制，持久化模式）
    #[cfg(feature = "v19-graphql-persist")]
    pub graphql_schema_db: crate::graphql::GraphQLSchemaDb,
    /// v1.8.0 WebSocket 连接管理器（feature gate 控制）
    #[cfg(feature = "v18-websocket")]
    pub ws_manager: Arc<sz_rust_websocket::manager::ConnectionManager>,
    /// v1.8.0 WebSocket 房间管理器（feature gate 控制）
    #[cfg(feature = "v18-websocket")]
    pub ws_rooms: Arc<sz_rust_websocket::room::RoomManager>,
    /// v1.8.0 SSE 服务（feature gate 控制）
    #[cfg(feature = "v18-sse")]
    pub sse_service: Arc<crate::services::sse_service::SseService>,
    /// v1.9.0 插件管理器（feature gate 控制，spec §5.10）
    #[cfg(feature = "v19-plugin-flow")]
    pub plugin_manager: Arc<crate::services::plugin_manager::PluginManager>,
}

impl AppState {
    /// v1.8.0 获取 RBAC 引擎引用（feature gate 关闭时返回 None）
    #[cfg(feature = "v18-rbac")]
    pub fn rbac_engine(&self) -> &sz_rust_auth_facade::RbacEngine {
        &self.rbac_engine
    }
}

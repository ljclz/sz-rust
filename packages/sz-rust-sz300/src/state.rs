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
    /// v1.9.0 查询缓存（order/list COUNT 结果缓存；订单写路径整体失效）
    pub query_cache: crate::query_cache::SharedQueryCache,
    /// PostgreSQL 连接池（可选，未配置时为 None）
    pub pg_pool: Option<Arc<Pool>>,
    /// Prometheus 指标注册中心
    pub metrics_registry: Arc<MetricsRegistry>,
    /// /metrics 端点访问令牌（环境变量 `SZ300_METRICS_TOKEN`；设置后需 Bearer 鉴权）
    pub metrics_token: Option<String>,
    /// 未配置令牌时是否允许匿名访问 /metrics（`SZ300_METRICS_PUBLIC`，默认 false = fail-closed）
    pub metrics_public: bool,
    /// v1.8.0 RBAC 引擎（feature gate 控制）
    #[cfg(feature = "v18-rbac")]
    pub rbac_engine: Arc<sz_rust_auth_facade::RbacEngine>,
    /// v1.8.0 JWT 密钥轮换管理器（feature gate 控制）
    #[cfg(feature = "v18-key-rotation")]
    pub key_manager: Arc<sz_rust_key_rotation::KeyManager>,
    /// v1.8.0 链式哈希审计器（feature gate 控制）
    #[cfg(feature = "v18-audit-chain")]
    pub chain_auditor: Arc<sz_rust_middleware_facade::audit_chain::ChainHashAuditor>,
    /// v1.8.0 审计落库异步缓冲（feature gate 控制；生产由 `main.rs` 接线后台落库任务）
    #[cfg(feature = "v18-audit-chain")]
    pub audit_sink: Arc<crate::middleware::audit_chain::AuditSink>,
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

#[cfg(test)]
impl AppState {
    /// 测试专用：构造最小 AppState（`connect_lazy` 不实际连接数据库，仅建池结构）
    ///
    /// 供 lib 内单元测试（如 controllers/ai.rs、controllers/capabilities.rs）使用；
    /// 集成测试请使用 `tests/common/mod.rs` 的 `make_state`。
    pub(crate) fn mock_app_state() -> AppState {
        use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
        use sz_rust_core::orm::PoolConfigBuilder;

        let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
            .max_connections(1)
            .connect_lazy("mysql://fake:fake@127.0.0.1:3306/fake")
            .expect("connect_lazy 不应失败");
        let factory =
            SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));
        let pool_cfg = PoolConfigBuilder::new()
            .max_size(1)
            .min_idle(0)
            .build()
            .expect("PoolConfig 构建失败");
        let pool = Pool::new(pool_cfg, Arc::new(factory)).expect("Pool 创建失败");

        #[cfg(feature = "v19-graphql-persist")]
        let graphql_schema_db = crate::graphql::build_schema_with_db(Arc::new(pool.clone()));

        AppState {
            db_pool: Arc::new(pool),
            query_cache: Arc::new(crate::query_cache::QueryCache::with_defaults()),
            pg_pool: None,
            metrics_registry: Arc::new(MetricsRegistry::default()),
            #[cfg(feature = "v18-rbac")]
            rbac_engine: Arc::new(crate::rbac::roles::init_rbac_engine()),
            #[cfg(feature = "v18-key-rotation")]
            key_manager: crate::services::auth_service::default_key_manager_for_tests(),
            #[cfg(feature = "v18-audit-chain")]
            chain_auditor: Arc::new(
                sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new(),
            ),
            #[cfg(feature = "v18-audit-chain")]
            audit_sink: {
                // 单元测试不接线后台落库：直接丢弃接收端（通道关闭），
                // 审计中间件在这些测试中不投递，因此不会有丢弃噪音。
                let (sink, _rx) = crate::middleware::audit_chain::AuditSink::new(None);
                sink
            },
            #[cfg(feature = "v18-upload")]
            upload_config: crate::config::upload_config(),
            #[cfg(feature = "v18-graphql")]
            graphql_schema: crate::graphql::build_schema(),
            #[cfg(feature = "v19-graphql-persist")]
            graphql_schema_db,
            #[cfg(feature = "v18-websocket")]
            ws_manager: Arc::new(sz_rust_websocket::manager::ConnectionManager::with_defaults()),
            #[cfg(feature = "v18-websocket")]
            ws_rooms: Arc::new(sz_rust_websocket::room::RoomManager::new()),
            #[cfg(feature = "v18-sse")]
            sse_service: crate::services::sse_service::SseService::with_defaults(),
            #[cfg(feature = "v19-plugin-flow")]
            plugin_manager: Arc::new(crate::services::plugin_manager::PluginManager::new(
                Arc::new(crate::services::plugin_manager::InMemoryMarketplace::new()),
                "test_pub_key".to_string(),
            )),
            metrics_token: None,
            // 单元测试不关心 /metrics 鉴权，保持公开以免影响既有用例
            metrics_public: true,
        }
    }
}

/// 测试专用自由函数：构造最小 AppState（转发 [`AppState::mock_app_state`]）
#[cfg(test)]
pub(crate) fn mock_app_state() -> AppState {
    AppState::mock_app_state()
}

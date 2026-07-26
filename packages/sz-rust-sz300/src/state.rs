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
}

use std::sync::Arc;
use sz_orm_core::Pool;

/// 应用共享状态，在路由处理函数与中间件之间共享数据库连接池
#[derive(Clone)]
pub struct AppState {
    /// MySQL 主数据库连接池
    pub db_pool: Arc<Pool>,
    /// PostgreSQL 连接池（可选，未配置时为 None）
    pub pg_pool: Option<Arc<Pool>>,
}

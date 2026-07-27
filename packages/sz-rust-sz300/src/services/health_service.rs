//! 健康检查服务模块（封装 DB 探活逻辑，2026-07-26 新增 — 修复控制器分层违反）
//!
//! 控制器 `health::readiness` 不应直接调用 `state.db_pool.acquire()` 与
//! `conn.query("SELECT 1")`，所有 DB 操作应下沉至 service 层。
//! 本模块提供 `ping_db` 异步函数，控制器通过 `AppState` 注入连接池后委托调用。

use std::sync::Arc;
use sz_rust_core::orm::Pool;

/// 探活数据库：执行 `SELECT 1` 验证连接可用性
///
/// ## 参数
///
/// - `db_pool`：数据库连接池
///
/// ## 返回
///
/// - `true`：DB 探活成功
/// - `false`：DB 探活失败（连接池 acquire 失败或 SELECT 1 失败）
///
/// ## 错误处理
///
/// DB 错误细节通过 `tracing::error!` 记录到日志，不向调用方暴露（避免信息泄露）。
pub async fn ping_db(db_pool: &Arc<Pool>) -> bool {
    match db_pool.acquire().await {
        Ok(mut conn) => match conn.query("SELECT 1").await {
            Ok(_) => true,
            Err(e) => {
                tracing::error!(error = %e, "health_service::ping_db：SELECT 1 失败");
                false
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "health_service::ping_db：连接池 acquire 失败");
            false
        }
    }
}

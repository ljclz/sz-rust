// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 健康检查服务模块（封装 DB 探活逻辑，2026-07-26 新增 — 修复控制器分层违反）
//!
//! 控制器 `health::readiness` 不应直接调用 `state.db_pool.acquire()` 与
//! `conn.query("SELECT 1")`，所有 DB 操作应下沉至 service 层。
//! 本模块提供 `ping_db` 异步函数，控制器通过 `AppState` 注入连接池后委托调用。

use std::sync::Arc;
use std::time::Duration;
use sz_rust_core::orm::Pool;

/// 探活总预算：超过即判定不可用
///
/// readiness 必须**有界返回**。2026-10-29 演练实测：DB 中断约 28s 后连接池进入
/// 不可恢复状态，`acquire()` 既不成功也不返回错误，`/health/ready` 挂起 >25s，
/// kubelet 只能等探针 timeoutSeconds 兜底，无法触发重启。
const PING_BUDGET: Duration = Duration::from_secs(3);

/// 探活数据库：执行 `SELECT 1` 验证连接可用性
///
/// ## 参数
///
/// - `db_pool`：数据库连接池
///
/// ## 返回
///
/// - `true`：DB 探活成功
/// - `false`：DB 探活失败（连接池 acquire 失败、SELECT 1 失败或超出 [`PING_BUDGET`]）
///
/// ## 错误处理
///
/// DB 错误细节通过 `tracing::error!` 记录到日志，不向调用方暴露（避免信息泄露）。
///
/// ## 超时处理
///
/// 探活放在独立 task 中执行、由 `timeout` 限时等待，而不是直接 `timeout(..., ping_inner())`：
/// 池内部 `acquire()` 被取消时的许可回收行为不归本仓库控制，取消可能把「池卡死」
/// 变成「池卡死 + 许可泄漏」。独立 task 让探活在后台自然结束，readiness 只负责限时上报。
pub async fn ping_db(db_pool: &Arc<Pool>) -> bool {
    let pool = db_pool.clone();
    let probe = tokio::spawn(async move {
        match pool.acquire().await {
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
    });

    match tokio::time::timeout(PING_BUDGET, probe).await {
        Ok(Ok(ok)) => ok,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "health_service::ping_db：探活任务异常退出");
            false
        }
        Err(_) => {
            tracing::error!(
                budget_ms = PING_BUDGET.as_millis(),
                "health_service::ping_db：探活超出预算，判定 DB 不可用"
            );
            false
        }
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 审计链式哈希中间件
//!
//! 拦截 POST/PUT/DELETE 请求（排除 /health /metrics），记录审计日志并维护链式哈希。
//!
//! ## 落库（2026-10-28 数据层修复）
//!
//! 此前审计记录只存在于内存（`ChainHashAuditor` 只保留 last_hash，记录本身直接丢弃），
//! 进程重启即丢失，不满足审计留痕要求。现在每条记录同时写入 `operate_log`：
//! `detail` 保存结构化 JSON（含 chain_hash），`operator` 取自 JWT 鉴权注入的
//! `CurrentUser`，`ip` 取自 `X-Forwarded-For` / `X-Real-IP`。
//!
//! 落库失败只记日志、不影响业务响应；单条写入最长等待 1s，避免慢库拖垮请求延迟。
//! `detail` 落库前经 `redact_sensitive` 脱敏。
//!
//! ## 使用方式
//!
//! ```ignore
//! use axum::middleware;
//! use crate::middleware::audit_chain::audit_chain_middleware;
//!
//! let app = Router::new()
//!     .layer(middleware::from_fn_with_state(auditor, audit_chain_middleware));
//! ```

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Extension, State},
    http::Request,
    middleware::Next,
    response::Response,
};
use sz_rust_core::orm::{Pool, Value};
use sz_rust_middleware_facade::audit_chain::{AuditRecord, ChainHashAuditor};

use crate::middleware::auth_middleware::CurrentUser;
use crate::services::redact_sensitive;

/// 排除审计的路径前缀
const EXCLUDED_PATHS: &[&str] = &["/health", "/metrics", "/api-docs"];

/// 审计落库最长等待时间（超时只丢审计记录，不阻塞业务响应）
const PERSIST_TIMEOUT: Duration = Duration::from_secs(1);

/// 判断路径是否应排除审计
fn should_audit(path: &str, method: &str) -> bool {
    // 仅审计写操作
    if method != "POST" && method != "PUT" && method != "DELETE" {
        return false;
    }
    // 排除健康检查、指标、文档路径
    !EXCLUDED_PATHS.iter().any(|prefix| path.starts_with(prefix))
}

/// 审计链式哈希中间件
///
/// 对 POST/PUT/DELETE 请求（排除 /health /metrics /api-docs）记录审计日志：
/// 先追加到内存链（保证哈希连续性），再写入 `operate_log`。
///
/// 连接池通过 `Extension<Arc<Pool>>` 注入（可选：未注入时仅维护内存链）。
pub async fn audit_chain_middleware(
    State(auditor): State<Arc<ChainHashAuditor>>,
    pool: Option<Extension<Arc<Pool>>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let should_record = should_audit(&path, &method);

    // 在 next.run 之前取出：鉴权中间件（外层）已把 CurrentUser 注入请求扩展
    let current_user = req.extensions().get::<CurrentUser>().cloned();
    let client_ip = client_ip(&req);

    let resp = next.run(req).await;

    if should_record {
        let status = resp.status();
        let result = if status.is_success() {
            "success"
        } else {
            "failure"
        };

        let mut record = AuditRecord::new(
            current_user
                .as_ref()
                .map(|u| u.username.clone())
                .unwrap_or_else(|| "system".to_string()),
            &method,
            &path,
            result,
        )
        .with_field("status_code", status.as_u16().to_string());

        auditor.append(&mut record);
        tracing::debug!(
            chain_hash = %record.chain_hash,
            method = %method,
            path = %path,
            "审计记录已追加"
        );

        if let Some(Extension(pool)) = pool {
            let merchant_id = current_user.as_ref().and_then(|u| u.merchant_id);
            persist_record(&pool, &record, merchant_id, &client_ip, status.as_u16()).await;
        }
    }

    resp
}

/// 将审计记录写入 `operate_log`
///
/// 失败只记 error 日志：审计是旁路，不能因为库抖动让写请求失败。
async fn persist_record(
    pool: &Pool,
    record: &AuditRecord,
    merchant_id: Option<i64>,
    ip: &str,
    status_code: u16,
) {
    let detail = serde_json::json!({
        "method": record.action,
        "path": record.target,
        "result": record.result,
        "status_code": status_code,
        "chain_hash": record.chain_hash,
        "timestamp": record.timestamp.to_rfc3339(),
    })
    .to_string();

    let sql =
        "INSERT INTO operate_log (merchant_id, operator, action, detail, ip) VALUES (?, ?, ?, ?, ?)";
    let params = [
        Value::I64(merchant_id.unwrap_or(0)),
        Value::String(truncate_chars(&record.actor, 50)),
        Value::String(truncate_chars(&record.action, 50)),
        Value::String(redact_sensitive(&detail)),
        Value::String(truncate_chars(ip, 45)),
    ];

    let write = async {
        // sz-orm 10.13 的 Pool::acquire 返回独立 PoolError，未提供 From<PoolError> for DbError，
        // 需显式下沉到 DbError::PoolError 才能与 execute_with_params 的错误类型统一。
        let mut conn = pool
            .acquire()
            .await
            .map_err(sz_rust_core::orm::DbError::PoolError)?;
        conn.execute_with_params(sql, &params).await
    };

    match tokio::time::timeout(PERSIST_TIMEOUT, write).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::error!(
            error = %e,
            path = %record.target,
            "审计记录落库失败（业务响应不受影响）"
        ),
        Err(_) => tracing::error!(
            path = %record.target,
            "审计记录落库超时（{}s），已丢弃", PERSIST_TIMEOUT.as_secs()
        ),
    }
}

/// 客户端 IP（优先 `X-Forwarded-For` 首段，其次 `X-Real-IP`）
///
/// 代理头可被客户端伪造，仅作审计线索，不用于任何授权判断。
fn client_ip(req: &Request<Body>) -> String {
    let forwarded = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(ip) = forwarded {
        return ip.to_string();
    }

    req.headers()
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 按字符（非字节）截断，避免切断多字节 UTF-8 导致写库失败
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

/// 提取审计记录的辅助函数（用于测试）
pub fn make_audit_record(method: &str, path: &str, result: &str) -> AuditRecord {
    AuditRecord::new("system", method, path, result)
}

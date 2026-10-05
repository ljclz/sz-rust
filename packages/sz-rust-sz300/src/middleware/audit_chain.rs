// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 审计链式哈希中间件
//!
//! 拦截 POST/PUT/DELETE 请求（排除 /health /metrics），记录审计日志并维护链式哈希。
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

use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};
use sz_rust_middleware_facade::audit_chain::{AuditRecord, ChainHashAuditor};

/// 排除审计的路径前缀
const EXCLUDED_PATHS: &[&str] = &["/health", "/metrics", "/api-docs"];

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
/// 对 POST/PUT/DELETE 请求（排除 /health /metrics /api-docs）记录审计日志。
pub async fn audit_chain_middleware(
    State(auditor): State<Arc<ChainHashAuditor>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let should_record = should_audit(&path, &method);

    let resp = next.run(req).await;

    if should_record {
        let status = resp.status();
        let result = if status.is_success() {
            "success"
        } else {
            "failure"
        };

        let mut record = AuditRecord::new("system", &method, &path, result)
            .with_field("status_code", status.as_u16().to_string());

        auditor.append(&mut record);
        tracing::debug!(
            chain_hash = %record.chain_hash,
            method = %method,
            path = %path,
            "审计记录已追加"
        );
    }

    resp
}

/// 提取审计记录的辅助函数（用于测试）
pub fn make_audit_record(method: &str, path: &str, result: &str) -> AuditRecord {
    AuditRecord::new("system", method, path, result)
}

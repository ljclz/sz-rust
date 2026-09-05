// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 安全审计日志中间件 — 记录所有请求的安全审计事件
//!
//! 对齐 spec §5.3.1（7 条业务规则）+ §6.3（AuditLogConfig）。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// 审计日志输出目标（spec §6.3 第 7 条）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum AuditOutputTarget {
    /// 输出到 tracing 管道（复用 sz-rust-tracing）
    #[default]
    Tracing,
    /// 输出到独立文件
    File,
    /// 输出到 stdout
    Stdout,
}

impl fmt::Display for AuditOutputTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tracing => write!(f, "tracing"),
            Self::File => write!(f, "file"),
            Self::Stdout => write!(f, "stdout"),
        }
    }
}

/// 审计事件类型（spec §5.3.1 规则 6）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AuditEventType {
    /// 普通 HTTP 请求
    HttpRequest,
    /// 认证成功
    AuthSuccess,
    /// 认证失败
    AuthFailure,
    /// 访问被拒绝
    AccessDenied,
    /// IP 被拒绝
    IpRejected,
    /// 请求体过大
    BodyTooLarge,
}

impl AuditEventType {
    /// 是否为安全事件（安全事件 100% 记录，不采样）
    pub fn is_security_event(self) -> bool {
        matches!(
            self,
            Self::AuthFailure | Self::AccessDenied | Self::IpRejected | Self::BodyTooLarge
        )
    }
}

/// 审计事件（spec §5.3.1 规则 1）
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    /// 事件时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 事件类型
    pub event_type: AuditEventType,
    /// 用户 ID（若已认证）
    pub user_id: Option<i64>,
    /// 客户端 IP
    pub client_ip: String,
    /// HTTP 方法
    pub method: String,
    /// 请求路径
    pub path: String,
    /// 响应状态码
    pub status: u16,
    /// 请求耗时（毫秒）
    pub duration_ms: u64,
    /// 请求 headers（脱敏后，仅 `log_headers == true` 时有值）
    pub headers: Option<serde_json::Value>,
    /// 请求 body（脱敏后，仅 `log_body == true` 时有值）
    pub body: Option<serde_json::Value>,
}

/// 安全审计日志配置（spec §6.3）
#[derive(Debug, Clone, Deserialize)]
pub struct AuditLogConfig {
    /// 是否启用审计日志（默认 false，向后兼容）
    #[serde(default)]
    pub enabled: bool,
    /// 采样率 [0.0, 1.0]，默认 1.0（全量记录）
    #[serde(default = "default_sample_rate")]
    pub sample_rate: f64,
    /// 排除路径列表（精确匹配）
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    /// 敏感字段名列表（匹配的字段值脱敏为 `[REDACTED]`）
    #[serde(default = "default_sensitive_fields")]
    pub sensitive_fields: Vec<String>,
    /// 是否记录请求 headers（敏感 header 脱敏）
    #[serde(default)]
    pub log_headers: bool,
    /// 是否记录请求 body（敏感字段脱敏，上限 4096 字节）
    #[serde(default)]
    pub log_body: bool,
    /// 输出目标
    #[serde(default)]
    pub output_target: AuditOutputTarget,
}

fn default_sample_rate() -> f64 {
    1.0
}

fn default_sensitive_fields() -> Vec<String> {
    vec![
        "password".to_string(),
        "token".to_string(),
        "authorization".to_string(),
        "credit_card".to_string(),
    ]
}

impl Default for AuditLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sample_rate: 1.0,
            exclude_paths: Vec::new(),
            sensitive_fields: default_sensitive_fields(),
            log_headers: false,
            log_body: false,
            output_target: AuditOutputTarget::Tracing,
        }
    }
}

/// 递归脱敏 JSON 中的敏感字段（spec §5.3.1 规则 4）
pub fn redact_sensitive_fields(value: &mut serde_json::Value, sensitive_fields: &[String]) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if sensitive_fields.contains(key) {
                    *val = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    redact_sensitive_fields(val, sensitive_fields);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_sensitive_fields(item, sensitive_fields);
            }
        }
        _ => {}
    }
}

/// 采样判定（spec §5.3.1 规则 5/6）
///
/// 安全事件 100% 记录；普通事件按 `sample_rate` 概率记录。
pub fn should_sample(sample_rate: f64, is_security_event: bool) -> bool {
    if is_security_event {
        return true;
    }
    if sample_rate <= 0.0 {
        return false;
    }
    if sample_rate >= 1.0 {
        return true;
    }
    use rand::Rng;
    rand::rngs::OsRng.gen::<f64>() < sample_rate
}

/// 异步写入审计日志（spec §6.3 第 7 条 + §4.3.6 防篡改追加）
async fn write_audit_log(
    event: &AuditEvent,
    target: AuditOutputTarget,
) -> Result<(), std::io::Error> {
    let json = serde_json::to_string(event).unwrap_or_default();
    match target {
        AuditOutputTarget::Tracing => {
            tracing::info!("audit: {json}");
        }
        AuditOutputTarget::Stdout => {
            println!("{json}");
        }
        AuditOutputTarget::File => {
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open("audit.log")
                .await?;
            file.write_all(json.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
    }
    Ok(())
}

/// 从响应状态码推断审计事件类型
fn infer_event_type(status: u16) -> AuditEventType {
    match status {
        401 => AuditEventType::AuthFailure,
        403 => AuditEventType::AccessDenied,
        413 => AuditEventType::BodyTooLarge,
        _ => AuditEventType::HttpRequest,
    }
}

/// 安全审计日志中间件
///
/// 若 `config.enabled == false` 直接放行（spec §4.5.1）。
/// 排除路径精确匹配则放行（spec §5.3.1 规则 7）。
/// 采样判定后异步写入审计日志（spec §4.2.3 不阻塞业务）。
pub async fn audit_log_middleware(
    axum::extract::State(config): axum::extract::State<AuditLogConfig>,
    req: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(req).await;
    }

    let path = req.uri().path().to_string();
    if config.exclude_paths.contains(&path) {
        return next.run(req).await;
    }

    let method = req.method().to_string();
    let client_ip = extract_client_ip_simple(req.headers());

    let start = Instant::now();
    let response = next.run(req).await;
    let duration_ms = start.elapsed().as_millis() as u64;
    let status = response.status().as_u16();

    let event_type = infer_event_type(status);
    if !should_sample(config.sample_rate, event_type.is_security_event()) {
        return response;
    }

    let event = AuditEvent {
        timestamp: chrono::Utc::now(),
        event_type,
        user_id: None,
        client_ip,
        method,
        path,
        status,
        duration_ms,
        headers: None,
        body: None,
    };

    let target = config.output_target;
    tokio::spawn(async move {
        if let Err(e) = write_audit_log(&event, target).await {
            tracing::error!("审计日志写入失败: {e}");
        }
    });

    response
}

fn extract_client_ip_simple(headers: &axum::http::HeaderMap) -> String {
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            if let Some(first) = value.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_disabled() {
        let cfg = AuditLogConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.sample_rate, 1.0);
        assert!(cfg.sensitive_fields.contains(&"password".to_string()));
        assert_eq!(cfg.output_target, AuditOutputTarget::Tracing);
    }

    #[test]
    fn test_audit_output_target_display() {
        assert_eq!(AuditOutputTarget::Tracing.to_string(), "tracing");
        assert_eq!(AuditOutputTarget::File.to_string(), "file");
        assert_eq!(AuditOutputTarget::Stdout.to_string(), "stdout");
    }

    #[test]
    fn test_event_type_is_security_event() {
        assert!(!AuditEventType::HttpRequest.is_security_event());
        assert!(!AuditEventType::AuthSuccess.is_security_event());
        assert!(AuditEventType::AuthFailure.is_security_event());
        assert!(AuditEventType::AccessDenied.is_security_event());
        assert!(AuditEventType::IpRejected.is_security_event());
        assert!(AuditEventType::BodyTooLarge.is_security_event());
    }

    #[test]
    fn test_redact_simple() {
        let mut value = serde_json::json!({"password": "secret123", "name": "alice"});
        redact_sensitive_fields(&mut value, &["password".to_string()]);
        assert_eq!(value["password"], "[REDACTED]");
        assert_eq!(value["name"], "alice");
    }

    #[test]
    fn test_redact_nested() {
        let mut value = serde_json::json!({"user": {"token": "xxx", "name": "bob"}});
        redact_sensitive_fields(&mut value, &["token".to_string()]);
        assert_eq!(value["user"]["token"], "[REDACTED]");
        assert_eq!(value["user"]["name"], "bob");
    }

    #[test]
    fn test_redact_array() {
        let mut value = serde_json::json!({"items": [{"password": "a"}, {"password": "b"}]});
        redact_sensitive_fields(&mut value, &["password".to_string()]);
        assert_eq!(value["items"][0]["password"], "[REDACTED]");
        assert_eq!(value["items"][1]["password"], "[REDACTED]");
    }

    #[test]
    fn test_should_sample_security_event_always() {
        assert!(should_sample(0.0, true));
        assert!(should_sample(1.0, true));
    }

    #[test]
    fn test_should_sample_zero_rate() {
        assert!(!should_sample(0.0, false));
    }

    #[test]
    fn test_should_sample_full_rate() {
        assert!(should_sample(1.0, false));
    }

    #[tokio::test]
    async fn test_middleware_disabled_passes_through() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = AuditLogConfig::default();

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                audit_log_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}

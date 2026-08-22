//! 安全响应头中间件 — 为所有 HTTP 响应注入 6 类安全响应头
//!
//! 对齐 spec §5.1.1（9 条业务规则）+ §6.1（SecurityHeadersConfig）。
//!
//! ## 头部清单
//!
//! | 头部 | 规则 | 默认值 |
//! |------|------|--------|
//! | `X-Frame-Options` | §5.1.1 规则 1 | `DENY` |
//! | `X-Content-Type-Options` | §5.1.1 规则 2 | `nosniff` |
//! | `Strict-Transport-Security` | §5.1.1 规则 3 | `max-age=31536000; includeSubDomains` |
//! | `Content-Security-Policy` | §5.1.1 规则 4 | 未配置（不注入） |
//! | `Referrer-Policy` | §5.1.1 规则 5 | `no-referrer` |
//! | `Permissions-Policy` | §5.1.1 规则 6 | 未配置（不注入） |

use serde::Deserialize;
use std::fmt;

/// X-Frame-Options 取值（spec §6.1 第 1 条）
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub enum FrameOptions {
    /// `DENY` — 禁止任何 frame 嵌入
    #[default]
    Deny,
    /// `SAMEORIGIN` — 仅同源可嵌入
    SameOrigin,
    /// `ALLOW-FROM <uri>` — 指定来源可嵌入
    AllowFrom(String),
}

impl fmt::Display for FrameOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deny => write!(f, "DENY"),
            Self::SameOrigin => write!(f, "SAMEORIGIN"),
            Self::AllowFrom(uri) => write!(f, "ALLOW-FROM {uri}"),
        }
    }
}

/// HSTS 配置（spec §6.1 第 3 条）
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HstsConfig {
    /// HSTS 有效期（秒），默认 31536000（1 年）；`0` 表示显式关闭
    #[serde(default = "default_hsts_max_age")]
    pub max_age: u64,
    /// 是否包含子域名
    #[serde(default = "default_true")]
    pub include_subdomains: bool,
    /// 是否启用 HSTS preload
    #[serde(default)]
    pub preload: bool,
}

fn default_hsts_max_age() -> u64 {
    31536000
}

fn default_true() -> bool {
    true
}

impl Default for HstsConfig {
    fn default() -> Self {
        Self {
            max_age: 31536000,
            include_subdomains: true,
            preload: false,
        }
    }
}

/// Referrer-Policy 取值（spec §6.1 第 5 条）
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub enum ReferrerPolicy {
    /// `no-referrer`
    #[default]
    NoReferrer,
    /// `no-referrer-when-downgrade`
    NoReferrerWhenDowngrade,
    /// `same-origin`
    SameOrigin,
    /// `origin`
    Origin,
    /// `strict-origin`
    StrictOrigin,
    /// `origin-when-cross-origin`
    OriginWhenCrossOrigin,
    /// `strict-origin-when-cross-origin`
    StrictOriginWhenCrossOrigin,
    /// `unsafe-url`
    UnsafeUrl,
}

impl fmt::Display for ReferrerPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoReferrer => write!(f, "no-referrer"),
            Self::NoReferrerWhenDowngrade => write!(f, "no-referrer-when-downgrade"),
            Self::SameOrigin => write!(f, "same-origin"),
            Self::Origin => write!(f, "origin"),
            Self::StrictOrigin => write!(f, "strict-origin"),
            Self::OriginWhenCrossOrigin => write!(f, "origin-when-cross-origin"),
            Self::StrictOriginWhenCrossOrigin => write!(f, "strict-origin-when-cross-origin"),
            Self::UnsafeUrl => write!(f, "unsafe-url"),
        }
    }
}

/// 安全响应头配置（spec §6.1）
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityHeadersConfig {
    /// 是否启用安全头注入（默认 true，spec §4.3.1 默认安全）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// X-Frame-Options 取值
    #[serde(default)]
    pub frame_options: FrameOptions,
    /// X-Content-Type-Options 是否启用（固定 nosniff，不可关闭）
    #[serde(default = "default_true")]
    pub content_type_options: bool,
    /// HSTS 配置
    #[serde(default)]
    pub hsts: HstsConfig,
    /// CSP 策略（可选，支持 `{nonce}` 占位符）
    #[serde(default)]
    pub csp: Option<String>,
    /// Referrer-Policy 取值
    #[serde(default)]
    pub referrer_policy: ReferrerPolicy,
    /// Permissions-Policy 策略（可选）
    #[serde(default)]
    pub permissions_policy: Option<String>,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            frame_options: FrameOptions::Deny,
            content_type_options: true,
            hsts: HstsConfig::default(),
            csp: None,
            referrer_policy: ReferrerPolicy::NoReferrer,
            permissions_policy: None,
        }
    }
}

/// 安全响应头错误
#[derive(Debug, thiserror::Error)]
pub enum SecurityHeadersError {
    /// CSP nonce 生成失败
    #[error("CSP nonce 生成失败: {0}")]
    NonceGenerationFailed(#[from] rand::Error),
}

use axum::extract::Request;
use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

/// 生成 CSP nonce（≥128 位熵，Base64 编码为 22 字符）
///
/// 使用 `OsRng`（操作系统级密码学安全 RNG），对齐 spec §4.3.3。
pub fn generate_csp_nonce() -> Result<String, SecurityHeadersError> {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    use base64::Engine;
    let nonce = base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes);
    Ok(nonce)
}

/// 向响应注入 6 类安全响应头
///
/// 下游已设置的头部不覆盖（spec §5.1.1 规则 7）。HSTS 仅 HTTPS 注入（spec §4.3.2）。
/// CSP nonce 生成失败时 fail-open 跳过 + `tracing::error`（spec §5.1.3 异常场景 2）。
pub fn inject_security_headers(
    response: &mut Response,
    config: &SecurityHeadersConfig,
    is_https: bool,
) -> Result<(), SecurityHeadersError> {
    let headers = response.headers_mut();

    // 1. X-Frame-Options（规则 1 + 规则 7 下游优先）
    if !headers.contains_key("x-frame-options") {
        if let Ok(val) = HeaderValue::from_str(&config.frame_options.to_string()) {
            headers.insert(HeaderName::from_static("x-frame-options"), val);
        }
    }

    // 2. X-Content-Type-Options: nosniff（规则 2）
    if config.content_type_options && !headers.contains_key("x-content-type-options") {
        headers.insert(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        );
    }

    // 3. Strict-Transport-Security（规则 3 + §4.3.2 仅 HTTPS）
    if is_https && config.hsts.max_age > 0 && !headers.contains_key("strict-transport-security") {
        let mut hsts_val = format!("max-age={}", config.hsts.max_age);
        if config.hsts.include_subdomains {
            hsts_val.push_str("; includeSubDomains");
        }
        if config.hsts.preload {
            hsts_val.push_str("; preload");
        }
        if let Ok(val) = HeaderValue::from_str(&hsts_val) {
            headers.insert(HeaderName::from_static("strict-transport-security"), val);
        }
    }

    // 4. Content-Security-Policy（规则 4 + nonce 占位符替换）
    if let Some(csp_template) = &config.csp {
        if !headers.contains_key("content-security-policy") {
            let csp_value = if csp_template.contains("{nonce}") {
                match generate_csp_nonce() {
                    Ok(nonce) => csp_template.replace("{nonce}", &nonce),
                    Err(e) => {
                        tracing::error!("CSP nonce 生成失败，跳过 CSP 注入: {e}");
                        return Ok(());
                    }
                }
            } else {
                csp_template.clone()
            };
            if let Ok(val) = HeaderValue::from_str(&csp_value) {
                headers.insert(HeaderName::from_static("content-security-policy"), val);
            }
        }
    }

    // 5. Referrer-Policy（规则 5）
    if !headers.contains_key("referrer-policy") {
        if let Ok(val) = HeaderValue::from_str(&config.referrer_policy.to_string()) {
            headers.insert(HeaderName::from_static("referrer-policy"), val);
        }
    }

    // 6. Permissions-Policy（规则 6）
    if let Some(pp) = &config.permissions_policy {
        if !headers.contains_key("permissions-policy") {
            if let Ok(val) = HeaderValue::from_str(pp) {
                headers.insert(HeaderName::from_static("permissions-policy"), val);
            }
        }
    }

    Ok(())
}

/// 安全响应头中间件
///
/// 若 `config.enabled == false` 直接放行（spec §4.5.1 向后兼容）。
/// 否则调用下游获取响应，再注入安全头（fail-open，spec §4.2.1）。
pub async fn security_headers_middleware(
    axum::extract::State(config): axum::extract::State<SecurityHeadersConfig>,
    req: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(req).await;
    }

    let is_https = req
        .uri()
        .scheme()
        .map(|s| s == &axum::http::uri::Scheme::HTTPS)
        .unwrap_or(false);

    let mut response = next.run(req).await;

    if let Err(e) = inject_security_headers(&mut response, &config, is_https) {
        tracing::error!("安全响应头注入失败（fail-open）: {e}");
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_secure() {
        let cfg = SecurityHeadersConfig::default();
        assert!(cfg.enabled, "默认应启用（spec §4.3.1 默认安全）");
        assert_eq!(cfg.frame_options, FrameOptions::Deny);
        assert!(cfg.content_type_options);
        assert_eq!(cfg.hsts.max_age, 31536000);
        assert!(cfg.hsts.include_subdomains);
        assert!(!cfg.hsts.preload);
        assert!(cfg.csp.is_none());
        assert_eq!(cfg.referrer_policy, ReferrerPolicy::NoReferrer);
        assert!(cfg.permissions_policy.is_none());
    }

    #[test]
    fn test_frame_options_display() {
        assert_eq!(FrameOptions::Deny.to_string(), "DENY");
        assert_eq!(FrameOptions::SameOrigin.to_string(), "SAMEORIGIN");
        assert_eq!(
            FrameOptions::AllowFrom("https://example.com".to_string()).to_string(),
            "ALLOW-FROM https://example.com"
        );
    }

    #[test]
    fn test_referrer_policy_display() {
        assert_eq!(ReferrerPolicy::NoReferrer.to_string(), "no-referrer");
        assert_eq!(
            ReferrerPolicy::NoReferrerWhenDowngrade.to_string(),
            "no-referrer-when-downgrade"
        );
        assert_eq!(ReferrerPolicy::SameOrigin.to_string(), "same-origin");
        assert_eq!(ReferrerPolicy::Origin.to_string(), "origin");
        assert_eq!(ReferrerPolicy::StrictOrigin.to_string(), "strict-origin");
        assert_eq!(
            ReferrerPolicy::OriginWhenCrossOrigin.to_string(),
            "origin-when-cross-origin"
        );
        assert_eq!(
            ReferrerPolicy::StrictOriginWhenCrossOrigin.to_string(),
            "strict-origin-when-cross-origin"
        );
        assert_eq!(ReferrerPolicy::UnsafeUrl.to_string(), "unsafe-url");
    }

    #[test]
    fn test_hsts_default() {
        let hsts = HstsConfig::default();
        assert_eq!(hsts.max_age, 31536000);
        assert!(hsts.include_subdomains);
        assert!(!hsts.preload);
    }

    #[test]
    fn test_generate_csp_nonce_unique() {
        let mut nonces = std::collections::HashSet::new();
        for _ in 0..100 {
            let nonce = generate_csp_nonce().unwrap();
            assert_eq!(nonce.len(), 22, "Base64 编码 16 字节应为 22 字符");
            nonces.insert(nonce);
        }
        assert_eq!(nonces.len(), 100, "100 个 nonce 应全部不同");
    }

    #[test]
    fn test_inject_default_headers_http() {
        let config = SecurityHeadersConfig::default();
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, false).unwrap();

        let headers = response.headers();
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
        assert!(
            headers.get("strict-transport-security").is_none(),
            "HTTP 不注入 HSTS"
        );
        assert_eq!(headers.get("referrer-policy").unwrap(), "no-referrer");
    }

    #[test]
    fn test_inject_hsts_https() {
        let config = SecurityHeadersConfig::default();
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, true).unwrap();

        let hsts = response.headers().get("strict-transport-security").unwrap();
        let hsts_str = hsts.to_str().unwrap();
        assert!(hsts_str.contains("max-age=31536000"));
        assert!(hsts_str.contains("includeSubDomains"));
    }

    #[test]
    fn test_inject_hsts_preload() {
        let mut config = SecurityHeadersConfig::default();
        config.hsts.preload = true;
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, true).unwrap();

        let hsts = response.headers().get("strict-transport-security").unwrap();
        assert!(hsts.to_str().unwrap().contains("preload"));
    }

    #[test]
    fn test_inject_csp_with_nonce() {
        let config = SecurityHeadersConfig {
            csp: Some("default-src 'self'; script-src 'self' 'nonce-{nonce}'".to_string()),
            ..SecurityHeadersConfig::default()
        };
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, false).unwrap();

        let csp = response.headers().get("content-security-policy").unwrap();
        let csp_str = csp.to_str().unwrap();
        assert!(csp_str.contains("'nonce-"));
        assert!(!csp_str.contains("{nonce}"), "占位符应被替换");
    }

    #[test]
    fn test_inject_csp_without_nonce() {
        let config = SecurityHeadersConfig {
            csp: Some("default-src 'self'".to_string()),
            ..SecurityHeadersConfig::default()
        };
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, false).unwrap();

        let csp = response.headers().get("content-security-policy").unwrap();
        assert_eq!(csp.to_str().unwrap(), "default-src 'self'");
    }

    #[test]
    fn test_inject_permissions_policy() {
        let config = SecurityHeadersConfig {
            permissions_policy: Some("geolocation=(), camera=()".to_string()),
            ..SecurityHeadersConfig::default()
        };
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, false).unwrap();

        let pp = response.headers().get("permissions-policy").unwrap();
        assert_eq!(pp.to_str().unwrap(), "geolocation=(), camera=()");
    }

    #[test]
    fn test_downstream_headers_not_overwritten() {
        let config = SecurityHeadersConfig::default();
        let mut response = Response::new(axum::body::Body::empty());
        response.headers_mut().insert(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("ALLOWALL"),
        );
        inject_security_headers(&mut response, &config, false).unwrap();

        assert_eq!(
            response.headers().get("x-frame-options").unwrap(),
            "ALLOWALL",
            "下游设置的头部不应被覆盖"
        );
    }

    #[test]
    fn test_hsts_max_age_zero_skipped() {
        let mut config = SecurityHeadersConfig::default();
        config.hsts.max_age = 0;
        let mut response = Response::new(axum::body::Body::empty());
        inject_security_headers(&mut response, &config, true).unwrap();

        assert!(response
            .headers()
            .get("strict-transport-security")
            .is_none());
    }

    #[tokio::test]
    async fn test_middleware_disabled_passes_through() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = SecurityHeadersConfig {
            enabled: false,
            ..SecurityHeadersConfig::default()
        };

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                security_headers_middleware,
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

        assert!(
            resp.headers().get("x-frame-options").is_none(),
            "disabled 不注入"
        );
    }

    #[tokio::test]
    async fn test_middleware_enabled_injects_headers() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = SecurityHeadersConfig::default();

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                security_headers_middleware,
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

        assert_eq!(resp.headers().get("x-frame-options").unwrap(), "DENY");
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            resp.headers().get("referrer-policy").unwrap(),
            "no-referrer"
        );
    }
}

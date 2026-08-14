//! IP 访问控制中间件 — 基于 IP 白名单/黑名单的访问控制
//!
//! 对齐 spec §5.2.1（11 条业务规则）+ §6.2（IpAccessControlConfig）。

use serde::Deserialize;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;

/// IP 访问控制模式（spec §6.2 第 1 条）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum IpAccessMode {
    /// 白名单模式：仅允许 `ip_list` 中的 IP
    #[default]
    Whitelist,
    /// 黑名单模式：拒绝 `ip_list` 中的 IP
    Blacklist,
}

impl fmt::Display for IpAccessMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Whitelist => write!(f, "whitelist"),
            Self::Blacklist => write!(f, "blacklist"),
        }
    }
}

/// 解析后的 IP 规则（CIDR 或单 IP）
#[derive(Debug, Clone)]
pub struct IpRule {
    /// CIDR 网段
    pub network: ipnet::IpNet,
}

impl IpRule {
    /// 判断 IP 是否在本规则网段内
    pub fn contains(&self, ip: IpAddr) -> bool {
        self.network.contains(&ip)
    }
}

/// IP 访问控制错误
#[derive(Debug, thiserror::Error)]
pub enum IpAccessControlError {
    /// CIDR 或 IP 解析失败
    #[error("IP/CIDR 解析失败: {entry} — {reason}")]
    InvalidIpOrCidr { entry: String, reason: String },
}

/// IP 访问控制配置（spec §6.2）
#[derive(Debug, Clone, Deserialize)]
pub struct IpAccessControlConfig {
    /// 是否启用 IP 校验（默认 false，向后兼容）
    #[serde(default)]
    pub enabled: bool,
    /// 访问控制模式
    #[serde(default)]
    pub mode: IpAccessMode,
    /// IP/CIDR 列表（支持 IPv4 与 IPv6，长度 ≤ 10000）
    #[serde(default)]
    pub ip_list: Vec<String>,
    /// 排除路径列表（精确匹配，不进行前缀匹配防绕过）
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    /// 可信代理 IP/CIDR 列表（仅这些来源的 X-Forwarded-For 才被采纳）
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// 是否 fail-open（默认 false = fail-close）
    #[serde(default)]
    pub fail_open: bool,
}

impl Default for IpAccessControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: IpAccessMode::Whitelist,
            ip_list: Vec::new(),
            exclude_paths: Vec::new(),
            trusted_proxies: Vec::new(),
            fail_open: false,
        }
    }
}

impl IpAccessControlConfig {
    /// 解析 `ip_list` 和 `trusted_proxies` 为 `IpRule` 列表
    ///
    /// 每条可为单 IP（`10.0.0.1`）或 CIDR（`10.0.0.0/8`），支持 IPv4 与 IPv6。
    pub fn parse_rules(&self) -> Result<(Vec<IpRule>, Vec<IpRule>), IpAccessControlError> {
        let rules = self.parse_ip_list(&self.ip_list)?;
        let trusted = self.parse_ip_list(&self.trusted_proxies)?;
        Ok((rules, trusted))
    }

    fn parse_ip_list(&self, list: &[String]) -> Result<Vec<IpRule>, IpAccessControlError> {
        let mut rules = Vec::with_capacity(list.len());
        for entry in list {
            let network = if let Ok(net) = ipnet::IpNet::from_str(entry) {
                net
            } else if let Ok(ip) = IpAddr::from_str(entry) {
                match ip {
                    IpAddr::V4(v4) => ipnet::IpNet::V4(ipnet::Ipv4Net::new(v4, 32).unwrap()),
                    IpAddr::V6(v6) => ipnet::IpNet::V6(ipnet::Ipv6Net::new(v6, 128).unwrap()),
                }
            } else {
                return Err(IpAccessControlError::InvalidIpOrCidr {
                    entry: entry.clone(),
                    reason: format!("'{entry}' 不是合法的 IP 或 CIDR"),
                });
            };
            rules.push(IpRule { network });
        }
        Ok(rules)
    }
}

/// 可信代理感知的客户端 IP 提取（spec §5.2.1 规则 9-10）
///
/// 优先级：若 peer_addr 在可信代理列表内 → `X-Real-IP` > `X-Forwarded-For` 首个 > peer_addr
/// 若不在可信代理列表 → 返回 peer_addr（忽略 XFF 防伪造）
pub fn extract_client_ip_trusted(
    headers: &HeaderMap,
    trusted_proxies: &[IpRule],
    peer_addr: Option<IpAddr>,
) -> Option<IpAddr> {
    let peer = peer_addr?;

    let is_trusted = trusted_proxies.iter().any(|r| r.contains(peer));
    if !is_trusted {
        return Some(peer);
    }

    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(s) = real_ip.to_str() {
            if let Ok(ip) = IpAddr::from_str(s.trim()) {
                return Some(ip);
            }
        }
    }

    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(s) = forwarded.to_str() {
            if let Some(first) = s.split(',').next() {
                if let Ok(ip) = IpAddr::from_str(first.trim()) {
                    return Some(ip);
                }
            }
        }
    }

    Some(peer)
}

/// 判断 IP 是否被允许访问（spec §5.2.1 规则 1-6/8）
///
/// - 白名单模式：在列表中 → true，不在 → false
/// - 黑名单模式：在列表中 → false，不在 → true
/// - 空列表：白名单和黑名单都不拦截（返回 true）
pub fn is_ip_allowed(ip: IpAddr, rules: &[IpRule], mode: IpAccessMode) -> bool {
    if rules.is_empty() {
        return true;
    }

    let in_list = rules.iter().any(|r| r.contains(ip));
    match mode {
        IpAccessMode::Whitelist => in_list,
        IpAccessMode::Blacklist => !in_list,
    }
}

/// 构造 403 IP 拒绝响应
fn ip_rejected_response() -> Response {
    let exception = sz_rust_http_facade::BaseException::forbidden("IP not allowed");
    let json = exception.to_json();
    let body = serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(axum::http::StatusCode::FORBIDDEN)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

/// IP 访问控制中间件
///
/// 若 `config.enabled == false` 直接放行（spec §4.5.1）。
/// 排除路径精确匹配则放行（spec §5.2.1 规则 7）。
/// IP 不在允许范围则返回 403（spec §5.2.1 规则 2）。
pub async fn ip_access_control_middleware(
    axum::extract::State(config): axum::extract::State<IpAccessControlConfig>,
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

    let (rules, trusted) = match config.parse_rules() {
        Ok(v) => v,
        Err(e) => {
            if config.fail_open {
                tracing::warn!("IP 规则解析失败（fail-open 放行）: {e}");
                return next.run(req).await;
            }
            tracing::error!("IP 规则解析失败（fail-close 拒绝）: {e}");
            return ip_rejected_response();
        }
    };

    let peer_addr = req
        .headers()
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| IpAddr::from_str(s.trim()).ok())
        .or_else(|| {
            req.headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| IpAddr::from_str(s.trim()).ok())
        })
        .unwrap_or(IpAddr::from([0u8, 0, 0, 0]));

    let client_ip = extract_client_ip_trusted(req.headers(), &trusted, Some(peer_addr))
        .unwrap_or(IpAddr::from([0u8, 0, 0, 0]));

    if !is_ip_allowed(client_ip, &rules, config.mode) {
        return ip_rejected_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_disabled() {
        let cfg = IpAccessControlConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.mode, IpAccessMode::Whitelist);
        assert!(cfg.ip_list.is_empty());
    }

    #[test]
    fn test_ip_access_mode_display() {
        assert_eq!(IpAccessMode::Whitelist.to_string(), "whitelist");
        assert_eq!(IpAccessMode::Blacklist.to_string(), "blacklist");
    }

    #[test]
    fn test_parse_rules_valid_cidr() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["10.0.0.0/8".to_string(), "::1/128".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_parse_rules_valid_single_ip() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["10.0.0.1".to_string(), "192.168.1.1".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn test_parse_rules_invalid() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["not-an-ip".to_string()],
            ..Default::default()
        };
        assert!(cfg.parse_rules().is_err());
    }

    #[test]
    fn test_is_ip_allowed_whitelist_match() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert!(is_ip_allowed(
            IpAddr::from([10u8, 255, 255, 255]),
            &rules,
            IpAccessMode::Whitelist
        ));
    }

    #[test]
    fn test_is_ip_allowed_whitelist_no_match() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert!(!is_ip_allowed(
            IpAddr::from([11u8, 0, 0, 1]),
            &rules,
            IpAccessMode::Whitelist
        ));
    }

    #[test]
    fn test_is_ip_allowed_blacklist_match() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert!(!is_ip_allowed(
            IpAddr::from([10u8, 0, 0, 1]),
            &rules,
            IpAccessMode::Blacklist
        ));
    }

    #[test]
    fn test_is_ip_allowed_blacklist_no_match() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert!(is_ip_allowed(
            IpAddr::from([11u8, 0, 0, 1]),
            &rules,
            IpAccessMode::Blacklist
        ));
    }

    #[test]
    fn test_is_ip_allowed_empty_whitelist() {
        let rules: Vec<IpRule> = vec![];
        assert!(is_ip_allowed(
            IpAddr::from([10u8, 0, 0, 1]),
            &rules,
            IpAccessMode::Whitelist
        ));
    }

    #[test]
    fn test_is_ip_allowed_empty_blacklist() {
        let rules: Vec<IpRule> = vec![];
        assert!(is_ip_allowed(
            IpAddr::from([10u8, 0, 0, 1]),
            &rules,
            IpAccessMode::Blacklist
        ));
    }

    #[test]
    fn test_is_ip_allowed_ipv6() {
        let cfg = IpAccessControlConfig {
            ip_list: vec!["::1/128".to_string(), "2001:db8::/32".to_string()],
            ..Default::default()
        };
        let (rules, _) = cfg.parse_rules().unwrap();
        assert!(is_ip_allowed(
            "2001:db8::1".parse().unwrap(),
            &rules,
            IpAccessMode::Whitelist
        ));
    }

    #[test]
    fn test_extract_client_ip_trusted_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "10.0.0.1".parse().unwrap());
        headers.insert("x-forwarded-for", "10.0.0.2".parse().unwrap());

        let cfg = IpAccessControlConfig {
            trusted_proxies: vec!["127.0.0.1".to_string()],
            ..Default::default()
        };
        let (_, trusted) = cfg.parse_rules().unwrap();

        let ip = extract_client_ip_trusted(&headers, &trusted, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(ip, Some("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_extract_client_ip_untrusted_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.2".parse().unwrap());

        let trusted: Vec<IpRule> = vec![];
        let ip = extract_client_ip_trusted(&headers, &trusted, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(ip, Some("127.0.0.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn test_middleware_disabled_passes_through() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = IpAccessControlConfig::default();

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                ip_access_control_middleware,
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

    #[tokio::test]
    async fn test_middleware_whitelist_allows_matching_ip() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = IpAccessControlConfig {
            enabled: true,
            mode: IpAccessMode::Whitelist,
            ip_list: vec!["10.0.0.1".to_string()],
            ..Default::default()
        };

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                ip_access_control_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header("x-real-ip", "10.0.0.1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(resp.status() == axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_whitelist_rejects_non_matching_ip() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = IpAccessControlConfig {
            enabled: true,
            mode: IpAccessMode::Whitelist,
            ip_list: vec!["10.0.0.1".to_string()],
            ..Default::default()
        };

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                ip_access_control_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header("x-real-ip", "10.0.0.2")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }
}

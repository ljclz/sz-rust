//! SSO 中间件 — 本地验签 + 远程校验
//!
//! 对齐 spec.md FR-6 ~ FR-7，design.md §3.2。
//!
//! ## 本地验签（默认）
//!
//! 业务系统与 SSO 认证中心共享 JWT secret，本地 `SsoJwtCodec::decode` 验签，零网络开销。
//!
//! ## 远程校验（feature = "remote-validate"）
//!
//! 密钥不共享场景，通过 HTTP 调用 SSO 认证中心 `/sso/validate` 端点校验。

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use sz_rust_auth_facade::refresh::{
    MemoryRefreshTokenStore, MemoryTokenBlacklist, RefreshTokenStore, RefreshTokenVerifier,
    RenewalConfig, SsoClaims, SsoJwtCodec, TokenBlacklist,
};

// ── AuthenticatedUser ──

/// 认证后的用户信息（注入 request extensions）
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// 用户 ID
    pub user_id: i64,
    /// 用户名
    pub username: String,
}

// ── SsoMiddlewareConfig ──

/// SSO 中间件配置
pub struct SsoMiddlewareConfig {
    /// JWT 编解码器（本地验签）
    codec: SsoJwtCodec,
    /// Token 黑名单
    blacklist: Arc<dyn TokenBlacklist>,
    /// Token 版本存储
    store: Arc<dyn RefreshTokenStore>,
    /// JWT 签发人
    issuer: String,
    /// 白名单路由（支持 `*` 通配符）
    allow_all_action: Vec<String>,
    /// 续期配置（`None` = 不启用续期）
    renewal_config: Option<RenewalConfig>,
}

impl SsoMiddlewareConfig {
    /// 创建本地验签配置
    pub fn local(
        secret: impl Into<String>,
        issuer: impl Into<String>,
        blacklist: Arc<dyn TokenBlacklist>,
        store: Arc<dyn RefreshTokenStore>,
        allow_all_action: Vec<String>,
    ) -> Self {
        Self::local_with_renewal(secret, issuer, blacklist, store, allow_all_action, None)
    }

    /// 创建本地验签配置（带续期配置）
    pub fn local_with_renewal(
        secret: impl Into<String>,
        issuer: impl Into<String>,
        blacklist: Arc<dyn TokenBlacklist>,
        store: Arc<dyn RefreshTokenStore>,
        allow_all_action: Vec<String>,
        renewal_config: Option<RenewalConfig>,
    ) -> Self {
        Self {
            codec: SsoJwtCodec::new(secret),
            blacklist,
            store,
            issuer: issuer.into(),
            allow_all_action,
            renewal_config,
        }
    }

    /// 创建本地验签配置（内存黑名单 + 内存存储，测试用）
    pub fn local_memory(
        secret: impl Into<String>,
        issuer: impl Into<String>,
        allow_all_action: Vec<String>,
    ) -> Self {
        Self::local_memory_with_renewal(secret, issuer, allow_all_action, None)
    }

    /// 创建本地验签配置（内存黑名单 + 内存存储 + 续期配置，测试用）
    pub fn local_memory_with_renewal(
        secret: impl Into<String>,
        issuer: impl Into<String>,
        allow_all_action: Vec<String>,
        renewal_config: Option<RenewalConfig>,
    ) -> Self {
        Self::local_with_renewal(
            secret,
            issuer,
            Arc::new(MemoryTokenBlacklist::new()),
            Arc::new(MemoryRefreshTokenStore::new()),
            allow_all_action,
            renewal_config,
        )
    }

    /// 检查路由是否在白名单中
    fn is_allowed(&self, path: &str) -> bool {
        for pattern in &self.allow_all_action {
            if pattern == "*" || pattern == path {
                return true;
            }
            if pattern.ends_with('*') && path.starts_with(&pattern[..pattern.len() - 1]) {
                return true;
            }
        }
        false
    }
}

impl std::fmt::Debug for SsoMiddlewareConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoMiddlewareConfig")
            .field("codec", &self.codec)
            .field("issuer", &self.issuer)
            .field("allow_all_action", &self.allow_all_action)
            .finish_non_exhaustive()
    }
}

// ── sso_middleware ──

/// SSO 中间件
///
/// 从 `Authorization: Bearer <token>` 提取 accessToken，
/// 执行本地验签 + 黑名单查询 + 版本校验，通过后注入 `AuthenticatedUser`。
pub async fn sso_middleware(
    State(config): State<Arc<SsoMiddlewareConfig>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    if config.is_allowed(&path) {
        return next.run(req).await;
    }

    let auth_header = match req.headers().get("authorization") {
        Some(v) => v.to_str().unwrap_or(""),
        None => return unauthorized("missing authorization header"),
    };

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return unauthorized("invalid authorization scheme"),
    };

    let verifier = RefreshTokenVerifier::new(
        config.codec.clone(),
        config.blacklist.clone(),
        config.store.clone(),
        config.issuer.clone(),
    );

    match verifier.verify_access(token).await {
        Ok(claims) => {
            let user_id = claims.user_id.unwrap_or(0);
            let username = claims.sub.clone();
            let mut req = req;
            req.extensions_mut()
                .insert(AuthenticatedUser { user_id, username });

            let renewed_token = config
                .renewal_config
                .as_ref()
                .filter(|rc| rc.enabled)
                .and_then(|rc| {
                    let now = chrono::Utc::now().timestamp();
                    let remaining_ttl = claims.exp - now;
                    if !rc.should_renew(remaining_ttl) {
                        return None;
                    }
                    let new_exp = now + rc.access_token_ttl.num_seconds();
                    let new_claims = SsoClaims {
                        sub: claims.sub.clone(),
                        exp: new_exp,
                        iat: now,
                        iss: claims.iss.clone(),
                        user_id: claims.user_id,
                        token_type: "access".to_string(),
                        jti: uuid::Uuid::new_v4().to_string(),
                        ver: claims.ver,
                        roles: claims.roles.clone(),
                        permissions: claims.permissions.clone(),
                        device_id: claims.device_id.clone(),
                    };
                    match config.codec.encode(&new_claims) {
                        Ok(token) => Some((token, new_exp)),
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to encode renewed token");
                            None
                        }
                    }
                });

            let mut response = next.run(req).await;
            if let Some((token, exp)) = renewed_token {
                if let (Ok(h1), Ok(h2)) = (
                    token.parse::<axum::http::HeaderValue>(),
                    exp.to_string().parse::<axum::http::HeaderValue>(),
                ) {
                    response.headers_mut().insert("X-Renewed-Access-Token", h1);
                    response.headers_mut().insert("X-Renewed-Expires-At", h2);
                }
            }
            response
        }
        Err(e) => {
            tracing::warn!(error = %e, "SSO token validation failed");
            unauthorized(&e.to_string())
        }
    }
}

fn unauthorized(msg: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("Cache-Control", "no-store"), ("Pragma", "no-cache")],
        format!("{{\"code\":-1,\"msg\":\"{msg}\"}}"),
    )
        .into_response()
}

// ── 远程校验（feature = "remote-validate"） ──

/// 连接池配置
#[cfg(feature = "remote-validate")]
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// 每个主机最大空闲连接数
    pub pool_max_idle_per_host: usize,
    /// 空闲连接超时
    pub pool_idle_timeout: Option<std::time::Duration>,
    /// TCP keepalive
    pub tcp_keepalive: Option<std::time::Duration>,
    /// TCP nodelay
    pub tcp_nodelay: bool,
}

#[cfg(feature = "remote-validate")]
impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pool_max_idle_per_host: 32,
            pool_idle_timeout: Some(std::time::Duration::from_secs(90)),
            tcp_keepalive: Some(std::time::Duration::from_secs(60)),
            tcp_nodelay: true,
        }
    }
}

/// 远程校验配置
#[cfg(feature = "remote-validate")]
pub struct RemoteValidateConfig {
    /// SSO 认证中心校验端点
    pub endpoint: String,
    /// 超时时间
    pub timeout: std::time::Duration,
    /// HTTP 客户端（单例复用连接池）
    client: reqwest::Client,
    /// 白名单路由
    pub allow_all_action: Vec<String>,
    /// 连接池配置
    pub pool_config: PoolConfig,
}

#[cfg(feature = "remote-validate")]
impl RemoteValidateConfig {
    /// 创建远程校验配置（向后兼容，内部委托 `new_checked().expect()`）
    pub fn new(
        endpoint: impl Into<String>,
        timeout: std::time::Duration,
        allow_all_action: Vec<String>,
    ) -> Self {
        Self::new_checked(endpoint, timeout, allow_all_action, PoolConfig::default())
            .expect("failed to build RemoteValidateConfig")
    }

    /// 创建远程校验配置（失败安全，返回 Result）
    pub fn new_checked(
        endpoint: impl Into<String>,
        timeout: std::time::Duration,
        allow_all_action: Vec<String>,
        pool_config: PoolConfig,
    ) -> Result<Self, sz_rust_auth_facade::refresh::RefreshTokenError> {
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .pool_max_idle_per_host(pool_config.pool_max_idle_per_host)
            .tcp_nodelay(pool_config.tcp_nodelay);

        if let Some(idle_timeout) = pool_config.pool_idle_timeout {
            builder = builder.pool_idle_timeout(idle_timeout);
        }
        if let Some(keepalive) = pool_config.tcp_keepalive {
            builder = builder.tcp_keepalive(keepalive);
        }

        let client = builder.build().map_err(|e| {
            sz_rust_auth_facade::refresh::RefreshTokenError::InvalidConfig(e.to_string())
        })?;

        Ok(Self {
            endpoint: endpoint.into(),
            timeout,
            client,
            allow_all_action,
            pool_config,
        })
    }

    /// 创建远程校验配置（失败回退默认 Client + warn 日志）
    pub fn new_or_default(
        endpoint: impl Into<String>,
        timeout: std::time::Duration,
        allow_all_action: Vec<String>,
        pool_config: PoolConfig,
    ) -> Self {
        match Self::new_checked(endpoint, timeout, allow_all_action, pool_config.clone()) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(error = %e, "failed to build reqwest client, falling back to default");
                let client = reqwest::Client::new();
                Self {
                    endpoint: String::new(),
                    timeout,
                    client,
                    allow_all_action: Vec::new(),
                    pool_config,
                }
            }
        }
    }

    /// 从外部传入预配置的 reqwest::Client 创建配置
    pub fn from_client(
        endpoint: impl Into<String>,
        timeout: std::time::Duration,
        allow_all_action: Vec<String>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout,
            client,
            allow_all_action,
            pool_config: PoolConfig::default(),
        }
    }

    /// Builder 模式创建配置
    pub fn builder(endpoint: impl Into<String>) -> RemoteValidateConfigBuilder {
        RemoteValidateConfigBuilder {
            endpoint: endpoint.into(),
            timeout: std::time::Duration::from_secs(10),
            allow_all_action: Vec::new(),
            pool_config: PoolConfig::default(),
        }
    }

    /// 检查路由是否在白名单中
    fn is_allowed(&self, path: &str) -> bool {
        for pattern in &self.allow_all_action {
            if pattern == "*" || pattern == path {
                return true;
            }
            if pattern.ends_with('*') && path.starts_with(&pattern[..pattern.len() - 1]) {
                return true;
            }
        }
        false
    }
}

/// 远程校验配置 Builder
#[cfg(feature = "remote-validate")]
pub struct RemoteValidateConfigBuilder {
    endpoint: String,
    timeout: std::time::Duration,
    allow_all_action: Vec<String>,
    pool_config: PoolConfig,
}

#[cfg(feature = "remote-validate")]
impl RemoteValidateConfigBuilder {
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn allow_all_action(mut self, actions: Vec<String>) -> Self {
        self.allow_all_action = actions;
        self
    }

    pub fn pool_config(mut self, pool_config: PoolConfig) -> Self {
        self.pool_config = pool_config;
        self
    }

    pub fn build(
        self,
    ) -> Result<RemoteValidateConfig, sz_rust_auth_facade::refresh::RefreshTokenError> {
        RemoteValidateConfig::new_checked(
            self.endpoint,
            self.timeout,
            self.allow_all_action,
            self.pool_config,
        )
    }
}

/// 远程校验中间件
#[cfg(feature = "remote-validate")]
pub async fn sso_middleware_remote(
    State(config): State<Arc<RemoteValidateConfig>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    if config.is_allowed(&path) {
        return next.run(req).await;
    }

    let auth_header = match req.headers().get("authorization") {
        Some(v) => v.to_str().unwrap_or(""),
        None => return unauthorized("missing authorization header"),
    };

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return unauthorized("invalid authorization scheme"),
    };

    let validate_url = format!("{}?token={}", config.endpoint, token);
    match config.client.get(&validate_url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(json) => {
                let data = json.get("data").cloned().unwrap_or_default();
                let user_id = data.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let mut req = req;
                req.extensions_mut().insert(AuthenticatedUser {
                    user_id,
                    username: String::new(),
                });
                next.run(req).await
            }
            Err(_) => service_unavailable(),
        },
        Ok(resp) if resp.status() == StatusCode::UNAUTHORIZED => {
            unauthorized("token invalid or expired")
        }
        _ => service_unavailable(),
    }
}

#[cfg(feature = "remote-validate")]
fn service_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("Cache-Control", "no-store"), ("Pragma", "no-cache")],
        r#"{"code":-1,"msg":"认证服务暂时不可用"}"#,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use sz_rust_auth_facade::refresh::{RefreshTokenConfig, RefreshTokenIssuer};
    use tower::ServiceExt;

    async fn handler(req: Request<Body>) -> String {
        let user = req.extensions().get::<AuthenticatedUser>();
        match user {
            Some(u) => format!("user_id={}, username={}", u.user_id, u.username),
            None => "no user".to_string(),
        }
    }

    fn make_app() -> (Router, RefreshTokenIssuer) {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
        let config = RefreshTokenConfig::default();
        let issuer = RefreshTokenIssuer::new(
            codec.clone(),
            blacklist.clone(),
            store.clone(),
            config.clone(),
        );
        let mw_config = Arc::new(SsoMiddlewareConfig::local(
            "test-secret",
            config.issuer.clone(),
            blacklist,
            store,
            vec!["/public/*".to_string()],
        ));
        let app = Router::new()
            .route("/protected", get(handler))
            .route("/public/health", get(handler))
            .layer(axum::middleware::from_fn_with_state(
                mw_config,
                sso_middleware,
            ));
        (app, issuer)
    }

    #[tokio::test]
    async fn test_middleware_allows_whitelist() {
        let (app, _) = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/public/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_rejects_missing_token() {
        let (app, _) = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_accepts_valid_token() {
        let (app, issuer) = make_app();
        let pair = issuer.issue(42, "alice").await.unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {}", pair.access_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("user_id=42"));
        assert!(text.contains("username=alice"));
    }

    #[tokio::test]
    async fn test_middleware_rejects_refresh_token_as_access() {
        let (app, issuer) = make_app();
        let pair = issuer.issue(42, "alice").await.unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {}", pair.refresh_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_middleware_rejects_invalid_token() {
        let (app, _) = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", "Bearer invalid.token.here")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 续期响应头集成测试 ──

    fn make_app_with_renewal() -> (Router, RefreshTokenIssuer) {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
        let config = RefreshTokenConfig {
            access_token_ttl: chrono::Duration::seconds(60),
            refresh_token_ttl: chrono::Duration::seconds(3600),
            issuer: "sz-rust-sso".to_string(),
        };
        let issuer = RefreshTokenIssuer::new(
            codec.clone(),
            blacklist.clone(),
            store.clone(),
            config.clone(),
        );
        let renewal_config = RenewalConfig {
            enabled: true,
            renewal_threshold: chrono::Duration::seconds(30),
            renewal_ratio: 0.2,
            access_token_ttl: chrono::Duration::seconds(60),
        };
        let mw_config = Arc::new(SsoMiddlewareConfig::local_with_renewal(
            "test-secret",
            config.issuer.clone(),
            blacklist,
            store,
            vec!["/public/*".to_string()],
            Some(renewal_config),
        ));
        let app = Router::new()
            .route("/protected", get(handler))
            .route("/public/health", get(handler))
            .layer(axum::middleware::from_fn_with_state(
                mw_config,
                sso_middleware,
            ));
        (app, issuer)
    }

    #[tokio::test]
    async fn test_middleware_renewal_header_when_ttl_low() {
        let (app, issuer) = make_app_with_renewal();
        let pair = issuer.issue(42, "alice").await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {}", pair.access_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().contains_key("X-Renewed-Access-Token"));
        assert!(resp.headers().contains_key("X-Renewed-Expires-At"));
    }

    #[tokio::test]
    async fn test_middleware_no_renewal_header_when_ttl_high() {
        let (app, issuer) = make_app_with_renewal();
        let pair = issuer.issue(42, "alice").await.unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {}", pair.access_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!resp.headers().contains_key("X-Renewed-Access-Token"));
        assert!(!resp.headers().contains_key("X-Renewed-Expires-At"));
    }

    #[tokio::test]
    async fn test_middleware_no_renewal_header_when_disabled() {
        let (app, issuer) = make_app();
        let pair = issuer.issue(42, "alice").await.unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {}", pair.access_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!resp.headers().contains_key("X-Renewed-Access-Token"));
    }

    #[tokio::test]
    async fn test_middleware_renewal_preserves_response_body() {
        let (app, issuer) = make_app_with_renewal();
        let pair = issuer.issue(42, "alice").await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {}", pair.access_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("user_id=42"));
        assert!(text.contains("username=alice"));
    }

    // ── T4: PoolConfig + RemoteValidateConfig 单元测试 ──

    #[cfg(feature = "remote-validate")]
    mod remote_validate_tests {
        use super::*;
        use std::time::Duration;

        #[test]
        fn test_pool_config_default() {
            let config = PoolConfig::default();
            assert_eq!(config.pool_max_idle_per_host, 32);
            assert!(config.pool_idle_timeout.is_some());
            assert!(config.tcp_keepalive.is_some());
            assert!(config.tcp_nodelay);
        }

        #[test]
        fn test_remote_validate_config_new_backward_compatible() {
            let config = RemoteValidateConfig::new(
                "http://localhost:8080/validate",
                Duration::from_secs(10),
                vec!["/public/*".to_string()],
            );
            assert_eq!(config.endpoint, "http://localhost:8080/validate");
            assert_eq!(config.timeout, Duration::from_secs(10));
            assert_eq!(config.allow_all_action, vec!["/public/*".to_string()]);
        }

        #[test]
        fn test_remote_validate_config_new_checked_success() {
            let result = RemoteValidateConfig::new_checked(
                "http://localhost:8080/validate",
                Duration::from_secs(5),
                vec![],
                PoolConfig::default(),
            );
            assert!(result.is_ok());
            let config = result.unwrap();
            assert_eq!(config.endpoint, "http://localhost:8080/validate");
        }

        #[test]
        fn test_remote_validate_config_new_or_default_success() {
            let config = RemoteValidateConfig::new_or_default(
                "http://localhost:8080/validate",
                Duration::from_secs(5),
                vec![],
                PoolConfig::default(),
            );
            assert_eq!(config.endpoint, "http://localhost:8080/validate");
        }

        #[test]
        fn test_remote_validate_config_from_client() {
            let client = reqwest::Client::new();
            let config = RemoteValidateConfig::from_client(
                "http://localhost:8080/validate",
                Duration::from_secs(10),
                vec!["*".to_string()],
                client,
            );
            assert_eq!(config.endpoint, "http://localhost:8080/validate");
            assert_eq!(config.allow_all_action, vec!["*".to_string()]);
        }

        #[test]
        fn test_remote_validate_config_builder() {
            let config = RemoteValidateConfig::builder("http://localhost:8080/validate")
                .timeout(Duration::from_secs(15))
                .allow_all_action(vec!["/api/*".to_string()])
                .build()
                .unwrap();
            assert_eq!(config.endpoint, "http://localhost:8080/validate");
            assert_eq!(config.timeout, Duration::from_secs(15));
            assert_eq!(config.allow_all_action, vec!["/api/*".to_string()]);
        }

        #[test]
        fn test_remote_validate_config_builder_default_timeout() {
            let config = RemoteValidateConfig::builder("http://localhost:8080/validate")
                .build()
                .unwrap();
            assert_eq!(config.timeout, Duration::from_secs(10));
        }

        #[test]
        fn test_pool_config_custom() {
            let config = PoolConfig {
                pool_max_idle_per_host: 64,
                pool_idle_timeout: Some(Duration::from_secs(120)),
                tcp_keepalive: None,
                tcp_nodelay: false,
            };
            assert_eq!(config.pool_max_idle_per_host, 64);
            assert!(config.tcp_keepalive.is_none());
            assert!(!config.tcp_nodelay);
        }

        #[test]
        fn test_is_allowed_wildcard() {
            let config = RemoteValidateConfig::new(
                "http://localhost:8080/validate",
                Duration::from_secs(10),
                vec!["*".to_string()],
            );
            assert!(config.is_allowed("/any/path"));
            assert!(config.is_allowed("/"));
        }

        #[test]
        fn test_is_allowed_prefix_match() {
            let config = RemoteValidateConfig::new(
                "http://localhost:8080/validate",
                Duration::from_secs(10),
                vec!["/public/*".to_string()],
            );
            assert!(config.is_allowed("/public/index"));
            assert!(!config.is_allowed("/private/data"));
        }
    }
}

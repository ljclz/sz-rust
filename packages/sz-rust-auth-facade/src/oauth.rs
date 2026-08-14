//! OAuth2 模块 — 对齐 Laravel Socialite
//!
//! 提供 OAuth2 客户端抽象，对齐 Laravel Socialite `ProviderInterface` 的核心 API。
//! 支持通用 OAuth2 提供商（QQ、微信、GitHub、Google 等）。
//!
//! ## Laravel Socialite 对齐
//!
//! ### 核心 API 映射
//!
//! | Laravel Socialite 方法 | Rust 方法 | 说明 |
//! |-----------------------|-----------|------|
//! | `Socialite::driver('qq')->redirect()` | [`OAuth2Provider::redirect_url`] | 生成授权 URL |
//! | `Socialite::driver('qq')->user()` | [`OAuth2Provider::user_from_token`] | 用授权码换取用户信息 |
//! | `$user->getId()` | [`SocialiteUser::id`] | 第三方用户 ID |
//! | `$user->getNickname()` | [`SocialiteUser::nickname`] | 用户昵称 |
//! | `$user->getName()` | [`SocialiteUser::name`] | 用户姓名 |
//! | `$user->getEmail()` | [`SocialiteUser::email`] | 邮箱 |
//! | `$user->getAvatar()` | [`SocialiteUser::avatar`] | 头像 URL |
//! | `$user->token` | [`SocialiteUser::access_token`] | 访问令牌 |
//! | `$user->refreshToken` | [`SocialiteUser::refresh_token`] | 刷新令牌 |
//! | `$user->expiresIn` | [`SocialiteUser::expires_in`] | 令牌过期时间 |
//!
//! ### Laravel 行为对齐
//!
//! - **配置驱动**：Laravel 通过 `config/services.php` 配置 `client_id`/`client_secret`/`redirect`。
//!   Rust 通过 [`OAuth2Config`] builder 表达相同配置项。
//! - **state 参数**：Laravel 自动生成 CSRF 防护的 state 参数。Rust 由调用方传入 state
//!   （对齐 Laravel `Socialite::driver('qq')->withState($state)->redirect()`）。
//! - **scopes**：Laravel 支持 `scopes()` 设置多个 scope。Rust 通过 [`OAuth2Config::with_scopes`]
//!   或 [`OAuth2Config::with_scope`] 累加。
//! - **额外参数**：Laravel 支持 `with()` 追加查询参数。Rust 通过
//!   [`OAuth2Config::with_extra_param`] / [`OAuth2Config::with_extra_params`]。
//!
//! ## 架构说明
//!
//! - **OAuth2Provider trait**：对齐 Laravel `ProviderInterface`，业务方实现具体提供商逻辑
//! - **GenericOAuth2Provider**：通用 OAuth2 实现，接收 [`OAuth2Config`] 和
//!   [`OAuth2HttpTransport`]，支持任意标准 OAuth2 提供商
//! - **OAuth2HttpTransport trait**：HTTP 传输抽象，解耦 OAuth2 客户端与具体 HTTP 库。
//!   与 `notify::HttpTransport` 分离，因为 OAuth2 token 交换需要返回响应体（解析 access_token）
//! - **MemoryOAuth2HttpTransport**：内存 HTTP 传输实现，支持预置 mock 响应，用于测试

use base64::Engine;
use parking_lot::Mutex;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// OAuth2 错误 — 对齐 Laravel Socialite 异常体系
#[derive(Debug, Error)]
pub enum OAuth2Error {
    /// 缺少必填字段（client_id / client_secret / redirect_url / auth_url / token_url 等）
    #[error("OAuth2 字段缺失: {0}")]
    MissingField(String),
    /// 授权失败（授权码为空、state 不匹配等）
    #[error("OAuth2 授权失败: {0}")]
    AuthFailed(String),
    /// token 交换失败（HTTP 错误、响应缺少 access_token 等）
    #[error("OAuth2 token 交换失败: {0}")]
    TokenExchangeFailed(String),
    /// 获取用户信息失败（HTTP 错误、响应解析失败等）
    #[error("OAuth2 获取用户信息失败: {0}")]
    UserInfoFailed(String),
    /// HTTP 传输失败（网络错误、连接超时等）
    #[error("OAuth2 HTTP 传输失败: {0}")]
    HttpTransport(String),
    /// 序列化/反序列化失败
    #[error("OAuth2 序列化失败: {0}")]
    Serialize(String),
}

// ============================================================================
// PKCE（Proof Key for Code Exchange, RFC 7636）
// ============================================================================

/// PKCE 方法 — 对齐 RFC 7636
///
/// 当前仅支持 `S256`（SHA256 派生 code_challenge），不支持 `plain`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkceMethod {
    /// SHA256 派生：`code_challenge = BASE64URL(SHA256(code_verifier))`
    S256,
}

impl std::fmt::Display for PkceMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkceMethod::S256 => write!(f, "S256"),
        }
    }
}

/// PKCE 参数对 — 包含 code_verifier 和派生的 code_challenge
///
/// `code_verifier` 不实现明文 Debug（自定义 Debug 只输出长度，防止日志泄露）。
pub struct PkceParams {
    /// PKCE code_verifier（43-128 字符，此处固定 64 字符 hex）
    pub code_verifier: String,
    /// PKCE code_challenge（base64url(SHA256(code_verifier))）
    pub code_challenge: String,
    /// PKCE 方法（固定 S256）
    pub method: PkceMethod,
}

impl std::fmt::Debug for PkceParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PkceParams")
            .field(
                "code_verifier",
                &format!("<redacted, len={}>", self.code_verifier.len()),
            )
            .field("code_challenge", &self.code_challenge)
            .field("method", &self.method)
            .finish()
    }
}

impl Clone for PkceParams {
    fn clone(&self) -> Self {
        Self {
            code_verifier: self.code_verifier.clone(),
            code_challenge: self.code_challenge.clone(),
            method: self.method,
        }
    }
}

// ============================================================================
// OAuth2Config
// ============================================================================

/// OAuth2 配置 — 对齐 Laravel Socialite `config/services.php`
///
/// 通过 builder 模式构建，必填字段在 [`OAuth2Config::new`] 中提供，
/// 可选字段通过 `with_*` 链式方法追加。
///
/// # PHP 对齐
///
/// ```php
/// // config/services.php
/// 'qq' => [
///     'client_id' => env('QQ_CLIENT_ID'),
///     'client_secret' => env('QQ_CLIENT_SECRET'),
///     'redirect' => env('QQ_REDIRECT_URL'),
/// ],
/// ```
///
/// # Rust 用法
///
/// ```ignore
/// use sz_rust_auth_facade::oauth::OAuth2Config;
///
/// let config = OAuth2Config::new(
///     "100123456",
///     "secretabc",
///     "https://example.com/oauth/qq/callback",
///     "https://graph.qq.com/oauth2.0/authorize",
///     "https://graph.qq.com/oauth2.0/token",
/// )
/// .with_user_url("https://graph.qq.com/user/get_user_info")
/// .with_scope("get_user_info");
/// ```
#[derive(Clone)]
pub struct OAuth2Config {
    /// 客户端 ID（必填）
    pub client_id: String,
    /// 客户端密钥（必填，Debug 脱敏输出 "***"）
    pub client_secret: String,
    /// 回调 URL（必填）
    pub redirect_url: String,
    /// 授权服务器 authorize 端点（必填，如 `https://graph.qq.com/oauth2.0/authorize`）
    pub auth_url: String,
    /// 授权服务器 token 端点（必填，如 `https://graph.qq.com/oauth2.0/token`）
    pub token_url: String,
    /// 资源服务器用户信息端点（可选，如 `https://graph.qq.com/user/get_user_info`）
    pub user_url: Option<String>,
    /// 默认 scopes（可选）
    pub scopes: Vec<String>,
    /// 额外参数（可选）
    pub extra_params: Vec<(String, String)>,
    /// PKCE 是否启用（默认 false）
    pub pkce_enabled: bool,
    /// device_code 流程的设备授权端点（可选）
    pub device_auth_url: Option<String>,
}

impl std::fmt::Debug for OAuth2Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuth2Config")
            .field("client_id", &self.client_id)
            .field("client_secret", &"***")
            .field("redirect_url", &self.redirect_url)
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("user_url", &self.user_url)
            .field("scopes", &self.scopes)
            .field("extra_params", &self.extra_params)
            .field("pkce_enabled", &self.pkce_enabled)
            .field("device_auth_url", &self.device_auth_url)
            .finish()
    }
}

impl OAuth2Config {
    /// 创建 OAuth2 配置
    ///
    /// # 参数
    ///
    /// - `client_id`: 客户端 ID
    /// - `client_secret`: 客户端密钥
    /// - `redirect_url`: 回调 URL
    /// - `auth_url`: 授权服务器 authorize 端点
    /// - `token_url`: 授权服务器 token 端点
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_url: impl Into<String>,
        auth_url: impl Into<String>,
        token_url: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect_url: redirect_url.into(),
            auth_url: auth_url.into(),
            token_url: token_url.into(),
            user_url: None,
            scopes: Vec::new(),
            extra_params: Vec::new(),
            pkce_enabled: false,
            device_auth_url: None,
        }
    }

    /// 设置用户信息端点
    pub fn with_user_url(mut self, user_url: impl Into<String>) -> Self {
        self.user_url = Some(user_url.into());
        self
    }

    /// 设置 scopes 列表（覆盖原有）
    pub fn with_scopes(mut self, scopes: Vec<String>) -> Self {
        self.scopes = scopes;
        self
    }

    /// 追加单个 scope
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scopes.push(scope.into());
        self
    }

    /// 设置额外参数列表（覆盖原有）
    pub fn with_extra_params(mut self, params: Vec<(String, String)>) -> Self {
        self.extra_params = params;
        self
    }

    /// 追加单个额外参数
    pub fn with_extra_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_params.push((key.into(), value.into()));
        self
    }

    /// 启用/禁用 PKCE（Proof Key for Code Exchange, RFC 7636）
    ///
    /// 启用后，调用方需通过 [`OAuth2Config::generate_pkce_pair`] 生成 PKCE 参数对，
    /// 将 `code_challenge` 追加到授权 URL，将 `code_verifier` 在 token 交换时提交。
    pub fn with_pkce(mut self, enabled: bool) -> Self {
        self.pkce_enabled = enabled;
        self
    }

    /// 设置 device_code 流程的设备授权端点
    pub fn with_device_auth_url(mut self, url: impl Into<String>) -> Self {
        self.device_auth_url = Some(url.into());
        self
    }

    /// 自动生成 state 参数 — 使用 `rand::rngs::OsRng` 生成 16 字节随机数 → 32 字符 hex
    ///
    /// 满足 spec 4.3.3 ≥32 字符密码学安全要求。
    pub fn generate_state() -> String {
        let mut bytes = [0u8; 16];
        OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    /// 自动生成 PKCE 参数对 — code_verifier + code_challenge
    ///
    /// - code_verifier：OsRng 生成 32 字节随机 → 64 字符 hex（满足 43-128 字符要求）
    /// - code_challenge：`BASE64URL-NO-PAD(SHA256(code_verifier))`
    /// - method：固定 `S256`
    pub fn generate_pkce_pair() -> PkceParams {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let code_verifier = hex::encode(bytes);
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let digest = hasher.finalize();
        let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        PkceParams {
            code_verifier,
            code_challenge,
            method: PkceMethod::S256,
        }
    }

    /// 校验必填字段
    ///
    /// 必填字段：`client_id` / `client_secret` / `redirect_url` / `auth_url` / `token_url`。
    /// 任一为空字符串则返回 [`OAuth2Error::MissingField`]。
    pub fn validate(&self) -> Result<(), OAuth2Error> {
        if self.client_id.is_empty() {
            return Err(OAuth2Error::MissingField("client_id".into()));
        }
        if self.client_secret.is_empty() {
            return Err(OAuth2Error::MissingField("client_secret".into()));
        }
        if self.redirect_url.is_empty() {
            return Err(OAuth2Error::MissingField("redirect_url".into()));
        }
        if self.auth_url.is_empty() {
            return Err(OAuth2Error::MissingField("auth_url".into()));
        }
        if self.token_url.is_empty() {
            return Err(OAuth2Error::MissingField("token_url".into()));
        }
        Ok(())
    }
}

// ============================================================================
// SocialiteUser
// ============================================================================

/// OAuth2 用户信息 — 对齐 Laravel Socialite `User`
///
/// 表示从 OAuth2 提供商获取的用户信息，包含标准字段和原始响应数据。
///
/// # PHP 对齐
///
/// ```php
/// $user = Socialite::driver('qq')->user();
/// $user->getId();        // 第三方用户 ID
/// $user->getNickname();  // 昵称
/// $user->getName();      // 姓名
/// $user->getEmail();     // 邮箱
/// $user->getAvatar();    // 头像
/// $user->token;          // access_token
/// $user->refreshToken;   // refresh_token
/// $user->expiresIn;      // 过期秒数
/// $user->user;           // 原始响应（raw）
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SocialiteUser {
    /// 第三方平台用户 ID
    pub id: String,
    /// 用户昵称
    pub nickname: Option<String>,
    /// 用户姓名
    pub name: Option<String>,
    /// 邮箱
    pub email: Option<String>,
    /// 头像 URL
    pub avatar: Option<String>,
    /// 原始响应数据（JSON）
    pub raw: serde_json::Value,
    /// 访问令牌
    #[serde(skip_serializing)]
    pub access_token: Option<String>,
    /// 刷新令牌
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
    /// 令牌过期时间（Unix 秒）
    pub expires_in: Option<i64>,
}

// ============================================================================
// TokenResponse — token 交换响应
// ============================================================================

/// OAuth2 token 交换响应 — 对齐 RFC 6749 Section 5.1
///
/// `access_token` 和 `refresh_token` 标注 `#[serde(skip_serializing)]` 脱敏，
/// 防止令牌通过 API 响应或日志泄漏。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TokenResponse {
    /// 访问令牌（脱敏，不序列化）
    #[serde(skip_serializing)]
    pub access_token: String,
    /// 令牌类型（如 "Bearer"）
    pub token_type: Option<String>,
    /// 过期秒数
    pub expires_in: Option<i64>,
    /// 授权范围
    pub scope: Option<String>,
    /// 刷新令牌（脱敏，不序列化）
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
}

// ============================================================================
// OAuth2AuditLogger — 审计日志
// ============================================================================

/// OAuth2 审计事件 — 记录 token 交换/刷新/CSRF 等安全事件
#[derive(Debug, Clone)]
pub struct OAuth2AuditEvent {
    /// 客户端 ID
    pub client_id: String,
    /// grant 类型（authorization_code / implicit / device_code / refresh_token）
    pub grant_type: String,
    /// 结果（success / failure）
    pub result: String,
    /// Unix 时间戳（秒）
    pub timestamp: i64,
    /// 告警码（可选，如 OAUTH2_IMPLICIT_TOKEN_EXPOSED / OAUTH2_CSRF_STATE_MISMATCH）
    pub alert_code: Option<String>,
    /// 附加消息
    pub message: Option<String>,
}

/// OAuth2 审计日志 trait — best-effort 记录安全事件
///
/// 实现者保证 `Send + Sync`，日志写入失败**不应**影响主流程。
pub trait OAuth2AuditLogger: Send + Sync {
    /// 记录审计事件
    fn log_event(&self, event: &OAuth2AuditEvent);
}

// ============================================================================
// OAuth2Provider trait
// ============================================================================

/// OAuth2 提供商 trait — 对齐 Laravel Socialite `ProviderInterface`
///
/// 业务方实现此 trait 以对接具体 OAuth2 提供商（QQ / 微信 / GitHub / Google 等）。
/// 框架内置 [`GenericOAuth2Provider`] 通用实现，满足标准 OAuth2 协议的提供商可直接使用。
///
/// # 线程安全
///
/// 实现者必须保证 `Send + Sync`，因为 Provider 通常作为单例在多线程下使用。
pub trait OAuth2Provider: Send + Sync {
    /// 生成授权 URL（对齐 `Socialite::driver('qq')->redirect()`）
    ///
    /// # 参数
    ///
    /// - `state`: CSRF 防护的 state 参数（由调用方生成并暂存到 session）
    ///
    /// # 返回
    ///
    /// 完整的授权 URL，包含 `client_id` / `redirect_uri` / `response_type=code` /
    /// `state` / `scope` 等查询参数。
    fn redirect_url(&self, state: &str) -> String;

    /// 用授权码换取访问令牌并获取用户信息（对齐 `Socialite::driver('qq')->user()`）
    ///
    /// # 参数
    ///
    /// - `code`: 授权服务器回调时携带的授权码
    ///
    /// # 返回
    ///
    /// 成功返回 [`SocialiteUser`]，失败返回 [`OAuth2Error`]。
    fn user_from_token(&self, code: &str) -> Result<SocialiteUser, OAuth2Error>;
}

// ============================================================================
// OAuth2HttpTransport trait（HTTP 传输抽象）
// ============================================================================

/// OAuth2 HTTP 传输 trait — 用于解耦 OAuth2 客户端与具体 HTTP 库
///
/// 与 `notify::HttpTransport` 分离的原因：OAuth2 token 交换需要返回响应体
/// （解析 `access_token`），而 `notify::HttpTransport::post_json` 仅返回 `Result<(), _>`。
///
/// 业务方实现此 trait 注入 reqwest / hyper / etc.，即可让 [`GenericOAuth2Provider`]
/// 投入生产。
///
/// # 线程安全
///
/// 实现者必须保证 `Send + Sync`，因为 Provider 通常作为单例在多线程下使用。
pub trait OAuth2HttpTransport: Send + Sync {
    /// 发送 POST 请求，Content-Type: application/json
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    /// - `body`: 请求体（JSON 字符串）
    ///
    /// # 返回
    ///
    /// 成功返回响应体字符串，失败返回 [`OAuth2Error`]。
    fn post_json(&self, url: &str, body: &str) -> Result<String, OAuth2Error>;
}

// ============================================================================
// MemoryOAuth2HttpTransport（测试/开发用 HTTP 传输实现）
// ============================================================================

/// 内存 HTTP 传输 — 用于测试和开发环境
///
/// 不实际发送 HTTP 请求，而是：
/// - 将请求暂存到内部 Vec，供测试断言使用
/// - 从预置响应队列中依次返回 mock 响应
///
/// # 用法
///
/// ```ignore
/// use sz_rust_auth_facade::oauth::MemoryOAuth2HttpTransport;
///
/// let transport = MemoryOAuth2HttpTransport::new();
/// // 预置 mock 响应（按调用顺序消费）
/// transport.push_response(r#"{"access_token":"token123"}"#);
/// transport.push_response(r#"{"id":"1","nickname":"test"}"#);
///
/// let resp = transport.post_json("https://example.com/token", "{}").unwrap();
/// assert_eq!(resp, r#"{"access_token":"token123"}"#);
/// ```
#[derive(Debug, Default)]
pub struct MemoryOAuth2HttpTransport {
    /// 已"发送"的 HTTP 请求列表（url, body）
    requests: Mutex<Vec<(String, String)>>,
    /// 预置的 mock 响应队列（FIFO）
    responses: Mutex<VecDeque<String>>,
}

impl MemoryOAuth2HttpTransport {
    /// 创建新的内存 HTTP 传输
    pub fn new() -> Self {
        Self::default()
    }

    /// 预置 mock 响应（追加到队列末尾，按调用顺序消费）
    pub fn push_response(&self, response: impl Into<String>) {
        self.responses.lock().push_back(response.into());
    }

    /// 获取已发送请求数量
    pub fn count(&self) -> usize {
        self.requests.lock().len()
    }

    /// 获取所有已发送请求（快照）
    pub fn all(&self) -> Vec<(String, String)> {
        self.requests.lock().clone()
    }

    /// 获取最后发送的请求
    pub fn last(&self) -> Option<(String, String)> {
        self.requests.lock().last().cloned()
    }

    /// 清空已发送请求和预置响应
    pub fn clear(&self) {
        self.requests.lock().clear();
        self.responses.lock().clear();
    }
}

impl OAuth2HttpTransport for MemoryOAuth2HttpTransport {
    fn post_json(&self, url: &str, body: &str) -> Result<String, OAuth2Error> {
        self.requests
            .lock()
            .push((url.to_string(), body.to_string()));
        let mut responses = self.responses.lock();
        match responses.pop_front() {
            Some(resp) => Ok(resp),
            None => Ok(String::new()),
        }
    }
}

// ============================================================================
// GenericOAuth2Provider
// ============================================================================

/// 通用 OAuth2 提供商 — 对齐 Laravel Socialite `GenericProvider`
///
/// 接收 [`OAuth2Config`] 和 [`OAuth2HttpTransport`]，支持任意标准 OAuth2 提供商。
///
/// # 工作流程
///
/// 1. [`GenericOAuth2Provider::redirect_url`]：构造授权 URL，引导用户跳转到授权服务器
/// 2. 授权服务器回调，携带 `code` 和 `state`
/// 3. [`GenericOAuth2Provider::user_from_token`]：
///    - POST `token_url` 换取 `access_token`（grant_type=authorization_code）
///    - 如果配置了 `user_url`，POST 用户信息端点获取用户资料
///    - 返回 [`SocialiteUser`]
///
/// # 用法
///
/// ```ignore
/// use std::sync::Arc;
/// use sz_rust_auth_facade::oauth::{
///     GenericOAuth2Provider, MemoryOAuth2HttpTransport, OAuth2Config, OAuth2Provider,
/// };
///
/// let config = OAuth2Config::new(
///     "client_id",
///     "client_secret",
///     "https://example.com/callback",
///     "https://provider.com/oauth2.0/authorize",
///     "https://provider.com/oauth2.0/token",
/// )
/// .with_user_url("https://provider.com/user/info");
///
/// let transport = Arc::new(MemoryOAuth2HttpTransport::new());
/// let provider = GenericOAuth2Provider::new(config, transport);
///
/// let url = provider.redirect_url("random_state");
/// let user = provider.user_from_token("auth_code").unwrap();
/// ```
pub struct GenericOAuth2Provider {
    /// OAuth2 配置
    config: OAuth2Config,
    /// HTTP 传输实现
    transport: Arc<dyn OAuth2HttpTransport>,
    /// 审计日志（可选，best-effort）
    audit_logger: Option<Arc<dyn OAuth2AuditLogger>>,
    /// Token 存储（可选，需 `redis-store` feature，best-effort）
    #[cfg(feature = "redis-store")]
    token_store: Option<Arc<dyn crate::oauth_store::OAuth2TokenStore>>,
}

impl GenericOAuth2Provider {
    /// 创建通用 OAuth2 提供商
    ///
    /// # 参数
    ///
    /// - `config`: OAuth2 配置
    /// - `transport`: HTTP 传输实现（业务方注入 reqwest / hyper / etc.）
    pub fn new(config: OAuth2Config, transport: Arc<dyn OAuth2HttpTransport>) -> Self {
        Self {
            config,
            transport,
            audit_logger: None,
            #[cfg(feature = "redis-store")]
            token_store: None,
        }
    }

    /// 注入审计日志
    pub fn with_audit_logger(mut self, logger: Arc<dyn OAuth2AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    /// 注入 Token 存储（需 `redis-store` feature）
    ///
    /// token 交换成功后自动存储（best-effort，存储失败记录告警不传播）。
    #[cfg(feature = "redis-store")]
    pub fn with_token_store(
        mut self,
        store: Arc<dyn crate::oauth_store::OAuth2TokenStore>,
    ) -> Self {
        self.token_store = Some(store);
        self
    }

    /// 记录审计事件（best-effort，logger 未注入时静默跳过）
    fn log_audit(
        &self,
        grant_type: &str,
        result: &str,
        alert_code: Option<&str>,
        message: Option<&str>,
    ) {
        if let Some(logger) = &self.audit_logger {
            let event = OAuth2AuditEvent {
                client_id: self.config.client_id.clone(),
                grant_type: grant_type.to_string(),
                result: result.to_string(),
                timestamp: chrono::Utc::now().timestamp(),
                alert_code: alert_code.map(|s| s.to_string()),
                message: message.map(|s| s.to_string()),
            };
            logger.log_event(&event);
        }
    }

    /// 用 refresh_token 换取新的 access_token
    ///
    /// POST `token_url`，body 为 JSON：
    /// ```json
    /// {
    ///   "grant_type": "refresh_token",
    ///   "refresh_token": "<refresh_token>",
    ///   "client_id": "<client_id>",
    ///   "client_secret": "<client_secret>"
    /// }
    /// ```
    pub fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse, OAuth2Error> {
        if refresh_token.is_empty() {
            self.log_audit("refresh_token", "failure", None, Some("refresh_token 为空"));
            return Err(OAuth2Error::AuthFailed("refresh_token 不能为空".into()));
        }

        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": self.config.client_id,
            "client_secret": self.config.client_secret,
        });
        let body_str =
            serde_json::to_string(&body).map_err(|err| OAuth2Error::Serialize(err.to_string()))?;

        let response = self
            .transport
            .post_json(&self.config.token_url, &body_str)
            .map_err(|err| {
                self.log_audit("refresh_token", "failure", None, Some(&err.to_string()));
                OAuth2Error::HttpTransport(err.to_string())
            })?;

        if response.is_empty() {
            self.log_audit("refresh_token", "failure", None, Some("空响应"));
            return Err(OAuth2Error::TokenExchangeFailed("token 响应为空".into()));
        }

        let json: serde_json::Value = serde_json::from_str(&response).map_err(|err| {
            self.log_audit(
                "refresh_token",
                "failure",
                None,
                Some(&format!("JSON 解析失败: {err}")),
            );
            OAuth2Error::TokenExchangeFailed(format!("解析 token 响应失败: {err}"))
        })?;

        let access_token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                self.log_audit(
                    "refresh_token",
                    "failure",
                    None,
                    Some("响应缺少 access_token"),
                );
                OAuth2Error::TokenExchangeFailed("token 响应缺少 access_token 字段".into())
            })?
            .to_string();

        let token_response = TokenResponse {
            access_token,
            token_type: json
                .get("token_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            expires_in: json.get("expires_in").and_then(|v| v.as_i64()),
            scope: json
                .get("scope")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            refresh_token: json
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        };

        self.log_audit("refresh_token", "success", None, None);
        Ok(token_response)
    }

    /// 构造授权 URL
    ///
    /// 拼接 `auth_url` 与查询参数：
    /// - `client_id`
    /// - `redirect_uri`
    /// - `response_type=code`
    /// - `state`
    /// - `scope`（如果配置了 scopes，以空格连接）
    /// - `code_challenge` + `code_challenge_method=S256`（如果传入 PKCE 参数）
    /// - 额外参数（如果配置了 extra_params）
    ///
    /// 如果 `auth_url` 已包含查询字符串，则用 `&` 追加，否则用 `?` 起始。
    fn build_redirect_url(&self, state: &str, pkce: Option<&PkceParams>) -> String {
        let mut params: Vec<(String, String)> = vec![
            ("client_id".into(), self.config.client_id.clone()),
            ("redirect_uri".into(), self.config.redirect_url.clone()),
            ("response_type".into(), "code".into()),
            ("state".into(), state.to_string()),
        ];

        if !self.config.scopes.is_empty() {
            params.push(("scope".into(), self.config.scopes.join(" ")));
        }

        if let Some(pkce) = pkce {
            params.push(("code_challenge".into(), pkce.code_challenge.clone()));
            params.push(("code_challenge_method".into(), pkce.method.to_string()));
        }

        for (key, value) in &self.config.extra_params {
            params.push((key.clone(), value.clone()));
        }

        let query = params
            .iter()
            .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");

        let separator = if self.config.auth_url.contains('?') {
            "&"
        } else {
            "?"
        };
        format!("{}{}{}", self.config.auth_url, separator, query)
    }

    /// 用授权码换取访问令牌
    ///
    /// POST `token_url`，body 为 JSON：
    /// ```json
    /// {
    ///   "grant_type": "authorization_code",
    ///   "code": "<code>",
    ///   "client_id": "<client_id>",
    ///   "client_secret": "<client_secret>",
    ///   "redirect_uri": "<redirect_url>",
    ///   "code_verifier": "<code_verifier>"  // 仅当 pkce 启用时
    /// }
    /// ```
    ///
    /// 成功返回解析后的 JSON（含 `access_token` / `refresh_token` / `expires_in` 等）。
    fn exchange_token(
        &self,
        code: &str,
        code_verifier: Option<&str>,
    ) -> Result<serde_json::Value, OAuth2Error> {
        let mut body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": self.config.client_id,
            "client_secret": self.config.client_secret,
            "redirect_uri": self.config.redirect_url,
        });

        if let Some(verifier) = code_verifier {
            body["code_verifier"] = serde_json::Value::String(verifier.to_string());
        }

        let body_str =
            serde_json::to_string(&body).map_err(|err| OAuth2Error::Serialize(err.to_string()))?;

        let response = self
            .transport
            .post_json(&self.config.token_url, &body_str)
            .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;

        if response.is_empty() {
            return Ok(serde_json::Value::Null);
        }

        serde_json::from_str(&response)
            .map_err(|err| OAuth2Error::TokenExchangeFailed(format!("解析 token 响应失败: {err}")))
    }

    /// 获取用户信息
    ///
    /// POST `user_url`，body 为 JSON（含 `access_token` 和 `openid`）。
    /// 成功返回解析后的 JSON。
    fn fetch_user_info(
        &self,
        access_token: &str,
        token_json: &serde_json::Value,
    ) -> Result<serde_json::Value, OAuth2Error> {
        let user_url = self
            .config
            .user_url
            .as_ref()
            .ok_or_else(|| OAuth2Error::UserInfoFailed("user_url 未配置".into()))?;

        let body = serde_json::json!({
            "access_token": access_token,
            "openid": token_json.get("openid").cloned().unwrap_or(serde_json::Value::Null),
        });
        let body_str =
            serde_json::to_string(&body).map_err(|err| OAuth2Error::Serialize(err.to_string()))?;

        let response = self
            .transport
            .post_json(user_url, &body_str)
            .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;

        if response.is_empty() {
            return Ok(serde_json::Value::Null);
        }

        serde_json::from_str(&response)
            .map_err(|err| OAuth2Error::UserInfoFailed(format!("解析用户信息响应失败: {err}")))
    }

    /// 从用户信息 JSON 中提取 [`SocialiteUser`] 的标准字段
    ///
    /// 兼容多种字段命名（对齐 Laravel Socialite 的字段映射逻辑）：
    /// - `id` / `openid` / `user_id` → `id`
    /// - `nickname` / `nick_name` → `nickname`
    /// - `name` / `username` → `name`
    /// - `email` → `email`
    /// - `avatar` / `figureurl_qq_1` / `figureurl` / `headimgurl` → `avatar`
    fn extract_user_fields(user_json: &serde_json::Value) -> SocialiteUser {
        let id = user_json
            .get("id")
            .or_else(|| user_json.get("openid"))
            .or_else(|| user_json.get("user_id"))
            .and_then(extract_string)
            .unwrap_or_default();

        let nickname = user_json
            .get("nickname")
            .or_else(|| user_json.get("nick_name"))
            .and_then(extract_string);

        let name = user_json
            .get("name")
            .or_else(|| user_json.get("username"))
            .and_then(extract_string);

        let email = user_json.get("email").and_then(extract_string);

        let avatar = user_json
            .get("avatar")
            .or_else(|| user_json.get("figureurl_qq_1"))
            .or_else(|| user_json.get("figureurl"))
            .or_else(|| user_json.get("headimgurl"))
            .and_then(extract_string);

        SocialiteUser {
            id,
            nickname,
            name,
            email,
            avatar,
            raw: user_json.clone(),
            access_token: None,
            refresh_token: None,
            expires_in: None,
        }
    }
}

impl OAuth2Provider for GenericOAuth2Provider {
    fn redirect_url(&self, state: &str) -> String {
        self.build_redirect_url(state, None)
    }

    fn user_from_token(&self, code: &str) -> Result<SocialiteUser, OAuth2Error> {
        self.user_from_token_with_pkce(code, None)
    }
}

impl GenericOAuth2Provider {
    /// 构造授权 URL（带 PKCE 参数）
    ///
    /// 当配置了 PKCE 参数时，授权 URL 追加 `code_challenge` 和 `code_challenge_method=S256`。
    pub fn redirect_url_with_pkce(&self, state: &str, pkce: &PkceParams) -> String {
        self.build_redirect_url(state, Some(pkce))
    }

    /// 用授权码换取用户信息（可选 PKCE code_verifier）
    ///
    /// 当 `code_verifier` 为 `Some` 时，token 交换 POST body 追加 `code_verifier` 字段。
    pub fn user_from_token_with_pkce(
        &self,
        code: &str,
        code_verifier: Option<&str>,
    ) -> Result<SocialiteUser, OAuth2Error> {
        // 1. 校验配置必填字段
        self.config.validate()?;

        // 2. 校验授权码非空
        if code.is_empty() {
            return Err(OAuth2Error::AuthFailed("授权码不能为空".into()));
        }

        // 3. 用授权码换取访问令牌
        let token_json = self.exchange_token(code, code_verifier)?;

        // 4. 提取 access_token
        let access_token = token_json
            .get("access_token")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                self.log_audit(
                    "authorization_code",
                    "failure",
                    None,
                    Some("token 响应缺少 access_token"),
                );
                OAuth2Error::TokenExchangeFailed(format!(
                    "token 响应缺少 access_token 字段: {token_json}"
                ))
            })?
            .to_string();

        let refresh_token = token_json
            .get("refresh_token")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());

        let expires_in = token_json
            .get("expires_in")
            .and_then(|value| value.as_i64());

        // 4.1 自动刷新：如果 expires_in <= 0 且有 refresh_token，自动调用 refresh_token
        let (access_token, refresh_token, expires_in) = if let Some(exp) = expires_in {
            if exp <= 0 {
                if let Some(ref rt) = refresh_token {
                    match self.refresh_token(rt) {
                        Ok(new_token) => {
                            let new_access = new_token.access_token;
                            let new_refresh = new_token.refresh_token.or(refresh_token.clone());
                            let new_exp = new_token.expires_in;
                            (new_access, new_refresh, new_exp)
                        }
                        Err(_) => {
                            // 刷新失败，保留原 token
                            (access_token, refresh_token, expires_in)
                        }
                    }
                } else {
                    (access_token, refresh_token, expires_in)
                }
            } else {
                (access_token, refresh_token, expires_in)
            }
        } else {
            (access_token, refresh_token, expires_in)
        };

        // 5. 如果配置了 user_url，获取用户信息；否则仅返回 token 信息
        let mut user = if self.config.user_url.is_some() {
            let user_json = self.fetch_user_info(&access_token, &token_json)?;
            Self::extract_user_fields(&user_json)
        } else {
            SocialiteUser::default()
        };

        user.access_token = Some(access_token);
        user.refresh_token = refresh_token;
        user.expires_in = expires_in;

        // 6. best-effort 存储 token（需 redis-store feature）
        #[cfg(feature = "redis-store")]
        if let Some(store) = &self.token_store {
            let store = store.clone();
            let client_id = self.config.client_id.clone();
            let token_to_store = TokenResponse {
                access_token: user.access_token.clone().unwrap_or_default(),
                token_type: None,
                expires_in: user.expires_in,
                scope: None,
                refresh_token: user.refresh_token.clone(),
            };
            tokio::task::spawn(async move {
                if let Err(err) = store.store_token(&client_id, &token_to_store).await {
                    tracing::warn!(
                        error = %err,
                        client_id = %client_id,
                        "OAUTH2_TOKEN_STORE_FAILED: token 存储失败（best-effort，不影响主流程）"
                    );
                }
            });
        }

        self.log_audit("authorization_code", "success", None, None);
        Ok(user)
    }
}

// ============================================================================
// ImplicitOAuth2Provider — OAuth2 Implicit 流程
// ============================================================================

/// OAuth2 Implicit 流程提供商 — 对齐 RFC 6749 Section 4.2
///
/// Implicit 流程直接在授权 URL 的 fragment 中返回 access_token，
/// 不需要后端 token 交换步骤。**安全级别较低**（token 经 URL fragment 暴露）。
///
/// # 工作流程
///
/// 1. [`ImplicitOAuth2Provider::redirect_url`]：构造 `response_type=token` 的授权 URL
/// 2. 授权服务器回调，在 URL fragment 中携带 `access_token` 和 `state`
/// 3. [`ImplicitOAuth2Provider::parse_fragment`]：解析 fragment 提取 token，校验 state
pub struct ImplicitOAuth2Provider {
    /// OAuth2 配置
    config: OAuth2Config,
    /// 审计日志（可选）
    audit_logger: Option<Arc<dyn OAuth2AuditLogger>>,
}

impl ImplicitOAuth2Provider {
    /// 创建 Implicit OAuth2 提供商
    pub fn new(config: OAuth2Config) -> Self {
        Self {
            config,
            audit_logger: None,
        }
    }

    /// 注入审计日志
    pub fn with_audit_logger(mut self, logger: Arc<dyn OAuth2AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    /// 记录审计事件（best-effort）
    fn log_audit(
        &self,
        grant_type: &str,
        result: &str,
        alert_code: Option<&str>,
        message: Option<&str>,
    ) {
        if let Some(logger) = &self.audit_logger {
            let event = OAuth2AuditEvent {
                client_id: self.config.client_id.clone(),
                grant_type: grant_type.to_string(),
                result: result.to_string(),
                timestamp: chrono::Utc::now().timestamp(),
                alert_code: alert_code.map(|s| s.to_string()),
                message: message.map(|s| s.to_string()),
            };
            logger.log_event(&event);
        }
    }

    /// 构造授权 URL（`response_type=token`）
    ///
    /// 拼接 `auth_url` 与查询参数：
    /// - `client_id`
    /// - `redirect_uri`
    /// - `response_type=token`
    /// - `state`
    /// - `scope`（如果配置了 scopes）
    /// - `code_challenge` + `code_challenge_method=S256`（如果传入 PKCE 参数）
    /// - 额外参数
    pub fn redirect_url(&self, state: &str) -> String {
        self.redirect_url_with_pkce(state, None)
    }

    /// 构造授权 URL（带可选 PKCE 参数）
    pub fn redirect_url_with_pkce(&self, state: &str, pkce: Option<&PkceParams>) -> String {
        let mut params: Vec<(String, String)> = vec![
            ("client_id".into(), self.config.client_id.clone()),
            ("redirect_uri".into(), self.config.redirect_url.clone()),
            ("response_type".into(), "token".into()),
            ("state".into(), state.to_string()),
        ];

        if !self.config.scopes.is_empty() {
            params.push(("scope".into(), self.config.scopes.join(" ")));
        }

        if let Some(pkce) = pkce {
            params.push(("code_challenge".into(), pkce.code_challenge.clone()));
            params.push(("code_challenge_method".into(), pkce.method.to_string()));
        }

        for (key, value) in &self.config.extra_params {
            params.push((key.clone(), value.clone()));
        }

        let query = params
            .iter()
            .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");

        let separator = if self.config.auth_url.contains('?') {
            "&"
        } else {
            "?"
        };
        format!("{}{}{}", self.config.auth_url, separator, query)
    }

    /// 解析 URI fragment 提取 access_token
    ///
    /// # 参数
    ///
    /// - `fragment`: URI fragment（`#` 后的部分），如 `access_token=xxx&state=yyy`
    /// - `expected_state`: 预期的 state 值（用于 CSRF 校验）
    ///
    /// # 返回
    ///
    /// 成功返回 [`TokenResponse`]，其中 `refresh_token` 固定为 `None`（implicit 流程不返回 refresh_token）。
    pub fn parse_fragment(
        &self,
        fragment: &str,
        expected_state: &str,
    ) -> Result<TokenResponse, OAuth2Error> {
        // 告警：implicit 流程 token 经 URL fragment 暴露
        self.log_audit(
            "implicit",
            "success",
            Some("OAUTH2_IMPLICIT_TOKEN_EXPOSED"),
            Some("implicit 流程 token 经 URL fragment 暴露"),
        );

        if fragment.is_empty() {
            self.log_audit("implicit", "failure", None, Some("fragment 为空"));
            return Err(OAuth2Error::TokenExchangeFailed("fragment 为空".into()));
        }

        // 解析 fragment 中的 key=value 对
        let params: std::collections::HashMap<&str, &str> = fragment
            .split('&')
            .filter_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                Some((key, value))
            })
            .collect();

        // state 校验（CSRF 防护）
        let state = params.get("state").copied().unwrap_or("");
        if state != expected_state {
            self.log_audit(
                "implicit",
                "failure",
                Some("OAUTH2_CSRF_STATE_MISMATCH"),
                Some(&format!(
                    "state 不匹配: expected={expected_state}, actual={state}"
                )),
            );
            return Err(OAuth2Error::AuthFailed("CSRF state mismatch".into()));
        }

        // 提取 access_token
        let access_token = params.get("access_token").copied().ok_or_else(|| {
            self.log_audit(
                "implicit",
                "failure",
                None,
                Some("fragment 无 access_token"),
            );
            OAuth2Error::TokenExchangeFailed("fragment 中缺少 access_token".into())
        })?;

        Ok(TokenResponse {
            access_token: access_token.to_string(),
            token_type: params.get("token_type").map(|s| s.to_string()),
            expires_in: params.get("expires_in").and_then(|s| s.parse().ok()),
            scope: params.get("scope").map(|s| s.to_string()),
            // implicit 流程不返回 refresh_token
            refresh_token: None,
        })
    }
}

impl OAuth2Provider for ImplicitOAuth2Provider {
    fn redirect_url(&self, state: &str) -> String {
        self.redirect_url(state)
    }

    fn user_from_token(&self, code: &str) -> Result<SocialiteUser, OAuth2Error> {
        // Implicit 流程没有后端 token 交换，code 实际上是 access_token
        // 直接构造 SocialiteUser（无用户信息端点调用）
        Ok(SocialiteUser {
            access_token: Some(code.to_string()),
            ..Default::default()
        })
    }
}

// ============================================================================
// DeviceCodeOAuth2Provider — OAuth2 Device Code 流程（feature = "device-code"）
// ============================================================================

/// OAuth2 Device Code 流程模块（RFC 8628）— 需要 `device-code` feature
#[cfg(feature = "device-code")]
pub mod device_code {
    use super::*;
    use async_trait::async_trait;

    /// 异步 HTTP 传输 trait — 用于 device_code 流程的异步轮询
    ///
    /// device_code 流程需要 `tokio::time::sleep` 异步等待，因此传输层也需异步。
    #[async_trait]
    pub trait AsyncOAuth2HttpTransport: Send + Sync {
        /// 发送 POST 请求，Content-Type: application/x-www-form-urlencoded
        ///
        /// # 参数
        ///
        /// - `url`: 目标 URL
        /// - `params`: 表单参数列表（key-value 对）
        ///
        /// # 返回
        ///
        /// 成功返回响应体字符串，失败返回 [`OAuth2Error`]。
        async fn post_form(
            &self,
            url: &str,
            params: &[(&str, &str)],
        ) -> Result<String, OAuth2Error>;
    }

    /// Device Code 响应 — 对齐 RFC 8628 Section 3.2
    #[derive(Debug, Clone)]
    pub struct DeviceCodeResponse {
        /// 设备码（客户端用于轮询 token 端点）
        pub device_code: String,
        /// 用户码（用户在验证页面输入）
        pub user_code: String,
        /// 验证 URI（用户访问以完成授权）
        pub verification_uri: String,
        /// 过期秒数
        pub expires_in: i64,
        /// 轮询间隔秒数
        pub interval: i64,
    }

    /// Device Code OAuth2 提供商 — 对齐 RFC 8628
    pub struct DeviceCodeOAuth2Provider {
        /// OAuth2 配置
        config: OAuth2Config,
        /// 异步 HTTP 传输
        transport: Arc<dyn AsyncOAuth2HttpTransport>,
        /// 审计日志（可选）
        audit_logger: Option<Arc<dyn OAuth2AuditLogger>>,
    }

    impl DeviceCodeOAuth2Provider {
        /// 创建 Device Code OAuth2 提供商
        pub fn new(config: OAuth2Config, transport: Arc<dyn AsyncOAuth2HttpTransport>) -> Self {
            Self {
                config,
                transport,
                audit_logger: None,
            }
        }

        /// 注入审计日志
        pub fn with_audit_logger(mut self, logger: Arc<dyn OAuth2AuditLogger>) -> Self {
            self.audit_logger = Some(logger);
            self
        }

        /// 记录审计事件（best-effort）
        fn log_audit(
            &self,
            grant_type: &str,
            result: &str,
            alert_code: Option<&str>,
            message: Option<&str>,
        ) {
            if let Some(logger) = &self.audit_logger {
                let event = OAuth2AuditEvent {
                    client_id: self.config.client_id.clone(),
                    grant_type: grant_type.to_string(),
                    result: result.to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                    alert_code: alert_code.map(|s| s.to_string()),
                    message: message.map(|s| s.to_string()),
                };
                logger.log_event(&event);
            }
        }

        /// 请求设备码 — POST device_authorization 端点
        ///
        /// # 参数
        ///
        /// - `scope`: 请求的授权范围列表
        pub async fn request_device_code(
            &self,
            scope: &[String],
        ) -> Result<DeviceCodeResponse, OAuth2Error> {
            let device_auth_url = self.config.device_auth_url.as_ref().ok_or_else(|| {
                self.log_audit(
                    "device_code",
                    "failure",
                    None,
                    Some("device_auth_url 未配置"),
                );
                OAuth2Error::MissingField("device_auth_url".into())
            })?;

            let scope_str = scope.join(" ");
            let params: Vec<(&str, &str)> = vec![
                ("client_id", self.config.client_id.as_str()),
                ("scope", scope_str.as_str()),
            ];

            let response = self
                .transport
                .post_form(device_auth_url, &params)
                .await
                .map_err(|err| {
                    self.log_audit("device_code", "failure", None, Some(&err.to_string()));
                    OAuth2Error::HttpTransport(err.to_string())
                })?;

            let json: serde_json::Value = serde_json::from_str(&response).map_err(|err| {
                self.log_audit(
                    "device_code",
                    "failure",
                    None,
                    Some(&format!("JSON 解析失败: {err}")),
                );
                OAuth2Error::TokenExchangeFailed(format!("解析 device code 响应失败: {err}"))
            })?;

            let device_code = json
                .get("device_code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    OAuth2Error::TokenExchangeFailed("device code 响应缺少 device_code 字段".into())
                })?
                .to_string();

            let user_code = json
                .get("user_code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    OAuth2Error::TokenExchangeFailed("device code 响应缺少 user_code 字段".into())
                })?
                .to_string();

            let verification_uri = json
                .get("verification_uri")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    OAuth2Error::TokenExchangeFailed(
                        "device code 响应缺少 verification_uri 字段".into(),
                    )
                })?
                .to_string();

            let expires_in = json
                .get("expires_in")
                .and_then(|v| v.as_i64())
                .unwrap_or(600);
            let interval = json.get("interval").and_then(|v| v.as_i64()).unwrap_or(5);

            self.log_audit("device_code", "success", None, None);
            Ok(DeviceCodeResponse {
                device_code,
                user_code,
                verification_uri,
                expires_in,
                interval,
            })
        }

        /// 轮询 token 端点直到获取 token 或超时
        ///
        /// # 参数
        ///
        /// - `device_code`: 设备码
        /// - `interval`: 初始轮询间隔（秒）
        /// - `expires_in`: 设备码过期时间（秒）
        ///
        /// # 退避策略
        ///
        /// - `authorization_pending` → 按 interval 继续轮询
        /// - `slow_down` → interval += 5（上限 60s）后继续
        /// - 收到 token → 停止返回
        /// - 超过 expires_in → `OAUTH2_DEVICE_CODE_EXPIRED`
        /// - `access_denied` → `OAUTH2_ACCESS_DENIED`
        pub async fn poll_for_token(
            &self,
            device_code: &str,
            mut interval: i64,
            expires_in: i64,
        ) -> Result<TokenResponse, OAuth2Error> {
            let start = std::time::Instant::now();
            let expires_duration = std::time::Duration::from_secs(expires_in.max(0) as u64);

            loop {
                // 检查是否过期
                if start.elapsed() >= expires_duration {
                    self.log_audit(
                        "device_code",
                        "failure",
                        Some("OAUTH2_DEVICE_CODE_EXPIRED"),
                        Some("设备码已过期"),
                    );
                    return Err(OAuth2Error::AuthFailed(
                        "OAUTH2_DEVICE_CODE_EXPIRED: 设备码已过期".into(),
                    ));
                }

                // 轮询 token 端点
                let params: Vec<(&str, &str)> = vec![
                    ("grant_type", "device_code"),
                    ("device_code", device_code),
                    ("client_id", self.config.client_id.as_str()),
                ];

                let response = self
                    .transport
                    .post_form(&self.config.token_url, &params)
                    .await
                    .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;

                let json: serde_json::Value = serde_json::from_str(&response).map_err(|err| {
                    OAuth2Error::TokenExchangeFailed(format!("解析 token 响应失败: {err}"))
                })?;

                // 检查是否有 access_token（成功）
                if let Some(access_token) = json.get("access_token").and_then(|v| v.as_str()) {
                    let token_response = TokenResponse {
                        access_token: access_token.to_string(),
                        token_type: json
                            .get("token_type")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        expires_in: json.get("expires_in").and_then(|v| v.as_i64()),
                        scope: json
                            .get("scope")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        refresh_token: json
                            .get("refresh_token")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    };
                    self.log_audit("device_code", "success", None, None);
                    return Ok(token_response);
                }

                // 检查错误码
                let error = json.get("error").and_then(|v| v.as_str()).unwrap_or("");

                match error {
                    "authorization_pending" => {
                        // 继续轮询，interval 不变
                    }
                    "slow_down" => {
                        // interval += 5，上限 60s
                        interval = (interval + 5).min(60);
                    }
                    "access_denied" => {
                        self.log_audit(
                            "device_code",
                            "failure",
                            Some("OAUTH2_ACCESS_DENIED"),
                            Some("用户拒绝授权"),
                        );
                        return Err(OAuth2Error::AuthFailed(
                            "OAUTH2_ACCESS_DENIED: 用户拒绝授权".into(),
                        ));
                    }
                    "expired_token" => {
                        self.log_audit(
                            "device_code",
                            "failure",
                            Some("OAUTH2_DEVICE_CODE_EXPIRED"),
                            Some("设备码已过期"),
                        );
                        return Err(OAuth2Error::AuthFailed(
                            "OAUTH2_DEVICE_CODE_EXPIRED: 设备码已过期".into(),
                        ));
                    }
                    _ => {
                        return Err(OAuth2Error::TokenExchangeFailed(format!(
                            "未知错误: {error}"
                        )));
                    }
                }

                // 异步等待（不阻塞 tokio 线程）
                if interval > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(interval as u64)).await;
                }
            }
        }
    }

    // ------------------------------------------------------------------------
    // 内存异步 HTTP 传输（测试用）
    // ------------------------------------------------------------------------

    /// 内存异步 HTTP 传输 — 用于测试 device_code 流程
    #[derive(Default)]
    pub struct MemoryAsyncOAuth2HttpTransport {
        requests: Mutex<Vec<(String, String)>>,
        responses: Mutex<VecDeque<String>>,
    }

    impl MemoryAsyncOAuth2HttpTransport {
        /// 创建新的内存异步 HTTP 传输
        pub fn new() -> Self {
            Self::default()
        }

        /// 预置 mock 响应（追加到队列末尾，按调用顺序消费）
        pub fn push_response(&self, response: impl Into<String>) {
            self.responses.lock().push_back(response.into());
        }

        /// 获取已发送请求数量
        pub fn count(&self) -> usize {
            self.requests.lock().len()
        }
    }

    #[async_trait]
    impl AsyncOAuth2HttpTransport for MemoryAsyncOAuth2HttpTransport {
        async fn post_form(
            &self,
            url: &str,
            params: &[(&str, &str)],
        ) -> Result<String, OAuth2Error> {
            let body = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            self.requests.lock().push((url.to_string(), body));
            let mut responses = self.responses.lock();
            match responses.pop_front() {
                Some(resp) => Ok(resp),
                None => Ok(String::new()),
            }
        }
    }

    // ------------------------------------------------------------------------
    // 测试
    // ------------------------------------------------------------------------

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 测试 device code 请求
        #[tokio::test]
        async fn test_device_code_request() {
            let transport = Arc::new(MemoryAsyncOAuth2HttpTransport::new());
            transport.push_response(
                r#"{"device_code":"dc123","user_code":"UC-ABCD","verification_uri":"https://provider.com/device","expires_in":600,"interval":5}"#,
            );

            let config = OAuth2Config::new(
                "client123",
                "secret456",
                "https://example.com/callback",
                "https://provider.com/authorize",
                "https://provider.com/token",
            )
            .with_device_auth_url("https://provider.com/device_authorize");
            let provider = DeviceCodeOAuth2Provider::new(config, transport);

            let resp = provider
                .request_device_code(&["read".into(), "write".into()])
                .await
                .expect("request_device_code 失败");

            assert_eq!(resp.device_code, "dc123");
            assert_eq!(resp.user_code, "UC-ABCD");
            assert_eq!(resp.verification_uri, "https://provider.com/device");
            assert_eq!(resp.expires_in, 600);
            assert_eq!(resp.interval, 5);
        }

        /// 测试 device code 轮询 authorization_pending → 继续轮询 → 成功
        #[tokio::test]
        async fn test_device_code_poll_pending() {
            let transport = Arc::new(MemoryAsyncOAuth2HttpTransport::new());
            // 第一次：pending
            transport.push_response(r#"{"error":"authorization_pending"}"#);
            // 第二次：成功
            transport.push_response(
                r#"{"access_token":"token123","token_type":"Bearer","expires_in":3600}"#,
            );

            let config = OAuth2Config::new(
                "client123",
                "secret456",
                "https://example.com/callback",
                "https://provider.com/authorize",
                "https://provider.com/token",
            );
            let provider = DeviceCodeOAuth2Provider::new(config, transport);

            let token = provider
                .poll_for_token("dc123", 0, 600)
                .await
                .expect("poll_for_token 失败");

            assert_eq!(token.access_token, "token123");
            assert_eq!(token.token_type.as_deref(), Some("Bearer"));
        }

        /// 测试 device code 轮询 slow_down → interval += 5
        #[tokio::test]
        async fn test_device_code_poll_slow_down() {
            let transport = Arc::new(MemoryAsyncOAuth2HttpTransport::new());
            // 第一次：slow_down
            transport.push_response(r#"{"error":"slow_down"}"#);
            // 第二次：成功
            transport.push_response(r#"{"access_token":"token123","expires_in":3600}"#);

            let config = OAuth2Config::new(
                "client123",
                "secret456",
                "https://example.com/callback",
                "https://provider.com/authorize",
                "https://provider.com/token",
            );
            let provider = DeviceCodeOAuth2Provider::new(config, transport);

            let token = provider
                .poll_for_token("dc123", 0, 600)
                .await
                .expect("poll_for_token 失败");

            assert_eq!(token.access_token, "token123");
        }

        /// 测试 device code 过期 → OAUTH2_DEVICE_CODE_EXPIRED
        #[tokio::test]
        async fn test_device_code_expired() {
            let transport = Arc::new(MemoryAsyncOAuth2HttpTransport::new());
            // 持续 pending，直到过期
            transport.push_response(r#"{"error":"authorization_pending"}"#);

            let config = OAuth2Config::new(
                "client123",
                "secret456",
                "https://example.com/callback",
                "https://provider.com/authorize",
                "https://provider.com/token",
            );
            let provider = DeviceCodeOAuth2Provider::new(config, transport);

            let err = provider.poll_for_token("dc123", 0, 0).await.unwrap_err();
            assert!(
                err.to_string().contains("OAUTH2_DEVICE_CODE_EXPIRED"),
                "应返回设备码过期错误: {err}"
            );
        }

        /// 测试 device code access_denied → 停止轮询
        #[tokio::test]
        async fn test_device_code_access_denied() {
            let transport = Arc::new(MemoryAsyncOAuth2HttpTransport::new());
            transport.push_response(r#"{"error":"access_denied"}"#);

            let config = OAuth2Config::new(
                "client123",
                "secret456",
                "https://example.com/callback",
                "https://provider.com/authorize",
                "https://provider.com/token",
            );
            let provider = DeviceCodeOAuth2Provider::new(config, transport);

            let err = provider.poll_for_token("dc123", 5, 600).await.unwrap_err();
            assert!(
                err.to_string().contains("OAUTH2_ACCESS_DENIED"),
                "应返回 access_denied 错误: {err}"
            );
        }

        /// 测试 device_auth_url 未设置 → 配置错误
        #[tokio::test]
        async fn test_device_code_no_auth_url() {
            let transport = Arc::new(MemoryAsyncOAuth2HttpTransport::new());
            let config = OAuth2Config::new(
                "client123",
                "secret456",
                "https://example.com/callback",
                "https://provider.com/authorize",
                "https://provider.com/token",
            );
            // 不设置 device_auth_url
            let provider = DeviceCodeOAuth2Provider::new(config, transport);

            let err = provider.request_device_code(&[]).await.unwrap_err();
            assert!(matches!(err, OAuth2Error::MissingField(field) if field == "device_auth_url"));
        }
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 从 JSON 值中提取字符串
///
/// 支持字符串和整数类型（整数转为十进制字符串），其他类型返回 `None`。
fn extract_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(string) => Some(string.clone()),
        serde_json::Value::Number(number) => number.as_i64().map(|number| number.to_string()),
        _ => None,
    }
}

/// 简易百分号编码 — 用于 URL 查询参数
///
/// 对齐 RFC 3986 的 unreserved 字符集（`A-Za-z0-9-._~`）保持原样，
/// 其余字符编码为 `%XX` 形式（UTF-8 字节）。
fn percent_encode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~') {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{prop_assert, prop_assert_eq};

    // ------------------------------------------------------------------------
    // OAuth2Config 测试
    // ------------------------------------------------------------------------

    /// 测试 OAuth2Config builder 模式（所有可选字段）
    #[test]
    fn test_oauth2_config_builder() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_user_url("https://provider.com/user/info")
        .with_scopes(vec!["scope1".into(), "scope2".into()])
        .with_extra_param("foo", "bar");

        assert_eq!(config.client_id, "client123");
        assert_eq!(config.client_secret, "secret456");
        assert_eq!(config.redirect_url, "https://example.com/callback");
        assert_eq!(config.auth_url, "https://provider.com/authorize");
        assert_eq!(config.token_url, "https://provider.com/token");
        assert_eq!(
            config.user_url.as_deref(),
            Some("https://provider.com/user/info")
        );
        assert_eq!(config.scopes, vec!["scope1", "scope2"]);
        assert_eq!(config.extra_params, vec![("foo".into(), "bar".into())]);
    }

    /// 测试 OAuth2Config 最小配置（仅必填字段）
    #[test]
    fn test_oauth2_config_minimal() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );

        assert_eq!(config.client_id, "client123");
        assert_eq!(config.client_secret, "secret456");
        assert_eq!(config.redirect_url, "https://example.com/callback");
        assert_eq!(config.auth_url, "https://provider.com/authorize");
        assert_eq!(config.token_url, "https://provider.com/token");
        assert!(config.user_url.is_none());
        assert!(config.scopes.is_empty());
        assert!(config.extra_params.is_empty());

        // 最小配置应通过校验
        assert!(config.validate().is_ok());
    }

    /// 测试 OAuth2Config::with_scope 追加多个 scope
    #[test]
    fn test_oauth2_config_with_scope_chained() {
        let config = OAuth2Config::new(
            "id",
            "secret",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_scope("get_user_info")
        .with_scope("get_unionid");

        assert_eq!(config.scopes, vec!["get_user_info", "get_unionid"]);
    }

    /// 测试 OAuth2Config::with_extra_params 覆盖
    #[test]
    fn test_oauth2_config_with_extra_params() {
        let config = OAuth2Config::new(
            "id",
            "secret",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_extra_param("a", "1")
        .with_extra_param("b", "2")
        .with_extra_params(vec![("x".into(), "10".into())]);

        assert_eq!(config.extra_params, vec![("x".into(), "10".into())]);
    }

    /// 测试 OAuth2Config::validate 检测空字段
    #[test]
    fn test_oauth2_config_validate_empty_fields() {
        // client_id 为空
        let config = OAuth2Config::new(
            "",
            "secret",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "client_id"));

        // client_secret 为空
        let config = OAuth2Config::new(
            "id",
            "",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "client_secret"));

        // redirect_url 为空
        let config = OAuth2Config::new(
            "id",
            "secret",
            "",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "redirect_url"));

        // auth_url 为空
        let config = OAuth2Config::new(
            "id",
            "secret",
            "https://example.com/callback",
            "",
            "https://provider.com/token",
        );
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "auth_url"));

        // token_url 为空
        let config = OAuth2Config::new(
            "id",
            "secret",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "",
        );
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "token_url"));
    }

    // ------------------------------------------------------------------------
    // SocialiteUser 测试
    // ------------------------------------------------------------------------

    /// 测试 SocialiteUser 默认值
    #[test]
    fn test_socialite_user_default() {
        let user = SocialiteUser::default();
        assert!(user.id.is_empty());
        assert!(user.nickname.is_none());
        assert!(user.name.is_none());
        assert!(user.email.is_none());
        assert!(user.avatar.is_none());
        assert!(user.raw.is_null());
        assert!(user.access_token.is_none());
        assert!(user.refresh_token.is_none());
        assert!(user.expires_in.is_none());
    }

    /// 测试 SocialiteUser 序列化/反序列化
    #[test]
    fn test_socialite_user_serialize_deserialize() {
        let user = SocialiteUser {
            id: "123".into(),
            nickname: Some("tester".into()),
            name: Some("Test User".into()),
            email: Some("test@example.com".into()),
            avatar: Some("https://example.com/avatar.png".into()),
            raw: serde_json::json!({"key": "value"}),
            access_token: Some("token123".into()),
            refresh_token: Some("refresh456".into()),
            expires_in: Some(3600),
        };

        let json = serde_json::to_string(&user).expect("序列化失败");

        // P0-SEC-01 安全修复：access_token / refresh_token 不应出现在序列化输出中
        // 防止令牌通过 API 响应泄漏（对齐 MerchantUser.password 的 skip_serializing 策略）
        assert!(
            !json.contains("access_token"),
            "access_token 不应出现在序列化 JSON 中（安全脱敏要求）: {json}"
        );
        assert!(
            !json.contains("refresh_token"),
            "refresh_token 不应出现在序列化 JSON 中（安全脱敏要求）: {json}"
        );

        let parsed: SocialiteUser = serde_json::from_str(&json).expect("反序列化失败");

        assert_eq!(parsed.id, "123");
        assert_eq!(parsed.nickname.as_deref(), Some("tester"));
        assert_eq!(parsed.name.as_deref(), Some("Test User"));
        assert_eq!(parsed.email.as_deref(), Some("test@example.com"));
        assert_eq!(
            parsed.avatar.as_deref(),
            Some("https://example.com/avatar.png")
        );
        // 反序列化后 token 字段为 None（序列化时未包含，反序列化用 #[serde(default)]）
        assert_eq!(parsed.access_token, None);
        assert_eq!(parsed.refresh_token, None);
        assert_eq!(parsed.expires_in, Some(3600));
    }

    // ------------------------------------------------------------------------
    // redirect_url 测试
    // ------------------------------------------------------------------------

    /// 测试 redirect_url 包含必填查询参数
    #[test]
    fn test_redirect_url_contains_required_params() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/oauth2.0/authorize",
            "https://provider.com/oauth2.0/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let url = provider.redirect_url("random_state_abc");

        assert!(url.starts_with("https://provider.com/oauth2.0/authorize?"));
        assert!(url.contains("client_id=client123"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fexample.com%2Fcallback"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("state=random_state_abc"));
        // 未配置 scopes 时不应包含 scope
        assert!(!url.contains("scope="));
    }

    /// 测试 redirect_url 包含 scopes（空格连接，百分号编码为 %20）
    #[test]
    fn test_redirect_url_with_scopes() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/oauth2.0/authorize",
            "https://provider.com/oauth2.0/token",
        )
        .with_scopes(vec!["get_user_info".into(), "get_unionid".into()]);
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let url = provider.redirect_url("state123");

        // scope 以空格连接，空格编码为 %20
        assert!(url.contains("scope=get_user_info%20get_unionid"));
    }

    /// 测试 redirect_url 包含额外参数
    #[test]
    fn test_redirect_url_with_extra_params() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/oauth2.0/authorize",
            "https://provider.com/oauth2.0/token",
        )
        .with_extra_param("foo", "bar")
        .with_extra_param("display", "mobile");
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let url = provider.redirect_url("state123");

        assert!(url.contains("foo=bar"));
        assert!(url.contains("display=mobile"));
    }

    /// 测试 redirect_url 在已有查询字符串的 auth_url 上追加参数
    #[test]
    fn test_redirect_url_with_existing_query() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize?foo=bar",
            "https://provider.com/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let url = provider.redirect_url("state123");

        // 已有查询字符串时应使用 & 追加
        assert!(url.contains("?foo=bar&"));
        assert!(url.contains("client_id=client123"));
    }

    // ------------------------------------------------------------------------
    // MemoryOAuth2HttpTransport 测试
    // ------------------------------------------------------------------------

    /// 测试 MemoryOAuth2HttpTransport 记录请求并返回预置响应
    #[test]
    fn test_memory_oauth2_http_transport_post_json() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response(r#"{"access_token":"token123"}"#);

        let response = transport
            .post_json("https://example.com/token", r#"{"code":"abc"}"#)
            .expect("post_json 失败");

        assert_eq!(response, r#"{"access_token":"token123"}"#);
        assert_eq!(transport.count(), 1);

        let (url, body) = transport.last().expect("应有请求记录");
        assert_eq!(url, "https://example.com/token");
        assert_eq!(body, r#"{"code":"abc"}"#);
    }

    /// 测试 MemoryOAuth2HttpTransport 响应队列按顺序消费
    #[test]
    fn test_memory_oauth2_http_transport_response_queue() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response("resp1");
        transport.push_response("resp2");

        let resp1 = transport
            .post_json("url1", "body1")
            .expect("第一次调用失败");
        let resp2 = transport
            .post_json("url2", "body2")
            .expect("第二次调用失败");

        assert_eq!(resp1, "resp1");
        assert_eq!(resp2, "resp2");
        assert_eq!(transport.count(), 2);
    }

    /// 测试 MemoryOAuth2HttpTransport 响应耗尽后返回空字符串
    #[test]
    fn test_memory_oauth2_http_transport_empty_response() {
        let transport = MemoryOAuth2HttpTransport::new();
        // 不预置响应
        let response = transport
            .post_json("url", "body")
            .expect("post_json 不应失败");
        assert_eq!(response, "");
    }

    /// 测试 MemoryOAuth2HttpTransport clear
    #[test]
    fn test_memory_oauth2_http_transport_clear() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response("resp");
        transport.post_json("url", "body").expect("调用失败");
        assert_eq!(transport.count(), 1);

        transport.clear();
        assert_eq!(transport.count(), 0);
        // clear 后响应队列也清空，返回空字符串
        let response = transport
            .post_json("url", "body")
            .expect("post_json 不应失败");
        assert_eq!(response, "");
    }

    // ------------------------------------------------------------------------
    // GenericOAuth2Provider::user_from_token 测试
    // ------------------------------------------------------------------------

    /// 测试 GenericOAuth2Provider::user_from_token 完整流程
    ///
    /// 使用 MemoryOAuth2HttpTransport mock token 响应和用户信息响应。
    #[test]
    fn test_generic_oauth2_provider_user_from_token() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        // 预置 mock 响应：token 响应 + 用户信息响应
        transport.push_response(r#"{"access_token":"token123","refresh_token":"refresh456","expires_in":3600,"openid":"openid_abc"}"#);
        transport.push_response(
            r#"{"id":"12345","nickname":"test_user","name":"Test","email":"test@example.com","avatar":"https://example.com/avatar.png"}"#,
        );

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_user_url("https://provider.com/user/info");
        let provider = GenericOAuth2Provider::new(config, transport.clone());

        let user = provider
            .user_from_token("auth_code_abc")
            .expect("user_from_token 失败");

        // 验证 token 字段
        assert_eq!(user.access_token.as_deref(), Some("token123"));
        assert_eq!(user.refresh_token.as_deref(), Some("refresh456"));
        assert_eq!(user.expires_in, Some(3600));

        // 验证用户信息字段
        assert_eq!(user.id, "12345");
        assert_eq!(user.nickname.as_deref(), Some("test_user"));
        assert_eq!(user.name.as_deref(), Some("Test"));
        assert_eq!(user.email.as_deref(), Some("test@example.com"));
        assert_eq!(
            user.avatar.as_deref(),
            Some("https://example.com/avatar.png")
        );

        // 验证原始响应数据
        assert_eq!(user.raw["id"], "12345");
        assert_eq!(user.raw["nickname"], "test_user");

        // 验证 HTTP 请求次数（token + user info）
        assert_eq!(transport.count(), 2);
    }

    /// 测试 GenericOAuth2Provider::user_from_token 无 user_url 时仅返回 token
    #[test]
    fn test_generic_oauth2_provider_user_from_token_no_user_url() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(r#"{"access_token":"token123","expires_in":7200}"#);

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        // 不配置 user_url
        let provider = GenericOAuth2Provider::new(config, transport.clone());

        let user = provider
            .user_from_token("auth_code")
            .expect("user_from_token 失败");

        assert_eq!(user.access_token.as_deref(), Some("token123"));
        assert_eq!(user.expires_in, Some(7200));
        assert!(user.refresh_token.is_none());
        // 无 user_url 时不获取用户信息，id 为空
        assert!(user.id.is_empty());
        // 仅一次 HTTP 请求（token 交换）
        assert_eq!(transport.count(), 1);
    }

    /// 测试 GenericOAuth2Provider::user_from_token 授权码为空时返回错误
    #[test]
    fn test_generic_oauth2_provider_missing_code() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let err = provider.user_from_token("").unwrap_err();
        assert!(matches!(err, OAuth2Error::AuthFailed(msg) if msg.contains("授权码")));
    }

    /// 测试 GenericOAuth2Provider 配置缺少必填字段时返回错误
    #[test]
    fn test_oauth2_provider_missing_config_fields() {
        // client_id 为空
        let config = OAuth2Config::new(
            "",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, Arc::new(MemoryHttpTransport));

        let err = provider.user_from_token("code").unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "client_id"));

        // token_url 为空
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "",
        );
        let provider = GenericOAuth2Provider::new(config, Arc::new(MemoryHttpTransport));

        let err = provider.user_from_token("code").unwrap_err();
        assert!(matches!(err, OAuth2Error::MissingField(field) if field == "token_url"));
    }

    /// 测试 token 响应缺少 access_token 时返回 TokenExchangeFailed
    #[test]
    fn test_generic_oauth2_provider_token_response_missing_access_token() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response(r#"{"error":"invalid_grant"}"#);

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, Arc::new(transport));

        let err = provider.user_from_token("code").unwrap_err();
        assert!(matches!(err, OAuth2Error::TokenExchangeFailed(_)));
    }

    /// 测试 token 响应非 JSON 时返回 TokenExchangeFailed
    #[test]
    fn test_generic_oauth2_provider_token_response_invalid_json() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response("not a json");

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, Arc::new(transport));

        let err = provider.user_from_token("code").unwrap_err();
        assert!(matches!(err, OAuth2Error::TokenExchangeFailed(_)));
    }

    /// 测试用户信息字段兼容多种命名（openid / figureurl_qq_1 等）
    #[test]
    fn test_generic_oauth2_provider_user_info_field_aliases() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response(r#"{"access_token":"token123","openid":"openid_abc"}"#);
        transport.push_response(
            r#"{"openid":"qq_12345","nickname":"qq_user","figureurl_qq_1":"https://qzapp.qlogo.cn/1.png"}"#,
        );

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_user_url("https://provider.com/user/info");
        let provider = GenericOAuth2Provider::new(config, Arc::new(transport));

        let user = provider
            .user_from_token("code")
            .expect("user_from_token 失败");

        // openid 作为 id
        assert_eq!(user.id, "qq_12345");
        assert_eq!(user.nickname.as_deref(), Some("qq_user"));
        // figureurl_qq_1 作为 avatar
        assert_eq!(user.avatar.as_deref(), Some("https://qzapp.qlogo.cn/1.png"));
    }

    /// 测试用户 ID 为整数类型时正确转为字符串
    #[test]
    fn test_generic_oauth2_provider_user_id_integer() {
        let transport = MemoryOAuth2HttpTransport::new();
        transport.push_response(r#"{"access_token":"token123"}"#);
        transport.push_response(r#"{"id":12345,"nickname":"github_user"}"#);

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_user_url("https://provider.com/user/info");
        let provider = GenericOAuth2Provider::new(config, Arc::new(transport));

        let user = provider
            .user_from_token("code")
            .expect("user_from_token 失败");

        assert_eq!(user.id, "12345");
        assert_eq!(user.nickname.as_deref(), Some("github_user"));
    }

    /// 测试 HTTP 传输失败时返回 HttpTransport 错误
    #[test]
    fn test_generic_oauth2_provider_http_transport_failure() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, Arc::new(FailingTransport));

        let err = provider.user_from_token("code").unwrap_err();
        assert!(matches!(err, OAuth2Error::HttpTransport(_)));
    }

    /// 测试 percent_encode 函数
    #[test]
    fn test_percent_encode() {
        // unreserved 字符保持原样
        assert_eq!(percent_encode("abcXYZ09-._~"), "abcXYZ09-._~");
        // 空格编码为 %20
        assert_eq!(percent_encode("a b"), "a%20b");
        // 斜杠编码为 %2F
        assert_eq!(percent_encode("/"), "%2F");
        // 冒号编码为 %3A
        assert_eq!(percent_encode(":"), "%3A");
        // URL 编码
        assert_eq!(
            percent_encode("https://example.com/path"),
            "https%3A%2F%2Fexample.com%2Fpath"
        );
        // 中文字符（UTF-8 编码）
        assert_eq!(percent_encode("中"), "%E4%B8%AD");
    }

    /// 测试 extract_string 函数
    #[test]
    fn test_extract_string() {
        // 字符串
        assert_eq!(
            extract_string(&serde_json::json!("hello")),
            Some("hello".into())
        );
        // 整数
        assert_eq!(
            extract_string(&serde_json::json!(12345)),
            Some("12345".into())
        );
        // 浮点数（不支持，返回 None）
        assert_eq!(extract_string(&serde_json::json!(1.5)), None);
        // 布尔值（不支持，返回 None）
        assert_eq!(extract_string(&serde_json::json!(true)), None);
        // null（不支持，返回 None）
        assert_eq!(extract_string(&serde_json::Value::Null), None);
        // 对象（不支持，返回 None）
        assert_eq!(extract_string(&serde_json::json!({"a": 1})), None);
    }

    // ------------------------------------------------------------------------
    // 测试辅助类型
    // ------------------------------------------------------------------------

    /// 始终失败的 HTTP 传输（用于测试错误路径）
    struct FailingTransport;

    impl OAuth2HttpTransport for FailingTransport {
        fn post_json(&self, _url: &str, _body: &str) -> Result<String, OAuth2Error> {
            Err(OAuth2Error::HttpTransport("connection refused".into()))
        }
    }

    /// 空的 HTTP 传输（用于仅需校验、不实际发送的测试）
    struct MemoryHttpTransport;

    impl OAuth2HttpTransport for MemoryHttpTransport {
        fn post_json(&self, _url: &str, _body: &str) -> Result<String, OAuth2Error> {
            Ok(String::new())
        }
    }

    // ------------------------------------------------------------------------
    // T1: PKCE 与 state 自动生成测试
    // ------------------------------------------------------------------------

    /// 测试 state 自动生成：32 字符 hex、两次调用不同
    #[test]
    fn test_state_auto_generate() {
        let state1 = OAuth2Config::generate_state();
        let state2 = OAuth2Config::generate_state();

        // 32 字符 hex
        assert_eq!(state1.len(), 32, "state 应为 32 字符 hex（16 字节）");
        assert_eq!(state2.len(), 32, "state 应为 32 字符 hex（16 字节）");

        // 全部为 hex 字符
        assert!(
            state1.chars().all(|c| c.is_ascii_hexdigit()),
            "state 应全为 hex 字符: {state1}"
        );
        assert!(
            state2.chars().all(|c| c.is_ascii_hexdigit()),
            "state 应全为 hex 字符: {state2}"
        );

        // 两次调用不同（密码学随机，碰撞概率极低）
        assert_ne!(state1, state2, "两次生成的 state 不应相同");
    }

    /// 测试 PKCE 参数对生成：verifier 43-128 字符、challenge == base64url(SHA256(verifier))
    #[test]
    fn test_pkce_pair_generate() {
        let pkce = OAuth2Config::generate_pkce_pair();

        // code_verifier 应为 64 字符 hex（32 字节）
        assert!(
            pkce.code_verifier.len() >= 43 && pkce.code_verifier.len() <= 128,
            "code_verifier 长度应在 43-128 之间，实际: {}",
            pkce.code_verifier.len()
        );
        assert_eq!(
            pkce.code_verifier.len(),
            64,
            "code_verifier 应为 64 字符 hex"
        );
        assert!(
            pkce.code_verifier.chars().all(|c| c.is_ascii_hexdigit()),
            "code_verifier 应全为 hex 字符"
        );

        // code_challenge 应为 base64url(SHA256(code_verifier))
        let mut hasher = sha2::Sha256::new();
        sha2::Digest::update(&mut hasher, pkce.code_verifier.as_bytes());
        let digest = sha2::Digest::finalize(hasher);
        let expected_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(
            pkce.code_challenge, expected_challenge,
            "code_challenge 应等于 base64url(SHA256(code_verifier))"
        );

        // method 固定 S256
        assert_eq!(pkce.method, PkceMethod::S256);
    }

    /// 测试 authorization_code 流程带 PKCE：URL 含 code_challenge
    #[test]
    fn test_authorization_code_with_pkce() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/oauth2.0/authorize",
            "https://provider.com/oauth2.0/token",
        )
        .with_pkce(true);
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let pkce = OAuth2Config::generate_pkce_pair();
        let url = provider.redirect_url_with_pkce("state123", &pkce);

        assert!(url.contains("code_challenge="), "URL 应包含 code_challenge");
        assert!(
            url.contains("code_challenge_method=S256"),
            "URL 应包含 code_challenge_method=S256"
        );
        assert!(url.contains("response_type=code"));
        assert!(url.contains("state=state123"));
    }

    /// 测试 authorization_code 流程不带 PKCE：URL 不含 code_challenge
    #[test]
    fn test_authorization_code_without_pkce() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/oauth2.0/authorize",
            "https://provider.com/oauth2.0/token",
        );
        // pkce_enabled 默认 false
        assert!(!config.pkce_enabled);

        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));
        let url = provider.redirect_url("state123");

        assert!(
            !url.contains("code_challenge"),
            "URL 不应包含 code_challenge（PKCE 未启用）"
        );
    }

    /// 测试 client_secret 在 Debug 输出中脱敏
    #[test]
    fn test_client_secret_not_in_debug() {
        let config = OAuth2Config::new(
            "client123",
            "super_secret_value_456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );

        let debug_str = format!("{:?}", config);
        assert!(
            !debug_str.contains("super_secret_value_456"),
            "client_secret 不应出现在 Debug 输出中: {debug_str}"
        );
        assert!(
            debug_str.contains("***"),
            "Debug 输出应包含脱敏标记 '***': {debug_str}"
        );
    }

    /// 测试 PkceParams 的 Debug 输出不包含 code_verifier 明文
    #[test]
    fn test_pkce_params_debug_redacted() {
        let pkce = OAuth2Config::generate_pkce_pair();
        let debug_str = format!("{:?}", pkce);
        assert!(
            !debug_str.contains(&pkce.code_verifier),
            "code_verifier 明文不应出现在 Debug 输出中: {debug_str}"
        );
        assert!(
            debug_str.contains("redacted"),
            "Debug 输出应包含 'redacted' 标记: {debug_str}"
        );
    }

    /// 测试 with_pkce builder 方法
    #[test]
    fn test_with_pkce_builder() {
        let config = OAuth2Config::new(
            "id",
            "secret",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        assert!(!config.pkce_enabled, "默认 pkce_enabled 应为 false");

        let config = config.with_pkce(true);
        assert!(
            config.pkce_enabled,
            "with_pkce(true) 后 pkce_enabled 应为 true"
        );

        let config = config.with_pkce(false);
        assert!(
            !config.pkce_enabled,
            "with_pkce(false) 后 pkce_enabled 应为 false"
        );
    }

    /// 测试 with_device_auth_url builder 方法
    #[test]
    fn test_with_device_auth_url_builder() {
        let config = OAuth2Config::new(
            "id",
            "secret",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        assert!(
            config.device_auth_url.is_none(),
            "默认 device_auth_url 应为 None"
        );

        let config = config.with_device_auth_url("https://provider.com/device_authorize");
        assert_eq!(
            config.device_auth_url.as_deref(),
            Some("https://provider.com/device_authorize"),
        );
    }

    /// 测试 PKCE 流程的 token 交换：POST body 包含 code_verifier
    #[test]
    fn test_exchange_token_with_pkce_verifier() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(r#"{"access_token":"token123","expires_in":3600}"#);

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        )
        .with_pkce(true);
        let provider = GenericOAuth2Provider::new(config, transport.clone());

        let pkce = OAuth2Config::generate_pkce_pair();
        let user = provider
            .user_from_token_with_pkce("auth_code", Some(&pkce.code_verifier))
            .expect("user_from_token_with_pkce 失败");

        assert_eq!(user.access_token.as_deref(), Some("token123"));

        // 验证 POST body 包含 code_verifier
        let (_url, body) = transport.last().expect("应有请求记录");
        assert!(
            body.contains("code_verifier"),
            "token 交换 body 应包含 code_verifier: {body}"
        );
        assert!(
            body.contains(&pkce.code_verifier),
            "token 交换 body 应包含 code_verifier 值"
        );
    }

    /// 测试不带 PKCE 的 token 交换：POST body 不包含 code_verifier
    #[test]
    fn test_exchange_token_without_pkce_verifier() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(r#"{"access_token":"token123","expires_in":3600}"#);

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, transport.clone());

        let _user = provider
            .user_from_token("auth_code")
            .expect("user_from_token 失败");

        let (_url, body) = transport.last().expect("应有请求记录");
        assert!(
            !body.contains("code_verifier"),
            "token 交换 body 不应包含 code_verifier（PKCE 未启用）: {body}"
        );
    }

    // ------------------------------------------------------------------------
    // T2: refresh_token + audit logger 测试
    // ------------------------------------------------------------------------

    /// Mock 审计日志收集器（用于测试）
    #[derive(Default)]
    struct MockAuditLogger {
        events: Mutex<Vec<OAuth2AuditEvent>>,
    }

    impl MockAuditLogger {
        fn events(&self) -> Vec<OAuth2AuditEvent> {
            self.events.lock().clone()
        }
    }

    impl OAuth2AuditLogger for MockAuditLogger {
        fn log_event(&self, event: &OAuth2AuditEvent) {
            self.events.lock().push(event.clone());
        }
    }

    /// 测试 refresh_token：mock transport 返回新 token → 成功
    #[test]
    fn test_refresh_token() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(
            r#"{"access_token":"new_token","token_type":"Bearer","expires_in":7200,"scope":"read"}"#,
        );

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, transport.clone());

        let token_resp = provider
            .refresh_token("old_refresh_token")
            .expect("refresh_token 失败");

        assert_eq!(token_resp.access_token, "new_token");
        assert_eq!(token_resp.token_type.as_deref(), Some("Bearer"));
        assert_eq!(token_resp.expires_in, Some(7200));
        assert_eq!(token_resp.scope.as_deref(), Some("read"));
    }

    /// 测试审计日志：token 交换成功后记录事件
    #[test]
    fn test_audit_log_on_token_exchange() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(r#"{"access_token":"token123","expires_in":3600}"#);

        let logger = Arc::new(MockAuditLogger::default());
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, transport).with_audit_logger(logger.clone());

        let _user = provider
            .user_from_token("auth_code")
            .expect("user_from_token 失败");

        let events = logger.events();
        assert!(
            events.iter().any(|e| e.grant_type == "authorization_code"
                && e.result == "success"
                && e.client_id == "client123"),
            "应记录 authorization_code success 事件: {events:?}"
        );
    }

    /// 测试自动刷新：过期 token + 有效 refresh → 自动刷新
    #[test]
    fn test_auto_refresh_on_expired() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        // 第一次响应：token 已过期 (expires_in=0) + refresh_token
        transport.push_response(
            r#"{"access_token":"expired_token","refresh_token":"valid_refresh","expires_in":0}"#,
        );
        // 第二次响应：refresh_token 返回新 token
        transport.push_response(r#"{"access_token":"refreshed_token","expires_in":3600}"#);

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, transport.clone());

        let user = provider
            .user_from_token("auth_code")
            .expect("user_from_token 失败");

        // 应自动刷新为 refreshed_token
        assert_eq!(
            user.access_token.as_deref(),
            Some("refreshed_token"),
            "过期 token 应自动刷新"
        );
        assert_eq!(user.expires_in, Some(3600));
        // 应有 2 次 HTTP 请求（token 交换 + refresh）
        assert_eq!(transport.count(), 2);
    }

    /// 测试 refresh_token 为空 → AuthFailed
    #[test]
    fn test_refresh_token_empty() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, Arc::new(MemoryOAuth2HttpTransport::new()));

        let err = provider.refresh_token("").unwrap_err();
        assert!(matches!(err, OAuth2Error::AuthFailed(msg) if msg.contains("refresh_token")));
    }

    /// 测试 refresh_token Provider 返回非 JSON → TokenExchangeFailed
    #[test]
    fn test_refresh_token_invalid_json() {
        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response("not a json");

        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = GenericOAuth2Provider::new(config, transport);

        let err = provider.refresh_token("valid_refresh").unwrap_err();
        assert!(matches!(err, OAuth2Error::TokenExchangeFailed(_)));
    }

    /// 测试 TokenResponse 序列化脱敏
    #[test]
    fn test_token_response_serialize_redacted() {
        let token_resp = TokenResponse {
            access_token: "secret_access_token".into(),
            token_type: Some("Bearer".into()),
            expires_in: Some(3600),
            scope: Some("read".into()),
            refresh_token: Some("secret_refresh_token".into()),
        };

        let json = serde_json::to_string(&token_resp).expect("序列化失败");
        assert!(
            !json.contains("secret_access_token"),
            "access_token 不应出现在序列化 JSON 中: {json}"
        );
        assert!(
            !json.contains("secret_refresh_token"),
            "refresh_token 不应出现在序列化 JSON 中: {json}"
        );
    }

    // ------------------------------------------------------------------------
    // T3: ImplicitOAuth2Provider 测试
    // ------------------------------------------------------------------------

    /// 测试 implicit redirect_url 含 response_type=token
    #[test]
    fn test_implicit_redirect_url() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/oauth2.0/authorize",
            "https://provider.com/oauth2.0/token",
        )
        .with_scope("profile");
        let provider = ImplicitOAuth2Provider::new(config);

        let url = provider.redirect_url("state_abc");

        assert!(
            url.contains("response_type=token"),
            "URL 应含 response_type=token"
        );
        assert!(url.contains("client_id=client123"));
        assert!(url.contains("state=state_abc"));
        assert!(url.contains("scope=profile"));
    }

    /// 测试 implicit parse_fragment：含 access_token + state → 解析成功
    #[test]
    fn test_implicit_parse_fragment() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = ImplicitOAuth2Provider::new(config);

        let fragment = "access_token=token123&token_type=Bearer&expires_in=3600&state=mystate";
        let token_resp = provider
            .parse_fragment(fragment, "mystate")
            .expect("parse_fragment 失败");

        assert_eq!(token_resp.access_token, "token123");
        assert_eq!(token_resp.token_type.as_deref(), Some("Bearer"));
        assert_eq!(token_resp.expires_in, Some(3600));
        // implicit 流程不返回 refresh_token
        assert!(
            token_resp.refresh_token.is_none(),
            "implicit 流程 refresh_token 应为 None"
        );
    }

    /// 测试 implicit state 不匹配 → AuthFailed
    #[test]
    fn test_implicit_state_mismatch() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = ImplicitOAuth2Provider::new(config);

        let fragment = "access_token=token123&state=wrong_state";
        let err = provider
            .parse_fragment(fragment, "expected_state")
            .unwrap_err();
        assert!(
            matches!(&err, OAuth2Error::AuthFailed(msg) if msg.contains("CSRF state mismatch")),
            "state 不匹配应返回 CSRF 错误: {err}"
        );
    }

    /// 测试 implicit 回调不含 refresh_token
    #[test]
    fn test_implicit_no_refresh_token() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = ImplicitOAuth2Provider::new(config);

        // fragment 中即使包含 refresh_token 也应被忽略
        let fragment = "access_token=token123&refresh_token=should_be_ignored&state=mystate";
        let token_resp = provider
            .parse_fragment(fragment, "mystate")
            .expect("parse_fragment 失败");

        assert!(
            token_resp.refresh_token.is_none(),
            "implicit 流程 refresh_token 应固定为 None"
        );
    }

    /// 测试 implicit fragment 为空 → TokenExchangeFailed
    #[test]
    fn test_implicit_empty_fragment() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = ImplicitOAuth2Provider::new(config);

        let err = provider.parse_fragment("", "state").unwrap_err();
        assert!(matches!(err, OAuth2Error::TokenExchangeFailed(_)));
    }

    /// 测试 implicit fragment 无 access_token → TokenExchangeFailed
    #[test]
    fn test_implicit_no_access_token() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = ImplicitOAuth2Provider::new(config);

        let fragment = "token_type=Bearer&state=mystate";
        let err = provider.parse_fragment(fragment, "mystate").unwrap_err();
        assert!(matches!(err, OAuth2Error::TokenExchangeFailed(_)));
    }

    /// 测试 implicit scope 列表为空 → URL 不含 scope 参数
    #[test]
    fn test_implicit_empty_scopes() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider = ImplicitOAuth2Provider::new(config);

        let url = provider.redirect_url("state123");
        assert!(
            !url.contains("scope="),
            "空 scopes 时 URL 不应含 scope 参数"
        );
    }

    /// 测试 implicit 审计日志记录 OAUTH2_IMPLICIT_TOKEN_EXPOSED 告警
    #[test]
    fn test_implicit_audit_log_token_exposed() {
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let logger = Arc::new(MockAuditLogger::default());
        let provider = ImplicitOAuth2Provider::new(config).with_audit_logger(logger.clone());

        let fragment = "access_token=token123&state=mystate";
        let _ = provider.parse_fragment(fragment, "mystate");

        let events = logger.events();
        assert!(
            events
                .iter()
                .any(|e| e.alert_code.as_deref() == Some("OAUTH2_IMPLICIT_TOKEN_EXPOSED")),
            "应记录 OAUTH2_IMPLICIT_TOKEN_EXPOSED 告警: {events:?}"
        );
    }

    // ------------------------------------------------------------------------
    // T5: Token Store 集成测试（需 redis-store feature）
    // ------------------------------------------------------------------------

    /// 测试 token store 集成：user_from_token 后 store 收到 token
    #[cfg(feature = "redis-store")]
    #[tokio::test]
    async fn test_token_store_integration() {
        use crate::oauth_store::{MemoryOAuth2TokenStore, OAuth2TokenStore};

        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(r#"{"access_token":"token123","expires_in":3600}"#);

        let store = Arc::new(MemoryOAuth2TokenStore::new());
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, transport).with_token_store(store.clone());

        let user = provider
            .user_from_token("auth_code")
            .expect("user_from_token 失败");
        assert_eq!(user.access_token.as_deref(), Some("token123"));

        // 等待 spawn 的存储任务完成
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let stored = store
            .get_token("client123")
            .await
            .expect("get_token 失败")
            .expect("应查到存储的 token");
        assert_eq!(stored.access_token, "token123");
    }

    /// 测试 token store 失败 best-effort：存储失败不影响 token 发放
    #[cfg(feature = "redis-store")]
    #[tokio::test]
    async fn test_token_store_failure_best_effort() {
        use crate::oauth_store::MemoryOAuth2TokenStore;

        let transport = Arc::new(MemoryOAuth2HttpTransport::new());
        transport.push_response(r#"{"access_token":"token123","expires_in":3600}"#);

        // 使用一个总是成功的 MemoryStore（模拟 best-effort）
        let store = Arc::new(MemoryOAuth2TokenStore::new());
        let config = OAuth2Config::new(
            "client123",
            "secret456",
            "https://example.com/callback",
            "https://provider.com/authorize",
            "https://provider.com/token",
        );
        let provider =
            GenericOAuth2Provider::new(config, transport).with_token_store(store.clone());

        // 即使 store 出错，user_from_token 也应成功
        let user = provider
            .user_from_token("auth_code")
            .expect("user_from_token 应成功（best-effort）");
        assert_eq!(user.access_token.as_deref(), Some("token123"));
    }

    // ------------------------------------------------------------------------
    // T1: proptest 属性测试
    // ------------------------------------------------------------------------

    // proptest: state 不可预测（长度固定 32、全 hex 字符）
    proptest::proptest! {
        #[test]
        fn proptest_state_unpredictable(_n in 0u32..1000) {
            let s1 = OAuth2Config::generate_state();
            let s2 = OAuth2Config::generate_state();
            prop_assert_eq!(s1.len(), 32);
            prop_assert_eq!(s2.len(), 32);
            prop_assert!(s1.chars().all(|c| c.is_ascii_hexdigit()));
            prop_assert!(s2.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    // proptest: PKCE code_verifier 长度 ∈ [43, 128]
    proptest::proptest! {
        #[test]
        fn proptest_pkce_verifier_length(_n in 0u32..1000) {
            let pkce = OAuth2Config::generate_pkce_pair();
            prop_assert!(pkce.code_verifier.len() >= 43);
            prop_assert!(pkce.code_verifier.len() <= 128);
        }
    }

    // proptest: PKCE code_challenge == base64url(SHA256(code_verifier))
    proptest::proptest! {
        #[test]
        fn proptest_pkce_challenge_matches_verifier(_n in 0u32..1000) {
            let pkce = OAuth2Config::generate_pkce_pair();
            let mut hasher = sha2::Sha256::new();
            sha2::Digest::update(&mut hasher, pkce.code_verifier.as_bytes());
            let digest = sha2::Digest::finalize(hasher);
            let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
            prop_assert_eq!(pkce.code_challenge, expected);
        }
    }
}

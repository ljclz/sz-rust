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

use parking_lot::Mutex;
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
#[derive(Debug, Clone)]
pub struct OAuth2Config {
    /// 客户端 ID（必填）
    pub client_id: String,
    /// 客户端密钥（必填）
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
}

impl GenericOAuth2Provider {
    /// 创建通用 OAuth2 提供商
    ///
    /// # 参数
    ///
    /// - `config`: OAuth2 配置
    /// - `transport`: HTTP 传输实现（业务方注入 reqwest / hyper / etc.）
    pub fn new(config: OAuth2Config, transport: Arc<dyn OAuth2HttpTransport>) -> Self {
        Self { config, transport }
    }

    /// 构造授权 URL
    ///
    /// 拼接 `auth_url` 与查询参数：
    /// - `client_id`
    /// - `redirect_uri`
    /// - `response_type=code`
    /// - `state`
    /// - `scope`（如果配置了 scopes，以空格连接）
    /// - 额外参数（如果配置了 extra_params）
    ///
    /// 如果 `auth_url` 已包含查询字符串，则用 `&` 追加，否则用 `?` 起始。
    fn build_redirect_url(&self, state: &str) -> String {
        let mut params: Vec<(String, String)> = vec![
            ("client_id".into(), self.config.client_id.clone()),
            ("redirect_uri".into(), self.config.redirect_url.clone()),
            ("response_type".into(), "code".into()),
            ("state".into(), state.to_string()),
        ];

        if !self.config.scopes.is_empty() {
            params.push(("scope".into(), self.config.scopes.join(" ")));
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
    ///   "redirect_uri": "<redirect_url>"
    /// }
    /// ```
    ///
    /// 成功返回解析后的 JSON（含 `access_token` / `refresh_token` / `expires_in` 等）。
    fn exchange_token(&self, code: &str) -> Result<serde_json::Value, OAuth2Error> {
        let body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": self.config.client_id,
            "client_secret": self.config.client_secret,
            "redirect_uri": self.config.redirect_url,
        });
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
        self.build_redirect_url(state)
    }

    fn user_from_token(&self, code: &str) -> Result<SocialiteUser, OAuth2Error> {
        // 1. 校验配置必填字段
        self.config.validate()?;

        // 2. 校验授权码非空
        if code.is_empty() {
            return Err(OAuth2Error::AuthFailed("授权码不能为空".into()));
        }

        // 3. 用授权码换取访问令牌
        let token_json = self.exchange_token(code)?;

        // 4. 提取 access_token
        let access_token = token_json
            .get("access_token")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
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

        Ok(user)
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
}

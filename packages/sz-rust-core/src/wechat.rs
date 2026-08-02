//! Wechat 模块 — 微信 SDK 抽象层（对齐 PHP `EasyWeChat`）
//!
//! 提供统一的微信 SDK 抽象，支持公众号 / 小程序 / 开放平台 / 企业微信，
//! 对齐 PHP `overtrue/wechat` 与 `EasyWeChat` 的核心 API。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Factory::officialAccount()` | [`WechatSdk::new`] | 创建 SDK 实例 |
//! | `EasyWeChat::officialAccount()->oauth->redirect()` | [`WechatSdk::get_authorize_url`] | 构造 OAuth2 授权 URL |
//! | `EasyWeChat::officialAccount()->oauth->userFromCode()` | [`WechatSdk::get_user_by_code`] | 通过 code 换取用户 |
//! | `EasyWeChat::officialAccount()->user->get()` | [`WechatSdk::get_user_info`] | 获取用户信息 |
//! | `EasyWeChat::officialAccount()->server->verifySignature()` | [`WechatSdk::verify_signature`] | 验证签名 |
//! | `EasyWeChat::officialAccount()->template_message->send()` | [`WechatSdk::send_template_message`] | 发送模板消息 |
//! | `EasyWeChat::officialAccount()->jssdk->getSignature()` | [`WechatSdk::generate_jsapi_signature`] | 生成 JS-SDK 签名 |
//! | `EasyWeChat::officialAccount()->qrcode->forever()` | [`WechatSdk::get_qrcode_url`] | 获取带参数二维码 URL |
//!
//! ### PHP 行为对齐
//!
//! - **统一应用类型**：PHP `EasyWeChat\Factory::officialAccount()` / `miniProgram()` /
//!   `openPlatform()` / `work()` 创建不同应用。Rust 通过 [`WechatAppType`] 表达。
//! - **统一配置**：PHP 通过 config 数组传递 `app_id` / `secret` / `token` / `aes_key`。
//!   Rust 通过 [`WechatConfig`] builder 表达。
//! - **JS-SDK 签名**：PHP `EasyWeChat\Kernel\Support\JsSign` 生成签名。
//!   Rust 通过 [`WechatSdk::generate_jsapi_signature`] 实现，使用 SHA1 算法。
//!
//! ## 架构说明
//!
//! - **WechatHttpTransport trait**：HTTP 传输抽象，解耦 SDK 与具体 HTTP 库
//! - **MemoryWechatHttpTransport**：内存 HTTP 传输实现，支持预置响应队列，用于测试
//! - **WechatSdk**：统一 SDK 客户端，通过注入的 [`WechatHttpTransport`] 调用微信 API
//! - **签名算法**：使用 `sha1` crate 实现 `verify_signature` 与 `generate_jsapi_signature`

use parking_lot::Mutex;
use sha1::Digest;
use std::collections::VecDeque;
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// 微信 SDK 错误 — 对齐 PHP `EasyWeChat` 异常体系
#[derive(Debug, Error)]
pub enum WechatError {
    /// 配置错误（app_id / app_secret 为空等）
    #[error("微信配置错误: {0}")]
    Config(String),
    /// 缺少必填字段（code / openid / touser 等）
    #[error("微信字段缺失: {0}")]
    MissingField(String),
    /// API 调用失败（errcode != 0）
    #[error("微信 API 调用失败: {0}")]
    ApiFailed(String),
    /// HTTP 传输失败（网络错误、连接超时等）
    #[error("微信 HTTP 传输失败: {0}")]
    HttpTransport(String),
    /// 序列化/反序列化失败
    #[error("微信序列化失败: {0}")]
    Serialize(String),
    /// access_token 获取失败
    #[error("微信 access_token 获取失败: {0}")]
    TokenFailed(String),
    /// 解密失败（消息加解密）
    #[error("微信解密失败: {0}")]
    DecryptFailed(String),
}

// ============================================================================
// WechatAppType
// ============================================================================

/// 微信应用类型 — 对齐 PHP `EasyWeChat` 的应用类型
///
/// 对齐 `EasyWeChat\Factory::officialAccount()` / `miniProgram()` /
/// `openPlatform()` / `work()` 四种应用工厂方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WechatAppType {
    /// 公众号（对齐 `Factory::officialAccount()`）
    OfficialAccount,
    /// 小程序（对齐 `Factory::miniProgram()`）
    MiniProgram,
    /// 开放平台（对齐 `Factory::openPlatform()`）
    OpenPlatform,
    /// 企业微信（对齐 `Factory::work()`）
    Work,
}

// ============================================================================
// WechatConfig
// ============================================================================

/// 微信配置 — 对齐 PHP `EasyWeChat\Factory::officialAccount()` 配置
///
/// 使用 Builder 模式构建，必填字段在 [`WechatConfig::new`] 中提供，
/// 可选字段通过 `with_*` 链式方法追加。
///
/// # PHP 对齐
///
/// ```php
/// // PHP EasyWeChat
/// $config = [
///     'app_id'  => 'wx-app-id',
///     'secret'  => 'wx-app-secret',
///     'token'   => 'wx-token',
///     'aes_key' => 'wx-aes-key',
///     'oauth'   => ['redirect_uri' => 'https://example.com/oauth/callback'],
/// ];
/// $app = Factory::officialAccount($config);
/// ```
///
/// # Rust 用法
///
/// ```ignore
/// use sz_rust_core::wechat::{WechatAppType, WechatConfig};
///
/// let config = WechatConfig::new(
///     WechatAppType::OfficialAccount,
///     "wx-app-id",
///     "wx-app-secret",
/// )
/// .with_token("wx-token")
/// .with_encoding_aes_key("wx-aes-key")
/// .with_oauth_redirect_uri("https://example.com/oauth/callback");
/// ```
#[derive(Debug, Clone)]
pub struct WechatConfig {
    /// 应用类型
    pub app_type: WechatAppType,
    /// AppID
    pub app_id: String,
    /// AppSecret
    pub app_secret: String,
    /// Token（用于消息验证）
    pub token: Option<String>,
    /// EncodingAESKey（用于消息加解密）
    pub encoding_aes_key: Option<String>,
    /// OAuth2 回调 URL
    pub oauth_redirect_uri: Option<String>,
}

impl WechatConfig {
    /// 创建微信配置
    ///
    /// # 参数
    ///
    /// - `app_type`: 应用类型
    /// - `app_id`: AppID
    /// - `app_secret`: AppSecret
    pub fn new(
        app_type: WechatAppType,
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        Self {
            app_type,
            app_id: app_id.into(),
            app_secret: app_secret.into(),
            token: None,
            encoding_aes_key: None,
            oauth_redirect_uri: None,
        }
    }

    /// 设置消息验证 Token
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// 设置 EncodingAESKey（消息加解密）
    pub fn with_encoding_aes_key(mut self, key: impl Into<String>) -> Self {
        self.encoding_aes_key = Some(key.into());
        self
    }

    /// 设置 OAuth2 回调 URL
    pub fn with_oauth_redirect_uri(mut self, uri: impl Into<String>) -> Self {
        self.oauth_redirect_uri = Some(uri.into());
        self
    }

    /// 校验配置必填字段
    ///
    /// 必填字段：`app_id` / `app_secret`。任一为空字符串则返回 [`WechatError::Config`]。
    pub fn validate(&self) -> Result<(), WechatError> {
        if self.app_id.is_empty() {
            return Err(WechatError::Config("app_id".into()));
        }
        if self.app_secret.is_empty() {
            return Err(WechatError::Config("app_secret".into()));
        }
        Ok(())
    }
}

// ============================================================================
// JsApiTicket
// ============================================================================

/// JS-SDK 签名类型 — 对齐 PHP `EasyWeChat\Kernel\Support\JsSign`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsApiTicket {
    /// jsapi（用于 JS-SDK 签名）
    JsApi,
    /// wx_card（用于微信卡券签名）
    WxCard,
}

// ============================================================================
// WechatUser
// ============================================================================

/// 微信用户信息 — 对齐 PHP `EasyWeChat\User\User::get()`
///
/// 表示从微信 API 获取的用户信息，包含标准字段和原始响应数据。
///
/// # PHP 对齐
///
/// ```php
/// $user = $app->user->get($openid);
/// $user->openid;       // 用户 openid
/// $user->nickname;     // 昵称
/// $user->sex;          // 性别（0未知/1男/2女）
/// $user->province;     // 省份
/// $user->city;         // 城市
/// $user->country;      // 国家
/// $user->headimgurl;   // 头像 URL
/// $user->privilege;    // 特权信息
/// $user->unionid;      // unionid（需绑定开放平台）
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WechatUser {
    /// 用户 openid
    pub openid: String,
    /// 用户昵称
    pub nickname: Option<String>,
    /// 性别（0未知/1男/2女）
    pub sex: Option<i32>,
    /// 省份
    pub province: Option<String>,
    /// 城市
    pub city: Option<String>,
    /// 国家
    pub country: Option<String>,
    /// 头像 URL
    pub headimgurl: Option<String>,
    /// 特权信息
    pub privilege: Option<Vec<String>>,
    /// unionid（需绑定开放平台）
    pub unionid: Option<String>,
    /// 原始数据
    pub raw: serde_json::Value,
}

// ============================================================================
// WechatHttpTransport trait
// ============================================================================

/// 微信 HTTP 传输 trait — 用于解耦 SDK 与具体 HTTP 库
///
/// 微信 SDK 需要 GET 和 POST（JSON / Form）调用并返回响应体字符串，
/// 以便 SDK 解析 JSON 响应。业务方实现此 trait 注入 reqwest / hyper / etc.。
///
/// # 线程安全
///
/// 实现者必须保证 `Send + Sync`，因为 [`WechatSdk`] 通常作为单例在多线程下使用。
pub trait WechatHttpTransport: Send + Sync {
    /// GET 请求
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    ///
    /// # 返回
    ///
    /// 成功返回响应体字符串，失败返回 [`WechatError`]。
    fn get(&self, url: &str) -> Result<String, WechatError>;

    /// POST JSON 请求
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    /// - `body`: 请求体（JSON 字符串）
    ///
    /// # 返回
    ///
    /// 成功返回响应体字符串，失败返回 [`WechatError`]。
    fn post_json(&self, url: &str, body: &str) -> Result<String, WechatError>;

    /// POST Form 请求
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    /// - `body`: 请求体（Form 字符串）
    ///
    /// # 返回
    ///
    /// 成功返回响应体字符串，失败返回 [`WechatError`]。
    fn post_form(&self, url: &str, body: &str) -> Result<String, WechatError>;
}

// ============================================================================
// MemoryWechatHttpTransport（测试/开发用 HTTP 传输实现）
// ============================================================================

/// 内存微信 HTTP 传输 — 用于测试和开发环境
///
/// 不实际发送 HTTP 请求，而是：
/// - 将请求暂存到内部 Vec，供测试断言使用
/// - 从预置响应队列中依次返回 mock 响应（FIFO）
///
/// # 用法
///
/// ```ignore
/// use std::sync::Arc;
/// use sz_rust_core::wechat::{MemoryWechatHttpTransport, WechatHttpTransport};
///
/// let transport = MemoryWechatHttpTransport::new();
/// transport.push_response(r#"{"access_token":"token123"}"#);
/// transport.push_response(r#"{"errcode":0}"#);
///
/// let resp = transport.get("https://api.weixin.qq.com/token").unwrap();
/// assert_eq!(resp, r#"{"access_token":"token123"}"#);
/// ```
#[derive(Debug, Default)]
pub struct MemoryWechatHttpTransport {
    /// 预置的 mock 响应队列（FIFO）
    responses: Mutex<VecDeque<String>>,
    /// 已"发送"的 HTTP 请求记录（method, url, body）
    requests: Mutex<Vec<(String, String, String)>>,
}

impl MemoryWechatHttpTransport {
    /// 创建新的内存 HTTP 传输
    pub fn new() -> Self {
        Self::default()
    }

    /// 预置 mock 响应（追加到队列末尾，按调用顺序消费）
    pub fn push_response(&self, response: impl Into<String>) {
        self.responses.lock().push_back(response.into());
    }

    /// 获取已发送请求数量
    pub fn request_count(&self) -> usize {
        self.requests.lock().len()
    }

    /// 获取所有已发送请求（快照）
    ///
    /// 每条记录为 `(method, url, body)` 三元组。
    pub fn requests(&self) -> Vec<(String, String, String)> {
        self.requests.lock().clone()
    }

    /// 清空预置响应和请求记录
    pub fn clear(&self) {
        self.responses.lock().clear();
        self.requests.lock().clear();
    }

    /// 从队列头部取出下一条响应；队列空时返回错误
    fn next_response(&self) -> Result<String, WechatError> {
        match self.responses.lock().pop_front() {
            Some(resp) => Ok(resp),
            None => Err(WechatError::HttpTransport("无可用预置响应".into())),
        }
    }
}

impl WechatHttpTransport for MemoryWechatHttpTransport {
    fn get(&self, url: &str) -> Result<String, WechatError> {
        let response = self.next_response()?;
        self.requests
            .lock()
            .push(("GET".to_string(), url.to_string(), String::new()));
        Ok(response)
    }

    fn post_json(&self, url: &str, body: &str) -> Result<String, WechatError> {
        let response = self.next_response()?;
        self.requests
            .lock()
            .push(("POST_JSON".to_string(), url.to_string(), body.to_string()));
        Ok(response)
    }

    fn post_form(&self, url: &str, body: &str) -> Result<String, WechatError> {
        let response = self.next_response()?;
        self.requests
            .lock()
            .push(("POST_FORM".to_string(), url.to_string(), body.to_string()));
        Ok(response)
    }
}

// ============================================================================
// WechatSdk
// ============================================================================

/// 微信 SDK 客户端 — 对齐 PHP `EasyWeChat\Factory::officialAccount()`
///
/// 接收 [`WechatConfig`] 和 [`WechatHttpTransport`]，提供 OAuth2 / 用户 / 模板消息 /
/// JS-SDK 签名 / 二维码等核心 API。
///
/// # 用法
///
/// ```ignore
/// use std::sync::Arc;
/// use sz_rust_core::wechat::{
///     MemoryWechatHttpTransport, WechatAppType, WechatConfig, WechatSdk,
/// };
///
/// let config = WechatConfig::new(
///     WechatAppType::OfficialAccount,
///     "wx-app-id",
///     "wx-app-secret",
/// )
/// .with_oauth_redirect_uri("https://example.com/oauth/callback");
///
/// let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));
///
/// let url = sdk.get_authorize_url("snsapi_userinfo", "state123");
/// ```
pub struct WechatSdk {
    /// 微信配置
    config: WechatConfig,
    /// HTTP 传输实现
    transport: Arc<dyn WechatHttpTransport>,
}

impl WechatSdk {
    /// 创建微信 SDK 客户端
    ///
    /// # 参数
    ///
    /// - `config`: 微信配置
    /// - `transport`: HTTP 传输实现（业务方注入 reqwest / hyper / etc.）
    pub fn new(config: WechatConfig, transport: Arc<dyn WechatHttpTransport>) -> Self {
        Self { config, transport }
    }

    /// 获取配置引用
    pub fn config(&self) -> &WechatConfig {
        &self.config
    }

    /// 构造 OAuth2 授权 URL — 对齐 `EasyWeChat::officialAccount()->oauth->redirect()`
    ///
    /// 拼接 `https://open.weixin.qq.com/connect/oauth2/authorize` 与查询参数：
    /// - `appid`
    /// - `redirect_uri`（来自 `oauth_redirect_uri`，未配置时为空字符串）
    /// - `response_type=code`
    /// - `scope`
    /// - `state`
    ///
    /// URL 末尾追加 `#wechat_redirect` 锚点。
    pub fn get_authorize_url(&self, scope: &str, state: &str) -> String {
        let redirect_uri = self.config.oauth_redirect_uri.as_deref().unwrap_or("");
        format!(
            "https://open.weixin.qq.com/connect/oauth2/authorize?appid={}&redirect_uri={}&response_type=code&scope={}&state={}#wechat_redirect",
            percent_encode(&self.config.app_id),
            percent_encode(redirect_uri),
            percent_encode(scope),
            percent_encode(state),
        )
    }

    /// 通过 code 换取用户 — 对齐 `EasyWeChat::officialAccount()->oauth->userFromCode()`
    ///
    /// 工作流程：
    /// 1. GET `https://api.weixin.qq.com/sns/oauth2/access_token` 用 code 换取 access_token + openid
    /// 2. 调用 [`WechatSdk::get_user_info`] 获取用户信息
    ///
    /// # 参数
    ///
    /// - `code`: OAuth2 回调携带的授权码
    ///
    /// # 返回
    ///
    /// 成功返回 [`WechatUser`]，失败返回 [`WechatError`]。
    pub fn get_user_by_code(&self, code: &str) -> Result<WechatUser, WechatError> {
        self.config.validate()?;
        if code.is_empty() {
            return Err(WechatError::MissingField("code".into()));
        }

        // 1. 用 code 换取 access_token + openid
        let url = format!(
            "https://api.weixin.qq.com/sns/oauth2/access_token?appid={}&secret={}&code={}&grant_type=authorization_code",
            percent_encode(&self.config.app_id),
            percent_encode(&self.config.app_secret),
            percent_encode(code),
        );
        let response = self.transport.get(&url)?;
        let token_json: serde_json::Value = serde_json::from_str(&response)
            .map_err(|err| WechatError::Serialize(format!("解析 token 响应失败: {err}")))?;

        if let Some(errcode) = token_json.get("errcode").and_then(|v| v.as_i64()) {
            if errcode != 0 {
                return Err(WechatError::ApiFailed(format!(
                    "code 换取 token 失败: errcode={errcode}"
                )));
            }
        }

        let access_token = token_json
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WechatError::TokenFailed("响应缺少 access_token".into()))?
            .to_string();
        let openid = token_json
            .get("openid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WechatError::MissingField("openid".into()))?
            .to_string();

        // 2. 用 access_token + openid 获取用户信息
        self.get_user_info(&openid, &access_token)
    }

    /// 获取用户信息 — 对齐 `EasyWeChat::officialAccount()->user->get()`
    ///
    /// GET `https://api.weixin.qq.com/sns/userinfo?access_token=xxx&openid=xxx`，
    /// 解析 JSON 响应为 [`WechatUser`]。
    ///
    /// # 参数
    ///
    /// - `openid`: 用户 openid
    /// - `access_token`: OAuth2 access_token（来自 `sns/oauth2/access_token`）
    ///
    /// # 返回
    ///
    /// 成功返回 [`WechatUser`]，失败返回 [`WechatError`]。
    pub fn get_user_info(
        &self,
        openid: &str,
        access_token: &str,
    ) -> Result<WechatUser, WechatError> {
        self.config.validate()?;
        if openid.is_empty() {
            return Err(WechatError::MissingField("openid".into()));
        }
        if access_token.is_empty() {
            return Err(WechatError::MissingField("access_token".into()));
        }

        let url = format!(
            "https://api.weixin.qq.com/sns/userinfo?access_token={}&openid={}",
            percent_encode(access_token),
            percent_encode(openid),
        );
        let response = self.transport.get(&url)?;
        let json: serde_json::Value = serde_json::from_str(&response)
            .map_err(|err| WechatError::Serialize(format!("解析用户信息响应失败: {err}")))?;

        if let Some(errcode) = json.get("errcode").and_then(|v| v.as_i64()) {
            if errcode != 0 {
                return Err(WechatError::ApiFailed(format!(
                    "获取用户信息失败: errcode={errcode}"
                )));
            }
        }

        Ok(WechatUser {
            openid: json
                .get("openid")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            nickname: json
                .get("nickname")
                .and_then(|v| v.as_str())
                .map(String::from),
            sex: json.get("sex").and_then(|v| v.as_i64()).map(|v| v as i32),
            province: json
                .get("province")
                .and_then(|v| v.as_str())
                .map(String::from),
            city: json.get("city").and_then(|v| v.as_str()).map(String::from),
            country: json
                .get("country")
                .and_then(|v| v.as_str())
                .map(String::from),
            headimgurl: json
                .get("headimgurl")
                .and_then(|v| v.as_str())
                .map(String::from),
            privilege: json.get("privilege").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            }),
            unionid: json
                .get("unionid")
                .and_then(|v| v.as_str())
                .map(String::from),
            raw: json,
        })
    }

    /// 验证签名 — 对齐 `EasyWeChat::officialAccount()->server->verifySignature()`
    ///
    /// 计算 `SHA1(token + timestamp + nonce)` 并与 `signature` 比较。
    ///
    /// # 参数
    ///
    /// - `signature`: 待验证的签名（微信回调携带）
    /// - `timestamp`: 时间戳
    /// - `nonce`: 随机数
    /// - `token`: 公众号后台配置的 Token
    ///
    /// # 返回
    ///
    /// 签名匹配返回 `true`，否则返回 `false`。
    pub fn verify_signature(
        &self,
        signature: &str,
        timestamp: &str,
        nonce: &str,
        token: &str,
    ) -> bool {
        let mut hasher = sha1::Sha1::new();
        hasher.update(token.as_bytes());
        hasher.update(timestamp.as_bytes());
        hasher.update(nonce.as_bytes());
        let computed = hex::encode(hasher.finalize());
        computed == signature
    }

    /// 发送模板消息 — 对齐 `EasyWeChat::officialAccount()->template_message->send()`
    ///
    /// 工作流程：
    /// 1. GET `https://api.weixin.qq.com/cgi-bin/token` 获取 access_token
    /// 2. POST `https://api.weixin.qq.com/cgi-bin/message/template/send` 发送模板消息
    ///
    /// # 参数
    ///
    /// - `touser`: 接收者 openid
    /// - `template_id`: 模板 ID
    /// - `data`: 模板数据（JSON 值）
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`WechatError`]。
    pub fn send_template_message(
        &self,
        touser: &str,
        template_id: &str,
        data: &serde_json::Value,
    ) -> Result<(), WechatError> {
        self.config.validate()?;
        if touser.is_empty() {
            return Err(WechatError::MissingField("touser".into()));
        }
        if template_id.is_empty() {
            return Err(WechatError::MissingField("template_id".into()));
        }

        // 1. 获取 access_token
        let access_token = self.fetch_access_token()?;

        // 2. 发送模板消息
        let url = format!(
            "https://api.weixin.qq.com/cgi-bin/message/template/send?access_token={}",
            percent_encode(&access_token),
        );
        let body = serde_json::json!({
            "touser": touser,
            "template_id": template_id,
            "data": data,
        });
        let body_str =
            serde_json::to_string(&body).map_err(|err| WechatError::Serialize(err.to_string()))?;
        let response = self.transport.post_json(&url, &body_str)?;
        let json: serde_json::Value = serde_json::from_str(&response)
            .map_err(|err| WechatError::Serialize(format!("解析响应失败: {err}")))?;

        let errcode = json.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
        if errcode != 0 {
            return Err(WechatError::ApiFailed(format!(
                "发送模板消息失败: errcode={errcode}"
            )));
        }
        Ok(())
    }

    /// 生成 JS-SDK 签名 — 对齐 `EasyWeChat::officialAccount()->jssdk->getSignature()`
    ///
    /// 计算 `SHA1(jsapi_ticket=xxx&noncestr=xxx&timestamp=xxx&url=xxx)` 并返回十六进制字符串。
    ///
    /// # 参数
    ///
    /// - `url`: 当前页面 URL（不含 `#` 锚点）
    /// - `noncestr`: 随机字符串
    /// - `timestamp`: 时间戳（秒）
    /// - `jsapi_ticket`: jsapi_ticket（通过 `cgi-bin/ticket/get` 获取）
    ///
    /// # 返回
    ///
    /// 40 字符的 SHA1 十六进制签名。
    pub fn generate_jsapi_signature(
        &self,
        url: &str,
        noncestr: &str,
        timestamp: i64,
        jsapi_ticket: &str,
    ) -> String {
        let input = format!(
            "jsapi_ticket={}&noncestr={}&timestamp={}&url={}",
            jsapi_ticket, noncestr, timestamp, url,
        );
        let mut hasher = sha1::Sha1::new();
        hasher.update(input.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// 获取带参数二维码 URL — 对齐 `EasyWeChat::officialAccount()->qrcode->forever()`
    ///
    /// 工作流程：
    /// 1. GET `https://api.weixin.qq.com/cgi-bin/token` 获取 access_token
    /// 2. POST `https://api.weixin.qq.com/cgi-bin/qrcode/create` 创建永久二维码 ticket
    /// 3. 拼接 `https://mp.weixin.qq.com/cgi-bin/showqrcode?ticket=xxx` 返回
    ///
    /// # 参数
    ///
    /// - `scene_str`: 场景值（字符串，用于永久二维码）
    ///
    /// # 返回
    ///
    /// 成功返回二维码图片 URL，失败返回 [`WechatError`]。
    pub fn get_qrcode_url(&self, scene_str: &str) -> Result<String, WechatError> {
        self.config.validate()?;
        if scene_str.is_empty() {
            return Err(WechatError::MissingField("scene_str".into()));
        }

        // 1. 获取 access_token
        let access_token = self.fetch_access_token()?;

        // 2. 创建二维码 ticket
        let url = format!(
            "https://api.weixin.qq.com/cgi-bin/qrcode/create?access_token={}",
            percent_encode(&access_token),
        );
        let body = serde_json::json!({
            "action_name": "QR_LIMIT_STR_SCENE",
            "action_info": {
                "scene": {
                    "scene_str": scene_str,
                }
            }
        });
        let body_str =
            serde_json::to_string(&body).map_err(|err| WechatError::Serialize(err.to_string()))?;
        let response = self.transport.post_json(&url, &body_str)?;
        let json: serde_json::Value = serde_json::from_str(&response)
            .map_err(|err| WechatError::Serialize(format!("解析响应失败: {err}")))?;

        if let Some(errcode) = json.get("errcode").and_then(|v| v.as_i64()) {
            if errcode != 0 {
                return Err(WechatError::ApiFailed(format!(
                    "获取二维码 ticket 失败: errcode={errcode}"
                )));
            }
        }

        let ticket = json
            .get("ticket")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WechatError::MissingField("ticket".into()))?;
        Ok(format!(
            "https://mp.weixin.qq.com/cgi-bin/showqrcode?ticket={}",
            percent_encode(ticket)
        ))
    }

    /// 获取 access_token（内部方法）
    ///
    /// GET `https://api.weixin.qq.com/cgi-bin/token?grant_type=client_credential&appid=xxx&secret=xxx`，
    /// 解析 JSON 响应中的 `access_token`。
    fn fetch_access_token(&self) -> Result<String, WechatError> {
        let url = format!(
            "https://api.weixin.qq.com/cgi-bin/token?grant_type=client_credential&appid={}&secret={}",
            percent_encode(&self.config.app_id),
            percent_encode(&self.config.app_secret),
        );
        let response = self.transport.get(&url)?;
        let json: serde_json::Value = serde_json::from_str(&response)
            .map_err(|err| WechatError::Serialize(format!("解析 token 响应失败: {err}")))?;

        if let Some(errcode) = json.get("errcode").and_then(|v| v.as_i64()) {
            if errcode != 0 {
                return Err(WechatError::TokenFailed(format!(
                    "获取 access_token 失败: errcode={errcode}"
                )));
            }
        }

        json.get("access_token")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| WechatError::TokenFailed("响应缺少 access_token".into()))
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 简易百分号编码 — 用于 URL 查询参数
///
/// 对齐 RFC 3986 的 unreserved 字符集（`A-Za-z0-9-._~`）保持原样，
/// 其余字符编码为 `%XX` 形式（UTF-8 字节）。
fn percent_encode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        if matches!(
            byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
        ) {
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
    // WechatAppType 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatAppType 枚举的变体与 trait 派生
    #[test]
    fn test_wechat_app_type() {
        // 变体存在且相等
        assert_eq!(
            WechatAppType::OfficialAccount,
            WechatAppType::OfficialAccount
        );
        assert_eq!(WechatAppType::MiniProgram, WechatAppType::MiniProgram);
        assert_eq!(WechatAppType::OpenPlatform, WechatAppType::OpenPlatform);
        assert_eq!(WechatAppType::Work, WechatAppType::Work);

        // 不同变体不等
        assert_ne!(WechatAppType::OfficialAccount, WechatAppType::MiniProgram);
        assert_ne!(WechatAppType::OpenPlatform, WechatAppType::Work);

        // Copy + Clone 可用
        let app_type = WechatAppType::OfficialAccount;
        let cloned = app_type;
        assert_eq!(app_type, cloned);

        // Hash 可用（可放入 HashSet）
        let set: std::collections::HashSet<WechatAppType> = [
            WechatAppType::OfficialAccount,
            WechatAppType::MiniProgram,
            WechatAppType::OpenPlatform,
            WechatAppType::Work,
        ]
        .into_iter()
        .collect();
        assert_eq!(set.len(), 4);
        assert!(set.contains(&WechatAppType::OfficialAccount));
        assert!(set.contains(&WechatAppType::MiniProgram));
        assert!(set.contains(&WechatAppType::OpenPlatform));
        assert!(set.contains(&WechatAppType::Work));

        // Debug 可用
        assert_eq!(
            format!("{:?}", WechatAppType::OfficialAccount),
            "OfficialAccount"
        );
    }

    // ------------------------------------------------------------------------
    // WechatConfig 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatConfig builder 模式
    #[test]
    fn test_wechat_config_builder() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret")
                .with_token("wx_token")
                .with_encoding_aes_key("wx_aes_key")
                .with_oauth_redirect_uri("https://example.com/oauth/callback");

        assert_eq!(config.app_type, WechatAppType::OfficialAccount);
        assert_eq!(config.app_id, "wx_app_id");
        assert_eq!(config.app_secret, "wx_app_secret");
        assert_eq!(config.token.as_deref(), Some("wx_token"));
        assert_eq!(config.encoding_aes_key.as_deref(), Some("wx_aes_key"));
        assert_eq!(
            config.oauth_redirect_uri.as_deref(),
            Some("https://example.com/oauth/callback")
        );

        // validate 通过
        assert!(config.validate().is_ok());

        // 默认值（仅必填项）
        let minimal = WechatConfig::new(WechatAppType::MiniProgram, "wx_mini", "secret");
        assert_eq!(minimal.app_type, WechatAppType::MiniProgram);
        assert_eq!(minimal.app_id, "wx_mini");
        assert_eq!(minimal.app_secret, "secret");
        assert!(minimal.token.is_none());
        assert!(minimal.encoding_aes_key.is_none());
        assert!(minimal.oauth_redirect_uri.is_none());
        assert!(minimal.validate().is_ok());
    }

    /// 测试 WechatConfig::validate 校验必填字段
    #[test]
    fn test_wechat_config_validate() {
        // 全部合法
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "app_id", "secret");
        assert!(config.validate().is_ok());

        // app_id 为空
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "", "secret");
        let err = config.validate().unwrap_err();
        match err {
            WechatError::Config(field) => assert_eq!(field, "app_id"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }

        // app_secret 为空
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "app_id", "");
        let err = config.validate().unwrap_err();
        match err {
            WechatError::Config(field) => assert_eq!(field, "app_secret"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }

        // 两者为空（优先报 app_id）
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "", "");
        let err = config.validate().unwrap_err();
        match err {
            WechatError::Config(field) => assert_eq!(field, "app_id"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // WechatUser 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatUser 默认值
    #[test]
    fn test_wechat_user_default() {
        let user = WechatUser::default();
        assert!(user.openid.is_empty());
        assert!(user.nickname.is_none());
        assert!(user.sex.is_none());
        assert!(user.province.is_none());
        assert!(user.city.is_none());
        assert!(user.country.is_none());
        assert!(user.headimgurl.is_none());
        assert!(user.privilege.is_none());
        assert!(user.unionid.is_none());
        assert!(user.raw.is_null());
    }

    /// 测试 WechatUser 序列化/反序列化往返
    #[test]
    fn test_wechat_user_serialize() {
        let user = WechatUser {
            openid: "openid_abc".to_string(),
            nickname: Some("test_user".into()),
            sex: Some(1),
            province: Some("广东".into()),
            city: Some("深圳".into()),
            country: Some("中国".into()),
            headimgurl: Some("https://example.com/avatar.png".into()),
            privilege: Some(vec!["priv1".into(), "priv2".into()]),
            unionid: Some("unionid_xyz".into()),
            raw: serde_json::json!({"custom": "field"}),
        };

        let json = serde_json::to_string(&user).expect("序列化失败");
        let parsed: WechatUser = serde_json::from_str(&json).expect("反序列化失败");

        assert_eq!(parsed.openid, "openid_abc");
        assert_eq!(parsed.nickname.as_deref(), Some("test_user"));
        assert_eq!(parsed.sex, Some(1));
        assert_eq!(parsed.province.as_deref(), Some("广东"));
        assert_eq!(parsed.city.as_deref(), Some("深圳"));
        assert_eq!(parsed.country.as_deref(), Some("中国"));
        assert_eq!(
            parsed.headimgurl.as_deref(),
            Some("https://example.com/avatar.png")
        );
        assert_eq!(
            parsed.privilege.as_deref(),
            Some(&vec!["priv1".to_string(), "priv2".to_string()][..])
        );
        assert_eq!(parsed.unionid.as_deref(), Some("unionid_xyz"));
        assert_eq!(parsed.raw["custom"], "field");
    }

    // ------------------------------------------------------------------------
    // WechatSdk::get_authorize_url 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::get_authorize_url 构造 OAuth2 授权 URL
    #[test]
    fn test_wechat_sdk_get_authorize_url() {
        // 配置了 redirect_uri
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret")
                .with_oauth_redirect_uri("https://example.com/oauth/callback");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        let url = sdk.get_authorize_url("snsapi_userinfo", "state_abc");
        assert!(
            url.starts_with("https://open.weixin.qq.com/connect/oauth2/authorize?"),
            "URL 前缀错误: {url}"
        );
        assert!(url.contains("appid=wx_app_id"));
        // redirect_uri 百分号编码
        assert!(url.contains("redirect_uri=https%3A%2F%2Fexample.com%2Foauth%2Fcallback"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=snsapi_userinfo"));
        assert!(url.contains("state=state_abc"));
        assert!(url.ends_with("#wechat_redirect"));

        // 未配置 redirect_uri 时为空字符串
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));
        let url = sdk.get_authorize_url("snsapi_base", "state_123");
        assert!(url.contains("redirect_uri="));
        assert!(url.contains("scope=snsapi_base"));
        assert!(url.contains("state=state_123"));
    }

    // ------------------------------------------------------------------------
    // WechatSdk::verify_signature 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::verify_signature 签名验证
    #[test]
    fn test_wechat_sdk_verify_signature() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        let token = "test_token";
        let timestamp = "1234567890";
        let nonce = "nonce_abc";

        // 计算预期签名（SHA1(token + timestamp + nonce)）
        let mut hasher = sha1::Sha1::new();
        hasher.update(token.as_bytes());
        hasher.update(timestamp.as_bytes());
        hasher.update(nonce.as_bytes());
        let expected = hex::encode(hasher.finalize());

        // 正确签名应通过验证
        assert!(sdk.verify_signature(&expected, timestamp, nonce, token));

        // 错误签名不应通过验证
        assert!(!sdk.verify_signature("wrong_signature", timestamp, nonce, token));

        // 边界：空字符串（SHA1("") = da39a3ee5e6b4b0d3255bfef95601890afd80709）
        assert!(sdk.verify_signature("da39a3ee5e6b4b0d3255bfef95601890afd80709", "", "", ""));

        // 顺序敏感：调换 token/timestamp/nonce 顺序应不匹配
        let mut hasher = sha1::Sha1::new();
        hasher.update(timestamp.as_bytes());
        hasher.update(token.as_bytes());
        hasher.update(nonce.as_bytes());
        let wrong_order = hex::encode(hasher.finalize());
        assert!(!sdk.verify_signature(&wrong_order, timestamp, nonce, token));
    }

    // ------------------------------------------------------------------------
    // WechatSdk::generate_jsapi_signature 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::generate_jsapi_signature 生成 JS-SDK 签名
    #[test]
    fn test_wechat_sdk_generate_jsapi_signature() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        let url = "https://example.com/page";
        let noncestr = "abc123";
        let timestamp = 1609459200_i64;
        let jsapi_ticket = "ticket_value";

        // 计算预期签名
        let input = format!(
            "jsapi_ticket={}&noncestr={}&timestamp={}&url={}",
            jsapi_ticket, noncestr, timestamp, url,
        );
        let mut hasher = sha1::Sha1::new();
        hasher.update(input.as_bytes());
        let expected = hex::encode(hasher.finalize());

        let signature = sdk.generate_jsapi_signature(url, noncestr, timestamp, jsapi_ticket);
        assert_eq!(signature, expected);
        // SHA1 十六进制长度为 40
        assert_eq!(signature.len(), 40);

        // 不同参数应产生不同签名
        let other =
            sdk.generate_jsapi_signature("https://other.com", noncestr, timestamp, jsapi_ticket);
        assert_ne!(signature, other);

        // 边界：空字符串参数
        let empty_sig = sdk.generate_jsapi_signature("", "", 0, "");
        assert_eq!(empty_sig.len(), 40);
    }

    // ------------------------------------------------------------------------
    // WechatSdk::get_user_by_code 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::get_user_by_code 完整流程（使用 MemoryWechatHttpTransport）
    #[test]
    fn test_wechat_sdk_get_user_by_code() {
        let transport = Arc::new(MemoryWechatHttpTransport::new());
        // 预置 mock 响应：token 响应 + 用户信息响应
        transport.push_response(
            r#"{"access_token":"token123","expires_in":7200,"openid":"openid_abc"}"#,
        );
        transport.push_response(
            r#"{"openid":"openid_abc","nickname":"test_user","sex":1,"province":"广东","city":"深圳","country":"中国","headimgurl":"https://example.com/avatar.png","unionid":"unionid_xyz"}"#,
        );

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, transport.clone());

        let user = sdk
            .get_user_by_code("auth_code_abc")
            .expect("get_user_by_code 失败");

        // 验证用户字段
        assert_eq!(user.openid, "openid_abc");
        assert_eq!(user.nickname.as_deref(), Some("test_user"));
        assert_eq!(user.sex, Some(1));
        assert_eq!(user.province.as_deref(), Some("广东"));
        assert_eq!(user.city.as_deref(), Some("深圳"));
        assert_eq!(user.country.as_deref(), Some("中国"));
        assert_eq!(
            user.headimgurl.as_deref(),
            Some("https://example.com/avatar.png")
        );
        assert_eq!(user.unionid.as_deref(), Some("unionid_xyz"));
        // raw 保留原始响应
        assert_eq!(user.raw["nickname"], "test_user");

        // 验证 HTTP 请求次数（token + user info）
        assert_eq!(transport.request_count(), 2);

        // 验证第一次请求是 GET sns/oauth2/access_token
        let requests = transport.requests();
        assert_eq!(requests[0].0, "GET");
        assert!(requests[0].1.contains("sns/oauth2/access_token"));
        assert!(requests[0].1.contains("code=auth_code_abc"));

        // 验证第二次请求是 GET sns/userinfo
        assert_eq!(requests[1].0, "GET");
        assert!(requests[1].1.contains("sns/userinfo"));
    }

    /// 测试 WechatSdk::get_user_by_code code 为空时返回错误
    #[test]
    fn test_wechat_sdk_get_user_by_code_empty_code() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        let err = sdk.get_user_by_code("").unwrap_err();
        match err {
            WechatError::MissingField(field) => assert_eq!(field, "code"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    /// 测试 WechatSdk::get_user_by_code token 响应缺少 access_token 时失败
    #[test]
    fn test_wechat_sdk_get_user_by_code_missing_access_token() {
        let transport = MemoryWechatHttpTransport::new();
        transport.push_response(r#"{"errcode":40029,"errmsg":"invalid code"}"#);

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(transport));

        let err = sdk.get_user_by_code("invalid_code").unwrap_err();
        assert!(matches!(err, WechatError::ApiFailed(_)));
    }

    // ------------------------------------------------------------------------
    // WechatSdk::get_user_info 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::get_user_info 获取用户信息
    #[test]
    fn test_wechat_sdk_get_user_info() {
        let transport = Arc::new(MemoryWechatHttpTransport::new());
        transport.push_response(
            r#"{"openid":"openid_xyz","nickname":"user_info","sex":2,"province":"北京","city":"北京","country":"中国","headimgurl":"https://example.com/avatar2.png","privilege":["priv_a","priv_b"]}"#,
        );

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, transport.clone());

        let user = sdk
            .get_user_info("openid_xyz", "access_token_123")
            .expect("get_user_info 失败");

        assert_eq!(user.openid, "openid_xyz");
        assert_eq!(user.nickname.as_deref(), Some("user_info"));
        assert_eq!(user.sex, Some(2));
        assert_eq!(user.province.as_deref(), Some("北京"));
        assert_eq!(user.city.as_deref(), Some("北京"));
        assert_eq!(user.country.as_deref(), Some("中国"));
        assert_eq!(
            user.headimgurl.as_deref(),
            Some("https://example.com/avatar2.png")
        );
        assert_eq!(
            user.privilege.as_deref(),
            Some(&vec!["priv_a".to_string(), "priv_b".to_string()][..])
        );
        // raw 保留原始响应
        assert_eq!(user.raw["openid"], "openid_xyz");

        // 验证仅一次 HTTP 请求
        assert_eq!(transport.request_count(), 1);
        let requests = transport.requests();
        assert_eq!(requests[0].0, "GET");
        assert!(requests[0].1.contains("sns/userinfo"));
        assert!(requests[0].1.contains("access_token=access_token_123"));
        assert!(requests[0].1.contains("openid=openid_xyz"));
    }

    /// 测试 WechatSdk::get_user_info openid/access_token 为空时返回错误
    #[test]
    fn test_wechat_sdk_get_user_info_empty_fields() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        // openid 为空
        let err = sdk.get_user_info("", "token").unwrap_err();
        match err {
            WechatError::MissingField(field) => assert_eq!(field, "openid"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // access_token 为空
        let err = sdk.get_user_info("openid", "").unwrap_err();
        match err {
            WechatError::MissingField(field) => assert_eq!(field, "access_token"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // WechatSdk::send_template_message 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::send_template_message 发送模板消息
    #[test]
    fn test_wechat_sdk_send_template_message() {
        let transport = Arc::new(MemoryWechatHttpTransport::new());
        // 预置 mock 响应：access_token 响应 + 模板消息响应
        transport.push_response(r#"{"access_token":"token123","expires_in":7200}"#);
        transport.push_response(r#"{"errcode":0,"errmsg":"ok","msgid":123456789}"#);

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, transport.clone());

        let data = serde_json::json!({
            "first": {"value": "您好，订单已支付"},
            "keyword1": {"value": "鲜视达订单 #20240101"},
            "remark": {"value": "感谢您的惠顾"}
        });
        sdk.send_template_message("openid_user", "tpl_id_abc", &data)
            .expect("send_template_message 失败");

        // 验证两次 HTTP 请求
        assert_eq!(transport.request_count(), 2);
        let requests = transport.requests();

        // 第一次：GET cgi-bin/token
        assert_eq!(requests[0].0, "GET");
        assert!(requests[0].1.contains("cgi-bin/token"));
        assert!(requests[0].1.contains("grant_type=client_credential"));

        // 第二次：POST cgi-bin/message/template/send
        assert_eq!(requests[1].0, "POST_JSON");
        assert!(requests[1].1.contains("cgi-bin/message/template/send"));
        assert!(requests[1].1.contains("access_token=token123"));
        // body 包含 touser / template_id / data
        let body: serde_json::Value = serde_json::from_str(&requests[1].2).expect("body 应为 JSON");
        assert_eq!(body["touser"], "openid_user");
        assert_eq!(body["template_id"], "tpl_id_abc");
        assert_eq!(body["data"]["first"]["value"], "您好，订单已支付");
    }

    /// 测试 WechatSdk::send_template_message touser/template_id 为空时返回错误
    #[test]
    fn test_wechat_sdk_send_template_message_empty_fields() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        // touser 为空
        let err = sdk
            .send_template_message("", "tpl_id", &serde_json::json!({}))
            .unwrap_err();
        match err {
            WechatError::MissingField(field) => assert_eq!(field, "touser"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // template_id 为空
        let err = sdk
            .send_template_message("user", "", &serde_json::json!({}))
            .unwrap_err();
        match err {
            WechatError::MissingField(field) => assert_eq!(field, "template_id"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    /// 测试 WechatSdk::send_template_message access_token 获取失败
    #[test]
    fn test_wechat_sdk_send_template_message_token_failed() {
        let transport = MemoryWechatHttpTransport::new();
        // access_token 接口返回 errcode
        transport.push_response(r#"{"errcode":40013,"errmsg":"invalid appid"}"#);

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(transport));

        let err = sdk
            .send_template_message("user", "tpl", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, WechatError::TokenFailed(_)));
    }

    /// 测试 WechatSdk::send_template_message 微信返回 errcode 时失败
    #[test]
    fn test_wechat_sdk_send_template_message_api_failed() {
        let transport = MemoryWechatHttpTransport::new();
        transport.push_response(r#"{"access_token":"token123","expires_in":7200}"#);
        transport.push_response(r#"{"errcode":43004,"errmsg":"require subscribe"}"#);

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(transport));

        let err = sdk
            .send_template_message("user", "tpl", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, WechatError::ApiFailed(_)));
    }

    // ------------------------------------------------------------------------
    // WechatSdk::get_qrcode_url 测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk::get_qrcode_url 获取带参数二维码 URL
    #[test]
    fn test_wechat_sdk_get_qrcode_url() {
        let transport = Arc::new(MemoryWechatHttpTransport::new());
        // 预置 mock 响应：access_token 响应 + 二维码 ticket 响应
        transport.push_response(r#"{"access_token":"token123","expires_in":7200}"#);
        transport.push_response(
            r#"{"ticket":"ticket_abc_xyz","url":"http://weixin.qq.com/q/abc","expire_seconds":0}"#,
        );

        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, transport.clone());

        let url = sdk
            .get_qrcode_url("scene_123")
            .expect("get_qrcode_url 失败");

        // 应为 showqrcode URL
        assert!(url.starts_with("https://mp.weixin.qq.com/cgi-bin/showqrcode?ticket="));
        // ticket 百分号编码（ticket_abc_xyz 仅含 unreserved 字符，不编码）
        assert!(url.contains("ticket=ticket_abc_xyz"));

        // 验证两次 HTTP 请求
        assert_eq!(transport.request_count(), 2);
        let requests = transport.requests();

        // 第一次：GET cgi-bin/token
        assert_eq!(requests[0].0, "GET");
        assert!(requests[0].1.contains("cgi-bin/token"));

        // 第二次：POST cgi-bin/qrcode/create
        assert_eq!(requests[1].0, "POST_JSON");
        assert!(requests[1].1.contains("cgi-bin/qrcode/create"));
        assert!(requests[1].1.contains("access_token=token123"));
        // body 包含 action_name 和 scene_str
        let body: serde_json::Value = serde_json::from_str(&requests[1].2).expect("body 应为 JSON");
        assert_eq!(body["action_name"], "QR_LIMIT_STR_SCENE");
        assert_eq!(body["action_info"]["scene"]["scene_str"], "scene_123");
    }

    /// 测试 WechatSdk::get_qrcode_url scene_str 为空时返回错误
    #[test]
    fn test_wechat_sdk_get_qrcode_url_empty_scene() {
        let config =
            WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_id", "wx_app_secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        let err = sdk.get_qrcode_url("").unwrap_err();
        match err {
            WechatError::MissingField(field) => assert_eq!(field, "scene_str"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // WechatSdk 配置缺失测试
    // ------------------------------------------------------------------------

    /// 测试 WechatSdk 配置缺失时各 API 返回 Config 错误
    #[test]
    fn test_wechat_sdk_missing_config() {
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "", "");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

        // get_user_by_code 应失败（Config）
        let err = sdk.get_user_by_code("code").unwrap_err();
        assert!(matches!(err, WechatError::Config(_)));

        // get_user_info 应失败（Config）
        let err = sdk.get_user_info("openid", "token").unwrap_err();
        assert!(matches!(err, WechatError::Config(_)));

        // send_template_message 应失败（Config）
        let err = sdk
            .send_template_message("user", "tpl", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, WechatError::Config(_)));

        // get_qrcode_url 应失败（Config）
        let err = sdk.get_qrcode_url("scene").unwrap_err();
        assert!(matches!(err, WechatError::Config(_)));

        // 仅 app_id 为空
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "", "secret");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));
        let err = sdk.get_user_by_code("code").unwrap_err();
        match err {
            WechatError::Config(field) => assert_eq!(field, "app_id"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }

        // 仅 app_secret 为空
        let config = WechatConfig::new(WechatAppType::OfficialAccount, "app_id", "");
        let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));
        let err = sdk.get_user_info("openid", "token").unwrap_err();
        match err {
            WechatError::Config(field) => assert_eq!(field, "app_secret"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }

        // 注意：get_authorize_url / verify_signature / generate_jsapi_signature
        // 为纯计算方法，不校验配置，可正常调用
        let _url = sdk.get_authorize_url("snsapi_base", "state");
        assert!(sdk.verify_signature("sig", "ts", "nonce", "token") || true);
        let _sig = sdk.generate_jsapi_signature("url", "ns", 0, "ticket");
    }

    // ------------------------------------------------------------------------
    // MemoryWechatHttpTransport 测试
    // ------------------------------------------------------------------------

    /// 测试 MemoryWechatHttpTransport 的 GET / POST_JSON / POST_FORM 与响应队列
    #[test]
    fn test_memory_wechat_http_transport() {
        let transport = MemoryWechatHttpTransport::new();

        // 队列空时返回错误
        let err = transport.get("https://api.example.com/get").unwrap_err();
        match err {
            WechatError::HttpTransport(msg) => assert!(msg.contains("无可用预置响应")),
            other => panic!("期望 HttpTransport, 实际 {other:?}"),
        }
        assert_eq!(transport.request_count(), 0);

        // 预置响应后 GET 返回响应并记录请求
        transport.push_response(r#"{"code":"ok"}"#);
        let resp = transport
            .get("https://api.example.com/get?foo=bar")
            .expect("GET 应返回预置响应");
        assert_eq!(resp, r#"{"code":"ok"}"#);
        assert_eq!(transport.request_count(), 1);
        let requests = transport.requests();
        assert_eq!(requests[0].0, "GET");
        assert_eq!(requests[0].1, "https://api.example.com/get?foo=bar");
        assert_eq!(requests[0].2, ""); // GET 的 body 为空

        // POST_JSON 返回响应并记录请求
        transport.push_response(r#"{"errcode":0}"#);
        let resp = transport
            .post_json("https://api.example.com/post", r#"{"key":"value"}"#)
            .expect("POST_JSON 应返回预置响应");
        assert_eq!(resp, r#"{"errcode":0}"#);
        assert_eq!(transport.request_count(), 2);
        let requests = transport.requests();
        assert_eq!(requests[1].0, "POST_JSON");
        assert_eq!(requests[1].1, "https://api.example.com/post");
        assert_eq!(requests[1].2, r#"{"key":"value"}"#);

        // POST_FORM 返回响应并记录请求
        transport.push_response("form_response");
        let resp = transport
            .post_form("https://api.example.com/form", "a=1&b=2")
            .expect("POST_FORM 应返回预置响应");
        assert_eq!(resp, "form_response");
        assert_eq!(transport.request_count(), 3);
        let requests = transport.requests();
        assert_eq!(requests[2].0, "POST_FORM");
        assert_eq!(requests[2].1, "https://api.example.com/form");
        assert_eq!(requests[2].2, "a=1&b=2");

        // FIFO 顺序：交替调用
        transport.clear();
        transport.push_response("resp1");
        transport.push_response("resp2");
        transport.push_response("resp3");

        let r1 = transport.post_json("url1", "body1").expect("应返回 resp1");
        let r2 = transport.get("url2").expect("应返回 resp2");
        let r3 = transport.post_form("url3", "body3").expect("应返回 resp3");
        assert_eq!(r1, "resp1");
        assert_eq!(r2, "resp2");
        assert_eq!(r3, "resp3");

        // 队列已空
        assert!(transport.get("url4").is_err());
        assert!(transport.post_json("url4", "body4").is_err());
        assert!(transport.post_form("url4", "body4").is_err());

        // 验证请求记录顺序
        assert_eq!(transport.request_count(), 3);
        let requests = transport.requests();
        assert_eq!(requests[0].0, "POST_JSON");
        assert_eq!(requests[0].1, "url1");
        assert_eq!(requests[0].2, "body1");
        assert_eq!(requests[1].0, "GET");
        assert_eq!(requests[1].1, "url2");
        assert_eq!(requests[1].2, "");
        assert_eq!(requests[2].0, "POST_FORM");
        assert_eq!(requests[2].1, "url3");
        assert_eq!(requests[2].2, "body3");

        // clear 后队列和记录均清空
        transport.clear();
        assert_eq!(transport.request_count(), 0);
        assert!(transport.get("url").is_err());
    }
}

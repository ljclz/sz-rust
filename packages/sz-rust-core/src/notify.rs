//! Notify 模块 — 通知抽象层（对齐 PHP `think\facade\Notify`）
//!
//! 提供通知发送的统一抽象，支持多渠道（Slack、短信、邮件等）扩展。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Notify::channel($name)` | [`Notification::channel`] | 设置通知渠道 |
//! | `Notify::title($title)` | [`Notification::title`] | 设置通知标题 |
//! | `Notify::content($content)` | [`Notification::content`] | 设置通知内容 |
//! | `Notify::level($level)` | [`Notification::level`] | 设置通知级别 |
//! | `Notify::send()` | [`Notifier::send`] | 发送通知 |
//!
//! ### PHP 行为对齐
//!
//! - **Builder 模式**：PHP `Notify::channel()->title()->content()->send()` 链式调用。
//!   Rust 通过 [`Notification`] builder 实现相同链式 API。
//! - **多级别**：PHP 支持 info/warning/error/critical 四种级别。Rust 通过 [`NotifyLevel`] 表达。
//! - **多渠道**：PHP 支持按渠道分发（slack/sms/mail）。Rust 通过 [`Notifier`] trait 抽象。
//!
//! ## 架构说明
//!
//! - **Notifier trait 抽象**：业务方实现具体发送逻辑（Slack Webhook / 短信 API / 日志等）
//! - **MemoryNotifier**：内置内存实现，将通知暂存到 Vec，用于测试和开发环境
//! - **SlackNotifier**：Slack Webhook 实现，通过 [`HttpTransport`] trait 抽象 HTTP 发送，
//!   业务方注入具体 HTTP 客户端（如 reqwest）即可投入生产
//! - **HttpTransport trait**：HTTP 传输抽象，解耦 SlackNotifier 与具体 HTTP 库
//! - **MemoryHttpTransport**：内存 HTTP 传输实现，记录所有请求供测试断言

use parking_lot::Mutex;
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// Notify 错误
#[derive(Debug, Error)]
pub enum NotifyError {
    /// 缺少必填字段（渠道、标题、内容等）
    #[error("通知字段缺失: {0}")]
    MissingField(String),
    /// 通知发送失败
    #[error("通知发送失败: {0}")]
    SendFailed(String),
    /// HTTP 传输失败
    #[error("HTTP 传输失败: {0}")]
    HttpTransport(String),
    /// 序列化失败
    #[error("序列化失败: {0}")]
    Serialize(String),
}

// ============================================================================
// 通知级别
// ============================================================================

/// 通知级别 — 对齐 PHP `think\notify\Level`
///
/// 支持四种级别，按严重程度递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum NotifyLevel {
    /// 信息（最低级别，蓝色）
    #[default]
    Info = 0,
    /// 警告（黄色）
    Warning = 1,
    /// 错误（红色）
    Error = 2,
    /// 严重（最高级别，加粗红色）
    Critical = 3,
}

impl NotifyLevel {
    /// 转换为 Slack 颜色（hex 颜色码或 Slack 内置颜色名）
    ///
    /// - `Info` → `#36a64f`（绿色）
    /// - `Warning` → `#ffcc00`（黄色）
    /// - `Error` → `#ff0000`（红色）
    /// - `Critical` → `#b22222`（深红）
    pub fn slack_color(self) -> &'static str {
        match self {
            Self::Info => "#36a64f",
            Self::Warning => "#ffcc00",
            Self::Error => "#ff0000",
            Self::Critical => "#b22222",
        }
    }

    /// 转换为字符串标识（对齐 PHP `strtolower(Level::class)`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }
}

impl std::fmt::Display for NotifyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for NotifyLevel {
    type Err = NotifyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Self::Info),
            "warning" | "warn" => Ok(Self::Warning),
            "error" | "err" => Ok(Self::Error),
            "critical" | "crit" => Ok(Self::Critical),
            other => Err(NotifyError::MissingField(format!("未知通知级别: {other}"))),
        }
    }
}

// ============================================================================
// 通知消息（Builder 模式）
// ============================================================================

/// 通知消息 — 对齐 PHP `think\notify\Message`
///
/// 使用 Builder 模式构建通知内容，通过 [`Notifier::send`] 发送。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\facade\Notify
/// Notify::channel('slack')
///     ->title('部署完成')
///     ->content('服务已成功部署到生产环境')
///     ->level('info')
///     ->send();
/// ```
///
/// # Rust 用法
///
/// ```rust,ignore
/// use sz_rust_core::notify::{Notification, MemoryNotifier, NotifyLevel};
///
/// let msg = Notification::new()
///     .channel("slack")
///     .title("部署完成")
///     .content("服务已成功部署到生产环境")
///     .level(NotifyLevel::Info);
///
/// let notifier = MemoryNotifier::new();
/// notifier.send(msg).unwrap();
/// ```
#[derive(Debug, Clone, Default)]
pub struct Notification {
    /// 通知渠道（如 `slack`、`sms`、`mail`）
    pub channel: String,
    /// 通知标题
    pub title: String,
    /// 通知正文
    pub content: String,
    /// 通知级别
    pub level: NotifyLevel,
    /// 附加元数据（JSON 值，供具体 Notifier 使用）
    pub metadata: serde_json::Value,
}

impl Notification {
    /// 创建空通知消息
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置通知渠道
    pub fn channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = channel.into();
        self
    }

    /// 设置通知标题
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// 设置通知正文
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    /// 设置通知级别
    pub fn level(mut self, level: NotifyLevel) -> Self {
        self.level = level;
        self
    }

    /// 设置附加元数据
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// 校验必填字段
    ///
    /// # 返回
    ///
    /// - 渠道为空 → [`NotifyError::MissingField`]("channel")
    /// - 标题为空 → [`NotifyError::MissingField`]("title")
    /// - 内容为空 → [`NotifyError::MissingField`]("content")
    pub fn validate(&self) -> Result<(), NotifyError> {
        if self.channel.is_empty() {
            return Err(NotifyError::MissingField("channel".into()));
        }
        if self.title.is_empty() {
            return Err(NotifyError::MissingField("title".into()));
        }
        if self.content.is_empty() {
            return Err(NotifyError::MissingField("content".into()));
        }
        Ok(())
    }
}

// ============================================================================
// Notifier trait
// ============================================================================

/// 通知发送器 trait — 对齐 PHP `think\notify\Notifier`
///
/// 抽象通知发送行为，业务方实现具体发送逻辑（Slack Webhook / 短信 API / 日志等）。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\notify\Notifier 接口
/// interface Notifier {
///     public function send(Message $message): bool;
/// }
/// ```
pub trait Notifier: Send + Sync {
    /// 发送通知
    ///
    /// # 参数
    ///
    /// - `notification`: 通知消息
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`NotifyError`]。
    fn send(&self, notification: Notification) -> Result<(), NotifyError>;
}

// ============================================================================
// MemoryNotifier（测试/开发用实现）
// ============================================================================

/// 内存通知发送器 — 用于测试和开发环境
///
/// 不实际发送通知，而是将通知暂存到内部 Vec，供测试断言使用。
///
/// # 线程安全
///
/// 通过 `Arc<Mutex<Vec<Notification>>>` 保护，支持并发写入。
#[derive(Debug, Clone, Default)]
pub struct MemoryNotifier {
    /// 已"发送"的通知列表
    sent: Arc<Mutex<Vec<Notification>>>,
}

impl MemoryNotifier {
    /// 创建新的内存通知发送器
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取已发送通知数量
    pub fn count(&self) -> usize {
        self.sent.lock().len()
    }

    /// 获取所有已发送通知（快照）
    pub fn all(&self) -> Vec<Notification> {
        self.sent.lock().clone()
    }

    /// 获取最后发送的通知
    pub fn last(&self) -> Option<Notification> {
        self.sent.lock().last().cloned()
    }

    /// 清空已发送通知
    pub fn clear(&self) {
        self.sent.lock().clear();
    }
}

impl Notifier for MemoryNotifier {
    fn send(&self, notification: Notification) -> Result<(), NotifyError> {
        // 校验必要字段
        notification.validate()?;

        // 暂存到内存
        self.sent.lock().push(notification);
        Ok(())
    }
}

// ============================================================================
// HttpTransport trait（HTTP 传输抽象）
// ============================================================================

/// HTTP 传输 trait — 用于解耦 Notifier 与具体 HTTP 库
///
/// 业务方实现此 trait 注入 reqwest / hyper / etc.，即可让 [`SlackNotifier`] 等基于 HTTP 的
/// 通知器投入生产。
///
/// # 线程安全
///
/// 实现者必须保证 `Send + Sync`，因为 Notifier 通常作为单例在多线程下使用。
pub trait HttpTransport: Send + Sync {
    /// 发送 POST 请求，Content-Type: application/json
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    /// - `body`: 请求体（JSON 字符串）
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`NotifyError`]。
    fn post_json(&self, url: &str, body: &str) -> Result<(), NotifyError>;
}

// ============================================================================
// MemoryHttpTransport（测试/开发用 HTTP 传输实现）
// ============================================================================

/// 内存 HTTP 传输 — 用于测试和开发环境
///
/// 不实际发送 HTTP 请求，而是将请求暂存到内部 Vec，供测试断言使用。
#[derive(Debug, Default)]
pub struct MemoryHttpTransport {
    /// 已"发送"的 HTTP 请求列表（url, body）
    requests: Mutex<Vec<(String, String)>>,
}

impl MemoryHttpTransport {
    /// 创建新的内存 HTTP 传输
    pub fn new() -> Self {
        Self::default()
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

    /// 清空已发送请求
    pub fn clear(&self) {
        self.requests.lock().clear();
    }
}

impl HttpTransport for MemoryHttpTransport {
    fn post_json(&self, url: &str, body: &str) -> Result<(), NotifyError> {
        self.requests
            .lock()
            .push((url.to_string(), body.to_string()));
        Ok(())
    }
}

// ============================================================================
// Slack 配置
// ============================================================================

/// Slack Webhook 配置
///
/// 对齐 PHP `think\notify\driver\Slack` 的配置项。
#[derive(Debug, Clone)]
pub struct SlackConfig {
    /// Slack Webhook URL（必填，格式：`https://hooks.slack.com/services/...`）
    pub webhook_url: String,
    /// 目标频道（可选，对齐 PHP `channel` 配置；Webhook URL 已绑定频道时可为 None）
    pub channel: Option<String>,
    /// 发送者显示名（可选，对齐 PHP `username` 配置）
    pub username: Option<String>,
    /// 发送者图标 emoji（可选，对齐 PHP `icon_emoji` 配置，如 `:alarm_clock:`）
    pub icon_emoji: Option<String>,
}

impl SlackConfig {
    /// 创建 Slack 配置
    ///
    /// # 参数
    ///
    /// - `webhook_url`: Slack Webhook URL
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            channel: None,
            username: None,
            icon_emoji: None,
        }
    }

    /// 设置目标频道
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// 设置发送者显示名
    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// 设置发送者图标 emoji
    pub fn with_icon_emoji(mut self, icon_emoji: impl Into<String>) -> Self {
        self.icon_emoji = Some(icon_emoji.into());
        self
    }
}

// ============================================================================
// Slack Webhook 请求体
// ============================================================================

/// Slack Webhook 请求体 — 对齐 Slack API 的 `chat.postMessage` 兼容格式
///
/// 参考：https://api.slack.com/messaging/webhooks
#[derive(Debug, Clone, serde::Serialize)]
struct SlackPayload {
    /// 通知文本（必填，作为消息预览和 fallback）
    text: String,
    /// 目标频道（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
    /// 发送者显示名（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    /// 发送者图标 emoji（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    icon_emoji: Option<String>,
    /// Slack Block Kit 附件（用于富文本展示，含颜色条）
    attachments: Vec<SlackAttachment>,
}

/// Slack 附件 — 用于在消息旁显示颜色条（按通知级别着色）
#[derive(Debug, Clone, serde::Serialize)]
struct SlackAttachment {
    /// 颜色条颜色（hex 颜色码，如 `#ff0000`）
    color: String,
    /// 附件标题
    title: String,
    /// 附件正文
    text: String,
    /// 时间戳（Unix 秒，用于 Slack 消息时间显示）
    ts: i64,
}

// ============================================================================
// SlackNotifier
// ============================================================================

/// Slack 通知发送器 — 基于 Slack Webhook API
///
/// 通过 [`HttpTransport`] trait 抽象 HTTP 发送，业务方注入具体 HTTP 客户端即可使用。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_core::notify::{
///     SlackConfig, SlackNotifier, MemoryHttpTransport, Notification, NotifyLevel, Notifier,
/// };
///
/// let transport = std::sync::Arc::new(MemoryHttpTransport::new());
/// let config = SlackConfig::new("https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXX")
///     .with_channel("#alerts")
///     .with_username("SZ-Rust Bot")
///     .with_icon_emoji(":alarm_clock:");
/// let notifier = SlackNotifier::new(config, transport);
///
/// let msg = Notification::new()
///     .channel("slack")
///     .title("部署完成")
///     .content("服务已成功部署到生产环境")
///     .level(NotifyLevel::Info);
///
/// notifier.send(msg).unwrap();
/// ```
pub struct SlackNotifier {
    /// Slack 配置
    config: SlackConfig,
    /// HTTP 传输实现
    transport: Arc<dyn HttpTransport>,
}

impl SlackNotifier {
    /// 创建 Slack 通知发送器
    ///
    /// # 参数
    ///
    /// - `config`: Slack 配置
    /// - `transport`: HTTP 传输实现（业务方注入 reqwest / hyper / etc.）
    pub fn new(config: SlackConfig, transport: Arc<dyn HttpTransport>) -> Self {
        Self { config, transport }
    }

    /// 构造 Slack Webhook 请求体
    ///
    /// # 参数
    ///
    /// - `notification`: 通知消息
    ///
    /// # 返回
    ///
    /// 成功返回 JSON 字符串，失败返回 [`NotifyError`]。
    fn build_payload(&self, notification: &Notification) -> Result<String, NotifyError> {
        let ts = chrono::Utc::now().timestamp();
        let payload = SlackPayload {
            // Slack 要求 text 字段非空（作为消息预览和 fallback）
            text: format!(
                "[{}] {} — {}",
                notification.level.as_str().to_uppercase(),
                notification.title,
                notification.content
            ),
            channel: self.config.channel.clone(),
            username: self.config.username.clone(),
            icon_emoji: self.config.icon_emoji.clone(),
            attachments: vec![SlackAttachment {
                color: notification.level.slack_color().to_string(),
                title: notification.title.clone(),
                text: notification.content.clone(),
                ts,
            }],
        };

        serde_json::to_string(&payload).map_err(|e| NotifyError::Serialize(e.to_string()))
    }
}

impl Notifier for SlackNotifier {
    fn send(&self, notification: Notification) -> Result<(), NotifyError> {
        // 1. 校验通知字段
        notification.validate()?;

        // 2. 校验 webhook_url 非空
        if self.config.webhook_url.is_empty() {
            return Err(NotifyError::MissingField("webhook_url".into()));
        }

        // 3. 构造 Slack Webhook 请求体
        let body = self.build_payload(&notification)?;

        // 4. 通过 HttpTransport 发送
        self.transport
            .post_json(&self.config.webhook_url, &body)
            .map_err(|e| NotifyError::HttpTransport(format!("Slack Webhook 发送失败: {e}")))?;

        Ok(())
    }
}

// ============================================================================
// 短信消息（SmsMessage + SmsNotifier + MemorySmsNotifier）
// ============================================================================

/// 短信消息 — 对齐 PHP `think\notify\SmsMessage`
///
/// 使用 Builder 模式构建短信内容，通过 [`SmsNotifier::send_sms`] 发送。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\notify\SmsMessage
/// $sms = (new SmsMessage())
///     ->setPhone('13800138000')
///     ->setTemplateId('123456')
///     ->setTemplateParams(['1234']);
/// ```
///
/// # Rust 用法
///
/// ```rust,ignore
/// use sz_rust_core::notify::{SmsMessage, MemorySmsNotifier, SmsNotifier};
///
/// let msg = SmsMessage::new()
///     .phone("+8613800138000")
///     .template_id("123456")
///     .template_param("1234");
///
/// let notifier = MemorySmsNotifier::new();
/// notifier.send_sms(msg).unwrap();
/// ```
#[derive(Debug, Clone, Default)]
pub struct SmsMessage {
    /// 手机号（必填）
    pub phone: String,
    /// 短信模板 ID（必填，对齐腾讯云 TemplateId）
    pub template_id: String,
    /// 模板参数（按顺序匹配模板中的 {1}、{2}... 占位符）
    pub template_params: Vec<String>,
    /// 短信签名（可选，默认使用配置中的签名）
    pub sign_name: Option<String>,
    /// 附加元数据
    pub metadata: serde_json::Value,
}

impl SmsMessage {
    /// 创建空短信消息
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置手机号
    pub fn phone(mut self, phone: impl Into<String>) -> Self {
        self.phone = phone.into();
        self
    }

    /// 设置短信模板 ID
    pub fn template_id(mut self, template_id: impl Into<String>) -> Self {
        self.template_id = template_id.into();
        self
    }

    /// 追加单个模板参数
    pub fn template_param(mut self, param: impl Into<String>) -> Self {
        self.template_params.push(param.into());
        self
    }

    /// 替换全部模板参数
    pub fn template_params(mut self, params: Vec<String>) -> Self {
        self.template_params = params;
        self
    }

    /// 设置短信签名
    pub fn sign_name(mut self, sign_name: impl Into<String>) -> Self {
        self.sign_name = Some(sign_name.into());
        self
    }

    /// 设置附加元数据
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// 校验必填字段
    ///
    /// # 返回
    ///
    /// - 手机号为空 → [`NotifyError::MissingField`]("phone")
    /// - 模板 ID 为空 → [`NotifyError::MissingField`]("template_id")
    pub fn validate(&self) -> Result<(), NotifyError> {
        if self.phone.is_empty() {
            return Err(NotifyError::MissingField("phone".into()));
        }
        if self.template_id.is_empty() {
            return Err(NotifyError::MissingField("template_id".into()));
        }
        Ok(())
    }
}

/// 短信通知发送器 trait — 对齐 PHP `think\notify\SmsNotifier`
///
/// 抽象短信发送行为，业务方实现具体发送逻辑（腾讯云 / 阿里云 / etc.）。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\notify\SmsNotifier 接口
/// interface SmsNotifier {
///     public function sendSms(SmsMessage $message): bool;
/// }
/// ```
pub trait SmsNotifier: Send + Sync {
    /// 发送短信
    ///
    /// # 参数
    ///
    /// - `message`: 短信消息
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`NotifyError`]。
    fn send_sms(&self, message: SmsMessage) -> Result<(), NotifyError>;
}

/// 内存短信通知发送器 — 用于测试和开发环境
///
/// 不实际发送短信，而是将消息暂存到内部 Vec，供测试断言使用。
///
/// # 线程安全
///
/// 通过 `Arc<Mutex<Vec<SmsMessage>>>` 保护，支持并发写入。
#[derive(Debug, Clone, Default)]
pub struct MemorySmsNotifier {
    /// 已"发送"的短信列表
    sent: Arc<Mutex<Vec<SmsMessage>>>,
}

impl MemorySmsNotifier {
    /// 创建新的内存短信通知发送器
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取已发送短信数量
    pub fn count(&self) -> usize {
        self.sent.lock().len()
    }

    /// 获取所有已发送短信（快照）
    pub fn all(&self) -> Vec<SmsMessage> {
        self.sent.lock().clone()
    }

    /// 获取最后发送的短信
    pub fn last(&self) -> Option<SmsMessage> {
        self.sent.lock().last().cloned()
    }

    /// 清空已发送短信
    pub fn clear(&self) {
        self.sent.lock().clear();
    }
}

impl SmsNotifier for MemorySmsNotifier {
    fn send_sms(&self, message: SmsMessage) -> Result<(), NotifyError> {
        // 校验必要字段
        message.validate()?;
        // 暂存到内存
        self.sent.lock().push(message);
        Ok(())
    }
}

// ============================================================================
// 腾讯云短信配置
// ============================================================================

/// 腾讯云短信配置 — 对齐 `tencentcloud/tencentcloud-sdk-php` SmsClient
///
/// 对齐 PHP SDK `TencentCloud\Sms\V20210111\Models\SendSmsRequest` 的配置项。
#[derive(Debug, Clone)]
pub struct TencentSmsConfig {
    /// SecretId（必填）
    pub secret_id: String,
    /// SecretKey（必填）
    pub secret_key: String,
    /// 短信 AppId（必填，如 `1400000000`）
    pub app_id: String,
    /// 默认短信签名（可选，如 `鲜视达科技`）
    pub default_sign_name: Option<String>,
    /// 地域（默认 `ap-guangzhou`）
    pub region: String,
    /// API 端点（默认 `sms.tencentcloudapi.com`）
    pub endpoint: String,
}

impl TencentSmsConfig {
    /// 创建腾讯云短信配置
    ///
    /// # 参数
    ///
    /// - `secret_id`: 腾讯云 SecretId
    /// - `secret_key`: 腾讯云 SecretKey
    /// - `app_id`: 短信 AppId
    pub fn new(
        secret_id: impl Into<String>,
        secret_key: impl Into<String>,
        app_id: impl Into<String>,
    ) -> Self {
        Self {
            secret_id: secret_id.into(),
            secret_key: secret_key.into(),
            app_id: app_id.into(),
            default_sign_name: None,
            region: "ap-guangzhou".to_string(),
            endpoint: "sms.tencentcloudapi.com".to_string(),
        }
    }

    /// 设置默认短信签名
    pub fn with_default_sign_name(mut self, sign_name: impl Into<String>) -> Self {
        self.default_sign_name = Some(sign_name.into());
        self
    }

    /// 设置地域
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    /// 设置 API 端点
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

// ============================================================================
// 腾讯云短信请求体
// ============================================================================

/// 腾讯云短信请求体 — 对齐 `SendSms` action
///
/// 参考：https://cloud.tencent.com/document/product/382/55981
///
/// 注意：真实签名由业务方在实现 [`HttpTransport`] 时注入（如通过 header 携带
/// `Authorization`、`X-TC-Action`、`X-TC-Region` 等），此处仅构造 JSON body。
#[derive(Debug, Clone, serde::Serialize)]
struct TencentSmsPayload {
    /// 手机号集合（对齐腾讯云 `PhoneNumbers`）
    #[serde(rename = "PhoneNumbers")]
    phone_numbers: Vec<String>,
    /// 模板 ID（对齐腾讯云 `TemplateId`）
    #[serde(rename = "TemplateId")]
    template_id: String,
    /// 模板参数集合（对齐腾讯云 `TemplateParamSet`）
    #[serde(rename = "TemplateParamSet")]
    template_param_set: Vec<String>,
    /// 短信 SDK AppId（对齐腾讯云 `SmsSdkAppId`）
    #[serde(rename = "SmsSdkAppId")]
    sms_sdk_app_id: String,
    /// 短信签名（对齐腾讯云 `SignName`，可选）
    #[serde(rename = "SignName", skip_serializing_if = "Option::is_none")]
    sign_name: Option<String>,
}

// ============================================================================
// TencentSmsNotifier
// ============================================================================

/// 腾讯云短信通知发送器 — 基于腾讯云短信 API（`SendSms` action）
///
/// 通过 [`HttpTransport`] trait 抽象 HTTP 发送，业务方注入具体 HTTP 客户端即可使用。
/// 真实签名由业务方在实现 [`HttpTransport`] 时注入（如通过 header 携带
/// `Authorization`、`X-TC-Action` 等），本实现仅构造请求 JSON body。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_core::notify::{
///     TencentSmsConfig, TencentSmsNotifier, MemoryHttpTransport, SmsMessage, SmsNotifier,
/// };
/// use std::sync::Arc;
///
/// let transport = Arc::new(MemoryHttpTransport::new());
/// let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000")
///     .with_default_sign_name("鲜视达科技");
/// let notifier = TencentSmsNotifier::new(config, transport);
///
/// let msg = SmsMessage::new()
///     .phone("+8613800138000")
///     .template_id("123456")
///     .template_param("1234");
///
/// notifier.send_sms(msg).unwrap();
/// ```
pub struct TencentSmsNotifier {
    /// 腾讯云短信配置
    config: TencentSmsConfig,
    /// HTTP 传输实现
    transport: Arc<dyn HttpTransport>,
}

impl TencentSmsNotifier {
    /// 创建腾讯云短信通知发送器
    ///
    /// # 参数
    ///
    /// - `config`: 腾讯云短信配置
    /// - `transport`: HTTP 传输实现（业务方注入 reqwest / hyper / etc.）
    pub fn new(config: TencentSmsConfig, transport: Arc<dyn HttpTransport>) -> Self {
        Self { config, transport }
    }

    /// 构造腾讯云短信请求体
    ///
    /// # 参数
    ///
    /// - `message`: 短信消息
    ///
    /// # 返回
    ///
    /// 成功返回 JSON 字符串，失败返回 [`NotifyError`]。
    ///
    /// # 说明
    ///
    /// 若 `message.sign_name` 为 `None`，则回退使用配置中的默认签名
    /// `config.default_sign_name`；两者均为 `None` 时，请求体中不包含 `SignName` 字段。
    fn build_payload(&self, message: &SmsMessage) -> Result<String, NotifyError> {
        let sign_name = message
            .sign_name
            .clone()
            .or_else(|| self.config.default_sign_name.clone());

        let payload = TencentSmsPayload {
            phone_numbers: vec![message.phone.clone()],
            template_id: message.template_id.clone(),
            template_param_set: message.template_params.clone(),
            sms_sdk_app_id: self.config.app_id.clone(),
            sign_name,
        };

        serde_json::to_string(&payload).map_err(|e| NotifyError::Serialize(e.to_string()))
    }
}

impl SmsNotifier for TencentSmsNotifier {
    fn send_sms(&self, message: SmsMessage) -> Result<(), NotifyError> {
        // 1. 校验短信消息字段
        message.validate()?;

        // 2. 校验腾讯云凭据
        if self.config.secret_id.is_empty() {
            return Err(NotifyError::MissingField("secret_id".into()));
        }
        if self.config.secret_key.is_empty() {
            return Err(NotifyError::MissingField("secret_key".into()));
        }
        if self.config.app_id.is_empty() {
            return Err(NotifyError::MissingField("app_id".into()));
        }

        // 3. 构造请求体
        let body = self.build_payload(&message)?;

        // 4. 构造请求 URL（对齐腾讯云 API 端点格式）
        let url = format!("https://{}/", self.config.endpoint);

        // 5. 通过 HttpTransport 发送
        self.transport
            .post_json(&url, &body)
            .map_err(|e| NotifyError::HttpTransport(format!("腾讯云短信发送失败: {e}")))?;

        Ok(())
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // NotifyLevel 测试
    // ------------------------------------------------------------------------

    /// 测试 NotifyLevel 默认值为 Info
    #[test]
    fn test_notify_level_default() {
        let level = NotifyLevel::default();
        assert_eq!(level, NotifyLevel::Info);
    }

    /// 测试 NotifyLevel::slack_color 返回正确的颜色码
    #[test]
    fn test_notify_level_slack_color() {
        assert_eq!(NotifyLevel::Info.slack_color(), "#36a64f");
        assert_eq!(NotifyLevel::Warning.slack_color(), "#ffcc00");
        assert_eq!(NotifyLevel::Error.slack_color(), "#ff0000");
        assert_eq!(NotifyLevel::Critical.slack_color(), "#b22222");
    }

    /// 测试 NotifyLevel::as_str 返回正确的字符串标识
    #[test]
    fn test_notify_level_as_str() {
        assert_eq!(NotifyLevel::Info.as_str(), "info");
        assert_eq!(NotifyLevel::Warning.as_str(), "warning");
        assert_eq!(NotifyLevel::Error.as_str(), "error");
        assert_eq!(NotifyLevel::Critical.as_str(), "critical");
    }

    /// 测试 NotifyLevel::Display 实现
    #[test]
    fn test_notify_level_display() {
        assert_eq!(format!("{}", NotifyLevel::Info), "info");
        assert_eq!(format!("{}", NotifyLevel::Warning), "warning");
        assert_eq!(format!("{}", NotifyLevel::Error), "error");
        assert_eq!(format!("{}", NotifyLevel::Critical), "critical");
    }

    /// 测试 NotifyLevel::FromStr 实现（含别名和大小写不敏感）
    #[test]
    fn test_notify_level_from_str() {
        // 标准名称
        assert_eq!("info".parse::<NotifyLevel>().unwrap(), NotifyLevel::Info);
        assert_eq!(
            "warning".parse::<NotifyLevel>().unwrap(),
            NotifyLevel::Warning
        );
        assert_eq!("error".parse::<NotifyLevel>().unwrap(), NotifyLevel::Error);
        assert_eq!(
            "critical".parse::<NotifyLevel>().unwrap(),
            NotifyLevel::Critical
        );

        // 别名
        assert_eq!("warn".parse::<NotifyLevel>().unwrap(), NotifyLevel::Warning);
        assert_eq!("err".parse::<NotifyLevel>().unwrap(), NotifyLevel::Error);
        assert_eq!(
            "crit".parse::<NotifyLevel>().unwrap(),
            NotifyLevel::Critical
        );

        // 大小写不敏感
        assert_eq!("INFO".parse::<NotifyLevel>().unwrap(), NotifyLevel::Info);
        assert_eq!(
            "Critical".parse::<NotifyLevel>().unwrap(),
            NotifyLevel::Critical
        );

        // 未知级别
        assert!("unknown".parse::<NotifyLevel>().is_err());
    }

    // ------------------------------------------------------------------------
    // Notification 测试
    // ------------------------------------------------------------------------

    /// 测试 Notification builder 模式
    #[test]
    fn test_notification_builder() {
        let notification = Notification::new()
            .channel("slack")
            .title("部署完成")
            .content("服务已成功部署到生产环境")
            .level(NotifyLevel::Info)
            .metadata(serde_json::json!({"env": "prod"}));

        assert_eq!(notification.channel, "slack");
        assert_eq!(notification.title, "部署完成");
        assert_eq!(notification.content, "服务已成功部署到生产环境");
        assert_eq!(notification.level, NotifyLevel::Info);
        assert_eq!(notification.metadata["env"], "prod");
    }

    /// 测试 Notification 默认值
    #[test]
    fn test_notification_default() {
        let notification = Notification::default();
        assert!(notification.channel.is_empty());
        assert!(notification.title.is_empty());
        assert!(notification.content.is_empty());
        assert_eq!(notification.level, NotifyLevel::Info);
        assert!(notification.metadata.is_null());
    }

    /// 测试 Notification::validate 校验必填字段
    #[test]
    fn test_notification_validate_ok() {
        let notification = Notification::new()
            .channel("slack")
            .title("标题")
            .content("内容");
        assert!(notification.validate().is_ok());
    }

    /// 测试 Notification::validate 缺少渠道
    #[test]
    fn test_notification_validate_missing_channel() {
        let notification = Notification::new().title("标题").content("内容");
        let err = notification.validate().unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "channel"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    /// 测试 Notification::validate 缺少标题
    #[test]
    fn test_notification_validate_missing_title() {
        let notification = Notification::new().channel("slack").content("内容");
        let err = notification.validate().unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "title"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    /// 测试 Notification::validate 缺少内容
    #[test]
    fn test_notification_validate_missing_content() {
        let notification = Notification::new().channel("slack").title("标题");
        let err = notification.validate().unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "content"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // MemoryNotifier 测试
    // ------------------------------------------------------------------------

    /// 测试 MemoryNotifier 发送通知
    #[test]
    fn test_memory_notifier_send() {
        let notifier = MemoryNotifier::new();
        let notification = Notification::new()
            .channel("slack")
            .title("标题")
            .content("内容")
            .level(NotifyLevel::Warning);

        notifier.send(notification).unwrap();
        assert_eq!(notifier.count(), 1);

        let last = notifier.last().unwrap();
        assert_eq!(last.channel, "slack");
        assert_eq!(last.title, "标题");
        assert_eq!(last.content, "内容");
        assert_eq!(last.level, NotifyLevel::Warning);
    }

    /// 测试 MemoryNotifier 发送多封通知
    #[test]
    fn test_memory_notifier_send_multiple() {
        let notifier = MemoryNotifier::new();
        for i in 0..5 {
            notifier
                .send(
                    Notification::new()
                        .channel("slack")
                        .title(format!("标题{i}"))
                        .content("内容"),
                )
                .unwrap();
        }
        assert_eq!(notifier.count(), 5);

        let all = notifier.all();
        assert_eq!(all[0].title, "标题0");
        assert_eq!(all[4].title, "标题4");
    }

    /// 测试 MemoryNotifier 发送无效通知返回错误
    #[test]
    fn test_memory_notifier_send_invalid() {
        let notifier = MemoryNotifier::new();
        let notification = Notification::new().title("标题").content("内容");
        // 缺少 channel
        assert!(notifier.send(notification).is_err());
        assert_eq!(notifier.count(), 0);
    }

    /// 测试 MemoryNotifier clear
    #[test]
    fn test_memory_notifier_clear() {
        let notifier = MemoryNotifier::new();
        notifier
            .send(
                Notification::new()
                    .channel("slack")
                    .title("标题")
                    .content("内容"),
            )
            .unwrap();
        assert_eq!(notifier.count(), 1);

        notifier.clear();
        assert_eq!(notifier.count(), 0);
        assert!(notifier.last().is_none());
    }

    // ------------------------------------------------------------------------
    // MemoryHttpTransport 测试
    // ------------------------------------------------------------------------

    /// 测试 MemoryHttpTransport 记录请求
    #[test]
    fn test_memory_http_transport_post_json() {
        let transport = MemoryHttpTransport::new();
        transport
            .post_json("https://hooks.slack.com/services/xxx", r#"{"text":"hi"}"#)
            .unwrap();

        assert_eq!(transport.count(), 1);
        let (url, body) = transport.last().unwrap();
        assert_eq!(url, "https://hooks.slack.com/services/xxx");
        assert_eq!(body, r#"{"text":"hi"}"#);
    }

    /// 测试 MemoryHttpTransport clear
    #[test]
    fn test_memory_http_transport_clear() {
        let transport = MemoryHttpTransport::new();
        transport.post_json("url", "body").unwrap();
        assert_eq!(transport.count(), 1);

        transport.clear();
        assert_eq!(transport.count(), 0);
    }

    // ------------------------------------------------------------------------
    // SlackConfig 测试
    // ------------------------------------------------------------------------

    /// 测试 SlackConfig builder 模式
    #[test]
    fn test_slack_config_builder() {
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X")
            .with_channel("#alerts")
            .with_username("SZ-Rust Bot")
            .with_icon_emoji(":alarm_clock:");

        assert_eq!(config.webhook_url, "https://hooks.slack.com/services/T/B/X");
        assert_eq!(config.channel.as_deref(), Some("#alerts"));
        assert_eq!(config.username.as_deref(), Some("SZ-Rust Bot"));
        assert_eq!(config.icon_emoji.as_deref(), Some(":alarm_clock:"));
    }

    /// 测试 SlackConfig 默认值（仅 webhook_url）
    #[test]
    fn test_slack_config_minimal() {
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        assert_eq!(config.webhook_url, "https://hooks.slack.com/services/T/B/X");
        assert!(config.channel.is_none());
        assert!(config.username.is_none());
        assert!(config.icon_emoji.is_none());
    }

    // ------------------------------------------------------------------------
    // SlackNotifier 测试
    // ------------------------------------------------------------------------

    /// 测试 SlackNotifier 发送通知（通过 MemoryHttpTransport 验证请求体）
    #[test]
    fn test_slack_notifier_send() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X")
            .with_channel("#alerts")
            .with_username("SZ-Rust Bot")
            .with_icon_emoji(":alarm_clock:");
        let notifier = SlackNotifier::new(config, transport.clone());

        let notification = Notification::new()
            .channel("slack")
            .title("部署完成")
            .content("服务已成功部署到生产环境")
            .level(NotifyLevel::Info);

        notifier.send(notification).unwrap();

        // 验证 HTTP 请求
        assert_eq!(transport.count(), 1);
        let (url, body) = transport.last().unwrap();
        assert_eq!(url, "https://hooks.slack.com/services/T/B/X");

        // 解析 body 验证结构
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(payload["text"].as_str().unwrap().contains("部署完成"));
        assert!(payload["text"]
            .as_str()
            .unwrap()
            .contains("服务已成功部署到生产环境"));
        assert!(payload["text"].as_str().unwrap().contains("[INFO]"));
        assert_eq!(payload["channel"], "#alerts");
        assert_eq!(payload["username"], "SZ-Rust Bot");
        assert_eq!(payload["icon_emoji"], ":alarm_clock:");

        // 验证 attachments
        let attachments = payload["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["color"], "#36a64f"); // Info level color
        assert_eq!(attachments[0]["title"], "部署完成");
        assert_eq!(attachments[0]["text"], "服务已成功部署到生产环境");
        assert!(attachments[0]["ts"].as_i64().is_some());
    }

    /// 测试 SlackNotifier 不同级别使用不同颜色
    #[test]
    fn test_slack_notifier_level_colors() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        let notifier = SlackNotifier::new(config, transport.clone());

        // Warning
        notifier
            .send(
                Notification::new()
                    .channel("slack")
                    .title("w")
                    .content("c")
                    .level(NotifyLevel::Warning),
            )
            .unwrap();
        let (_, body) = transport.last().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["attachments"][0]["color"], "#ffcc00");

        // Error
        notifier
            .send(
                Notification::new()
                    .channel("slack")
                    .title("w")
                    .content("c")
                    .level(NotifyLevel::Error),
            )
            .unwrap();
        let (_, body) = transport.last().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["attachments"][0]["color"], "#ff0000");

        // Critical
        notifier
            .send(
                Notification::new()
                    .channel("slack")
                    .title("w")
                    .content("c")
                    .level(NotifyLevel::Critical),
            )
            .unwrap();
        let (_, body) = transport.last().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["attachments"][0]["color"], "#b22222");

        assert_eq!(transport.count(), 3);
    }

    /// 测试 SlackNotifier 缺少 channel 字段返回错误
    #[test]
    fn test_slack_notifier_missing_channel() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        let notifier = SlackNotifier::new(config, transport.clone());

        let notification = Notification::new().title("标题").content("内容");
        let err = notifier.send(notification).unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "channel"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 不应发送任何 HTTP 请求
        assert_eq!(transport.count(), 0);
    }

    /// 测试 SlackNotifier 缺少 webhook_url 返回错误
    #[test]
    fn test_slack_notifier_missing_webhook_url() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new(""); // 空 webhook_url
        let notifier = SlackNotifier::new(config, transport.clone());

        let notification = Notification::new()
            .channel("slack")
            .title("标题")
            .content("内容");
        let err = notifier.send(notification).unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "webhook_url"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        assert_eq!(transport.count(), 0);
    }

    /// 测试 SlackNotifier 在 HttpTransport 失败时返回错误
    #[test]
    fn test_slack_notifier_http_failure() {
        // 自定义失败 HttpTransport
        struct FailingTransport;
        impl HttpTransport for FailingTransport {
            fn post_json(&self, _url: &str, _body: &str) -> Result<(), NotifyError> {
                Err(NotifyError::HttpTransport("connection refused".to_string()))
            }
        }

        let transport = Arc::new(FailingTransport);
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        let notifier = SlackNotifier::new(config, transport);

        let notification = Notification::new()
            .channel("slack")
            .title("标题")
            .content("内容");
        let err = notifier.send(notification).unwrap_err();
        match err {
            NotifyError::HttpTransport(msg) => assert!(msg.contains("connection refused")),
            other => panic!("期望 HttpTransport, 实际 {other:?}"),
        }
    }

    /// 测试 SlackNotifier build_payload 序列化结果正确
    #[test]
    fn test_slack_notifier_build_payload() {
        let transport: Arc<dyn HttpTransport> = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X")
            .with_channel("#alerts")
            .with_username("Bot")
            .with_icon_emoji(":bell:");
        let notifier = SlackNotifier::new(config, transport.clone());

        let notification = Notification::new()
            .channel("slack")
            .title("Test Title")
            .content("Test Content")
            .level(NotifyLevel::Error);

        let payload_json = notifier.build_payload(&notification).unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();

        // 验证 text 字段格式
        assert_eq!(
            payload["text"].as_str().unwrap(),
            "[ERROR] Test Title — Test Content"
        );

        // 验证可选字段
        assert_eq!(payload["channel"], "#alerts");
        assert_eq!(payload["username"], "Bot");
        assert_eq!(payload["icon_emoji"], ":bell:");

        // 验证 attachments
        let attachments = payload["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["color"], "#ff0000");
        assert_eq!(attachments[0]["title"], "Test Title");
        assert_eq!(attachments[0]["text"], "Test Content");
        assert!(attachments[0]["ts"].as_i64().is_some());
    }

    /// 测试 SlackNotifier 多次发送计数
    #[test]
    fn test_slack_notifier_send_multiple() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        let notifier = SlackNotifier::new(config, transport.clone());

        for i in 0..3 {
            notifier
                .send(
                    Notification::new()
                        .channel("slack")
                        .title(format!("Title {i}"))
                        .content("content"),
                )
                .unwrap();
        }
        assert_eq!(transport.count(), 3);
    }

    /// 测试 SlackNotifier 使用最小配置（仅 webhook_url）
    #[test]
    fn test_slack_notifier_minimal_config() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        let notifier = SlackNotifier::new(config, transport.clone());

        notifier
            .send(
                Notification::new()
                    .channel("slack")
                    .title("Title")
                    .content("Content"),
            )
            .unwrap();

        let (_, body) = transport.last().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();

        // 可选字段不应出现在序列化结果中（skip_serializing_if）
        assert!(payload.get("channel").is_none());
        assert!(payload.get("username").is_none());
        assert!(payload.get("icon_emoji").is_none());
    }

    /// 测试 SlackNotifier metadata 字段不影响发送
    #[test]
    fn test_slack_notifier_with_metadata() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = SlackConfig::new("https://hooks.slack.com/services/T/B/X");
        let notifier = SlackNotifier::new(config, transport.clone());

        notifier
            .send(
                Notification::new()
                    .channel("slack")
                    .title("Title")
                    .content("Content")
                    .metadata(serde_json::json!({"env": "prod", "version": "1.0.0"})),
            )
            .unwrap();

        assert_eq!(transport.count(), 1);
    }

    // ------------------------------------------------------------------------
    // SmsMessage 测试
    // ------------------------------------------------------------------------

    /// 测试 SmsMessage builder 模式
    #[test]
    fn test_sms_message_builder() {
        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456")
            .template_param("1234")
            .template_param("5")
            .sign_name("鲜视达科技")
            .metadata(serde_json::json!({"scene": "login"}));

        assert_eq!(msg.phone, "+8613800138000");
        assert_eq!(msg.template_id, "123456");
        assert_eq!(msg.template_params, vec!["1234", "5"]);
        assert_eq!(msg.sign_name.as_deref(), Some("鲜视达科技"));
        assert_eq!(msg.metadata["scene"], "login");
    }

    /// 测试 SmsMessage::validate 成功
    #[test]
    fn test_sms_message_validate_ok() {
        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456");
        assert!(msg.validate().is_ok());
    }

    /// 测试 SmsMessage::validate 缺少手机号
    #[test]
    fn test_sms_message_validate_missing_phone() {
        let msg = SmsMessage::new().template_id("123456");
        let err = msg.validate().unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "phone"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    /// 测试 SmsMessage::validate 缺少模板 ID
    #[test]
    fn test_sms_message_validate_missing_template_id() {
        let msg = SmsMessage::new().phone("+8613800138000");
        let err = msg.validate().unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "template_id"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // MemorySmsNotifier 测试
    // ------------------------------------------------------------------------

    /// 测试 MemorySmsNotifier 发送短信
    #[test]
    fn test_memory_sms_notifier_send() {
        let notifier = MemorySmsNotifier::new();
        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456")
            .template_param("1234");

        notifier.send_sms(msg).unwrap();
        assert_eq!(notifier.count(), 1);

        let last = notifier.last().unwrap();
        assert_eq!(last.phone, "+8613800138000");
        assert_eq!(last.template_id, "123456");
        assert_eq!(last.template_params, vec!["1234"]);
    }

    /// 测试 MemorySmsNotifier 发送多条短信
    #[test]
    fn test_memory_sms_notifier_send_multiple() {
        let notifier = MemorySmsNotifier::new();
        for i in 0..5 {
            notifier
                .send_sms(
                    SmsMessage::new()
                        .phone(format!("+861380013{i:04}"))
                        .template_id("123456"),
                )
                .unwrap();
        }
        assert_eq!(notifier.count(), 5);

        let all = notifier.all();
        assert_eq!(all[0].phone, "+8613800130000");
        assert_eq!(all[4].phone, "+8613800130004");
    }

    /// 测试 MemorySmsNotifier 发送无效短信返回错误
    #[test]
    fn test_memory_sms_notifier_send_invalid() {
        let notifier = MemorySmsNotifier::new();
        let msg = SmsMessage::new().template_id("123456");
        // 缺少 phone
        assert!(notifier.send_sms(msg).is_err());
        assert_eq!(notifier.count(), 0);
    }

    /// 测试 MemorySmsNotifier clear
    #[test]
    fn test_memory_sms_notifier_clear() {
        let notifier = MemorySmsNotifier::new();
        notifier
            .send_sms(
                SmsMessage::new()
                    .phone("+8613800138000")
                    .template_id("123456"),
            )
            .unwrap();
        assert_eq!(notifier.count(), 1);

        notifier.clear();
        assert_eq!(notifier.count(), 0);
        assert!(notifier.last().is_none());
    }

    // ------------------------------------------------------------------------
    // TencentSmsConfig 测试
    // ------------------------------------------------------------------------

    /// 测试 TencentSmsConfig builder 模式
    #[test]
    fn test_tencent_sms_config_builder() {
        let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000")
            .with_default_sign_name("鲜视达科技")
            .with_region("ap-beijing")
            .with_endpoint("sms.tencentcloudapi.com");

        assert_eq!(config.secret_id, "AKIDxxx");
        assert_eq!(config.secret_key, "SKxxx");
        assert_eq!(config.app_id, "1400000000");
        assert_eq!(config.default_sign_name.as_deref(), Some("鲜视达科技"));
        assert_eq!(config.region, "ap-beijing");
        assert_eq!(config.endpoint, "sms.tencentcloudapi.com");
    }

    /// 测试 TencentSmsConfig 默认值（仅必填项）
    #[test]
    fn test_tencent_sms_config_minimal() {
        let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000");
        assert_eq!(config.secret_id, "AKIDxxx");
        assert_eq!(config.secret_key, "SKxxx");
        assert_eq!(config.app_id, "1400000000");
        assert!(config.default_sign_name.is_none());
        assert_eq!(config.region, "ap-guangzhou");
        assert_eq!(config.endpoint, "sms.tencentcloudapi.com");
    }

    // ------------------------------------------------------------------------
    // TencentSmsNotifier 测试
    // ------------------------------------------------------------------------

    /// 测试 TencentSmsNotifier 发送短信（通过 MemoryHttpTransport 验证请求体）
    #[test]
    fn test_tencent_sms_notifier_send() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000")
            .with_default_sign_name("鲜视达科技");
        let notifier = TencentSmsNotifier::new(config, transport.clone());

        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456")
            .template_param("1234")
            .template_param("5");

        notifier.send_sms(msg).unwrap();

        // 验证 HTTP 请求
        assert_eq!(transport.count(), 1);
        let (url, body) = transport.last().unwrap();
        assert_eq!(url, "https://sms.tencentcloudapi.com/");

        // 解析 body 验证结构
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["PhoneNumbers"][0], "+8613800138000");
        assert_eq!(payload["TemplateId"], "123456");
        assert_eq!(payload["TemplateParamSet"][0], "1234");
        assert_eq!(payload["TemplateParamSet"][1], "5");
        assert_eq!(payload["SmsSdkAppId"], "1400000000");
        assert_eq!(payload["SignName"], "鲜视达科技");
    }

    /// 测试 TencentSmsNotifier 缺少手机号返回错误
    #[test]
    fn test_tencent_sms_notifier_missing_phone() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000");
        let notifier = TencentSmsNotifier::new(config, transport.clone());

        let msg = SmsMessage::new().template_id("123456");
        let err = notifier.send_sms(msg).unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "phone"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 不应发送任何 HTTP 请求
        assert_eq!(transport.count(), 0);
    }

    /// 测试 TencentSmsNotifier 缺少凭据返回错误
    #[test]
    fn test_tencent_sms_notifier_missing_credentials() {
        let transport = Arc::new(MemoryHttpTransport::new());
        // 空 secret_id
        let config = TencentSmsConfig::new("", "SKxxx", "1400000000");
        let notifier = TencentSmsNotifier::new(config, transport.clone());

        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456");
        let err = notifier.send_sms(msg).unwrap_err();
        match err {
            NotifyError::MissingField(field) => assert_eq!(field, "secret_id"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 不应发送任何 HTTP 请求
        assert_eq!(transport.count(), 0);

        // 验证空 secret_key
        let config2 = TencentSmsConfig::new("AKIDxxx", "", "1400000000");
        let notifier2 = TencentSmsNotifier::new(config2, transport.clone());
        let msg2 = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456");
        let err2 = notifier2.send_sms(msg2).unwrap_err();
        match err2 {
            NotifyError::MissingField(field) => assert_eq!(field, "secret_key"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 验证空 app_id
        let config3 = TencentSmsConfig::new("AKIDxxx", "SKxxx", "");
        let notifier3 = TencentSmsNotifier::new(config3, transport.clone());
        let msg3 = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456");
        let err3 = notifier3.send_sms(msg3).unwrap_err();
        match err3 {
            NotifyError::MissingField(field) => assert_eq!(field, "app_id"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 全程不应发送任何 HTTP 请求
        assert_eq!(transport.count(), 0);
    }

    /// 测试 TencentSmsNotifier 使用默认签名
    #[test]
    fn test_tencent_sms_notifier_uses_default_sign_name() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000")
            .with_default_sign_name("鲜视达科技");
        let notifier = TencentSmsNotifier::new(config, transport.clone());

        // 消息未设置 sign_name，应使用 config 的 default_sign_name
        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456");

        notifier.send_sms(msg).unwrap();

        let (_, body) = transport.last().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["SignName"], "鲜视达科技");

        // 验证消息级 sign_name 优先于 config 的 default_sign_name
        let msg2 = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456")
            .sign_name("覆盖签名");
        notifier.send_sms(msg2).unwrap();

        let (_, body2) = transport.last().unwrap();
        let payload2: serde_json::Value = serde_json::from_str(&body2).unwrap();
        assert_eq!(payload2["SignName"], "覆盖签名");

        assert_eq!(transport.count(), 2);
    }

    /// 测试 TencentSmsNotifier 无签名时请求体不包含 SignName 字段
    #[test]
    fn test_tencent_sms_notifier_no_sign_name() {
        let transport = Arc::new(MemoryHttpTransport::new());
        let config = TencentSmsConfig::new("AKIDxxx", "SKxxx", "1400000000");
        let notifier = TencentSmsNotifier::new(config, transport.clone());

        let msg = SmsMessage::new()
            .phone("+8613800138000")
            .template_id("123456");

        notifier.send_sms(msg).unwrap();

        let (_, body) = transport.last().unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        // 无签名时 SignName 字段不应出现在请求体中
        assert!(payload.get("SignName").is_none());
    }
}

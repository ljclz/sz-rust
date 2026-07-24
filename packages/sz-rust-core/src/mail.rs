//! Mail 模块 — 对齐 PHP `think\facade\Mail`
//!
//! Phase P3-16 交付物。本模块实现邮件抽象层，对齐 PHP `think\facade\Mail` 的核心 API。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Mail::to($address)` | [`MailMessage::to`] | 添加收件人 |
//! | `Mail::cc($address)` | [`MailMessage::cc`] | 添加抄送人 |
//! | `Mail::bcc($address)` | [`MailMessage::bcc`] | 添加密送人 |
//! | `Mail::from($address)` | [`MailMessage::from`] | 设置发件人 |
//! | `Mail::subject($subject)` | [`MailMessage::subject`] | 设置主题 |
//! | `Mail::html($content)` | [`MailMessage::html`] | 设置 HTML 内容 |
//! | `Mail::text($content)` | [`MailMessage::text`] | 设置纯文本内容 |
//! | `Mail::attach($file)` | [`MailMessage::attach`] | 添加附件 |
//! | `Mail::send()` | [`Mailer::send`] | 发送邮件 |
//!
//! ### PHP 行为对齐
//!
//! - **Builder 模式**：PHP `Mail::to()->subject()->html()->send()` 链式调用。
//!   Rust 通过 [`MailMessage`] builder 实现相同链式 API。
//! - **多收件人**：PHP `to()` 支持数组或逗号分隔字符串。Rust 通过多次调用 `to()` 累加。
//! - **双内容格式**：PHP 支持 `html()` 和 `text()` 两种内容。Rust 同样支持两种。
//!
//! ## 架构说明
//!
//! - **Mailer trait 抽象**：对齐 PHP `think\mail\Mailer` 接口，业务方实现具体发送逻辑
//! - **MemoryMailer**：内置内存实现，将邮件暂存到 Vec，用于测试和开发环境
//! - **无外部依赖**：不依赖 `lettre` crate，保持框架核心包依赖最小化
//! - **SMTP 实现延后**：生产环境 SMTP 发送由业务方通过 feature gate 或独立包实现

use parking_lot::Mutex;
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// Mail 错误
#[derive(Debug, Error)]
pub enum MailError {
    /// 缺少必填字段（收件人、主题、内容等）
    #[error("邮件字段缺失: {0}")]
    MissingField(String),
    /// 邮件发送失败
    #[error("邮件发送失败: {0}")]
    SendFailed(String),
    /// 附件读取失败
    #[error("附件读取失败: {path} — {source}")]
    AttachmentRead {
        /// 附件路径
        path: String,
        /// 底层 IO 错误
        #[source]
        source: std::io::Error,
    },
}

// ============================================================================
// 邮件地址
// ============================================================================

/// 邮件地址（含可选显示名）
///
/// 对齐 PHP `think\mail\Address` 的地址表示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailAddress {
    /// 邮箱地址
    pub email: String,
    /// 可选显示名（如 `"张三" <zhangsan@example.com>` 中的 `张三`）
    pub name: Option<String>,
}

impl MailAddress {
    /// 创建仅含邮箱的地址
    ///
    /// # 参数
    ///
    /// - `email`: 邮箱地址
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            name: None,
        }
    }

    /// 创建含显示名的地址
    ///
    /// # 参数
    ///
    /// - `email`: 邮箱地址
    /// - `name`: 显示名
    pub fn with_name(email: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            name: Some(name.into()),
        }
    }

    /// 格式化为 RFC 5322 地址字符串
    ///
    /// - 无显示名：`user@example.com`
    /// - 有显示名：`"张三" <user@example.com>`
    pub fn to_rfc5322_string(&self) -> String {
        match &self.name {
            Some(name) => format!("\"{}\" <{}>", name, self.email),
            None => self.email.clone(),
        }
    }
}

impl From<&str> for MailAddress {
    fn from(email: &str) -> Self {
        Self::new(email)
    }
}

impl From<String> for MailAddress {
    fn from(email: String) -> Self {
        Self::new(email)
    }
}

// ============================================================================
// 附件
// ============================================================================

/// 邮件附件
///
/// 对齐 PHP `think\mail\Attachment`。
#[derive(Debug, Clone)]
pub struct MailAttachment {
    /// 附件文件名
    pub filename: String,
    /// 附件内容（字节）
    pub content: Vec<u8>,
    /// MIME 类型（如 `application/pdf`）
    pub mime_type: String,
}

impl MailAttachment {
    /// 从字节数据创建附件
    ///
    /// # 参数
    ///
    /// - `filename`: 文件名
    /// - `content`: 文件内容字节
    /// - `mime_type`: MIME 类型
    pub fn new(filename: impl Into<String>, content: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            content,
            mime_type: mime_type.into(),
        }
    }

    /// 从文件路径创建附件
    ///
    /// # 参数
    ///
    /// - `path`: 文件路径
    /// - `filename`: 指定文件名（`None` 时使用路径中的文件名）
    /// - `mime_type`: MIME 类型
    ///
    /// # 返回
    ///
    /// 成功返回 [`MailAttachment`]，失败返回 [`MailError::AttachmentRead`]。
    pub fn from_file(
        path: impl AsRef<std::path::Path>,
        filename: Option<&str>,
        mime_type: impl Into<String>,
    ) -> Result<Self, MailError> {
        let path_ref = path.as_ref();
        let content = std::fs::read(path_ref).map_err(|e| MailError::AttachmentRead {
            path: path_ref.display().to_string(),
            source: e,
        })?;

        let filename = match filename {
            Some(name) => name.to_string(),
            None => path_ref
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment")
                .to_string(),
        };

        Ok(Self::new(filename, content, mime_type))
    }
}

// ============================================================================
// 邮件消息（Builder 模式）
// ============================================================================

/// 邮件消息 — 对齐 PHP `think\mail\Message`
///
/// 使用 Builder 模式构建邮件内容，通过 [`Mailer::send`] 发送。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\facade\Mail
/// Mail::to('user@example.com')
///     ->subject('Hello')
///     ->html('<h1>Welcome</h1>')
///     ->send();
/// ```
///
/// # Rust 用法
///
/// ```rust,ignore
/// use sz_rust_core::mail::{MailMessage, MemoryMailer};
///
/// let msg = MailMessage::new()
///     .to("user@example.com")
///     .subject("Hello")
///     .html("<h1>Welcome</h1>");
///
/// let mailer = MemoryMailer::new();
/// mailer.send(msg).unwrap();
/// ```
#[derive(Debug, Clone, Default)]
pub struct MailMessage {
    /// 发件人
    pub from: Option<MailAddress>,
    /// 回复地址
    pub reply_to: Option<MailAddress>,
    /// 收件人列表
    pub to: Vec<MailAddress>,
    /// 抄送人列表
    pub cc: Vec<MailAddress>,
    /// 密送人列表
    pub bcc: Vec<MailAddress>,
    /// 邮件主题
    pub subject: String,
    /// HTML 内容
    pub html_body: Option<String>,
    /// 纯文本内容
    pub text_body: Option<String>,
    /// 附件列表
    pub attachments: Vec<MailAttachment>,
}

impl MailMessage {
    /// 创建空邮件消息
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置发件人
    ///
    /// # 参数
    ///
    /// - `address`: 发件人邮箱（实现 `Into<MailAddress>`，可传 `&str` 或 `MailAddress`）
    pub fn from(mut self, address: impl Into<MailAddress>) -> Self {
        self.from = Some(address.into());
        self
    }

    /// 设置回复地址
    pub fn reply_to(mut self, address: impl Into<MailAddress>) -> Self {
        self.reply_to = Some(address.into());
        self
    }

    /// 添加收件人（可多次调用累加）
    pub fn to(mut self, address: impl Into<MailAddress>) -> Self {
        self.to.push(address.into());
        self
    }

    /// 添加抄送人（可多次调用累加）
    pub fn cc(mut self, address: impl Into<MailAddress>) -> Self {
        self.cc.push(address.into());
        self
    }

    /// 添加密送人（可多次调用累加）
    pub fn bcc(mut self, address: impl Into<MailAddress>) -> Self {
        self.bcc.push(address.into());
        self
    }

    /// 设置邮件主题
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// 设置 HTML 内容
    pub fn html(mut self, content: impl Into<String>) -> Self {
        self.html_body = Some(content.into());
        self
    }

    /// 设置纯文本内容
    pub fn text(mut self, content: impl Into<String>) -> Self {
        self.text_body = Some(content.into());
        self
    }

    /// 添加附件
    pub fn attach(mut self, attachment: MailAttachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    /// 校验邮件必要字段
    ///
    /// 必须满足：至少一个收件人 + 非空主题 + 至少一种内容（HTML 或纯文本）
    pub fn validate(&self) -> Result<(), MailError> {
        if self.to.is_empty() {
            return Err(MailError::MissingField("收件人 (to) 不能为空".to_string()));
        }
        if self.subject.is_empty() {
            return Err(MailError::MissingField("主题 (subject) 不能为空".to_string()));
        }
        if self.html_body.is_none() && self.text_body.is_none() {
            return Err(MailError::MissingField(
                "内容 (html 或 text) 不能同时为空".to_string(),
            ));
        }
        Ok(())
    }
}

// ============================================================================
// Mailer trait
// ============================================================================

/// 邮件发送器 trait — 对齐 PHP `think\mail\Mailer`
///
/// 抽象邮件发送行为，业务方实现具体发送逻辑（SMTP / API / 日志等）。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\mail\Mailer 接口
/// interface Mailer {
///     public function send(Message $message): bool;
///     public function sendRaw(string $to, string $subject, string $body): bool;
/// }
/// ```
pub trait Mailer: Send + Sync {
    /// 发送邮件
    ///
    /// # 参数
    ///
    /// - `message`: 邮件消息
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`MailError`]。
    fn send(&self, message: MailMessage) -> Result<(), MailError>;
}

// ============================================================================
// MemoryMailer（测试/开发用实现）
// ============================================================================

/// 内存邮件发送器 — 用于测试和开发环境
///
/// 不实际发送邮件，而是将邮件暂存到内部 Vec，供测试断言使用。
///
/// # 线程安全
///
/// 通过 `Arc<Mutex<Vec<MailMessage>>>` 保护，支持并发写入。
#[derive(Debug, Clone, Default)]
pub struct MemoryMailer {
    /// 已"发送"的邮件列表
    sent: Arc<Mutex<Vec<MailMessage>>>,
}

impl MemoryMailer {
    /// 创建新的内存邮件发送器
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取已发送邮件数量
    pub fn count(&self) -> usize {
        self.sent.lock().len()
    }

    /// 获取所有已发送邮件（快照）
    pub fn all(&self) -> Vec<MailMessage> {
        self.sent.lock().clone()
    }

    /// 获取最后发送的邮件
    pub fn last(&self) -> Option<MailMessage> {
        self.sent.lock().last().cloned()
    }

    /// 清空已发送邮件
    pub fn clear(&self) {
        self.sent.lock().clear();
    }
}

impl Mailer for MemoryMailer {
    fn send(&self, message: MailMessage) -> Result<(), MailError> {
        // 校验必要字段
        message.validate()?;

        // 暂存到内存
        self.sent.lock().push(message);
        Ok(())
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 MailAddress 基本创建
    #[test]
    fn test_mail_address_new() {
        let addr = MailAddress::new("user@example.com");
        assert_eq!(addr.email, "user@example.com");
        assert_eq!(addr.name, None);
        assert_eq!(addr.to_rfc5322_string(), "user@example.com");
    }

    /// 测试 MailAddress 含显示名
    #[test]
    fn test_mail_address_with_name() {
        let addr = MailAddress::with_name("user@example.com", "张三");
        assert_eq!(addr.email, "user@example.com");
        assert_eq!(addr.name, Some("张三".to_string()));
        assert_eq!(addr.to_rfc5322_string(), "\"张三\" <user@example.com>");
    }

    /// 测试 MailAddress From<&str> 转换
    #[test]
    fn test_mail_address_from_str() {
        let addr: MailAddress = "test@example.com".into();
        assert_eq!(addr.email, "test@example.com");
        assert_eq!(addr.name, None);
    }

    /// 测试 MailMessage Builder 链式调用
    #[test]
    fn test_mail_message_builder_chain() {
        let msg = MailMessage::new()
            .from("sender@example.com")
            .to("user1@example.com")
            .to("user2@example.com")
            .cc("cc@example.com")
            .bcc("bcc@example.com")
            .subject("Test Subject")
            .html("<h1>Hello</h1>")
            .text("Hello");

        assert_eq!(msg.from.unwrap().email, "sender@example.com");
        assert_eq!(msg.to.len(), 2);
        assert_eq!(msg.to[0].email, "user1@example.com");
        assert_eq!(msg.to[1].email, "user2@example.com");
        assert_eq!(msg.cc.len(), 1);
        assert_eq!(msg.bcc.len(), 1);
        assert_eq!(msg.subject, "Test Subject");
        assert_eq!(msg.html_body, Some("<h1>Hello</h1>".to_string()));
        assert_eq!(msg.text_body, Some("Hello".to_string()));
    }

    /// 测试 MailMessage validate 成功
    #[test]
    fn test_validate_success() {
        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("Test")
            .html("<p>Hello</p>");

        assert!(msg.validate().is_ok());
    }

    /// 测试 MailMessage validate 缺少收件人
    #[test]
    fn test_validate_missing_to() {
        let msg = MailMessage::new().subject("Test").html("<p>Hello</p>");

        let result = msg.validate();
        assert!(result.is_err());
        match result {
            Err(MailError::MissingField(msg)) => {
                assert!(msg.contains("收件人"));
            }
            _ => panic!("期望 MissingField 错误"),
        }
    }

    /// 测试 MailMessage validate 缺少主题
    #[test]
    fn test_validate_missing_subject() {
        let msg = MailMessage::new()
            .to("user@example.com")
            .html("<p>Hello</p>");

        let result = msg.validate();
        assert!(result.is_err());
        match result {
            Err(MailError::MissingField(msg)) => {
                assert!(msg.contains("主题"));
            }
            _ => panic!("期望 MissingField 错误"),
        }
    }

    /// 测试 MailMessage validate 缺少内容
    #[test]
    fn test_validate_missing_content() {
        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("Test");

        let result = msg.validate();
        assert!(result.is_err());
        match result {
            Err(MailError::MissingField(msg)) => {
                assert!(msg.contains("内容"));
            }
            _ => panic!("期望 MissingField 错误"),
        }
    }

    /// 测试 MemoryMailer 发送邮件
    #[test]
    fn test_memory_mailer_send() {
        let mailer = MemoryMailer::new();
        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("Test")
            .html("<p>Hello</p>");

        let result = mailer.send(msg);
        assert!(result.is_ok());
        assert_eq!(mailer.count(), 1);

        let sent = mailer.last().unwrap();
        assert_eq!(sent.subject, "Test");
        assert_eq!(sent.to.len(), 1);
        assert_eq!(sent.to[0].email, "user@example.com");
    }

    /// 测试 MemoryMailer 发送多封邮件
    #[test]
    fn test_memory_mailer_send_multiple() {
        let mailer = MemoryMailer::new();

        for i in 0..3 {
            let msg = MailMessage::new()
                .to(format!("user{}@example.com", i))
                .subject(format!("Subject {}", i))
                .text(format!("Body {}", i));
            mailer.send(msg).unwrap();
        }

        assert_eq!(mailer.count(), 3);

        let all = mailer.all();
        assert_eq!(all[0].subject, "Subject 0");
        assert_eq!(all[2].subject, "Subject 2");
    }

    /// 测试 MemoryMailer 发送无效邮件返回错误
    #[test]
    fn test_memory_mailer_send_invalid_errors() {
        let mailer = MemoryMailer::new();
        let msg = MailMessage::new().subject("No recipient").html("<p>Hi</p>");

        let result = mailer.send(msg);
        assert!(result.is_err());
        assert_eq!(mailer.count(), 0);
    }

    /// 测试 MemoryMailer clear
    #[test]
    fn test_memory_mailer_clear() {
        let mailer = MemoryMailer::new();
        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("Test")
            .text("Hi");
        mailer.send(msg).unwrap();
        assert_eq!(mailer.count(), 1);

        mailer.clear();
        assert_eq!(mailer.count(), 0);
        assert!(mailer.last().is_none());
    }

    /// 测试 MailAttachment 从字节创建
    #[test]
    fn test_attachment_from_bytes() {
        let attachment = MailAttachment::new("doc.pdf", vec![1, 2, 3], "application/pdf");
        assert_eq!(attachment.filename, "doc.pdf");
        assert_eq!(attachment.content, vec![1, 2, 3]);
        assert_eq!(attachment.mime_type, "application/pdf");
    }

    /// 测试 MailAttachment 从文件创建
    #[test]
    fn test_attachment_from_file() {
        let temp_dir = std::env::temp_dir().join("sz_rust_mail_test");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("test.txt");
        std::fs::write(&file_path, b"hello attachment").unwrap();

        let attachment =
            MailAttachment::from_file(&file_path, None, "text/plain").unwrap();
        assert_eq!(attachment.filename, "test.txt");
        assert_eq!(attachment.content, b"hello attachment");
        assert_eq!(attachment.mime_type, "text/plain");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// 测试 MailAttachment 从不存在文件创建返回错误
    #[test]
    fn test_attachment_from_nonexistent_file_errors() {
        let result = MailAttachment::from_file(
            "/nonexistent/file.txt",
            None,
            "text/plain",
        );
        assert!(result.is_err());
        match result {
            Err(MailError::AttachmentRead { .. }) => {}
            _ => panic!("期望 AttachmentRead 错误"),
        }
    }

    /// 测试邮件含附件发送
    #[test]
    fn test_send_mail_with_attachment() {
        let mailer = MemoryMailer::new();
        let attachment = MailAttachment::new("report.pdf", vec![1, 2, 3, 4], "application/pdf");

        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("Report")
            .text("Please find the attached report.")
            .attach(attachment);

        mailer.send(msg).unwrap();
        let sent = mailer.last().unwrap();
        assert_eq!(sent.attachments.len(), 1);
        assert_eq!(sent.attachments[0].filename, "report.pdf");
    }

    /// 测试 MailMessage 默认值
    #[test]
    fn test_mail_message_default() {
        let msg = MailMessage::default();
        assert!(msg.from.is_none());
        assert!(msg.reply_to.is_none());
        assert!(msg.to.is_empty());
        assert!(msg.cc.is_empty());
        assert!(msg.bcc.is_empty());
        assert!(msg.subject.is_empty());
        assert!(msg.html_body.is_none());
        assert!(msg.text_body.is_none());
        assert!(msg.attachments.is_empty());
    }

    /// 测试纯文本邮件（无 HTML）
    #[test]
    fn test_text_only_email() {
        let mailer = MemoryMailer::new();
        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("Plain Text")
            .text("This is plain text content.");

        mailer.send(msg).unwrap();
        let sent = mailer.last().unwrap();
        assert!(sent.html_body.is_none());
        assert_eq!(sent.text_body, Some("This is plain text content.".to_string()));
    }

    /// 测试 HTML 邮件（无纯文本）
    #[test]
    fn test_html_only_email() {
        let mailer = MemoryMailer::new();
        let msg = MailMessage::new()
            .to("user@example.com")
            .subject("HTML Only")
            .html("<h1>HTML Content</h1>");

        mailer.send(msg).unwrap();
        let sent = mailer.last().unwrap();
        assert_eq!(sent.html_body, Some("<h1>HTML Content</h1>".to_string()));
        assert!(sent.text_body.is_none());
    }
}

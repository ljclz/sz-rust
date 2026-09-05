// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Pay 模块 — 支付聚合抽象层（对齐 PHP `yansongda/pay`）
//!
//! 提供统一的支付抽象，支持多平台（支付宝、微信支付等）扩展。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Pay::alipay()->app($order)` | [`PayProvider::pay`] | 发起支付 |
//! | `Pay::alipay()->find($order)` | [`PayProvider::query`] | 查询订单 |
//! | `Pay::alipay()->close($order)` | [`PayProvider::close`] | 关闭订单 |
//! | `Pay::alipay()->refund($order)` | [`PayProvider::refund`] | 退款 |
//! | `Pay::alipay()->callback($params)` | [`PayProvider::verify_notify`] | 验证回调 |
//!
//! ### PHP 行为对齐
//!
//! - **统一 Provider 抽象**：PHP `Yansongda\Pay\Contract\ProviderInterface` 抽象支付提供商。
//!   Rust 通过 [`PayProvider`] trait 表达。
//! - **多平台**：PHP 支持支付宝/微信支付。Rust 通过 [`PayPlatform`] 表达。
//! - **统一订单结构**：PHP `Pay::alipay()->app($order)` 接收订单数组。
//!   Rust 通过 [`PayOrder`] builder 表达。
//!
//! ## 架构说明
//!
//! - **PayProvider trait 抽象**：业务方实现具体支付逻辑（支付宝/微信支付/ etc.）
//! - **MemoryPayProvider**：内置内存实现，暂存支付/退款记录，用于测试和开发环境
//! - **PayHttpTransport trait**：HTTP 传输抽象，解耦 PayProvider 与具体 HTTP 库
//! - **MemoryPayHttpTransport**：内存 HTTP 传输实现，支持预置响应队列

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// Pay 错误
#[derive(Debug, Error)]
pub enum PayError {
    /// 配置错误
    #[error("支付配置错误: {0}")]
    Config(String),
    /// 缺少必填字段
    #[error("支付字段缺失: {0}")]
    MissingField(String),
    /// 请求失败
    #[error("支付请求失败: {0}")]
    RequestFailed(String),
    /// HTTP 传输失败
    #[error("HTTP 传输失败: {0}")]
    HttpTransport(String),
    /// 序列化失败
    #[error("序列化失败: {0}")]
    Serialize(String),
    /// 签名验证失败
    #[error("签名验证失败: {0}")]
    VerifyFailed(String),
    /// 退款失败
    #[error("退款失败: {0}")]
    RefundFailed(String),
    /// 查询失败
    #[error("查询失败: {0}")]
    QueryFailed(String),
}

// ============================================================================
// 支付平台
// ============================================================================

/// 支付平台 — 对齐 PHP `yansongda/pay` 支持的平台
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PayPlatform {
    /// 支付宝
    #[default]
    Alipay,
    /// 微信支付
    WechatPay,
    /// 其他平台（预留扩展）
    Other,
}

impl PayPlatform {
    /// 转换为字符串标识（对齐 PHP 平台名）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alipay => "alipay",
            Self::WechatPay => "wechatpay",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for PayPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PayPlatform {
    type Err = PayError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "alipay" | "ali" => Ok(Self::Alipay),
            "wechatpay" | "wechat" => Ok(Self::WechatPay),
            "other" => Ok(Self::Other),
            other => Err(PayError::Config(format!("未知支付平台: {other}"))),
        }
    }
}

// ============================================================================
// 支付订单（Builder 模式）
// ============================================================================

/// 支付订单 — 对齐 PHP `Yansongda\Pay\Pay::alipay()->app()` 的订单参数
///
/// 使用 Builder 模式构建订单内容，通过 [`PayProvider::pay`] 发起支付。
///
/// # PHP 对齐
///
/// ```php
/// // PHP yansongda/pay
/// $order = [
///     'out_trade_no' => '202401010001',
///     'total_amount' => '88.00',
///     'subject'      => '鲜视达商品',
/// ];
/// Pay::alipay()->app($order);
/// ```
///
/// # Rust 用法
///
/// ```rust,ignore
/// use sz_rust_core::pay::{PayOrder, MemoryPayProvider, PayProvider};
///
/// let order = PayOrder::new()
///     .out_trade_no("202401010001")
///     .total_amount(8800)
///     .subject("鲜视达商品");
///
/// let provider = MemoryPayProvider::new();
/// let result = provider.pay(order).unwrap();
/// ```
#[derive(Debug, Clone, Default)]
pub struct PayOrder {
    /// 商户订单号（必填）
    pub out_trade_no: String,
    /// 订单总金额（必填，单位：分）
    pub total_amount: i64,
    /// 订单标题（必填）
    pub subject: String,
    /// 订单描述
    pub body: Option<String>,
    /// 异步通知 URL
    pub notify_url: Option<String>,
    /// 同步跳转 URL
    pub return_url: Option<String>,
    /// 过期时间（秒）
    pub timeout_express: Option<i64>,
    /// 附加数据
    pub passback_params: Option<String>,
    /// 业务扩展参数
    pub extra: serde_json::Value,
}

impl PayOrder {
    /// 创建空支付订单
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置商户订单号
    pub fn out_trade_no(mut self, out_trade_no: impl Into<String>) -> Self {
        self.out_trade_no = out_trade_no.into();
        self
    }

    /// 设置订单总金额（单位：分）
    pub fn total_amount(mut self, total_amount: i64) -> Self {
        self.total_amount = total_amount;
        self
    }

    /// 设置订单标题
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// 设置订单描述
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// 设置异步通知 URL
    pub fn notify_url(mut self, notify_url: impl Into<String>) -> Self {
        self.notify_url = Some(notify_url.into());
        self
    }

    /// 设置同步跳转 URL
    pub fn return_url(mut self, return_url: impl Into<String>) -> Self {
        self.return_url = Some(return_url.into());
        self
    }

    /// 设置过期时间（秒）
    pub fn timeout_express(mut self, timeout_express: i64) -> Self {
        self.timeout_express = Some(timeout_express);
        self
    }

    /// 设置附加数据
    pub fn passback_params(mut self, passback_params: impl Into<String>) -> Self {
        self.passback_params = Some(passback_params.into());
        self
    }

    /// 设置业务扩展参数
    pub fn extra(mut self, extra: serde_json::Value) -> Self {
        self.extra = extra;
        self
    }

    /// 校验必填字段
    ///
    /// # 返回
    ///
    /// - 商户订单号为空 → [`PayError::MissingField`]("out_trade_no")
    /// - 订单金额 ≤ 0 → [`PayError::MissingField`]("total_amount")
    /// - 订单标题为空 → [`PayError::MissingField`]("subject")
    pub fn validate(&self) -> Result<(), PayError> {
        if self.out_trade_no.is_empty() {
            return Err(PayError::MissingField("out_trade_no".into()));
        }
        if self.total_amount <= 0 {
            return Err(PayError::MissingField("total_amount".into()));
        }
        if self.subject.is_empty() {
            return Err(PayError::MissingField("subject".into()));
        }
        Ok(())
    }
}

// ============================================================================
// 支付结果
// ============================================================================

/// 支付结果 — 统一返回格式
///
/// 各支付平台的响应统一映射到此结构，便于上层业务处理。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PayResult {
    /// 支付平台返回的流水号
    pub trade_no: String,
    /// 商户订单号
    pub out_trade_no: String,
    /// 实际支付金额（分）
    pub total_amount: i64,
    /// 交易状态
    pub trade_status: String,
    /// 支付平台原始响应
    pub raw: serde_json::Value,
}

// ============================================================================
// 退款订单（Builder 模式）
// ============================================================================

/// 退款订单 — 对齐 PHP `Pay::alipay()->refund()`
///
/// 使用 Builder 模式构建退款内容，通过 [`PayProvider::refund`] 发起退款。
#[derive(Debug, Clone, Default)]
pub struct RefundOrder {
    /// 商户订单号
    pub out_trade_no: String,
    /// 退款金额（分）
    pub refund_amount: i64,
    /// 退款单号
    pub out_request_no: String,
    /// 退款原因
    pub reason: Option<String>,
}

impl RefundOrder {
    /// 创建空退款订单
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置商户订单号
    pub fn out_trade_no(mut self, out_trade_no: impl Into<String>) -> Self {
        self.out_trade_no = out_trade_no.into();
        self
    }

    /// 设置退款金额（单位：分）
    pub fn refund_amount(mut self, refund_amount: i64) -> Self {
        self.refund_amount = refund_amount;
        self
    }

    /// 设置退款单号
    pub fn out_request_no(mut self, out_request_no: impl Into<String>) -> Self {
        self.out_request_no = out_request_no.into();
        self
    }

    /// 设置退款原因
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// 校验必填字段
    ///
    /// # 返回
    ///
    /// - 商户订单号为空 → [`PayError::MissingField`]("out_trade_no")
    /// - 退款金额 ≤ 0 → [`PayError::MissingField`]("refund_amount")
    /// - 退款单号为空 → [`PayError::MissingField`]("out_request_no")
    pub fn validate(&self) -> Result<(), PayError> {
        if self.out_trade_no.is_empty() {
            return Err(PayError::MissingField("out_trade_no".into()));
        }
        if self.refund_amount <= 0 {
            return Err(PayError::MissingField("refund_amount".into()));
        }
        if self.out_request_no.is_empty() {
            return Err(PayError::MissingField("out_request_no".into()));
        }
        Ok(())
    }
}

// ============================================================================
// 支付配置（Builder 模式）
// ============================================================================

/// 支付配置 — 对齐 PHP `Pay::config(config)` 的配置结构
///
/// 使用 Builder 模式构建配置，各支付提供商共享此配置结构。
#[derive(Debug, Clone)]
pub struct PayConfig {
    /// 支付平台
    pub platform: PayPlatform,
    /// 应用 ID
    pub app_id: String,
    /// 商户私钥（PEM 格式或 PKCS8 字符串）
    pub merchant_private_key: String,
    /// 平台公钥
    pub platform_public_key: String,
    /// 回调 URL
    pub notify_url: String,
    /// 返回 URL
    pub return_url: Option<String>,
    /// 沙箱模式
    pub sandbox: bool,
    /// 模式（如 web/app/mini/scan）
    pub mode: String,
}

impl PayConfig {
    /// 创建支付配置
    ///
    /// # 参数
    ///
    /// - `platform`: 支付平台
    /// - `app_id`: 应用 ID
    pub fn new(platform: PayPlatform, app_id: impl Into<String>) -> Self {
        Self {
            platform,
            app_id: app_id.into(),
            merchant_private_key: String::new(),
            platform_public_key: String::new(),
            notify_url: String::new(),
            return_url: None,
            sandbox: false,
            mode: "web".to_string(),
        }
    }

    /// 设置商户私钥
    pub fn with_merchant_private_key(mut self, key: impl Into<String>) -> Self {
        self.merchant_private_key = key.into();
        self
    }

    /// 设置平台公钥
    pub fn with_platform_public_key(mut self, key: impl Into<String>) -> Self {
        self.platform_public_key = key.into();
        self
    }

    /// 设置回调 URL
    pub fn with_notify_url(mut self, notify_url: impl Into<String>) -> Self {
        self.notify_url = notify_url.into();
        self
    }

    /// 设置返回 URL
    pub fn with_return_url(mut self, return_url: impl Into<String>) -> Self {
        self.return_url = Some(return_url.into());
        self
    }

    /// 设置沙箱模式
    pub fn with_sandbox(mut self, sandbox: bool) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// 设置模式
    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = mode.into();
        self
    }

    /// 校验配置必填字段
    ///
    /// # 返回
    ///
    /// - 应用 ID 为空 → [`PayError::Config`]("app_id")
    /// - 商户私钥为空 → [`PayError::Config`]("merchant_private_key")
    /// - 平台公钥为空 → [`PayError::Config`]("platform_public_key")
    /// - 回调 URL 为空 → [`PayError::Config`]("notify_url")
    pub fn validate(&self) -> Result<(), PayError> {
        if self.app_id.is_empty() {
            return Err(PayError::Config("app_id".into()));
        }
        if self.merchant_private_key.is_empty() {
            return Err(PayError::Config("merchant_private_key".into()));
        }
        if self.platform_public_key.is_empty() {
            return Err(PayError::Config("platform_public_key".into()));
        }
        if self.notify_url.is_empty() {
            return Err(PayError::Config("notify_url".into()));
        }
        Ok(())
    }
}

// ============================================================================
// PayProvider trait
// ============================================================================

/// 支付提供商 trait — 对齐 PHP `Yansongda\Pay\Contract\ProviderInterface`
///
/// 抽象支付行为，业务方实现具体支付逻辑（支付宝/微信支付/ etc.）。
///
/// # PHP 对齐
///
/// ```php
/// // PHP Yansongda\Pay\Contract\ProviderInterface
/// interface ProviderInterface {
///     public function pay(array $order): Collection;
///     public function find(array $order): Collection;
///     public function close(array $order): void;
///     public function refund(array $order): Collection;
///     public function callback(array $params): Collection;
/// }
/// ```
pub trait PayProvider: Send + Sync {
    /// 发起支付（对齐 `Pay::alipay()->app()` / `Pay::wechat()->app()`）
    ///
    /// # 参数
    ///
    /// - `order`: 支付订单
    ///
    /// # 返回
    ///
    /// 成功返回 [`PayResult`]，失败返回 [`PayError`]。
    fn pay(&self, order: PayOrder) -> Result<PayResult, PayError>;

    /// 查询订单（对齐 `Pay::alipay()->find()`）
    ///
    /// # 参数
    ///
    /// - `out_trade_no`: 商户订单号
    ///
    /// # 返回
    ///
    /// 成功返回 [`PayResult`]，失败返回 [`PayError`]。
    fn query(&self, out_trade_no: &str) -> Result<PayResult, PayError>;

    /// 关闭订单（对齐 `Pay::alipay()->close()`）
    ///
    /// # 参数
    ///
    /// - `out_trade_no`: 商户订单号
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`PayError`]。
    fn close(&self, out_trade_no: &str) -> Result<(), PayError>;

    /// 退款（对齐 `Pay::alipay()->refund()`）
    ///
    /// # 参数
    ///
    /// - `refund`: 退款订单
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`PayError`]。
    fn refund(&self, refund: RefundOrder) -> Result<(), PayError>;

    /// 验证回调通知（对齐 `Pay::alipay()->callback()`）
    ///
    /// # 参数
    ///
    /// - `params`: 回调通知参数（JSON 值）
    ///
    /// # 返回
    ///
    /// 成功返回 [`PayResult`]，失败返回 [`PayError`]。
    fn verify_notify(&self, params: &serde_json::Value) -> Result<PayResult, PayError>;
}

// ============================================================================
// MemoryPayProvider（测试/开发用实现）
// ============================================================================

/// 内存支付提供商 — 用于测试和开发环境
///
/// 不实际调用支付平台 API，而是将支付/退款记录暂存到内存，供测试断言使用。
///
/// # 线程安全
///
/// 通过 `Arc<Mutex<...>>` 保护内部状态，支持并发访问。
///
/// # 用法
///
/// ```rust,ignore
/// use sz_rust_core::pay::{MemoryPayProvider, PayOrder, PayProvider};
///
/// let provider = MemoryPayProvider::new();
/// let order = PayOrder::new()
///     .out_trade_no("202401010001")
///     .total_amount(8800)
///     .subject("鲜视达商品");
///
/// let result = provider.pay(order).unwrap();
/// assert_eq!(result.out_trade_no, "202401010001");
/// assert_eq!(provider.orders().len(), 1);
/// ```
#[derive(Debug, Default)]
pub struct MemoryPayProvider {
    /// 已发起的支付订单（按商户订单号索引）
    orders: Arc<Mutex<HashMap<String, PayResult>>>,
    /// 已发起的退款记录
    refunds: Arc<Mutex<Vec<RefundOrder>>>,
    /// 预置查询结果（用于测试 query 行为）
    query_result: Arc<Mutex<Option<PayResult>>>,
}

impl MemoryPayProvider {
    /// 创建新的内存支付提供商
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取所有已发起的支付订单（快照）
    pub fn orders(&self) -> Vec<PayResult> {
        self.orders.lock().values().cloned().collect()
    }

    /// 获取所有已发起的退款记录（快照）
    pub fn refunds(&self) -> Vec<RefundOrder> {
        self.refunds.lock().clone()
    }

    /// 预置查询结果（query 调用将返回此结果）
    ///
    /// 设置后，[`PayProvider::query`] 将直接返回此结果，不再查找已发起订单。
    /// 传 `None` 清除预置，恢复正常查找逻辑。
    pub fn set_query_result(&self, result: PayResult) {
        *self.query_result.lock() = Some(result);
    }

    /// 清空所有支付/退款记录及预置查询结果
    pub fn clear(&self) {
        self.orders.lock().clear();
        self.refunds.lock().clear();
        *self.query_result.lock() = None;
    }
}

impl PayProvider for MemoryPayProvider {
    fn pay(&self, order: PayOrder) -> Result<PayResult, PayError> {
        // 1. 校验订单必填字段
        order.validate()?;

        // 2. 校验订单未重复
        let mut orders = self.orders.lock();
        if orders.contains_key(&order.out_trade_no) {
            return Err(PayError::RequestFailed(format!(
                "订单号已存在: {}",
                order.out_trade_no
            )));
        }

        // 3. 构造支付结果（生成内存流水号）
        let trade_no = format!("MEM{}", order.out_trade_no);
        let raw = serde_json::json!({
            "out_trade_no": order.out_trade_no,
            "total_amount": order.total_amount,
            "subject": order.subject,
            "trade_no": trade_no,
        });
        let result = PayResult {
            trade_no,
            out_trade_no: order.out_trade_no.clone(),
            total_amount: order.total_amount,
            trade_status: "WAIT_BUYER_PAY".to_string(),
            raw,
        };

        // 4. 暂存到内存
        orders.insert(order.out_trade_no.clone(), result.clone());
        Ok(result)
    }

    fn query(&self, out_trade_no: &str) -> Result<PayResult, PayError> {
        // 1. 若预置查询结果，直接返回
        if let Some(result) = self.query_result.lock().clone() {
            return Ok(result);
        }

        // 2. 否则从已发起订单中查找
        self.orders
            .lock()
            .get(out_trade_no)
            .cloned()
            .ok_or_else(|| PayError::QueryFailed(format!("订单不存在: {out_trade_no}")))
    }

    fn close(&self, out_trade_no: &str) -> Result<(), PayError> {
        let mut orders = self.orders.lock();
        if let Some(result) = orders.get_mut(out_trade_no) {
            // 标记为已关闭
            result.trade_status = "CLOSED".to_string();
            Ok(())
        } else {
            Err(PayError::RequestFailed(format!(
                "订单不存在: {out_trade_no}"
            )))
        }
    }

    fn refund(&self, refund: RefundOrder) -> Result<(), PayError> {
        // 1. 校验退款订单必填字段
        refund.validate()?;

        // 2. 校验原订单存在
        if !self.orders.lock().contains_key(&refund.out_trade_no) {
            return Err(PayError::RefundFailed(format!(
                "原订单不存在: {}",
                refund.out_trade_no
            )));
        }

        // 3. 暂存到内存
        self.refunds.lock().push(refund);
        Ok(())
    }

    fn verify_notify(&self, params: &serde_json::Value) -> Result<PayResult, PayError> {
        // 1. 提取商户订单号（必填）
        let out_trade_no = params
            .get("out_trade_no")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PayError::VerifyFailed("缺少 out_trade_no".into()))?;

        // 2. 提取其他字段（可选，缺省回退）
        let trade_no = params
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let total_amount = params
            .get("total_amount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let trade_status = params
            .get("trade_status")
            .and_then(|v| v.as_str())
            .unwrap_or("TRADE_SUCCESS")
            .to_string();

        Ok(PayResult {
            trade_no,
            out_trade_no: out_trade_no.to_string(),
            total_amount,
            trade_status,
            raw: params.clone(),
        })
    }
}

// ============================================================================
// PayHttpTransport trait（HTTP 传输抽象）
// ============================================================================

/// 支付 HTTP 传输 trait — 用于解耦 PayProvider 与具体 HTTP 库
///
/// 与 [`crate::notify::HttpTransport`] 不同，此 trait 的 `post_json` / `get`
/// 返回响应体字符串，以便支付提供商解析平台响应。
///
/// # 线程安全
///
/// 实现者必须保证 `Send + Sync`，因为 PayProvider 通常作为单例在多线程下使用。
pub trait PayHttpTransport: Send + Sync {
    /// POST JSON 并返回响应体
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    /// - `body`: 请求体（JSON 字符串）
    ///
    /// # 返回
    ///
    /// 成功返回响应体字符串，失败返回 [`PayError`]。
    fn post_json(&self, url: &str, body: &str) -> Result<String, PayError>;

    /// GET 并返回响应体
    ///
    /// # 参数
    ///
    /// - `url`: 目标 URL
    ///
    /// # 返回
    ///
    /// 成功返回响应体字符串，失败返回 [`PayError`]。
    fn get(&self, url: &str) -> Result<String, PayError>;
}

// ============================================================================
// MemoryPayHttpTransport（测试/开发用 HTTP 传输实现）
// ============================================================================

/// 内存支付 HTTP 传输 — 用于测试和开发环境
///
/// 不实际发送 HTTP 请求，而是从预置响应队列中依次返回响应。
/// 同时记录所有请求供测试断言使用。
#[derive(Debug, Default)]
pub struct MemoryPayHttpTransport {
    /// 预置响应队列（FIFO）
    responses: Mutex<Vec<String>>,
    /// 已"发送"的请求记录（method, url, body）
    requests: Mutex<Vec<(String, String, String)>>,
}

impl MemoryPayHttpTransport {
    /// 创建新的内存支付 HTTP 传输
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加预置响应到队列尾部
    pub fn push_response(&self, response: impl Into<String>) {
        self.responses.lock().push(response.into());
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
    fn next_response(&self) -> Result<String, PayError> {
        let mut responses = self.responses.lock();
        if responses.is_empty() {
            Err(PayError::HttpTransport("无可用预置响应".into()))
        } else {
            Ok(responses.remove(0))
        }
    }
}

impl PayHttpTransport for MemoryPayHttpTransport {
    fn post_json(&self, url: &str, body: &str) -> Result<String, PayError> {
        let response = self.next_response()?;
        self.requests
            .lock()
            .push(("POST".to_string(), url.to_string(), body.to_string()));
        Ok(response)
    }

    fn get(&self, url: &str) -> Result<String, PayError> {
        let response = self.next_response()?;
        self.requests
            .lock()
            .push(("GET".to_string(), url.to_string(), String::new()));
        Ok(response)
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // PayPlatform 测试
    // ------------------------------------------------------------------------

    /// 测试 PayPlatform 的 as_str / Default / Display / FromStr
    #[test]
    fn test_pay_platform() {
        // as_str
        assert_eq!(PayPlatform::Alipay.as_str(), "alipay");
        assert_eq!(PayPlatform::WechatPay.as_str(), "wechatpay");
        assert_eq!(PayPlatform::Other.as_str(), "other");

        // Default
        assert_eq!(PayPlatform::default(), PayPlatform::Alipay);

        // Display
        assert_eq!(format!("{}", PayPlatform::Alipay), "alipay");
        assert_eq!(format!("{}", PayPlatform::WechatPay), "wechatpay");
        assert_eq!(format!("{}", PayPlatform::Other), "other");

        // FromStr — 标准名
        assert_eq!(
            "alipay".parse::<PayPlatform>().unwrap(),
            PayPlatform::Alipay
        );
        assert_eq!(
            "wechatpay".parse::<PayPlatform>().unwrap(),
            PayPlatform::WechatPay
        );
        assert_eq!("other".parse::<PayPlatform>().unwrap(), PayPlatform::Other);

        // FromStr — 别名 + 大小写不敏感
        assert_eq!("ali".parse::<PayPlatform>().unwrap(), PayPlatform::Alipay);
        assert_eq!(
            "wechat".parse::<PayPlatform>().unwrap(),
            PayPlatform::WechatPay
        );
        assert_eq!(
            "ALIPAY".parse::<PayPlatform>().unwrap(),
            PayPlatform::Alipay
        );

        // FromStr — 未知平台
        assert!("unknown".parse::<PayPlatform>().is_err());

        // Copy + Eq + Hash 可用
        let set = std::collections::HashSet::from([PayPlatform::Alipay, PayPlatform::WechatPay]);
        assert!(set.contains(&PayPlatform::Alipay));
        assert!(!set.contains(&PayPlatform::Other));
    }

    // ------------------------------------------------------------------------
    // PayConfig 测试
    // ------------------------------------------------------------------------

    /// 测试 PayConfig builder 模式
    #[test]
    fn test_pay_config_builder() {
        let config = PayConfig::new(PayPlatform::Alipay, "2021001")
            .with_merchant_private_key("MIIEvQIBADANB")
            .with_platform_public_key("MIIBIjANBgkqh")
            .with_notify_url("https://example.com/notify")
            .with_return_url("https://example.com/return")
            .with_sandbox(true)
            .with_mode("app");

        assert_eq!(config.platform, PayPlatform::Alipay);
        assert_eq!(config.app_id, "2021001");
        assert_eq!(config.merchant_private_key, "MIIEvQIBADANB");
        assert_eq!(config.platform_public_key, "MIIBIjANBgkqh");
        assert_eq!(config.notify_url, "https://example.com/notify");
        assert_eq!(
            config.return_url.as_deref(),
            Some("https://example.com/return")
        );
        assert!(config.sandbox);
        assert_eq!(config.mode, "app");

        // validate 通过
        assert!(config.validate().is_ok());

        // 默认值（仅必填项）
        let minimal = PayConfig::new(PayPlatform::WechatPay, "wx123");
        assert_eq!(minimal.platform, PayPlatform::WechatPay);
        assert_eq!(minimal.app_id, "wx123");
        assert!(minimal.merchant_private_key.is_empty());
        assert!(minimal.platform_public_key.is_empty());
        assert!(minimal.notify_url.is_empty());
        assert!(minimal.return_url.is_none());
        assert!(!minimal.sandbox);
        assert_eq!(minimal.mode, "web");

        // validate 失败：缺 app_id
        let bad = PayConfig::new(PayPlatform::Alipay, "");
        let err = bad.validate().unwrap_err();
        match err {
            PayError::Config(field) => assert_eq!(field, "app_id"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }

        // validate 失败：缺 merchant_private_key
        let bad = PayConfig::new(PayPlatform::Alipay, "app1");
        let err = bad.validate().unwrap_err();
        match err {
            PayError::Config(field) => assert_eq!(field, "merchant_private_key"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }

        // validate 失败：缺 notify_url
        let bad = PayConfig::new(PayPlatform::Alipay, "app1")
            .with_merchant_private_key("k1")
            .with_platform_public_key("k2");
        let err = bad.validate().unwrap_err();
        match err {
            PayError::Config(field) => assert_eq!(field, "notify_url"),
            other => panic!("期望 Config, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // PayOrder 测试
    // ------------------------------------------------------------------------

    /// 测试 PayOrder builder 模式
    #[test]
    fn test_pay_order_builder() {
        let order = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(8800)
            .subject("鲜视达商品")
            .body("新鲜蔬菜套餐")
            .notify_url("https://example.com/notify")
            .return_url("https://example.com/return")
            .timeout_express(1800)
            .passback_params("merchant_extra")
            .extra(serde_json::json!({"channel": "alipay_app"}));

        assert_eq!(order.out_trade_no, "202401010001");
        assert_eq!(order.total_amount, 8800);
        assert_eq!(order.subject, "鲜视达商品");
        assert_eq!(order.body.as_deref(), Some("新鲜蔬菜套餐"));
        assert_eq!(
            order.notify_url.as_deref(),
            Some("https://example.com/notify")
        );
        assert_eq!(
            order.return_url.as_deref(),
            Some("https://example.com/return")
        );
        assert_eq!(order.timeout_express, Some(1800));
        assert_eq!(order.passback_params.as_deref(), Some("merchant_extra"));
        assert_eq!(order.extra["channel"], "alipay_app");

        // validate 通过
        assert!(order.validate().is_ok());
    }

    /// 测试 PayOrder::validate 校验必填字段
    #[test]
    fn test_pay_order_validate() {
        // 全部合法
        let order = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(100)
            .subject("标题");
        assert!(order.validate().is_ok());

        // 缺 out_trade_no
        let order = PayOrder::new().total_amount(100).subject("标题");
        let err = order.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "out_trade_no"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // total_amount <= 0
        let order = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(0)
            .subject("标题");
        let err = order.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "total_amount"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 负数金额
        let order = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(-1)
            .subject("标题");
        let err = order.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "total_amount"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 缺 subject
        let order = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(100);
        let err = order.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "subject"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // 默认值校验失败（全部为空）
        let err = PayOrder::default().validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "out_trade_no"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // RefundOrder 测试
    // ------------------------------------------------------------------------

    /// 测试 RefundOrder builder 模式
    #[test]
    fn test_refund_order_builder() {
        let refund = RefundOrder::new()
            .out_trade_no("202401010001")
            .refund_amount(5000)
            .out_request_no("R202401010001")
            .reason("用户申请退款");

        assert_eq!(refund.out_trade_no, "202401010001");
        assert_eq!(refund.refund_amount, 5000);
        assert_eq!(refund.out_request_no, "R202401010001");
        assert_eq!(refund.reason.as_deref(), Some("用户申请退款"));

        // validate 通过
        assert!(refund.validate().is_ok());

        // validate 失败：缺 out_trade_no
        let refund = RefundOrder::new().refund_amount(5000).out_request_no("R1");
        let err = refund.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "out_trade_no"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // validate 失败：refund_amount <= 0
        let refund = RefundOrder::new()
            .out_trade_no("T1")
            .refund_amount(0)
            .out_request_no("R1");
        let err = refund.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "refund_amount"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }

        // validate 失败：缺 out_request_no
        let refund = RefundOrder::new().out_trade_no("T1").refund_amount(100);
        let err = refund.validate().unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "out_request_no"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
    }

    // ------------------------------------------------------------------------
    // PayResult 测试
    // ------------------------------------------------------------------------

    /// 测试 PayResult 默认值
    #[test]
    fn test_pay_result_default() {
        let result = PayResult::default();
        assert!(result.trade_no.is_empty());
        assert!(result.out_trade_no.is_empty());
        assert_eq!(result.total_amount, 0);
        assert!(result.trade_status.is_empty());
        assert!(result.raw.is_null());

        // serde 序列化/反序列化往返
        let result = PayResult {
            trade_no: "2024MEM001".to_string(),
            out_trade_no: "ORD001".to_string(),
            total_amount: 8800,
            trade_status: "TRADE_SUCCESS".to_string(),
            raw: serde_json::json!({"code": "00"}),
        };
        let json = serde_json::to_string(&result).expect("序列化失败");
        let back: PayResult = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(back.trade_no, "2024MEM001");
        assert_eq!(back.out_trade_no, "ORD001");
        assert_eq!(back.total_amount, 8800);
        assert_eq!(back.trade_status, "TRADE_SUCCESS");
        assert_eq!(back.raw["code"], "00");
    }

    // ------------------------------------------------------------------------
    // MemoryPayProvider 测试
    // ------------------------------------------------------------------------

    /// 测试 MemoryPayProvider 发起支付
    #[test]
    fn test_memory_pay_provider_pay() {
        let provider = MemoryPayProvider::new();
        let order = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(8800)
            .subject("鲜视达商品")
            .body("新鲜蔬菜");

        let result = provider.pay(order).expect("支付应成功");

        // 验证返回的 PayResult 字段
        assert_eq!(result.out_trade_no, "202401010001");
        assert_eq!(result.total_amount, 8800);
        assert_eq!(result.trade_status, "WAIT_BUYER_PAY");
        assert!(result.trade_no.starts_with("MEM"));
        assert_eq!(result.trade_no, "MEM202401010001");
        // raw 包含订单信息
        assert_eq!(result.raw["out_trade_no"], "202401010001");
        assert_eq!(result.raw["total_amount"], 8800);
        assert_eq!(result.raw["subject"], "鲜视达商品");

        // 已存储到内存
        assert_eq!(provider.orders().len(), 1);

        // 重复订单号应失败
        let dup = PayOrder::new()
            .out_trade_no("202401010001")
            .total_amount(100)
            .subject("重复订单");
        let err = provider.pay(dup).unwrap_err();
        match err {
            PayError::RequestFailed(msg) => assert!(msg.contains("订单号已存在")),
            other => panic!("期望 RequestFailed, 实际 {other:?}"),
        }
        // 不应新增记录
        assert_eq!(provider.orders().len(), 1);
    }

    /// 测试 MemoryPayProvider 查询订单
    #[test]
    fn test_memory_pay_provider_query() {
        let provider = MemoryPayProvider::new();

        // 1. 查询不存在的订单应失败
        let err = provider.query("NOT_EXIST").unwrap_err();
        match err {
            PayError::QueryFailed(msg) => assert!(msg.contains("订单不存在")),
            other => panic!("期望 QueryFailed, 实际 {other:?}"),
        }

        // 2. 发起支付后查询
        let order = PayOrder::new()
            .out_trade_no("Q001")
            .total_amount(1000)
            .subject("查询测试");
        provider.pay(order).expect("支付应成功");

        let result = provider.query("Q001").expect("查询应成功");
        assert_eq!(result.out_trade_no, "Q001");
        assert_eq!(result.total_amount, 1000);
        assert_eq!(result.trade_no, "MEMQ001");

        // 3. 预置查询结果优先返回
        let preset = PayResult {
            trade_no: "PRESET001".to_string(),
            out_trade_no: "ANY".to_string(),
            total_amount: 9999,
            trade_status: "TRADE_SUCCESS".to_string(),
            raw: serde_json::json!({"preset": true}),
        };
        provider.set_query_result(preset);

        // 即使订单不存在，也返回预置结果
        let result = provider.query("NOT_EXIST").expect("应返回预置结果");
        assert_eq!(result.trade_no, "PRESET001");
        assert_eq!(result.total_amount, 9999);
        assert_eq!(result.trade_status, "TRADE_SUCCESS");
        assert_eq!(result.raw["preset"], true);

        // clear 后预置结果被清除
        provider.clear();
        let err = provider.query("NOT_EXIST").unwrap_err();
        match err {
            PayError::QueryFailed(_) => {}
            other => panic!("期望 QueryFailed, 实际 {other:?}"),
        }
    }

    /// 测试 MemoryPayProvider 关闭订单
    #[test]
    fn test_memory_pay_provider_close() {
        let provider = MemoryPayProvider::new();

        // 1. 关闭不存在的订单应失败
        let err = provider.close("NOT_EXIST").unwrap_err();
        match err {
            PayError::RequestFailed(msg) => assert!(msg.contains("订单不存在")),
            other => panic!("期望 RequestFailed, 实际 {other:?}"),
        }

        // 2. 发起支付后关闭
        let order = PayOrder::new()
            .out_trade_no("C001")
            .total_amount(500)
            .subject("关闭测试");
        provider.pay(order).expect("支付应成功");

        // 关闭订单
        provider.close("C001").expect("关闭应成功");

        // 3. 查询确认状态为 CLOSED
        let result = provider.query("C001").expect("查询应成功");
        assert_eq!(result.trade_status, "CLOSED");
    }

    /// 测试 MemoryPayProvider 退款
    #[test]
    fn test_memory_pay_provider_refund() {
        let provider = MemoryPayProvider::new();

        // 1. 原订单不存在时退款应失败
        let refund = RefundOrder::new()
            .out_trade_no("NOT_EXIST")
            .refund_amount(100)
            .out_request_no("R001");
        let err = provider.refund(refund).unwrap_err();
        match err {
            PayError::RefundFailed(msg) => assert!(msg.contains("原订单不存在")),
            other => panic!("期望 RefundFailed, 实际 {other:?}"),
        }
        assert_eq!(provider.refunds().len(), 0);

        // 2. 发起支付后退款
        let order = PayOrder::new()
            .out_trade_no("R001")
            .total_amount(1000)
            .subject("退款测试");
        provider.pay(order).expect("支付应成功");

        let refund = RefundOrder::new()
            .out_trade_no("R001")
            .refund_amount(500)
            .out_request_no("RR001")
            .reason("商品缺货");
        provider.refund(refund).expect("退款应成功");

        // 退款记录已存储
        assert_eq!(provider.refunds().len(), 1);
        let stored = &provider.refunds()[0];
        assert_eq!(stored.out_trade_no, "R001");
        assert_eq!(stored.refund_amount, 500);
        assert_eq!(stored.out_request_no, "RR001");
        assert_eq!(stored.reason.as_deref(), Some("商品缺货"));

        // 3. 退款订单缺字段应失败
        let bad = RefundOrder::new()
            .out_trade_no("R001")
            .refund_amount(0) // 金额无效
            .out_request_no("RR002");
        let err = provider.refund(bad).unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "refund_amount"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
        // 不应新增退款记录
        assert_eq!(provider.refunds().len(), 1);
    }

    /// 测试 MemoryPayProvider 验证回调通知
    #[test]
    fn test_memory_pay_provider_verify_notify() {
        let provider = MemoryPayProvider::new();

        // 1. 完整回调参数
        let params = serde_json::json!({
            "out_trade_no": "CB001",
            "trade_no": "2024ALIPAY001",
            "total_amount": 8800,
            "trade_status": "TRADE_SUCCESS",
            "buyer_id": "2088000000000001"
        });
        let result = provider.verify_notify(&params).expect("验证应成功");
        assert_eq!(result.out_trade_no, "CB001");
        assert_eq!(result.trade_no, "2024ALIPAY001");
        assert_eq!(result.total_amount, 8800);
        assert_eq!(result.trade_status, "TRADE_SUCCESS");
        // raw 保留原始参数
        assert_eq!(result.raw["buyer_id"], "2088000000000001");

        // 2. 缺少 out_trade_no 应失败
        let params = serde_json::json!({
            "trade_no": "2024ALIPAY001",
            "total_amount": 8800
        });
        let err = provider.verify_notify(&params).unwrap_err();
        match err {
            PayError::VerifyFailed(msg) => assert!(msg.contains("out_trade_no")),
            other => panic!("期望 VerifyFailed, 实际 {other:?}"),
        }

        // 3. 缺省字段回退：trade_status 默认 TRADE_SUCCESS
        let params = serde_json::json!({
            "out_trade_no": "CB002",
            "trade_no": "T002"
        });
        let result = provider.verify_notify(&params).expect("验证应成功");
        assert_eq!(result.out_trade_no, "CB002");
        assert_eq!(result.trade_no, "T002");
        assert_eq!(result.total_amount, 0); // 缺省 0
        assert_eq!(result.trade_status, "TRADE_SUCCESS"); // 缺省值
    }

    /// 测试 MemoryPayProvider 支付时缺字段返回错误
    #[test]
    fn test_memory_pay_provider_missing_fields() {
        let provider = MemoryPayProvider::new();

        // 缺 out_trade_no
        let order = PayOrder::new().total_amount(100).subject("标题");
        let err = provider.pay(order).unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "out_trade_no"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
        assert_eq!(provider.orders().len(), 0);

        // total_amount <= 0
        let order = PayOrder::new()
            .out_trade_no("M001")
            .total_amount(0)
            .subject("标题");
        let err = provider.pay(order).unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "total_amount"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
        assert_eq!(provider.orders().len(), 0);

        // 缺 subject
        let order = PayOrder::new().out_trade_no("M002").total_amount(100);
        let err = provider.pay(order).unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "subject"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
        assert_eq!(provider.orders().len(), 0);

        // 空订单（全默认值）应失败
        let err = provider.pay(PayOrder::default()).unwrap_err();
        match err {
            PayError::MissingField(field) => assert_eq!(field, "out_trade_no"),
            other => panic!("期望 MissingField, 实际 {other:?}"),
        }
        assert_eq!(provider.orders().len(), 0);
    }

    // ------------------------------------------------------------------------
    // MemoryPayHttpTransport 测试
    // ------------------------------------------------------------------------

    /// 测试 MemoryPayHttpTransport post_json
    #[test]
    fn test_memory_pay_http_transport_post_json() {
        let transport = MemoryPayHttpTransport::new();

        // 队列空时返回错误
        let err = transport
            .post_json("https://api.example.com/pay", "{}")
            .unwrap_err();
        match err {
            PayError::HttpTransport(msg) => assert!(msg.contains("无可用预置响应")),
            other => panic!("期望 HttpTransport, 实际 {other:?}"),
        }
        assert_eq!(transport.request_count(), 0);

        // 预置响应后返回响应并记录请求
        transport.push_response(r#"{"code":"00","msg":"success"}"#);
        let resp = transport
            .post_json("https://api.example.com/pay", r#"{"out_trade_no":"P001"}"#)
            .expect("应返回预置响应");
        assert_eq!(resp, r#"{"code":"00","msg":"success"}"#);
        assert_eq!(transport.request_count(), 1);

        // 验证请求记录
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "POST");
        assert_eq!(requests[0].1, "https://api.example.com/pay");
        assert_eq!(requests[0].2, r#"{"out_trade_no":"P001"}"#);

        // 再次调用队列空返回错误
        let err = transport.post_json("url", "{}").unwrap_err();
        match err {
            PayError::HttpTransport(_) => {}
            other => panic!("期望 HttpTransport, 实际 {other:?}"),
        }
        // 失败请求不应记录
        assert_eq!(transport.request_count(), 1);
    }

    /// 测试 MemoryPayHttpTransport get
    #[test]
    fn test_memory_pay_http_transport_get() {
        let transport = MemoryPayHttpTransport::new();

        // 队列空时返回错误
        let err = transport.get("https://api.example.com/query").unwrap_err();
        match err {
            PayError::HttpTransport(msg) => assert!(msg.contains("无可用预置响应")),
            other => panic!("期望 HttpTransport, 实际 {other:?}"),
        }
        assert_eq!(transport.request_count(), 0);

        // 预置响应后返回响应并记录请求
        transport.push_response(r#"{"trade_status":"TRADE_SUCCESS"}"#);
        let resp = transport
            .get("https://api.example.com/query?out_trade_no=Q001")
            .expect("应返回预置响应");
        assert_eq!(resp, r#"{"trade_status":"TRADE_SUCCESS"}"#);
        assert_eq!(transport.request_count(), 1);

        // 验证请求记录（GET 的 body 为空）
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "GET");
        assert_eq!(
            requests[0].1,
            "https://api.example.com/query?out_trade_no=Q001"
        );
        assert_eq!(requests[0].2, "");

        // clear 后队列和记录均清空
        transport.clear();
        assert_eq!(transport.request_count(), 0);
        assert!(transport.get("url").is_err());
    }

    /// 测试 MemoryPayHttpTransport 响应队列 FIFO 顺序
    #[test]
    fn test_memory_pay_http_transport_queue() {
        let transport = MemoryPayHttpTransport::new();

        // 预置 3 条响应
        transport.push_response("resp1");
        transport.push_response("resp2");
        transport.push_response("resp3");

        // 交替调用 post_json / get，验证 FIFO 顺序
        let r1 = transport.post_json("url1", "body1").expect("应返回 resp1");
        assert_eq!(r1, "resp1");

        let r2 = transport.get("url2").expect("应返回 resp2");
        assert_eq!(r2, "resp2");

        let r3 = transport.post_json("url3", "body3").expect("应返回 resp3");
        assert_eq!(r3, "resp3");

        // 队列已空
        assert!(transport.post_json("url4", "body4").is_err());
        assert!(transport.get("url4").is_err());

        // 验证请求记录顺序与调用顺序一致
        assert_eq!(transport.request_count(), 3);
        let requests = transport.requests();
        assert_eq!(
            requests[0],
            ("POST".to_string(), "url1".to_string(), "body1".to_string())
        );
        assert_eq!(
            requests[1],
            ("GET".to_string(), "url2".to_string(), String::new())
        );
        assert_eq!(
            requests[2],
            ("POST".to_string(), "url3".to_string(), "body3".to_string())
        );
    }
}

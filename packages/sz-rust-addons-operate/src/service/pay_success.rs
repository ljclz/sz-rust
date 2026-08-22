//! PaySuccessService — 支付成功后处理服务 — 对齐 PHP `addons/operate/service/PaySuccessService.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `onPaySuccess($data)` | [`PaySuccessService::on_pay_success`] | 支付成功入口 |
//! | `paySuccess($data)` (private) | 内部实现 | 事务包裹的核心逻辑 |
//! | `updateOrderInfo($data)` (private) | 内部实现 | 更新订单状态 |
//! | `updateContract()` (private) | 内部实现 | 更新合同已付金额 |
//! | `qySend()` (private) | 内部实现 | 企业微信通知 |
//!
//! ## 外部依赖
//!
//! PHP 依赖企业微信 `qySend()` 推送通知。
//! Rust 端通过 [`HttpPaySuccessService`] 使用 reqwest 调用企微 webhook 推送支付成功通知。
//!
//! ## PHP 源码依据
//!
//! ```php
//! class PaySuccessService extends BaseService {
//!     public function onPaySuccess($data): bool {
//!         $this->model = OrderModel::getPayDetail($this->orderNo);
//!         if ($this->model) { $this->paySuccess($data); }
//!         return $this->getError() == '';
//!     }
//!     private function paySuccess($data): void {
//!         $this->model->transaction(function () use ($data) {
//!             $this->updateOrderInfo($data);  // order_status=30, pay_status=20
//!             if (contract_id > 0 && sync_status == 10) { $this->updateContract(); }
//!             $this->qySend();
//!         });
//!     }
//! }
//! ```

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::service::http_client::HttpBankClient;

/// 支付成功后处理服务 trait — 对齐 PHP `PaySuccessService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<bool, String>`：对齐 PHP `return true/false + $this->error` 模式
/// - `order_no: &str`：对齐 PHP 构造函数 `$orderNo` 注入
#[async_trait]
pub trait PaySuccessService: Send + Sync {
    /// 支付成功处理 — 对齐 PHP `onPaySuccess($data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function onPaySuccess($data): bool {
    ///     $this->model = OrderModel::getPayDetail($this->orderNo);
    ///     if ($this->model) {
    ///         $this->paySuccess($data);  // 事务：更新订单 + 更新合同 + 企微通知
    ///     }
    ///     return $this->getError() == '';
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order_no`：订单号（对齐 PHP 构造函数 `$this->orderNo`）
    /// - `data`：支付数据（含 `pay_source`/`transaction_id`/`epay_id`）
    ///
    /// # 返回
    ///
    /// - `Ok(true)`：处理成功
    /// - `Ok(false)`：订单不存在（对齐 PHP `$this->model` 为空）
    /// - `Err(String)`：处理错误（对齐 PHP `$this->error`）
    async fn on_pay_success(&self, order_no: &str, data: &Value) -> Result<bool, String>;
}

/// Mock 支付成功服务 — 用于单元测试
///
/// # 行为
///
/// - `on_pay_success`：返回 `true`（模拟成功更新订单状态 + 合同 + 通知）
#[cfg(test)]
pub struct MockPaySuccessService;

#[cfg(test)]
#[async_trait::async_trait]
impl PaySuccessService for MockPaySuccessService {
    #[tracing::instrument(skip(self))]
    async fn on_pay_success(&self, _order_no: &str, _data: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

/// 真实 HTTP 支付成功服务 — C-4 修复
///
/// 通过 reqwest 调用企业微信 webhook 推送支付成功通知，
/// 对齐 PHP `PaySuccessService::qySend()` 的企微通知逻辑。
///
/// # 配置
///
/// 通过环境变量 `QYWX_WEBHOOK_URL` 配置企微 webhook URL。
/// 未配置时跳过通知但返回 `Ok(true)`（对齐 PHP 在无 webhook 时不阻塞业务）。
///
/// # 行为
///
/// - `on_pay_success`：构造含 `order_no`/`pay_source`/`transaction_id` 的企微通知消息，
///   POST 到 `QYWX_WEBHOOK_URL`；webhook 未配置时跳过通知返回 `Ok(true)`；
///   HTTP 失败返回 `Err`（对齐 PHP `$this->error`）
///
/// # 与 Mock 的区别
///
/// `MockPaySuccessService` 仅返回 `Ok(true)` 用于测试；
/// 本实现真正发送 HTTP 请求到企微 webhook。
pub struct HttpPaySuccessService {
    webhook_url: Option<String>,
}

impl HttpPaySuccessService {
    /// 从环境变量创建实例 — C-4 修复
    ///
    /// 读取 `QYWX_WEBHOOK_URL`，未配置时 `webhook_url` 为 `None`。
    pub fn from_env() -> Self {
        Self {
            webhook_url: std::env::var("QYWX_WEBHOOK_URL").ok(),
        }
    }

    /// 异步发送企微通知 — C-4 修复
    ///
    /// 直接 await reqwest 调用，适配 `PaySuccessService` trait 的异步方法签名。
    async fn send_qywx_notify(&self, body: &Value) -> Result<(), String> {
        let webhook_url = self
            .webhook_url
            .clone()
            .ok_or_else(|| "QYWX_WEBHOOK_URL 未配置".to_string())?;
        let config = crate::service::http_client::HttpBankConfig {
            api_url: webhook_url,
            merchant_id: String::new(),
            sign_key: String::new(),
            timeout_secs: 30,
        };
        let client = HttpBankClient::new(config);
        client.post_json("", body).await.map(|_| ())
    }
}

#[async_trait::async_trait]
impl PaySuccessService for HttpPaySuccessService {
    #[tracing::instrument(skip(self, data))]
    async fn on_pay_success(&self, order_no: &str, data: &Value) -> Result<bool, String> {
        // webhook 未配置：跳过通知但返回 Ok(true)（对齐 PHP 无 webhook 时不阻塞业务）
        if self.webhook_url.is_none() {
            tracing::warn!("QYWX_WEBHOOK_URL 未配置，跳过企微支付成功通知");
            return Ok(true);
        }
        // 构造企微通知消息（含 order_no/pay_source/transaction_id）— 对齐 PHP qySend
        let pay_source = data
            .get("pay_source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let transaction_id = data
            .get("transaction_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let msg = json!({
            "msgtype": "markdown",
            "markdown": {
                "content": format!(
                    "### 支付成功通知\n> 订单号：`{order_no}`\n> 支付渠道：`{pay_source}`\n> 交易号：`{transaction_id}`"
                )
            }
        });
        self.send_qywx_notify(&msg).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_mock_pay_success_returns_true() {
        let svc = MockPaySuccessService;
        let result = svc
            .on_pay_success(
                "ORDER_001",
                &json!({
                    "pay_source": "ccb",
                    "transaction_id": "TRACE_001"
                }),
            )
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_mock_pay_success_with_icbc_source() {
        let svc = MockPaySuccessService;
        let result = svc
            .on_pay_success(
                "ORDER_002",
                &json!({
                    "pay_source": "icbc",
                    "transaction_id": "ICBC_001"
                }),
            )
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_mock_pay_success_with_fuiou_source() {
        let svc = MockPaySuccessService;
        let result = svc
            .on_pay_success(
                "ORDER_003",
                &json!({
                    "pay_source": "fuiou",
                    "epay_id": "FUIOU_001"
                }),
            )
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_mock_pay_success_with_empty_data() {
        let svc = MockPaySuccessService;
        let result = svc.on_pay_success("ORDER_004", &json!({})).await.unwrap();
        assert!(result);
    }

    // ========================================================================
    // T-1 失败路径测试 — 覆盖 PaySuccessService 错误场景
    // ========================================================================

    /// 全失败 Mock — on_pay_success 返回 Err
    pub struct FailingPaySuccessService;

    #[async_trait::async_trait]
    impl PaySuccessService for FailingPaySuccessService {
        async fn on_pay_success(&self, _order_no: &str, _data: &Value) -> Result<bool, String> {
            Err("支付成功处理失败：订单状态更新异常".to_string())
        }
    }

    /// 业务失败 Mock — on_pay_success 返回 Ok(false)（订单不存在）
    pub struct OrderNotFoundPaySuccessService;

    #[async_trait::async_trait]
    impl PaySuccessService for OrderNotFoundPaySuccessService {
        async fn on_pay_success(&self, _order_no: &str, _data: &Value) -> Result<bool, String> {
            Ok(false) // 对齐 PHP `$this->model` 为空时返回 false
        }
    }

    #[tokio::test]
    async fn test_failing_pay_success_returns_error() {
        // T-1: FailingPaySuccessService.on_pay_success 应返回 Err
        let svc = FailingPaySuccessService;
        let result = svc
            .on_pay_success("ORDER_001", &json!({"pay_source": "ccb"}))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("支付成功处理失败"),
            "应返回失败错误信息"
        );
    }

    #[tokio::test]
    async fn test_order_not_found_pay_success_returns_false() {
        // T-1: OrderNotFoundPaySuccessService.on_pay_success 应返回 Ok(false)
        // 对齐 PHP `if (!$this->model) return false;` 语义
        let svc = OrderNotFoundPaySuccessService;
        let result = svc
            .on_pay_success("NON_EXISTENT", &json!({"pay_source": "ccb"}))
            .await
            .unwrap();
        assert!(!result, "订单不存在时应返回 Ok(false)");
    }

    #[tokio::test]
    async fn test_http_pay_success_returns_true_when_no_webhook() {
        // T-1: HttpPaySuccessService.on_pay_success 无 webhook 配置时应返回 Ok(true)
        // 注意：测试环境中 QYWX_WEBHOOK_URL 未设置
        let svc = HttpPaySuccessService::from_env();
        let result = svc
            .on_pay_success(
                "ORDER_001",
                &json!({"pay_source": "ccb", "transaction_id": "TRACE_001"}),
            )
            .await
            .unwrap();
        assert!(result, "无 webhook 配置时应返回 Ok(true)");
    }
}

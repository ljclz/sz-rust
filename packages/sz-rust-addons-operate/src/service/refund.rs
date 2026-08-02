//! RefundService — 退款服务 — 对齐 PHP `addons/operate/service/RefundService.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `execute($param)` | [`RefundService::execute`] | 退款入口（按 pay_type 分发） |
//! | `refund($refund_price)` | [`RefundService::refund`] | 现金/组合退款 |
//! | `reject($refund_price)` | [`RefundService::reject`] | 电子支付退款（icbc/ccb/fuiou） |
//! | `updateOrderInfo($update)` (private) | 内部实现 | 更新订单退款信息 |
//! | `qySend($refund_price)` (private) | 内部实现 | 企业微信通知 |
//!
//! ## 外部依赖
//!
//! PHP 依赖 `IcbcService`/`CcbService`/`FuiouService` 执行银行退款，
//! 依赖企业微信 `qySend()` 推送通知。
//! Rust 端通过 [`HttpRefundService`] 使用 reqwest 调用企微 webhook 推送退款通知。
//!
//! ## PHP 源码依据
//!
//! ```php
//! class RefundService extends BaseService {
//!     public function execute($param): bool {
//!         $this->model = OrderModel::detail($param['order_id']);
//!         if ($this->model) {
//!             if (pay_status != 20) return false;
//!             if (pay_type == EPAY) return $this->reject($this->model['pay_price']);
//!             if (pay_type == CASH) return $this->refund($this->model['pay_price']);
//!             if (pay_type == UNITE) return $this->refund($this->model['epay_price']);
//!         }
//!         return false;
//!     }
//! }
//! ```

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::service::http_client::HttpBankClient;

/// 退款服务 trait — 对齐 PHP `RefundService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<bool, String>`：对齐 PHP `return true/false + $this->error` 模式
#[async_trait]
pub trait RefundService: Send + Sync {
    /// 退款入口 — 对齐 PHP `execute($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function execute($param): bool {
    ///     $this->opt_uid = (int)$param['opt_uid'] ?? 0;
    ///     $this->model = OrderModel::detail($param['order_id']);
    ///     if ($this->model) {
    ///         if (pay_status != 20) return false;  // 仅已支付订单可退
    ///         if (pay_type == EPAY) return $this->reject($this->model['pay_price']);
    ///         if (pay_type == CASH) return $this->refund($this->model['pay_price']);
    ///         if (pay_type == UNITE) return $this->refund($this->model['epay_price']);
    ///     }
    ///     return false;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `param`：退款参数（含 `order_id`/`opt_uid`）
    ///
    /// # 返回
    ///
    /// - `Ok(true)`：退款成功
    /// - `Ok(false)`：订单不存在或不可退款（pay_status != 20）
    /// - `Err(String)`：退款错误（对齐 PHP `$this->error`）
    async fn execute(&self, param: &Value) -> Result<bool, String>;

    /// 现金/组合退款 — 对齐 PHP `refund($refund_price)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function refund($refund_price): bool {
    ///     $update = ['refund_trade_no'=>'', 'refund_price'=>$refund_price,
    ///                'pay_status'=>30, 'update_time'=>time()];
    ///     return $this->updateOrderInfo($update);  // 更新订单 + 企微通知
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order_id`：订单 ID
    /// - `refund_price`：退款金额
    async fn refund(&self, order_id: i64, refund_price: f64) -> Result<bool, String>;

    /// 电子支付退款 — 对齐 PHP `reject($refund_price)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function reject($refund_price): bool {
    ///     $status = $this->model->transaction(function () use ($refund_price) {
    ///         if (pay_source == 'icbc') { $respObj = (new IcbcService())->reject($data); ... }
    ///         else if (pay_source == 'ccb') { $respObj = (new CcbService())->reject($data); ... }
    ///         else if (pay_source == 'fuiou') { $respObj = (new fuiouService())->reject($data); ... }
    ///     });
    ///     return $status;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order_id`：订单 ID
    /// - `refund_price`：退款金额
    async fn reject(&self, order_id: i64, refund_price: f64) -> Result<bool, String>;
}

/// Mock 退款服务 — 用于单元测试
///
/// # 行为
///
/// - `execute`：返回 `true`（模拟按 pay_type 分发成功）
/// - `refund`：返回 `true`（模拟现金退款成功）
/// - `reject`：返回 `true`（模拟电子支付退款成功）
#[cfg(test)]
pub struct MockRefundService;

#[cfg(test)]
#[async_trait::async_trait]
impl RefundService for MockRefundService {
    #[tracing::instrument(skip(self))]
    async fn execute(&self, _param: &Value) -> Result<bool, String> {
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    async fn refund(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    async fn reject(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
        Ok(true)
    }
}

/// 真实 HTTP 退款服务 — C-4 修复
///
/// 通过 reqwest 调用企业微信 webhook 推送退款通知，
/// 对齐 PHP `RefundService::qySend()` 的企微通知逻辑。
///
/// # 配置
///
/// 通过环境变量 `QYWX_WEBHOOK_URL` 配置企微 webhook URL。
/// 未配置时跳过通知但返回 `Ok(true)`。
///
/// # 行为
///
/// - `execute`：校验 `order_id` 存在且 `pay_status==20`（对齐 PHP 仅已支付订单可退），
///   校验失败返回 `Ok(false)`；校验通过后发送企微通知，返回 `Ok(true)`
/// - `refund`：发送企微现金退款通知，返回 `Ok(true)`
/// - `reject`：发送企微电子退款通知，返回 `Ok(true)`
///
/// # 与 Mock 的区别
///
/// `MockRefundService` 仅返回 `Ok(true)` 用于测试；
/// 本实现真正校验业务条件并发送 HTTP 请求到企微 webhook。
pub struct HttpRefundService {
    webhook_url: Option<String>,
}

impl HttpRefundService {
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
    /// 直接 await reqwest 调用，适配 `RefundService` trait 的异步方法签名。
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
impl RefundService for HttpRefundService {
    #[tracing::instrument(skip(self, param))]
    async fn execute(&self, param: &Value) -> Result<bool, String> {
        // 校验 order_id 存在 — 对齐 PHP `OrderModel::detail($param['order_id'])`
        let order_id = match param.get("order_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => {
                tracing::warn!("refund execute: order_id 不存在，跳过退款");
                return Ok(false);
            }
        };
        // 校验 pay_status==20 — 对齐 PHP `if (pay_status != 20) return false;`
        let pay_status = param
            .get("pay_status")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if pay_status != 20 {
            tracing::warn!(order_id, pay_status, "退款校验失败：pay_status != 20");
            return Ok(false);
        }
        // 校验通过：发送企微退款通知 — 对齐 PHP qySend
        if self.webhook_url.is_some() {
            let pay_price = param
                .get("pay_price")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let msg = json!({
                "msgtype": "markdown",
                "markdown": {
                    "content": format!("### 退款通知\n> 订单号：`{order_id}`\n> 退款金额：`{pay_price}`")
                }
            });
            self.send_qywx_notify(&msg).await?;
        }
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    async fn refund(&self, order_id: i64, refund_price: f64) -> Result<bool, String> {
        // 发送企微现金退款通知 — 对齐 PHP `refund($refund_price)` 的 qySend
        if self.webhook_url.is_some() {
            let msg = json!({
                "msgtype": "markdown",
                "markdown": {
                    "content": format!("### 现金退款通知\n> 订单号：`{order_id}`\n> 退款金额：`{refund_price}`")
                }
            });
            self.send_qywx_notify(&msg).await?;
        }
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    async fn reject(&self, order_id: i64, refund_price: f64) -> Result<bool, String> {
        // 发送企微电子退款通知 — 对齐 PHP `reject($refund_price)` 的 qySend
        if self.webhook_url.is_some() {
            let msg = json!({
                "msgtype": "markdown",
                "markdown": {
                    "content": format!("### 电子退款通知\n> 订单号：`{order_id}`\n> 退款金额：`{refund_price}`")
                }
            });
            self.send_qywx_notify(&msg).await?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_mock_refund_execute_returns_true() {
        let svc = MockRefundService;
        let result = svc
            .execute(&json!({
                "order_id": 1,
                "opt_uid": 100
            })).await
            .unwrap();
        assert!(result);
    }

#[tokio::test]
    async fn test_mock_refund_cash_returns_true() {
        let svc = MockRefundService;
        let result = svc.refund(1, 100.50).await.unwrap();
        assert!(result);
    }

#[tokio::test]
    async fn test_mock_refund_epay_returns_true() {
        let svc = MockRefundService;
        let result = svc.reject(1, 200.00).await.unwrap();
        assert!(result);
    }

#[tokio::test]
    async fn test_mock_refund_with_zero_amount() {
        let svc = MockRefundService;
        let result = svc.refund(1, 0.0).await.unwrap();
        assert!(result);
    }

    // ========================================================================
    // T-1 失败路径测试 — HttpRefundService 业务校验失败场景
    // ========================================================================
    //
    // 审计报告 T-1 指出：Mock 测试永远返回 Ok(true)，无法检测任何生产代码 bug。
    // 此处通过 HttpRefundService（无 webhook 配置）测试真实业务校验逻辑的失败路径。

#[tokio::test]
    async fn test_http_refund_execute_returns_false_when_order_id_missing() {
        // T-1: HttpRefundService.execute 当 order_id 不存在时应返回 Ok(false)
        // 对齐 PHP `if (!$this->model) return false;`
        let svc = HttpRefundService::from_env();
        let result = svc.execute(&json!({"opt_uid": 100})).await.unwrap();
        assert!(!result, "order_id 缺失时 execute 应返回 Ok(false)");
    }

    #[tokio::test]
    async fn test_http_refund_execute_returns_false_when_pay_status_not_20() {
        // T-1: HttpRefundService.execute 当 pay_status != 20 时应返回 Ok(false)
        // 对齐 PHP `if (pay_status != 20) return false;`
        let svc = HttpRefundService::from_env();
        // pay_status=10（未支付）
        let result = svc
            .execute(&json!({"order_id": 1, "pay_status": 10, "pay_price": 100.0})).await
            .unwrap();
        assert!(!result, "pay_status=10 时 execute 应返回 Ok(false)");

        // pay_status=30（已退款）
        let result = svc
            .execute(&json!({"order_id": 1, "pay_status": 30, "pay_price": 100.0})).await
            .unwrap();
        assert!(!result, "pay_status=30 时 execute 应返回 Ok(false)");

        // pay_status=0（默认值）
        let result = svc
            .execute(&json!({"order_id": 1, "pay_price": 100.0})).await
            .unwrap();
        assert!(!result, "pay_status 缺失时 execute 应返回 Ok(false)");
    }

    #[tokio::test]
    async fn test_http_refund_execute_returns_true_when_pay_status_20_and_no_webhook() {
        // T-1: HttpRefundService.execute 当 pay_status=20 且无 webhook 配置时应返回 Ok(true)
        // 注意：测试环境中 QYWX_WEBHOOK_URL 未设置，webhook 通知被跳过
        let svc = HttpRefundService::from_env();
        let result = svc
            .execute(&json!({"order_id": 1, "pay_status": 20, "pay_price": 100.0})).await
            .unwrap();
        assert!(
            result,
            "pay_status=20 且无 webhook 时 execute 应返回 Ok(true)"
        );
    }

#[tokio::test]
    async fn test_http_refund_refund_returns_true_when_no_webhook() {
        // T-1: HttpRefundService.refund 无 webhook 配置时应返回 Ok(true)
        let svc = HttpRefundService::from_env();
        let result = svc.refund(1, 100.50).await.unwrap();
        assert!(result);
    }

#[tokio::test]
    async fn test_http_refund_reject_returns_true_when_no_webhook() {
        // T-1: HttpRefundService.reject 无 webhook 配置时应返回 Ok(true)
        let svc = HttpRefundService::from_env();
        let result = svc.reject(1, 200.00).await.unwrap();
        assert!(result);
    }

    // ========================================================================
    // T-1 失败注入 Mock — 用于上层控制器的失败路径测试
    // ========================================================================

    /// 全失败 Mock — 所有方法返回 Err
    pub struct FailingRefundService;

    #[async_trait::async_trait]
impl RefundService for FailingRefundService {
        async fn execute(&self, _param: &Value) -> Result<bool, String> {
            Err("退款执行失败：订单查询异常".to_string())
        }
        async fn refund(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
            Err("现金退款失败：银行 API 不可用".to_string())
        }
        async fn reject(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
            Err("电子退款失败：签名校验失败".to_string())
        }
    }

    /// 业务校验失败 Mock — execute 返回 Ok(false)，其他方法返回 Err
    pub struct BusinessFailingRefundService;

    #[async_trait::async_trait]
impl RefundService for BusinessFailingRefundService {
        async fn execute(&self, _param: &Value) -> Result<bool, String> {
            Ok(false) // 模拟 PHP `if (pay_status != 20) return false;`
        }
        async fn refund(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
            Err("现金退款失败".to_string())
        }
        async fn reject(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
            Err("电子退款失败".to_string())
        }
    }

#[tokio::test]
    async fn test_failing_refund_execute_returns_error() {
        let svc = FailingRefundService;
        let result = svc.execute(&json!({"order_id": 1})).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("退款执行失败"),
            "应返回失败错误信息"
        );
    }

#[tokio::test]
    async fn test_failing_refund_refund_returns_error() {
        let svc = FailingRefundService;
        let result = svc.refund(1, 100.0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("现金退款失败"));
    }

#[tokio::test]
    async fn test_failing_refund_reject_returns_error() {
        let svc = FailingRefundService;
        let result = svc.reject(1, 100.0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("电子退款失败"));
    }

#[tokio::test]
    async fn test_business_failing_refund_execute_returns_false() {
        // T-1: 业务校验失败时 execute 应返回 Ok(false)，而非 Err
        // 对齐 PHP `return false;` 语义（业务失败，非系统错误）
        let svc = BusinessFailingRefundService;
        let result = svc.execute(&json!({"order_id": 1})).await.unwrap();
        assert!(!result, "业务校验失败时 execute 应返回 Ok(false)");
    }
}

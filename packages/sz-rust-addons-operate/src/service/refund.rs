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
//! Rust 端真实实现待后续 Phase 补全，当前提供 [`MockRefundService`] 用于测试。
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

use serde_json::Value;

/// 退款服务 trait — 对齐 PHP `RefundService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<bool, String>`：对齐 PHP `return true/false + $this->error` 模式
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
    fn execute(&self, param: &Value) -> Result<bool, String>;

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
    fn refund(&self, order_id: i64, refund_price: f64) -> Result<bool, String>;

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
    fn reject(&self, order_id: i64, refund_price: f64) -> Result<bool, String>;
}

/// Mock 退款服务 — 用于单元测试
///
/// # 行为
///
/// - `execute`：返回 `true`（模拟按 pay_type 分发成功）
/// - `refund`：返回 `true`（模拟现金退款成功）
/// - `reject`：返回 `true`（模拟电子支付退款成功）
pub struct MockRefundService;

impl RefundService for MockRefundService {
    #[tracing::instrument(skip(self))]
    fn execute(&self, _param: &Value) -> Result<bool, String> {
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    fn refund(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    fn reject(&self, _order_id: i64, _refund_price: f64) -> Result<bool, String> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mock_refund_execute_returns_true() {
        let svc = MockRefundService;
        let result = svc
            .execute(&json!({
                "order_id": 1,
                "opt_uid": 100
            }))
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_refund_cash_returns_true() {
        let svc = MockRefundService;
        let result = svc.refund(1, 100.50).unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_refund_epay_returns_true() {
        let svc = MockRefundService;
        let result = svc.reject(1, 200.00).unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_refund_with_zero_amount() {
        let svc = MockRefundService;
        let result = svc.refund(1, 0.0).unwrap();
        assert!(result);
    }
}

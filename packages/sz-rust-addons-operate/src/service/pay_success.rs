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
//! Rust 端真实实现待后续 Phase 补全，当前提供 [`MockPaySuccessService`] 用于测试。
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

use serde_json::Value;

/// 支付成功后处理服务 trait — 对齐 PHP `PaySuccessService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<bool, String>`：对齐 PHP `return true/false + $this->error` 模式
/// - `order_no: &str`：对齐 PHP 构造函数 `$orderNo` 注入
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
    fn on_pay_success(&self, order_no: &str, data: &Value) -> Result<bool, String>;
}

/// Mock 支付成功服务 — 用于单元测试
///
/// # 行为
///
/// - `on_pay_success`：返回 `true`（模拟成功更新订单状态 + 合同 + 通知）
pub struct MockPaySuccessService;

impl PaySuccessService for MockPaySuccessService {
    fn on_pay_success(&self, _order_no: &str, _data: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mock_pay_success_returns_true() {
        let svc = MockPaySuccessService;
        let result = svc
            .on_pay_success(
                "ORDER_001",
                &json!({
                    "pay_source": "ccb",
                    "transaction_id": "TRACE_001"
                }),
            )
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_pay_success_with_icbc_source() {
        let svc = MockPaySuccessService;
        let result = svc
            .on_pay_success(
                "ORDER_002",
                &json!({
                    "pay_source": "icbc",
                    "transaction_id": "ICBC_001"
                }),
            )
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_pay_success_with_fuiou_source() {
        let svc = MockPaySuccessService;
        let result = svc
            .on_pay_success(
                "ORDER_003",
                &json!({
                    "pay_source": "fuiou",
                    "epay_id": "FUIOU_001"
                }),
            )
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_pay_success_with_empty_data() {
        let svc = MockPaySuccessService;
        let result = svc.on_pay_success("ORDER_004", &json!({})).unwrap();
        assert!(result);
    }
}

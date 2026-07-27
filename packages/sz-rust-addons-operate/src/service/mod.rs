//! 服务模块 — 对齐 PHP `addons/operate/service/`
//!
//! ## PHP 对齐
//!
//! | PHP 服务 | Rust Trait | 说明 |
//! |---------|-----------|------|
//! | `SettledService` | [`SettledService`] | 收款下单服务（createOrder/onPayBuy/epayCheck/onRefund） |
//! | `CcbService` | [`ccb::CcbService`] | 建设银行支付服务（ccbPay/ccbCheck/reject/addBill） |
//! | `IcbcService` | [`icbc::IcbcService`] | 工商银行支付服务（icbcPay/icbcCheck/reject/addBill） |
//! | `FuiouService` | [`fuiou::FuiouService`] | 富友支付服务（fuiouPay/fuiouCheck/reject/addBill/editBill） |
//! | `PaySuccessService` | [`pay_success::PaySuccessService`] | 支付成功后处理服务（onPaySuccess） |
//! | `RefundService` | [`refund::RefundService`] | 退款服务（execute/refund/reject） |
//!
//! ## 架构设计
//!
//! ### PHP → Rust 映射
//!
//! PHP 服务通过 `new SettledService($data)` 直接实例化，
//! Rust 通过 trait 注入解耦，便于测试（mock）和生产替换。
//!
//! ### 外部依赖说明
//!
//! 5 个银行/通知服务（Ccb/Icbc/Fuiou/PaySuccess/Refund）依赖外部 SDK
//!（ccbPay/DefaultIcbcClient/Fuiou Constants/企业微信 qySend），
//! 真实实现待后续补全，当前提供 Mock 实现用于单元测试。
//!
//! ### 1:1 PHP 对齐
//!
//! Trait 方法签名严格对齐 PHP 公开方法：
//! - `createOrder()` → `create_order(data: &Value) -> Result<Value, String>`
//! - `onPayBuy()` → `pay_buy(detail: &Value, data: &Value) -> Result<Value, String>`
//! - `epayCheck()` → `epay_check(param: &Value) -> Result<Value, String>`
//! - `onRefund()` → `refund(detail: &Value, param: &Value) -> Result<(), String>`

pub mod ccb;
pub mod fuiou;
pub mod icbc;
pub mod pay_success;
pub mod refund;

pub use ccb::{CcbService, MockCcbService};
pub use fuiou::{FuiouService, MockFuiouService};
pub use icbc::{IcbcService, MockIcbcService};
pub use pay_success::{MockPaySuccessService, PaySuccessService};
pub use refund::{MockRefundService, RefundService};

use serde_json::Value;

/// 收款下单服务 trait — 对齐 PHP `SettledService`
///
/// # PHP 对齐
///
/// ```php
/// class SettledService extends BaseService {
///     public function createOrder() { ... }
///     // onPayBuy/epayCheck/onRefund 在 OrderModel 上，但实际委托给服务层
/// }
/// ```
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 的 `FromRef` 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
/// - `data: &Value`：对齐 PHP 构造函数 `$params` 注入
pub trait SettledService: Send + Sync {
    /// 创建订单 — 对齐 PHP `SettledService::createOrder()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function createOrder() {
    ///     $status = $this->model->transaction(fn => $this->add());
    ///     $pay_res = ['msg'=>'', 'data'=>[]];
    ///     if ($status && $this->params['pay_type']) {
    ///         if (EPAY/UNITE) { $pay_res = $this->model->onlinePayment($this->params); }
    ///         else { $this->model->onPayment($this->order_no, $this->params); }
    ///     }
    ///     $orderDetail = OrderModel::detail($this->model['order_id']);
    ///     $orderDetail['pay_res'] = $pay_res;
    ///     return $orderDetail;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `data`：订单数据（对齐 PHP `$this->params`，含 customer_id/dept_id/pay_type 等）
    ///
    /// # 返回
    ///
    /// - `Ok(Value)`：订单详情（含 `order_id` 和 `pay_res` 字段）
    /// - `Err(String)`：错误信息（对齐 PHP `$this->error`）
    fn create_order(&self, data: &Value) -> Result<Value, String>;

    /// 继续支付 — 对齐 PHP `OrderModel::onPayBuy($detail, $data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function onPayBuy($detail, $data) {
    ///     // 更新订单支付方式/金额，调用 onlinePayment/onPayment
    ///     return $orderDetail;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `detail`：当前订单详情
    /// - `data`：支付参数（pay_type/auth_code 等）
    ///
    /// # 返回
    ///
    /// - `Ok(Value)`：更新后的订单详情（含 `order_id` 和 `pay_res`）
    /// - `Err(String)`：错误信息
    fn pay_buy(&self, detail: &Value, data: &Value) -> Result<Value, String>;

    /// 支付状态查询 — 对齐 PHP `OrderModel::epayCheck($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function epayCheck($param) {
    ///     // 根据 bank_name 调用 CcbService/IcbcService/FuiouService 查询
    ///     return ['msg'=>$msg, 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `param`：查询参数（含 `order_no`/`bank_name`/`type`）
    ///
    /// # 返回
    ///
    /// - `Ok(Value)`：查询结果（含 `msg` 和 `respObj`）
    /// - `Err(String)`：错误信息
    fn epay_check(&self, param: &Value) -> Result<Value, String>;

    /// 退款 — 对齐 PHP `OrderModel::onRefund($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function onRefund($param) {
    ///     // 调用 CcbService/IcbcService/FuiouService 退款
    ///     // 更新订单 pay_status=30, refund_price
    ///     return true/false;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `detail`：当前订单详情
    /// - `param`：退款参数
    ///
    /// # 返回
    ///
    /// - `Ok(())`：退款成功
    /// - `Err(String)`：错误信息
    fn refund(&self, detail: &Value, param: &Value) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mock SettledService 实现 — 用于控制器单元测试
    pub struct MockSettledService;

    impl SettledService for MockSettledService {
        fn create_order(&self, data: &Value) -> Result<Value, String> {
            let order_id = data.get("order_id").and_then(|v| v.as_i64()).unwrap_or(1);
            Ok(json!({
                "order_id": order_id,
                "pay_res": {"msg": "success", "data": []}
            }))
        }

        fn pay_buy(&self, detail: &Value, _data: &Value) -> Result<Value, String> {
            let order_id = detail.get("order_id").and_then(|v| v.as_i64()).unwrap_or(1);
            Ok(json!({
                "order_id": order_id,
                "pay_res": {"msg": "success", "data": []}
            }))
        }

        fn epay_check(&self, _param: &Value) -> Result<Value, String> {
            Ok(json!({"msg": "查询成功", "respObj": {"RESULT": "Y"}}))
        }

        fn refund(&self, _detail: &Value, _param: &Value) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn test_mock_settled_service_create_order() {
        let svc = MockSettledService;
        let result = svc.create_order(&json!({"order_id": 42})).unwrap();
        assert_eq!(result["order_id"], 42);
        assert_eq!(result["pay_res"]["msg"], "success");
    }

    #[test]
    fn test_mock_settled_service_pay_buy() {
        let svc = MockSettledService;
        let result = svc.pay_buy(&json!({"order_id": 10}), &json!({})).unwrap();
        assert_eq!(result["order_id"], 10);
    }

    #[test]
    fn test_mock_settled_service_epay_check() {
        let svc = MockSettledService;
        let result = svc.epay_check(&json!({"order_no": "TEST001"})).unwrap();
        assert_eq!(result["msg"], "查询成功");
        assert_eq!(result["respObj"]["RESULT"], "Y");
    }

    #[test]
    fn test_mock_settled_service_refund() {
        let svc = MockSettledService;
        let result = svc.refund(&json!({"order_id": 1}), &json!({}));
        assert!(result.is_ok());
    }
}

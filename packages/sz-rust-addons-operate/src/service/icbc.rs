//! IcbcService — 工商银行支付服务 — 对齐 PHP `addons/operate/service/IcbcService.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `icbcPay($order)` | [`IcbcService::icbc_pay`] | 工行扫码支付 |
//! | `icbcCheck($order)` | [`IcbcService::icbc_check`] | 工行支付状态查询 |
//! | `reject($order)` | [`IcbcService::reject`] | 工行退款 |
//! | `addBill($data)` (static) | [`IcbcService::add_bill`] | 记录工行账单 |
//!
//! ## 外部依赖
//!
//! PHP 依赖 `app\common\service\icbcbank\DefaultIcbcClient` SDK 执行 HTTP 请求。
//! Rust 端真实实现待后续 Phase 补全，当前提供 [`MockIcbcService`] 用于测试。
//!
//! ## PHP 源码依据
//!
//! ```php
//! class IcbcService {
//!     public function icbcPay($order): array {
//!         $request = ['serviceUrl'=>'https://gw.open.icbc.com.cn/...', 'biz_content'=>[...]];
//!         $resp = (new DefaultIcbcClient())->execute($request, $msg_id, '');
//!         $respObj = json_decode($resp, true);
//!         if (return_code == 0 && pay_status == 1) { $this->addBill($respObj); (new PaySuccessService())->onPaySuccess(...); }
//!         return ['msg'=>$respObj['return_msg'], 'respObj'=>$respObj];
//!     }
//! }
//! ```

use serde_json::Value;

/// 工商银行支付服务 trait — 对齐 PHP `IcbcService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
pub trait IcbcService: Send + Sync {
    /// 工行扫码支付 — 对齐 PHP `icbcPay($order)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function icbcPay($order): array {
    ///     $request = ['biz_content'=>['qr_code'=>$order['auth_code'], 'mer_id'=>$order['bank_card'],
    ///                  'out_trade_no'=>$order['trade_no'], 'order_amt'=>strval($order['pay_price']*100), ...]];
    ///     $resp = (new DefaultIcbcClient())->execute($request, $msg_id, '');
    ///     if (return_code == 0) {
    ///         OrderModel::where(...)->update(['epay_id'=>$respObj['order_id']]);
    ///         if (pay_status == 1) { $this->addBill($respObj); (new PaySuccessService())->onPaySuccess(...); }
    ///         else if (pay_status == 0) { (new PushQueue())->checkPayStatus(...); }
    ///     }
    ///     return ['msg'=>$respObj['return_msg'], 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order`：订单数据（含 `bank_card`/`trade_no`/`auth_code`/`pay_price`/`order_no`）
    fn icbc_pay(&self, order: &Value) -> Result<Value, String>;

    /// 工行支付状态查询 — 对齐 PHP `icbcCheck($order)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function icbcCheck($order): array {
    ///     $request = ['biz_content'=>['mer_id'=>$order['bank_card'], 'out_trade_no'=>$order['trade_no'],
    ///                  'order_id'=>$order['epay_id'], ...]];
    ///     if (return_code == 0 && pay_status == 1 && type in ['pay','check']) {
    ///         $this->addBill($respObj); (new PaySuccessService())->onPaySuccess(...);
    ///     }
    ///     return ['msg'=>$respObj['return_msg'], 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order`：查询参数（含 `bank_card`/`trade_no`/`epay_id`/`type`/`order_no`）
    fn icbc_check(&self, order: &Value) -> Result<Value, String>;

    /// 工行退款 — 对齐 PHP `reject($order)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function reject($order) {
    ///     $request = ['biz_content'=>['mer_id'=>$order['bank_card'], 'out_trade_no'=>$order['trade_no'],
    ///                  'reject_no'=>$order['reject_no'], 'reject_amt'=>strval($order['refund_fee']*100), ...]];
    ///     return json_decode($resp, true);
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order`：退款参数（含 `bank_card`/`trade_no`/`reject_no`/`refund_fee`/`customer_id`）
    fn reject(&self, order: &Value) -> Result<Value, String>;

    /// 记录工行账单 — 对齐 PHP `addBill($data)` (静态方法)
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public static function addBill($data): bool {
    ///     $order = OrderModel::where(['trade_no'=>$data['out_trade_no']])->find();
    ///     if ($order) { (new EpayBank())->save($data); return true; }
    ///     return false;
    /// }
    /// ```
    fn add_bill(&self, data: &Value) -> Result<bool, String>;
}

/// Mock 工行支付服务 — 用于单元测试
pub struct MockIcbcService;

impl IcbcService for MockIcbcService {
    #[tracing::instrument(skip(self))]
    fn icbc_pay(&self, order: &Value) -> Result<Value, String> {
        let trade_no = order
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("MOCK_TRADE_NO");
        Ok(serde_json::json!({
            "msg": "success",
            "respObj": {
                "return_code": 0,
                "return_msg": "success",
                "order_id": format!("MOCK_ICBC_{trade_no}"),
                "pay_status": 1,
                "out_trade_no": trade_no
            }
        }))
    }

    #[tracing::instrument(skip(self))]
    fn icbc_check(&self, order: &Value) -> Result<Value, String> {
        let trade_no = order
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("MOCK_TRADE_NO");
        Ok(serde_json::json!({
            "msg": "success",
            "respObj": {
                "return_code": 0,
                "return_msg": "success",
                "pay_status": 1,
                "out_trade_no": trade_no
            }
        }))
    }

    #[tracing::instrument(skip(self))]
    fn reject(&self, _order: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({
            "return_code": 0,
            "return_msg": "success",
            "result_msg": "success",
            "status": 1,
            "reject_no": "MOCK_REJECT_001"
        }))
    }

    #[tracing::instrument(skip(self))]
    fn add_bill(&self, _data: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mock_icbc_pay_returns_success() {
        let svc = MockIcbcService;
        let result = svc
            .icbc_pay(&json!({
                "bank_card": "430104027383",
                "trade_no": "TEST_ICBC_001",
                "auth_code": "123456789012345678",
                "pay_price": 200.00,
                "order_no": "ORDER_001"
            }))
            .unwrap();
        assert_eq!(result["respObj"]["return_code"], 0);
        assert_eq!(result["respObj"]["pay_status"], 1);
    }

    #[test]
    fn test_mock_icbc_check_returns_success() {
        let svc = MockIcbcService;
        let result = svc
            .icbc_check(&json!({
                "bank_card": "430104027383",
                "trade_no": "TEST_ICBC_002",
                "epay_id": "MOCK_ICBC_002",
                "type": "check"
            }))
            .unwrap();
        assert_eq!(result["respObj"]["pay_status"], 1);
    }

    #[test]
    fn test_mock_icbc_reject_returns_success() {
        let svc = MockIcbcService;
        let result = svc
            .reject(&json!({
                "bank_card": "430104027383",
                "trade_no": "TEST_ICBC_001",
                "reject_no": "REJECT_001",
                "refund_fee": 100.00,
                "customer_id": 1001
            }))
            .unwrap();
        assert_eq!(result["return_code"], 0);
        assert_eq!(result["status"], 1);
    }

    #[test]
    fn test_mock_icbc_add_bill_returns_true() {
        let svc = MockIcbcService;
        let result = svc
            .add_bill(&json!({"out_trade_no": "TEST_ICBC_001"}))
            .unwrap();
        assert!(result);
    }
}

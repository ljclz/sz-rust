//! CcbService — 建设银行支付服务 — 对齐 PHP `addons/operate/service/CcbService.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `ccbPay($order)` | [`CcbService::ccb_pay`] | 建行扫码支付 |
//! | `ccbCheck($order)` | [`CcbService::ccb_check`] | 建行支付状态查询（轮询） |
//! | `reject($data)` | [`CcbService::reject`] | 建行退款 |
//! | `addBill($respObj)` (static) | [`CcbService::add_bill`] | 记录建行账单 |
//!
//! ## 外部依赖
//!
//! PHP 依赖 `app\common\service\ccb\ccbPay` SDK 执行 HTTP 请求。
//! Rust 端真实实现待后续 Phase 补全（需要 ccbPay SDK 移植），
//! 当前提供 [`MockCcbService`] 用于测试。
//!
//! ## PHP 源码依据
//!
//! ```php
//! class CcbService {
//!     public function ccbPay($order): array {
//!         $data = ['merchant_id'=>$order['bank_card'], 'posid'=>$order['bank_account'],
//!                  'ORDERID'=>$order['trade_no'], 'QRCODE'=>$order['auth_code'],
//!                  'AMOUNT'=>$order['pay_price']];
//!         $resp = (new ccbPay())->execute($data);
//!         if (str_starts_with(trim($resp), '{')) {
//!             $respObj = json_decode($resp, true);
//!             // RESULT == 'Y' 时记录账单 + 触发 PaySuccessService
//!         }
//!         return ['msg'=>$respObj['ERRMSG'], 'respObj'=>$respObj];
//!     }
//! }
//! ```

use serde_json::Value;

/// 建设银行支付服务 trait — 对齐 PHP `CcbService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
/// - `order: &Value`：对齐 PHP `$order` 参数（含 bank_card/bank_account/trade_no/auth_code/pay_price）
pub trait CcbService: Send + Sync {
    /// 建行扫码支付 — 对齐 PHP `ccbPay($order)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function ccbPay($order): array {
    ///     $data = ['merchant_id'=>$order['bank_card'], ...];
    ///     $resp = (new ccbPay())->execute($data);
    ///     if (RESULT == 'Y') { $this->addBill($respObj); (new PaySuccessService())->onPaySuccess(...); }
    ///     return ['msg'=>$respObj['ERRMSG'], 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order`：订单数据（含 `bank_card`/`bank_account`/`trade_no`/`auth_code`/`pay_price`/`order_no`）
    ///
    /// # 返回
    ///
    /// - `Ok(Value)`：支付结果（含 `msg` 和 `respObj`，对齐 PHP 返回数组）
    /// - `Err(String)`：错误信息
    fn ccb_pay(&self, order: &Value) -> Result<Value, String>;

    /// 建行支付状态查询 — 对齐 PHP `ccbCheck($order)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function ccbCheck($order): array {
    ///     $qryTime = $order['qry_time'] ?? 1;
    ///     $resp = (new ccbPay())->query([...]);
    ///     if (RESULT == 'Y') { $this->addBill($respObj); (new PaySuccessService())->onPaySuccess(...); }
    ///     else { Cache::set('ccb_qry_time_'.trade_no, qryTime+1, 30); }
    ///     return ['msg'=>$respObj['ERRMSG'], 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `order`：查询参数（含 `bank_card`/`bank_account`/`trade_no`/`qrcodetype`/`qry_time`/`type`/`order_no`）
    fn ccb_check(&self, order: &Value) -> Result<Value, String>;

    /// 建行退款 — 对齐 PHP `reject($data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function reject($data) {
    ///     $resp = (new ccbPay())->refund($data);
    ///     return json_decode($resp, true);
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `data`：退款参数（含 `mrchNo`/`refundAmt`/`payRecordNo`/`requestSn`）
    fn reject(&self, data: &Value) -> Result<Value, String>;

    /// 记录建行账单 — 对齐 PHP `addBill($respObj)` (静态方法)
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public static function addBill($respObj): bool {
    ///     $order = OrderModel::where(['trade_no'=>$respObj['ORDERID']])->find();
    ///     if ($order) { (new CcbBill())->save($data); return true; }
    ///     return false;
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `resp_obj`：建行响应对象（含 `ORDERID`/`TRACEID`/`AMOUNT`/`QRCODETYPE` 等）
    fn add_bill(&self, resp_obj: &Value) -> Result<bool, String>;
}

/// Mock 建行支付服务 — 用于单元测试
///
/// # 行为
///
/// - `ccb_pay`：返回模拟成功响应（`RESULT = "Y"`）
/// - `ccb_check`：返回模拟查询成功响应
/// - `reject`：返回模拟退款成功响应
/// - `add_bill`：返回 `true`
pub struct MockCcbService;

impl CcbService for MockCcbService {
    fn ccb_pay(&self, order: &Value) -> Result<Value, String> {
        let trade_no = order
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("MOCK_TRADE_NO");
        Ok(serde_json::json!({
            "msg": "",
            "respObj": {
                "RESULT": "Y",
                "TRACEID": format!("MOCK_TRACE_{trade_no}"),
                "ORDERID": trade_no,
                "AMOUNT": order.get("pay_price").cloned().unwrap_or(serde_json::json!(0)),
                "QRCODETYPE": "1",
                "ERRMSG": ""
            }
        }))
    }

    fn ccb_check(&self, order: &Value) -> Result<Value, String> {
        let trade_no = order
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("MOCK_TRADE_NO");
        Ok(serde_json::json!({
            "msg": "",
            "respObj": {
                "RESULT": "Y",
                "TRACEID": format!("MOCK_TRACE_{trade_no}"),
                "ORDERID": trade_no,
                "AMOUNT": order.get("pay_price").cloned().unwrap_or(serde_json::json!(0)),
                "ERRMSG": ""
            }
        }))
    }

    fn reject(&self, _data: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({
            "return_CODE": "000000",
            "return_MSG": "成功"
        }))
    }

    fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mock_ccb_pay_returns_success() {
        let svc = MockCcbService;
        let result = svc
            .ccb_pay(&json!({
                "bank_card": "105011773995373",
                "bank_account": "090378126",
                "trade_no": "TEST_CCB_001",
                "auth_code": "123456789012345678",
                "pay_price": 100.50,
                "order_no": "ORDER_001"
            }))
            .unwrap();
        assert_eq!(result["respObj"]["RESULT"], "Y");
        assert_eq!(result["respObj"]["TRACEID"], "MOCK_TRACE_TEST_CCB_001");
    }

    #[test]
    fn test_mock_ccb_check_returns_success() {
        let svc = MockCcbService;
        let result = svc
            .ccb_check(&json!({
                "trade_no": "TEST_CCB_002",
                "qry_time": 1,
                "type": "check"
            }))
            .unwrap();
        assert_eq!(result["respObj"]["RESULT"], "Y");
    }

    #[test]
    fn test_mock_ccb_reject_returns_success() {
        let svc = MockCcbService;
        let result = svc
            .reject(&json!({
                "mrchNo": "105011773995373",
                "refundAmt": 50.0,
                "payRecordNo": "TEST_CCB_001",
                "requestSn": "20260722000001"
            }))
            .unwrap();
        assert_eq!(result["return_CODE"], "000000");
    }

    #[test]
    fn test_mock_ccb_add_bill_returns_true() {
        let svc = MockCcbService;
        let result = svc
            .add_bill(&json!({
                "ORDERID": "TEST_CCB_001",
                "TRACEID": "MOCK_TRACE_001",
                "AMOUNT": 100.50
            }))
            .unwrap();
        assert!(result);
    }
}

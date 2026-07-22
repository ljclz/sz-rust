//! FuiouService — 富友支付服务 — 对齐 PHP `addons/operate/service/FuiouService.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `fuiouPay($param)` | [`FuiouService::fuiou_pay`] | 富友扫码支付 |
//! | `fuiouCheck($param)` | [`FuiouService::fuiou_check`] | 富友支付状态查询 |
//! | `reject($param)` | [`FuiouService::reject`] | 富友退款 |
//! | `addBill($respObj)` (static) | [`FuiouService::add_bill`] | 记录富友账单 |
//! | `editBill($respObj)` (static) | [`FuiouService::edit_bill`] | 编辑富友账单 |
//!
//! ## 外部依赖
//!
//! PHP 依赖 `app\common\service\fuiou\Constants` + `Signature` SDK 执行 XML HTTP 请求。
//! Rust 端真实实现待后续 Phase 补全，当前提供 [`MockFuiouService`] 用于测试。
//!
//! ## PHP 源码依据
//!
//! ```php
//! class fuiouService {
//!     public function fuiouPay($param): array {
//!         $buildData = ['version'=>'1', 'ins_cd'=>Constants::$ins_cd, 'mchnt_cd'=>Constants::$mchnt_cd, ...];
//!         $buildData['sign'] = Signature::generateSign($buildData);
//!         $postXml = Constants::buildRequestXml($buildData);
//!         $resultXml = URLdecode(Constants::httpPost($url, $postXml));
//!         $respObj = (array)simplexml_load_string($resultXml);
//!         if (result_code == '000000') { OrderModel::where(...)->update(...); (new PaySuccessService())->onPaySuccess(...); }
//!         return ['msg'=>$respObj['result_msg'], 'respObj'=>$respObj];
//!     }
//! }
//! ```

use serde_json::Value;

/// 富友支付服务 trait — 对齐 PHP `fuiouService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
pub trait FuiouService: Send + Sync {
    /// 富友扫码支付 — 对齐 PHP `fuiouPay($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function fuiouPay($param): array {
    ///     $buildData = ['mchnt_order_no'=>$param['trade_no'], 'order_amt'=>strval($param['pay_price']*100),
    ///                   'auth_code'=>$param['auth_code'], 'order_type'=>$param['bank_type'] ?? '', ...];
    ///     if (result_code == '000000' || result_code == '030010') {
    ///         if (result_code == '000000') { OrderModel::where(...)->update(['epay_id'=>...]); (new PaySuccessService())->onPaySuccess(...); }
    ///         $this->addBill($respObj);
    ///     }
    ///     return ['msg'=>$respObj['result_msg'], 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `param`：订单参数（含 `trade_no`/`pay_price`/`auth_code`/`bank_type`/`customer_name`/`order_no`）
    fn fuiou_pay(&self, param: &Value) -> Result<Value, String>;

    /// 富友支付状态查询 — 对齐 PHP `fuiouCheck($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function fuiouCheck($param): array {
    ///     $buildData = ['mchnt_order_no'=>$param['trade_no'], 'order_type'=>$param['bank_type'] ?? 'WECHAT', ...];
    ///     if (result_code == '000000' && trans_stat == 'SUCCESS' && type in ['pay','check']) {
    ///         (new PaySuccessService())->onPaySuccess(...); $this->editBill($respObj);
    ///     }
    ///     return ['msg'=>$respObj['result_msg'], 'respObj'=>$respObj];
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `param`：查询参数（含 `trade_no`/`bank_type`/`type`/`order_no`）
    fn fuiou_check(&self, param: &Value) -> Result<Value, String>;

    /// 富友退款 — 对齐 PHP `reject($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function reject($param): array {
    ///     $buildData = ['mchnt_order_no'=>$param['mchnt_order_no'], 'refund_order_no'=>$param['refund_order_no'],
    ///                   'total_amt'=>$param['total_amt'], 'refund_amt'=>$param['refund_amt'], ...];
    ///     return (array)simplexml_load_string($resultXml);
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `param`：退款参数（含 `mchnt_order_no`/`refund_order_no`/`total_amt`/`refund_amt`/`order_type`）
    fn reject(&self, param: &Value) -> Result<Value, String>;

    /// 记录富友账单 — 对齐 PHP `addBill($respObj)` (静态方法)
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public static function addBill($respObj): bool {
    ///     $order = OrderModel::where(['trade_no'=>$respObj['ORDERID']])->find();
    ///     if ($order) { (new FuiouBill())->save($data); return true; }
    ///     return false;
    /// }
    /// ```
    fn add_bill(&self, resp_obj: &Value) -> Result<bool, String>;

    /// 编辑富友账单 — 对齐 PHP `editBill($respObj)` (静态方法)
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public static function editBill($respObj): bool {
    ///     $respObj['channel'] = 'food';
    ///     $respObj['reserved_mchnt_order_no'] = $respObj['mchnt_order_no'];
    ///     $model = (new FuiouBill())->where(['reserved_mchnt_order_no'=>...])->find();
    ///     if ($model) { if (result_code == '000000') { $model->save($respObj); } else { return true; } }
    ///     return false;
    /// }
    /// ```
    fn edit_bill(&self, resp_obj: &Value) -> Result<bool, String>;
}

/// Mock 富友支付服务 — 用于单元测试
pub struct MockFuiouService;

impl FuiouService for MockFuiouService {
    fn fuiou_pay(&self, param: &Value) -> Result<Value, String> {
        let trade_no = param
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("MOCK_TRADE_NO");
        Ok(serde_json::json!({
            "msg": "成功",
            "respObj": {
                "result_code": "000000",
                "result_msg": "成功",
                "transaction_id": format!("MOCK_FUIOU_{trade_no}"),
                "mchnt_order_no": trade_no,
                "order_amt": param.get("pay_price").and_then(|v| v.as_f64()).map(|p| (p * 100.0) as i64).unwrap_or(0)
            }
        }))
    }

    fn fuiou_check(&self, param: &Value) -> Result<Value, String> {
        let trade_no = param
            .get("trade_no")
            .and_then(|v| v.as_str())
            .unwrap_or("MOCK_TRADE_NO");
        Ok(serde_json::json!({
            "msg": "成功",
            "respObj": {
                "result_code": "000000",
                "result_msg": "成功",
                "trans_stat": "SUCCESS",
                "transaction_id": format!("MOCK_FUIOU_{trade_no}"),
                "mchnt_order_no": trade_no
            }
        }))
    }

    fn reject(&self, _param: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({
            "result_code": "000000",
            "result_msg": "成功",
            "refund_order_no": "MOCK_REFUND_001"
        }))
    }

    fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }

    fn edit_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mock_fuiou_pay_returns_success() {
        let svc = MockFuiouService;
        let result = svc
            .fuiou_pay(&json!({
                "trade_no": "TEST_FUIOU_001",
                "pay_price": 150.00,
                "auth_code": "123456789012345678",
                "bank_type": "WECHAT",
                "customer_name": "测试商户",
                "order_no": "ORDER_001"
            }))
            .unwrap();
        assert_eq!(result["respObj"]["result_code"], "000000");
        assert_eq!(result["respObj"]["result_msg"], "成功");
        assert_eq!(
            result["respObj"]["transaction_id"],
            "MOCK_FUIOU_TEST_FUIOU_001"
        );
    }

    #[test]
    fn test_mock_fuiou_check_returns_success() {
        let svc = MockFuiouService;
        let result = svc
            .fuiou_check(&json!({
                "trade_no": "TEST_FUIOU_002",
                "bank_type": "WECHAT",
                "type": "check"
            }))
            .unwrap();
        assert_eq!(result["respObj"]["result_code"], "000000");
        assert_eq!(result["respObj"]["trans_stat"], "SUCCESS");
    }

    #[test]
    fn test_mock_fuiou_reject_returns_success() {
        let svc = MockFuiouService;
        let result = svc
            .reject(&json!({
                "mchnt_order_no": "TEST_FUIOU_001",
                "refund_order_no": "REFUND_001",
                "total_amt": "15000",
                "refund_amt": "15000",
                "order_type": "WECHAT"
            }))
            .unwrap();
        assert_eq!(result["result_code"], "000000");
    }

    #[test]
    fn test_mock_fuiou_add_bill_returns_true() {
        let svc = MockFuiouService;
        let result = svc.add_bill(&json!({"ORDERID": "TEST_FUIOU_001"})).unwrap();
        assert!(result);
    }

    #[test]
    fn test_mock_fuiou_edit_bill_returns_true() {
        let svc = MockFuiouService;
        let result = svc
            .edit_bill(&json!({"mchnt_order_no": "TEST_FUIOU_001"}))
            .unwrap();
        assert!(result);
    }
}

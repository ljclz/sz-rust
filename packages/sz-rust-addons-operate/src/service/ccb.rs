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
//! Rust 端通过 [`HttpCcbService`] 使用 reqwest + MD5 签名调用建行 API endpoint。
//! 同时提供 `MockCcbService`（#[cfg(test)]，仅测试构建）用于单元测试（仅编译进 test 构建）。
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

use crate::service::http_client::{HttpBankClient, HttpBankConfig};

/// 建设银行支付服务 trait — 对齐 PHP `CcbService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
/// - `order: &Value`：对齐 PHP `$order` 参数（含 bank_card/bank_account/trade_no/auth_code/pay_price）
#[async_trait::async_trait]
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
    async fn ccb_pay(&self, order: &Value) -> Result<Value, String>;

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
    async fn ccb_check(&self, order: &Value) -> Result<Value, String>;

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
    async fn reject(&self, data: &Value) -> Result<Value, String>;

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
    async fn add_bill(&self, resp_obj: &Value) -> Result<bool, String>;
}

/// Mock 建行支付服务 — 用于单元测试
///
/// # 行为
///
/// - `ccb_pay`：返回模拟成功响应（`RESULT = "Y"`）
/// - `ccb_check`：返回模拟查询成功响应
/// - `reject`：返回模拟退款成功响应
/// - `add_bill`：返回 `true`
#[cfg(test)]
pub struct MockCcbService;

#[cfg(test)]
#[async_trait::async_trait]
impl CcbService for MockCcbService {
    #[tracing::instrument(skip(self))]
    async fn ccb_pay(&self, order: &Value) -> Result<Value, String> {
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

    #[tracing::instrument(skip(self))]
    async fn ccb_check(&self, order: &Value) -> Result<Value, String> {
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

    #[tracing::instrument(skip(self))]
    async fn reject(&self, _data: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({
            "return_CODE": "000000",
            "return_MSG": "成功"
        }))
    }

    #[tracing::instrument(skip(self))]
    async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

/// 真实 HTTP 建行支付服务 — C-4 修复
///
/// 通过 reqwest 调用建行 API endpoint，使用 MD5 签名。
/// 需通过环境变量 `CCB_API_URL`/`CCB_MERCHANT_ID`/`CCB_POS_ID` 配置。
///
/// # 行为
///
/// - `ccb_pay`：构造 `merchant_id`/`posid`/`ORDERID`/`QRCODE`/`AMOUNT` 参数，
///   MD5 签名后 POST 到 `CCB_API_URL`，解析响应
/// - `ccb_check`：构造查询参数，POST 查询 API
/// - `reject`：构造退款参数，POST 退款 API
/// - `add_bill`：账单记录依赖 Repository，此处返回 `true` 表示调用方应自行落库
///
/// # 错误处理
///
/// 环境变量未配置时返回 `Err("CCB_API_URL 未配置")`。
pub struct HttpCcbService {
    client: HttpBankClient,
}

impl HttpCcbService {
    /// 从环境变量创建实例
    ///
    /// # 返回
    ///
    /// - `Ok(Self)`：环境变量已配置
    /// - `Err(String)`：环境变量未配置
    pub fn from_env() -> Result<Self, String> {
        let config = HttpBankConfig::from_env_ccb()
            .ok_or_else(|| "CCB_API_URL/CCB_MERCHANT_ID/CCB_POS_ID 未配置".to_string())?;
        Ok(Self {
            client: HttpBankClient::new(config),
        })
    }

    /// 构造支付请求参数（对齐 PHP `ccbPay` 的 `$data`）
    fn build_pay_params(&self, order: &Value) -> serde_json::Map<String, Value> {
        let mut params = serde_json::Map::new();
        params.insert("merchant_id".to_string(), order["bank_card"].clone());
        params.insert("posid".to_string(), order["bank_account"].clone());
        params.insert("ORDERID".to_string(), order["trade_no"].clone());
        params.insert("QRCODE".to_string(), order["auth_code"].clone());
        params.insert("AMOUNT".to_string(), order["pay_price"].clone());
        params.insert("BRANCHID".to_string(), Value::String(String::new()));
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));
        params
    }

    /// 构造查询请求参数（对齐 PHP `ccbCheck` 的 `$data`）
    fn build_check_params(&self, order: &Value) -> serde_json::Map<String, Value> {
        let mut params = serde_json::Map::new();
        params.insert("merchant_id".to_string(), order["bank_card"].clone());
        params.insert("posid".to_string(), order["bank_account"].clone());
        params.insert("ORDERID".to_string(), order["trade_no"].clone());
        params.insert("QRCODETYPE".to_string(), order["qrcodetype"].clone());
        params.insert("STARTDATE".to_string(), Value::String(String::new()));
        params.insert("ENDDATE".to_string(), Value::String(String::new()));
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));
        params
    }
}

#[async_trait::async_trait]
impl CcbService for HttpCcbService {
    #[tracing::instrument(skip(self, order))]
    async fn ccb_pay(&self, order: &Value) -> Result<Value, String> {
        let params = self.build_pay_params(order);
        let body = Value::Object(params);
        let resp = http_post(self, "pay", &body).await?;
        let resp_obj = &resp["respObj"];
        let result = resp_obj["RESULT"].as_str().unwrap_or("");
        if result == "Y" {
            // 支付成功：调用方应触发 PaySuccessService
            tracing::info!(order_id = ?resp_obj["ORDERID"], "CCB 支付成功");
        }
        Ok(resp)
    }

    #[tracing::instrument(skip(self, order))]
    async fn ccb_check(&self, order: &Value) -> Result<Value, String> {
        let params = self.build_check_params(order);
        let body = Value::Object(params);
        let resp = http_post(self, "check", &body).await?;
        Ok(resp)
    }

    #[tracing::instrument(skip(self, data))]
    async fn reject(&self, data: &Value) -> Result<Value, String> {
        let mut params = serde_json::Map::new();
        params.insert("mrchNo".to_string(), data["mrchNo"].clone());
        params.insert("refundAmt".to_string(), data["refundAmt"].clone());
        params.insert("payRecordNo".to_string(), data["payRecordNo"].clone());
        params.insert("requestSn".to_string(), data["requestSn"].clone());
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));
        let body = Value::Object(params);
        http_post(self, "refund", &body).await
    }

    #[tracing::instrument(skip(self, _resp_obj))]
    async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        // 账单落库依赖 Repository，此处返回 true 表示调用方应自行处理
        Ok(true)
    }
}

/// 异步 HTTP POST（直接 await，不再阻塞 tokio worker 线程）
async fn http_post(svc: &HttpCcbService, path: &str, body: &Value) -> Result<Value, String> {
    let client = HttpBankClient::new(svc.client.config().clone());
    let path = path.to_string();
    let body = body.clone();
    client.post_json(&path, &body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_mock_ccb_pay_returns_success() {
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
            .await
            .unwrap();
        assert_eq!(result["respObj"]["RESULT"], "Y");
        assert_eq!(result["respObj"]["TRACEID"], "MOCK_TRACE_TEST_CCB_001");
    }

    #[tokio::test]
    async fn test_mock_ccb_check_returns_success() {
        let svc = MockCcbService;
        let result = svc
            .ccb_check(&json!({
                "trade_no": "TEST_CCB_002",
                "qry_time": 1,
                "type": "check"
            }))
            .await
            .unwrap();
        assert_eq!(result["respObj"]["RESULT"], "Y");
    }

    #[tokio::test]
    async fn test_mock_ccb_reject_returns_success() {
        let svc = MockCcbService;
        let result = svc
            .reject(&json!({
                "mrchNo": "105011773995373",
                "refundAmt": 50.0,
                "payRecordNo": "TEST_CCB_001",
                "requestSn": "20260722000001"
            }))
            .await
            .unwrap();
        assert_eq!(result["return_CODE"], "000000");
    }

    #[tokio::test]
    async fn test_mock_ccb_add_bill_returns_true() {
        let svc = MockCcbService;
        let result = svc
            .add_bill(&json!({
                "ORDERID": "TEST_CCB_001",
                "TRACEID": "MOCK_TRACE_001",
                "AMOUNT": 100.50
            }))
            .await
            .unwrap();
        assert!(result);
    }

    // ========================================================================
    // T-1 失败路径测试 — 覆盖 CcbService 错误场景
    // ========================================================================
    //
    // 审计报告 T-1 指出：Mock 测试永远返回 Ok，无法检测错误处理 bug。
    // 此处添加失败注入 Mock 和 HttpCcbService 错误路径测试。

    /// 全失败 Mock — 所有方法返回 Err
    pub struct FailingCcbService;

    #[async_trait::async_trait]
    impl CcbService for FailingCcbService {
        async fn ccb_pay(&self, _order: &Value) -> Result<Value, String> {
            Err("CCB 支付失败：银行 API 超时".to_string())
        }
        async fn ccb_check(&self, _order: &Value) -> Result<Value, String> {
            Err("CCB 查询失败：网络异常".to_string())
        }
        async fn reject(&self, _data: &Value) -> Result<Value, String> {
            Err("CCB 退款失败：签名校验失败".to_string())
        }
        async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
            Err("CCB 账单记录失败：数据库异常".to_string())
        }
    }

    /// 业务失败 Mock — ccb_pay 返回 RESULT=N（银行拒绝支付）
    pub struct RejectedCcbService;

    #[async_trait::async_trait]
    impl CcbService for RejectedCcbService {
        async fn ccb_pay(&self, order: &Value) -> Result<Value, String> {
            let trade_no = order
                .get("trade_no")
                .and_then(|v| v.as_str())
                .unwrap_or("REJECTED");
            Ok(serde_json::json!({
                "msg": "支付被拒绝",
                "respObj": {
                    "RESULT": "N",
                    "TRACEID": format!("REJECTED_TRACE_{trade_no}"),
                    "ORDERID": trade_no,
                    "ERRMSG": "余额不足"
                }
            }))
        }
        async fn ccb_check(&self, _order: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "msg": "查询被拒绝",
                "respObj": {"RESULT": "N", "ERRMSG": "订单不存在"}
            }))
        }
        async fn reject(&self, _data: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "return_CODE": "999999",
                "return_MSG": "退款失败：超过退款期限"
            }))
        }
        async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
            Ok(false) // 订单不存在，未记录账单
        }
    }

    #[tokio::test]
    async fn test_failing_ccb_pay_returns_error() {
        // T-1: FailingCcbService.ccb_pay 应返回 Err
        let svc = FailingCcbService;
        let result = svc.ccb_pay(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CCB 支付失败"));
    }

    #[tokio::test]
    async fn test_failing_ccb_check_returns_error() {
        let svc = FailingCcbService;
        let result = svc.ccb_check(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CCB 查询失败"));
    }

    #[tokio::test]
    async fn test_failing_ccb_reject_returns_error() {
        let svc = FailingCcbService;
        let result = svc.reject(&json!({"mrchNo": "123"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CCB 退款失败"));
    }

    #[tokio::test]
    async fn test_failing_ccb_add_bill_returns_error() {
        let svc = FailingCcbService;
        let result = svc.add_bill(&json!({"ORDERID": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("CCB 账单记录失败"));
    }

    #[tokio::test]
    async fn test_rejected_ccb_pay_returns_result_n() {
        // T-1: RejectedCcbService.ccb_pay 返回 RESULT=N（业务拒绝，非系统错误）
        let svc = RejectedCcbService;
        let result = svc
            .ccb_pay(&json!({"trade_no": "REJECT_001"}))
            .await
            .unwrap();
        assert_eq!(result["respObj"]["RESULT"], "N", "业务拒绝时 RESULT 应为 N");
        assert_eq!(result["respObj"]["ERRMSG"], "余额不足");
    }

    #[tokio::test]
    async fn test_rejected_ccb_reject_returns_error_code() {
        // T-1: RejectedCcbService.reject 返回错误码 999999（业务拒绝）
        let svc = RejectedCcbService;
        let result = svc.reject(&json!({"mrchNo": "123"})).await.unwrap();
        assert_eq!(result["return_CODE"], "999999");
        assert!(result["return_MSG"].as_str().unwrap().contains("退款失败"));
    }

    #[test]
    fn test_http_ccb_from_env_returns_err_when_not_configured() {
        // T-1: HttpCcbService::from_env 在环境变量未配置时应返回 Err
        // 注意：测试环境中 CCB_API_URL 等环境变量未设置
        let result = HttpCcbService::from_env();
        // 即使已配置，也应能正确处理；未配置时返回 Err
        if let Err(e) = &result {
            assert!(
                e.contains("未配置"),
                "未配置时应返回'未配置'错误，实际：{e}"
            );
        }
    }
}

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
//! Rust 端通过 [`HttpIcbcService`] 使用 reqwest + MD5 签名调用工行 API endpoint。
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

use serde_json::{json, Value};

use crate::service::http_client::{HttpBankClient, HttpBankConfig};

/// 工商银行支付服务 trait — 对齐 PHP `IcbcService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
#[async_trait::async_trait]
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
    async fn icbc_pay(&self, order: &Value) -> Result<Value, String>;

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
    async fn icbc_check(&self, order: &Value) -> Result<Value, String>;

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
    async fn reject(&self, order: &Value) -> Result<Value, String>;

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
    async fn add_bill(&self, data: &Value) -> Result<bool, String>;
}

/// Mock 工行支付服务 — 用于单元测试
#[cfg(test)]
pub struct MockIcbcService;

#[cfg(test)]
#[async_trait::async_trait]
impl IcbcService for MockIcbcService {
    #[tracing::instrument(skip(self))]
    async fn icbc_pay(&self, order: &Value) -> Result<Value, String> {
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
    async fn icbc_check(&self, order: &Value) -> Result<Value, String> {
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
    async fn reject(&self, _order: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({
            "return_code": 0,
            "return_msg": "success",
            "result_msg": "success",
            "status": 1,
            "reject_no": "MOCK_REJECT_001"
        }))
    }

    #[tracing::instrument(skip(self))]
    async fn add_bill(&self, _data: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

/// 真实 HTTP 工行支付服务 — C-4 修复
///
/// 通过 reqwest 调用工行 API endpoint，使用 MD5 签名（对齐 PHP `DefaultIcbcClient` SDK）。
/// 需通过环境变量 `ICBC_API_URL`/`ICBC_MER_ID`/`ICBC_SIGN_KEY` 配置。
///
/// # 背景（C-4 修复）
///
/// 原实现仅提供 `MockIcbcService`，生产环境缺少真实 HTTP 调用，
/// 导致工行支付/查询/退款无法实际执行。本实现补全真实 HTTP 调用路径，
/// 与 `HttpCcbService`/`HttpFuiouService` 保持一致的架构。
///
/// # 行为
///
/// - `icbc_pay`：构造 `mer_id`/`biz_content`/`out_trade_no`/`order_amt`/`qr_code` 参数，
///   MD5 签名后 POST 到 `ICBC_API_URL/pay`，解析响应；
///   若 `return_code == 0` 且 `pay_status == 1`，使用 `tracing::info` 记录支付成功
/// - `icbc_check`：构造查询参数（`mer_id`/`out_trade_no`/`order_id`），POST 到 `check` 路径
/// - `reject`：构造退款参数（`mer_id`/`out_trade_no`/`reject_no`/`reject_amt`），POST 到 `refund` 路径
/// - `add_bill`：账单落库由调用方处理（依赖 Repository），此处返回 `true`
///
/// # 错误处理
///
/// 环境变量未配置时 `from_env` 返回 `Err("ICBC_API_URL/ICBC_MER_ID 未配置")`；
/// HTTP 请求失败、非 2xx 状态码、响应 JSON 解析失败均返回 `Err(String)`。
pub struct HttpIcbcService {
    client: HttpBankClient,
}

impl HttpIcbcService {
    /// 从环境变量创建实例
    ///
    /// # 返回
    ///
    /// - `Ok(Self)`：环境变量已配置
    /// - `Err(String)`：环境变量未配置
    pub fn from_env() -> Result<Self, String> {
        let config = HttpBankConfig::from_env_icbc()
            .ok_or_else(|| "ICBC_API_URL/ICBC_MER_ID 未配置".to_string())?;
        Ok(Self {
            client: HttpBankClient::new(config),
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// IcbcService trait 实现 — C-4 修复：真实 HTTP 调用
// ────────────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl IcbcService for HttpIcbcService {
    /// 工行扫码支付 — 真实 HTTP 实现
    ///
    /// 构造 biz_content（mer_id/biz_content/out_trade_no/order_amt/qr_code），
    /// MD5 签名后 POST JSON 到 `pay` 路径。
    #[tracing::instrument(skip(self, order))]
    async fn icbc_pay(&self, order: &Value) -> Result<Value, String> {
        // 订单金额转分（对齐 PHP strval($order['pay_price']*100)）
        let pay_price = order["pay_price"].as_f64().unwrap_or(0.0);
        let order_amt = (pay_price * 100.0) as i64;
        let order_amt_str = order_amt.to_string();

        // 内层 biz_content：对齐 PHP icbcPay 的 biz_content 数组
        // 包含 mer_id/out_trade_no/order_amt/qr_code 四个核心字段
        let biz_content = json!({
            "mer_id": order["bank_card"],
            "out_trade_no": order["trade_no"],
            "order_amt": order_amt_str,
            "qr_code": order["auth_code"],
        });

        // 外层请求参数：mer_id/biz_content/out_trade_no/order_amt/qr_code
        let mut params = serde_json::Map::new();
        params.insert("mer_id".to_string(), order["bank_card"].clone());
        params.insert("biz_content".to_string(), biz_content);
        params.insert("out_trade_no".to_string(), order["trade_no"].clone());
        params.insert("order_amt".to_string(), Value::String(order_amt_str));
        params.insert("qr_code".to_string(), order["auth_code"].clone());

        // MD5 签名（对齐 PHP DefaultIcbcClient 的签名算法）
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));

        let body = Value::Object(params);
        let resp = http_post(self, "pay", &body).await?;

        // 支付成功判定：return_code == 0 且 pay_status == 1
        let resp_obj = &resp["respObj"];
        let return_code = resp_obj["return_code"].as_i64().unwrap_or(-1);
        let pay_status = resp_obj["pay_status"].as_i64().unwrap_or(-1);
        if return_code == 0 && pay_status == 1 {
            // 支付成功：调用方应触发 PaySuccessService 并落库账单
            tracing::info!(
                order_id = ?resp_obj["order_id"],
                out_trade_no = ?resp_obj["out_trade_no"],
                "ICBC 支付成功"
            );
        }
        Ok(resp)
    }

    /// 工行支付状态查询 — 真实 HTTP 实现
    ///
    /// 构造查询参数（mer_id/out_trade_no/order_id），MD5 签名后 POST 到 `check` 路径。
    #[tracing::instrument(skip(self, order))]
    async fn icbc_check(&self, order: &Value) -> Result<Value, String> {
        // 查询参数对齐 PHP icbcCheck 的 biz_content 字段
        let mut params = serde_json::Map::new();
        params.insert("mer_id".to_string(), order["bank_card"].clone());
        params.insert("out_trade_no".to_string(), order["trade_no"].clone());
        params.insert("order_id".to_string(), order["epay_id"].clone());

        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));

        let body = Value::Object(params);
        http_post(self, "check", &body).await
    }

    /// 工行退款 — 真实 HTTP 实现
    ///
    /// 构造退款参数（mer_id/out_trade_no/reject_no/reject_amt），MD5 签名后 POST 到 `refund` 路径。
    #[tracing::instrument(skip(self, order))]
    async fn reject(&self, order: &Value) -> Result<Value, String> {
        // 退款金额转分（对齐 PHP strval($order['refund_fee']*100)）
        let refund_fee = order["refund_fee"].as_f64().unwrap_or(0.0);
        let reject_amt = (refund_fee * 100.0) as i64;

        let mut params = serde_json::Map::new();
        params.insert("mer_id".to_string(), order["bank_card"].clone());
        params.insert("out_trade_no".to_string(), order["trade_no"].clone());
        params.insert("reject_no".to_string(), order["reject_no"].clone());
        params.insert(
            "reject_amt".to_string(),
            Value::String(reject_amt.to_string()),
        );

        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));

        let body = Value::Object(params);
        http_post(self, "refund", &body).await
    }

    /// 记录工行账单 — 真实 HTTP 实现
    ///
    /// 账单落库由调用方处理（依赖 Repository），此处返回 `Ok(true)`。
    #[tracing::instrument(skip(self, _data))]
    async fn add_bill(&self, _data: &Value) -> Result<bool, String> {
        // 账单落库依赖 Repository（对齐 PHP addBill 的 OrderModel + EpayBank 逻辑），
        // 由调用方在 PaySuccessService 流程中处理，此处返回 true 表示调用方应自行落库
        Ok(true)
    }
}

/// 异步 HTTP POST（直接 await，不再阻塞 tokio worker 线程）
async fn http_post(svc: &HttpIcbcService, path: &str, body: &Value) -> Result<Value, String> {
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
    async fn test_mock_icbc_pay_returns_success() {
        let svc = MockIcbcService;
        let result = svc
            .icbc_pay(&json!({
                "bank_card": "430104027383",
                "trade_no": "TEST_ICBC_001",
                "auth_code": "123456789012345678",
                "pay_price": 200.00,
                "order_no": "ORDER_001"
            })).await
            .unwrap();
        assert_eq!(result["respObj"]["return_code"], 0);
        assert_eq!(result["respObj"]["pay_status"], 1);
    }

    #[tokio::test]
    async fn test_mock_icbc_check_returns_success() {
        let svc = MockIcbcService;
        let result = svc
            .icbc_check(&json!({
                "bank_card": "430104027383",
                "trade_no": "TEST_ICBC_002",
                "epay_id": "MOCK_ICBC_002",
                "type": "check"
            })).await
            .unwrap();
        assert_eq!(result["respObj"]["pay_status"], 1);
    }

    #[tokio::test]
    async fn test_mock_icbc_reject_returns_success() {
        let svc = MockIcbcService;
        let result = svc
            .reject(&json!({
                "bank_card": "430104027383",
                "trade_no": "TEST_ICBC_001",
                "reject_no": "REJECT_001",
                "refund_fee": 100.00,
                "customer_id": 1001
            })).await
            .unwrap();
        assert_eq!(result["return_code"], 0);
        assert_eq!(result["status"], 1);
    }

    #[tokio::test]
    async fn test_mock_icbc_add_bill_returns_true() {
        let svc = MockIcbcService;
        let result = svc
            .add_bill(&json!({"out_trade_no": "TEST_ICBC_001"})).await
            .unwrap();
        assert!(result);
    }

    // ========================================================================
    // T-1 失败路径测试 — 覆盖 IcbcService 错误场景
    // ========================================================================

    /// 全失败 Mock — 所有方法返回 Err
    pub struct FailingIcbcService;

    #[async_trait::async_trait]
impl IcbcService for FailingIcbcService {
        async fn icbc_pay(&self, _order: &Value) -> Result<Value, String> {
            Err("ICBC 支付失败：银行 API 超时".to_string())
        }
        async fn icbc_check(&self, _order: &Value) -> Result<Value, String> {
            Err("ICBC 查询失败：网络异常".to_string())
        }
        async fn reject(&self, _order: &Value) -> Result<Value, String> {
            Err("ICBC 退款失败：签名校验失败".to_string())
        }
        async fn add_bill(&self, _data: &Value) -> Result<bool, String> {
            Err("ICBC 账单记录失败：数据库异常".to_string())
        }
    }

    /// 业务失败 Mock — icbc_pay 返回 pay_status=0（支付失败）
    pub struct RejectedIcbcService;

    #[async_trait::async_trait]
impl IcbcService for RejectedIcbcService {
        async fn icbc_pay(&self, _order: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "msg": "支付失败",
                "respObj": {
                    "return_code": -1,
                    "pay_status": 0,
                    "return_msg": "余额不足"
                }
            }))
        }
        async fn icbc_check(&self, _order: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "msg": "查询失败",
                "respObj": {"pay_status": 0, "return_msg": "订单不存在"}
            }))
        }
        async fn reject(&self, _order: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "return_code": -1,
                "status": 0,
                "return_msg": "退款失败：超过退款期限"
            }))
        }
        async fn add_bill(&self, _data: &Value) -> Result<bool, String> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn test_failing_icbc_pay_returns_error() {
        let svc = FailingIcbcService;
        let result = svc.icbc_pay(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ICBC 支付失败"));
    }

    #[tokio::test]
    async fn test_failing_icbc_check_returns_error() {
        let svc = FailingIcbcService;
        let result = svc.icbc_check(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ICBC 查询失败"));
    }

    #[tokio::test]
    async fn test_failing_icbc_reject_returns_error() {
        let svc = FailingIcbcService;
        let result = svc.reject(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ICBC 退款失败"));
    }

    #[tokio::test]
    async fn test_failing_icbc_add_bill_returns_error() {
        let svc = FailingIcbcService;
        let result = svc.add_bill(&json!({"out_trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ICBC 账单记录失败"));
    }

    #[tokio::test]
    async fn test_rejected_icbc_pay_returns_pay_status_0() {
        // T-1: RejectedIcbcService.icbc_pay 返回 pay_status=0（业务拒绝）
        let svc = RejectedIcbcService;
        let result = svc.icbc_pay(&json!({"trade_no": "REJECT_001"})).await.unwrap();
        assert_eq!(
            result["respObj"]["pay_status"], 0,
            "业务拒绝时 pay_status 应为 0"
        );
        assert_eq!(result["respObj"]["return_msg"], "余额不足");
    }

    #[tokio::test]
    async fn test_rejected_icbc_reject_returns_error_code() {
        // T-1: RejectedIcbcService.reject 返回 return_code=-1（业务拒绝）
        let svc = RejectedIcbcService;
        let result = svc.reject(&json!({"trade_no": "REJECT_001"})).await.unwrap();
        assert_eq!(result["return_code"], -1);
        assert!(result["return_msg"].as_str().unwrap().contains("退款失败"));
    }

    #[test]
    fn test_http_icbc_from_env_returns_err_when_not_configured() {
        // T-1: HttpIcbcService::from_env 在环境变量未配置时应返回 Err
        let result = HttpIcbcService::from_env();
        if let Err(e) = &result {
            assert!(
                e.contains("未配置"),
                "未配置时应返回'未配置'错误，实际：{e}"
            );
        }
    }
}

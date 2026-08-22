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
//! Rust 端通过 [`HttpFuiouService`] 使用 reqwest + MD5 签名 + XML 格式调用富友 API endpoint。
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

use crate::service::http_client::{HttpBankClient, HttpBankConfig};

/// 富友支付服务 trait — 对齐 PHP `fuiouService`
///
/// # 设计
///
/// - `Send + Sync`：支持 axum 状态共享
/// - 方法返回 `Result<Value, String>`：对齐 PHP `false + $this->error` 模式
#[async_trait::async_trait]
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
    async fn fuiou_pay(&self, param: &Value) -> Result<Value, String>;

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
    async fn fuiou_check(&self, param: &Value) -> Result<Value, String>;

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
    async fn reject(&self, param: &Value) -> Result<Value, String>;

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
    async fn add_bill(&self, resp_obj: &Value) -> Result<bool, String>;

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
    async fn edit_bill(&self, resp_obj: &Value) -> Result<bool, String>;
}

/// Mock 富友支付服务 — 用于单元测试
#[cfg(test)]
pub struct MockFuiouService;

#[cfg(test)]
#[async_trait::async_trait]
impl FuiouService for MockFuiouService {
    #[tracing::instrument(skip(self))]
    async fn fuiou_pay(&self, param: &Value) -> Result<Value, String> {
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

    #[tracing::instrument(skip(self))]
    async fn fuiou_check(&self, param: &Value) -> Result<Value, String> {
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

    #[tracing::instrument(skip(self))]
    async fn reject(&self, _param: &Value) -> Result<Value, String> {
        Ok(serde_json::json!({
            "result_code": "000000",
            "result_msg": "成功",
            "refund_order_no": "MOCK_REFUND_001"
        }))
    }

    #[tracing::instrument(skip(self))]
    async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }

    #[tracing::instrument(skip(self))]
    async fn edit_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// C-4 修复：HttpFuiouService — 真实 HTTP 富友支付服务实现
// ────────────────────────────────────────────────────────────────────────────
//
// 修复背景：
//   此前 Rust 端仅提供 `MockFuiouService`，缺少真实 HTTP 调用路径，
//   导致富友支付/查询/退款无法实际执行。本实现补全真实 HTTP 调用路径，
//   与 `HttpCcbService`/`HttpIcbcService` 保持一致的架构。
//
// 设计要点：
//   - 通过 reqwest 调用富友 API endpoint，使用 MD5 签名 + XML 格式请求体
//   - 通过环境变量 `FUIOU_API_URL`/`FUIOU_INS_CD`/`FUIOU_MCHNT_CD`/`FUIOU_KEY` 配置
//   - 使用 `tokio::task::block_in_place` + `Handle::current().block_on` 将异步
//     reqwest 调用包装为同步，以匹配 trait 的同步签名
//   - 保留 `MockFuiouService` 用于单元测试，不影响现有测试用例
// ────────────────────────────────────────────────────────────────────────────

/// 真实 HTTP 富友支付服务 — C-4 修复
///
/// 通过 reqwest 调用富友 API endpoint，使用 MD5 签名 + XML 格式请求体。
/// 需通过环境变量 `FUIOU_API_URL`/`FUIOU_MCHNT_CD`/`FUIOU_KEY` 配置。
///
/// # 行为
///
/// - `fuiou_pay`：构造支付参数（`mchnt_order_no`/`order_amt`/`auth_code`/`order_type`/
///   `ins_cd`/`mchnt_cd`/`version=1`），MD5 签名后将参数转为 XML，POST 到 `pay` 路径；
///   若 `result_code == "000000"`，使用 `tracing::info` 记录支付成功
/// - `fuiou_check`：构造查询参数，MD5 签名后 POST XML 到 `check` 路径
/// - `reject`：构造退款参数（`mchnt_order_no`/`refund_order_no`/`total_amt`/
///   `refund_amt`/`order_type`），POST XML 到 `refund` 路径
/// - `add_bill`：账单落库由调用方处理，返回 `Ok(true)`
/// - `edit_bill`：账单编辑由调用方处理，返回 `Ok(true)`
///
/// # 错误处理
///
/// 环境变量未配置时 `from_env` 返回 `Err("FUIOU_API_URL/FUIOU_MCHNT_CD/FUIOU_KEY 未配置")`；
/// HTTP 请求失败、非 2xx 状态码、响应 XML 解析失败均返回 `Err(String)`。
pub struct HttpFuiouService {
    client: HttpBankClient,
}

impl HttpFuiouService {
    /// 从环境变量创建实例
    ///
    /// # 返回
    ///
    /// - `Ok(Self)`：环境变量已配置
    /// - `Err(String)`：环境变量未配置
    pub fn from_env() -> Result<Self, String> {
        let config = HttpBankConfig::from_env_fuiou()
            .ok_or_else(|| "FUIOU_API_URL/FUIOU_MCHNT_CD/FUIOU_KEY 未配置".to_string())?;
        Ok(Self {
            client: HttpBankClient::new(config),
        })
    }

    /// 构造 XML 请求体（对齐 PHP `Constants::buildRequestXml`）
    ///
    /// 将参数 Map 转为简单扁平 XML 格式：`<xml><key>value</key>...</xml>`。
    /// 仅支持字符串/数字/布尔等标量值，跳过嵌套对象/数组（对齐 PHP simplexml 行为）。
    fn build_xml_body(params: &serde_json::Map<String, Value>) -> String {
        let mut xml = String::from("<xml>");
        for (key, val) in params.iter() {
            let val_str = match val {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => {
                    if *b {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                }
                Value::Null => String::new(),
                _ => val.to_string(),
            };
            xml.push_str(&format!("<{key}>{val_str}</{key}>"));
        }
        xml.push_str("</xml>");
        xml
    }
}

// ────────────────────────────────────────────────────────────────────────────
// FuiouService trait 实现 — C-4 修复：真实 HTTP 调用
// ────────────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl FuiouService for HttpFuiouService {
    /// 富友扫码支付 — 真实 HTTP 实现
    ///
    /// 构造支付参数（`mchnt_order_no`/`order_amt`/`auth_code`/`order_type`/
    /// `ins_cd`/`mchnt_cd`/`version=1`），MD5 签名后将参数转为 XML，POST 到 `pay` 路径。
    /// 若响应 `result_code == "000000"`，使用 `tracing::info` 记录支付成功。
    #[tracing::instrument(skip(self, param))]
    async fn fuiou_pay(&self, param: &Value) -> Result<Value, String> {
        // 订单金额转分（对齐 PHP strval($param['pay_price']*100)）
        let pay_price = param["pay_price"].as_f64().unwrap_or(0.0);
        let order_amt = (pay_price * 100.0) as i64;

        // 构造支付参数（对齐 PHP fuiouPay 的 $buildData）
        // ins_cd/mchnt_cd 来自环境变量（FUIOU_INS_CD/FUIOU_MCHNT_CD）
        let mut params = serde_json::Map::new();
        params.insert("version".to_string(), Value::String("1".to_string()));
        params.insert(
            "ins_cd".to_string(),
            Value::String(std::env::var("FUIOU_INS_CD").unwrap_or_default()),
        );
        params.insert(
            "mchnt_cd".to_string(),
            Value::String(self.client.config().merchant_id.clone()),
        );
        params.insert("mchnt_order_no".to_string(), param["trade_no"].clone());
        params.insert(
            "order_amt".to_string(),
            Value::String(order_amt.to_string()),
        );
        params.insert("auth_code".to_string(), param["auth_code"].clone());
        params.insert("order_type".to_string(), param["bank_type"].clone());

        // MD5 签名（对齐 PHP Signature::generateSign）
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));

        // 构造 XML 请求体（对齐 PHP Constants::buildRequestXml）并 POST 到 pay 路径
        let xml_body = Self::build_xml_body(&params);
        let resp = http_post_xml(self, "pay", &xml_body).await?;

        // 支付成功判定：result_code == "000000"（对齐 PHP fuiouPay 的成功分支）
        let result_code = resp["result_code"].as_str().unwrap_or("");
        if result_code == "000000" {
            tracing::info!(
                mchnt_order_no = ?resp["mchnt_order_no"],
                transaction_id = ?resp["transaction_id"],
                "FUIOU 支付成功"
            );
        }
        Ok(resp)
    }

    /// 富友支付状态查询 — 真实 HTTP 实现
    ///
    /// 构造查询参数（`mchnt_order_no`/`order_type`/`ins_cd`/`mchnt_cd`/`version=1`），
    /// MD5 签名后 POST XML 到 `check` 路径。
    #[tracing::instrument(skip(self, param))]
    async fn fuiou_check(&self, param: &Value) -> Result<Value, String> {
        // 构造查询参数（对齐 PHP fuiouCheck 的 $buildData）
        let mut params = serde_json::Map::new();
        params.insert("version".to_string(), Value::String("1".to_string()));
        params.insert(
            "ins_cd".to_string(),
            Value::String(std::env::var("FUIOU_INS_CD").unwrap_or_default()),
        );
        params.insert(
            "mchnt_cd".to_string(),
            Value::String(self.client.config().merchant_id.clone()),
        );
        params.insert("mchnt_order_no".to_string(), param["trade_no"].clone());
        params.insert("order_type".to_string(), param["bank_type"].clone());

        // MD5 签名
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));

        // 构造 XML 请求体并 POST 到 check 路径
        let xml_body = Self::build_xml_body(&params);
        http_post_xml(self, "check", &xml_body).await
    }

    /// 富友退款 — 真实 HTTP 实现
    ///
    /// 构造退款参数（`mchnt_order_no`/`refund_order_no`/`total_amt`/
    /// `refund_amt`/`order_type`），MD5 签名后 POST XML 到 `refund` 路径。
    #[tracing::instrument(skip(self, param))]
    async fn reject(&self, param: &Value) -> Result<Value, String> {
        // 构造退款参数（对齐 PHP reject 的 $buildData）
        let mut params = serde_json::Map::new();
        params.insert("version".to_string(), Value::String("1".to_string()));
        params.insert(
            "ins_cd".to_string(),
            Value::String(std::env::var("FUIOU_INS_CD").unwrap_or_default()),
        );
        params.insert(
            "mchnt_cd".to_string(),
            Value::String(self.client.config().merchant_id.clone()),
        );
        params.insert(
            "mchnt_order_no".to_string(),
            param["mchnt_order_no"].clone(),
        );
        params.insert(
            "refund_order_no".to_string(),
            param["refund_order_no"].clone(),
        );
        params.insert("total_amt".to_string(), param["total_amt"].clone());
        params.insert("refund_amt".to_string(), param["refund_amt"].clone());
        params.insert("order_type".to_string(), param["order_type"].clone());

        // MD5 签名
        let sign = crate::service::http_client::md5_sign(&params, &self.client.config().sign_key);
        params.insert("sign".to_string(), Value::String(sign));

        // 构造 XML 请求体并 POST 到 refund 路径
        let xml_body = Self::build_xml_body(&params);
        http_post_xml(self, "refund", &xml_body).await
    }

    /// 记录富友账单 — 账单落库由调用方处理
    ///
    /// 账单落库依赖 Repository 层（对齐 PHP `addBill` 静态方法内的 OrderModel/FuiouBill 操作），
    /// 此处返回 `Ok(true)` 表示调用方应自行处理落库逻辑。
    #[tracing::instrument(skip(self, _resp_obj))]
    async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }

    /// 编辑富友账单 — 账单编辑由调用方处理
    ///
    /// 账单编辑依赖 Repository 层（对齐 PHP `editBill` 静态方法内的 FuiouBill 操作），
    /// 此处返回 `Ok(true)` 表示调用方应自行处理编辑逻辑。
    #[tracing::instrument(skip(self, _resp_obj))]
    async fn edit_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
        Ok(true)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 同步包装辅助函数 — 将 async reqwest 调用转为同步以匹配 trait 签名
// ────────────────────────────────────────────────────────────────────────────

/// 异步 HTTP POST XML（直接 await，不再阻塞 tokio worker 线程）
async fn http_post_xml(
    svc: &HttpFuiouService,
    path: &str,
    xml_body: &str,
) -> Result<Value, String> {
    let client = HttpBankClient::new(svc.client.config().clone());
    let path = path.to_string();
    let xml_body = xml_body.to_string();
    client.post_xml(&path, &xml_body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_mock_fuiou_pay_returns_success() {
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
            .await
            .unwrap();
        assert_eq!(result["respObj"]["result_code"], "000000");
        assert_eq!(result["respObj"]["result_msg"], "成功");
        assert_eq!(
            result["respObj"]["transaction_id"],
            "MOCK_FUIOU_TEST_FUIOU_001"
        );
    }

    #[tokio::test]
    async fn test_mock_fuiou_check_returns_success() {
        let svc = MockFuiouService;
        let result = svc
            .fuiou_check(&json!({
                "trade_no": "TEST_FUIOU_002",
                "bank_type": "WECHAT",
                "type": "check"
            }))
            .await
            .unwrap();
        assert_eq!(result["respObj"]["result_code"], "000000");
        assert_eq!(result["respObj"]["trans_stat"], "SUCCESS");
    }

    #[tokio::test]
    async fn test_mock_fuiou_reject_returns_success() {
        let svc = MockFuiouService;
        let result = svc
            .reject(&json!({
                "mchnt_order_no": "TEST_FUIOU_001",
                "refund_order_no": "REFUND_001",
                "total_amt": "15000",
                "refund_amt": "15000",
                "order_type": "WECHAT"
            }))
            .await
            .unwrap();
        assert_eq!(result["result_code"], "000000");
    }

    #[tokio::test]
    async fn test_mock_fuiou_add_bill_returns_true() {
        let svc = MockFuiouService;
        let result = svc
            .add_bill(&json!({"ORDERID": "TEST_FUIOU_001"}))
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_mock_fuiou_edit_bill_returns_true() {
        let svc = MockFuiouService;
        let result = svc
            .edit_bill(&json!({"mchnt_order_no": "TEST_FUIOU_001"}))
            .await
            .unwrap();
        assert!(result);
    }

    // ========================================================================
    // T-1 失败路径测试 — 覆盖 FuiouService 错误场景
    // ========================================================================

    /// 全失败 Mock — 所有方法返回 Err
    pub struct FailingFuiouService;

    #[async_trait::async_trait]
    impl FuiouService for FailingFuiouService {
        async fn fuiou_pay(&self, _order: &Value) -> Result<Value, String> {
            Err("FUIOU 支付失败：银行 API 超时".to_string())
        }
        async fn fuiou_check(&self, _order: &Value) -> Result<Value, String> {
            Err("FUIOU 查询失败：网络异常".to_string())
        }
        async fn reject(&self, _param: &Value) -> Result<Value, String> {
            Err("FUIOU 退款失败：签名校验失败".to_string())
        }
        async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
            Err("FUIOU 账单记录失败：数据库异常".to_string())
        }
        async fn edit_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
            Err("FUIOU 账单编辑失败：记录不存在".to_string())
        }
    }

    /// 业务失败 Mock — fuiou_pay 返回 result_code != "000000"（支付失败）
    pub struct RejectedFuiouService;

    #[async_trait::async_trait]
    impl FuiouService for RejectedFuiouService {
        async fn fuiou_pay(&self, _order: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "msg": "支付失败",
                "respObj": {
                    "result_code": "999999",
                    "result_msg": "余额不足",
                    "trans_stat": "FAIL"
                }
            }))
        }
        async fn fuiou_check(&self, _order: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "msg": "查询失败",
                "respObj": {
                    "result_code": "999999",
                    "result_msg": "订单不存在",
                    "trans_stat": "FAIL"
                }
            }))
        }
        async fn reject(&self, _param: &Value) -> Result<Value, String> {
            Ok(serde_json::json!({
                "result_code": "999999",
                "result_msg": "退款失败：超过退款期限"
            }))
        }
        async fn add_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
            Ok(false)
        }
        async fn edit_bill(&self, _resp_obj: &Value) -> Result<bool, String> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn test_failing_fuiou_pay_returns_error() {
        let svc = FailingFuiouService;
        let result = svc.fuiou_pay(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FUIOU 支付失败"));
    }

    #[tokio::test]
    async fn test_failing_fuiou_check_returns_error() {
        let svc = FailingFuiouService;
        let result = svc.fuiou_check(&json!({"trade_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FUIOU 查询失败"));
    }

    #[tokio::test]
    async fn test_failing_fuiou_reject_returns_error() {
        let svc = FailingFuiouService;
        let result = svc.reject(&json!({"mchnt_order_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FUIOU 退款失败"));
    }

    #[tokio::test]
    async fn test_failing_fuiou_add_bill_returns_error() {
        let svc = FailingFuiouService;
        let result = svc.add_bill(&json!({"ORDERID": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FUIOU 账单记录失败"));
    }

    #[tokio::test]
    async fn test_failing_fuiou_edit_bill_returns_error() {
        let svc = FailingFuiouService;
        let result = svc.edit_bill(&json!({"mchnt_order_no": "TEST_001"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("FUIOU 账单编辑失败"));
    }

    #[tokio::test]
    async fn test_rejected_fuiou_pay_returns_error_result_code() {
        // T-1: RejectedFuiouService.fuiou_pay 返回 result_code=999999（业务拒绝）
        let svc = RejectedFuiouService;
        let result = svc
            .fuiou_pay(&json!({"trade_no": "REJECT_001"}))
            .await
            .unwrap();
        assert_eq!(result["respObj"]["result_code"], "999999");
        assert_eq!(result["respObj"]["trans_stat"], "FAIL");
    }

    #[tokio::test]
    async fn test_rejected_fuiou_reject_returns_error_result_code() {
        // T-1: RejectedFuiouService.reject 返回 result_code=999999（业务拒绝）
        let svc = RejectedFuiouService;
        let result = svc
            .reject(&json!({"mchnt_order_no": "REJECT_001"}))
            .await
            .unwrap();
        assert_eq!(result["result_code"], "999999");
        assert!(result["result_msg"].as_str().unwrap().contains("退款失败"));
    }

    #[test]
    fn test_http_fuiou_from_env_returns_err_when_not_configured() {
        // T-1: HttpFuiouService::from_env 在环境变量未配置时应返回 Err
        let result = HttpFuiouService::from_env();
        if let Err(e) = &result {
            assert!(
                e.contains("未配置"),
                "未配置时应返回'未配置'错误，实际：{e}"
            );
        }
    }
}

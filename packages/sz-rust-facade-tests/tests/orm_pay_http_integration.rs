//! P9-FACADE-02：orm + pay + http 集成测试
//!
//! 验证 `sz-rust-orm-facade`（参数化查询 / SQL 宏）、
//! `sz-rust-pay-facade`（订单 / 支付 / 脱敏）、
//! `sz-rust-http-facade`（API 响应组装）的业务链路协作。

use serde_json::json;

use sz_rust_http_facade::request::{parse_query, url_decode};
use sz_rust_http_facade::{ApiResponse, BaseException};
use sz_rust_orm_facade::{sql_string, DbType, SelectQuery, Value};
use sz_rust_pay_facade::{MemoryPayProvider, PayConfig, PayOrder, PayPlatform, PayProvider};

/// 完整链路：参数化查询订单 → 支付 → API 响应
#[test]
fn query_pay_respond_chain() {
    // 1. orm-facade：参数化构建订单查询（WHERE 全部走绑定参数，防注入）
    let built = SelectQuery::new()
        .column("id")
        .column("out_trade_no")
        .column("total_amount")
        .from("orders")
        .where_eq("user_id", 1001i64.into())
        .where_in("status", vec![Value::I32(0), Value::I32(1)])
        .order_by("id", false)
        .limit(20)
        .build_with_params(DbType::MySQL);
    assert!(built.sql.contains('?'), "P9-FACADE-02: SQL 必须使用占位符");
    assert_eq!(
        built.params.len(),
        3,
        "P9-FACADE-02: 绑定参数数 = 2 个 where + 1 个 limit"
    );

    // 2. pay-facade：构建订单并支付（MemoryPayProvider 纯内存）
    let provider = MemoryPayProvider::new();
    let order = PayOrder::new()
        .out_trade_no("202608030001")
        .total_amount(8800)
        .subject("鲜视达商品")
        .notify_url("https://example.com/notify");
    assert!(order.validate().is_ok(), "P9-FACADE-02: 订单字段应通过校验");
    let result = provider.pay(order).unwrap();
    assert_eq!(result.total_amount, 8800, "P9-FACADE-02: 支付金额应一致");

    // 3. http-facade：包装为统一 API 响应
    let resp = ApiResponse::success(
        json!({
            "trade_no": result.trade_no,
            "out_trade_no": result.out_trade_no,
            "total_amount": result.total_amount,
        }),
        "支付成功",
    );
    let body = resp.to_json_string();
    assert!(
        body.contains("\"code\":1"),
        "P9-FACADE-02: 成功响应 code 应为 1"
    );

    // 4. http-facade：错误路径（未登录）
    let err = BaseException::not_login("请先登录");
    assert_eq!(
        err.to_json()["code"],
        json!(-1),
        "P9-FACADE-02: 未登录错误码应为 -1"
    );
}

/// SQL 编译期校验宏 + 参数化绑定（规则：WHERE 必须参数化）
#[test]
fn sql_macro_and_parameterized_binding() {
    // 编译期 SQL 语法 + 注入模式校验
    let sql = sql_string!("SELECT id, name FROM users WHERE id = ?"; params: 1);
    assert!(sql.contains("users"));

    // 显式列投影（规则：禁止 SELECT *）
    let projected = SelectQuery::new()
        .columns(&["id", "name", "email"])
        .from("users")
        .where_eq("status", "active".into())
        .build_with_params(DbType::PostgreSQL);
    assert!(!projected.sql.contains('*'), "P9-FACADE-02: 禁止 SELECT *");
    assert_eq!(projected.params.len(), 1);
}

/// 支付配置脱敏跨 facade 验证（pay 配置 → 日志/调试输出不可泄漏私钥）
#[test]
fn pay_config_redaction_visible_across_facade() {
    let config = PayConfig::new(PayPlatform::WechatPay, "wx_app_001")
        .with_merchant_private_key("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcw")
        .with_platform_public_key("MIIBcWjANBgkqhkiG9w0BAQEFAAOCAQ8A")
        .with_notify_url("https://example.com/notify")
        .with_sandbox(true);
    assert!(config.validate().is_ok());

    let debug_str = format!("{config:?}");
    assert!(
        !debug_str.contains("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcw"),
        "P9-FACADE-02: Debug 输出不得泄漏商户私钥"
    );
    assert!(
        debug_str.contains("<redacted>"),
        "P9-FACADE-02: 私钥应显示 <redacted>"
    );
}

/// http-facade 请求工具（query 解析 / url 解码）—— 供上游业务直接使用
#[test]
fn http_request_helpers_usable_by_business() {
    let q = parse_query("page=2&keyword=%E9%B2%9C%E8%A7%86%E8%BE%BE&order=desc");
    assert_eq!(q.get("page").unwrap(), "2");
    assert_eq!(q.get("keyword").unwrap(), "鲜视达");
    assert_eq!(q.get("order").unwrap(), "desc");

    assert_eq!(url_decode("%E5%B7%A5%E5%85%B7%E7%AE%B1"), "工具箱");
}

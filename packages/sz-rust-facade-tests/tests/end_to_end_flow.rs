//! P9-FACADE-04：端到端业务流集成测试
//!
//! 模拟完整业务链路：微信回调验证 → JWT 鉴权 → 会话 → 下单 → 缓存 →
//! 事件通知 → 支付 → API 响应。验证 7 个 facade 的跨 crate 协作无断层。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use sha1::Digest;

use sz_rust_auth_facade::wechat::{
    MemoryWechatHttpTransport, WechatAppType, WechatConfig, WechatSdk,
};
use sz_rust_cache_facade::{Cache, MemoryCacheDriver};
use sz_rust_http_facade::{ApiResponse, BaseException};
use sz_rust_orm_facade::jwt::{JwtClaims, JwtEncoder};
use sz_rust_orm_facade::{DbType, Query};
use sz_rust_pay_facade::{MemoryPayProvider, PayOrder, PayProvider};
use sz_rust_state_facade::event::{ClosureListener, EventDispatcher};
use sz_rust_state_facade::session::{MemorySessionStore, Session};

/// 微信回调 → JWT → 会话 → 下单 → 缓存 → 事件 → 支付 → 响应 全链路
#[test]
fn end_to_end_payment_flow() {
    // ── 1. auth-facade：微信服务器回调签名验证 ──
    let config = WechatConfig::new(WechatAppType::MiniProgram, "wx_mini_001", "secret_002")
        .with_token("token_2026");
    let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));
    let mut hasher = sha1::Sha1::new();
    hasher.update(b"token_2026");
    hasher.update(b"1754150400");
    hasher.update(b"nonce_9");
    let sig = hex::encode(hasher.finalize());
    assert!(sdk.verify_signature(&sig, "1754150400", "nonce_9", "token_2026"));

    // ── 2. orm-facade::jwt：业务 JWT 签发与解码 ──
    let encoder = JwtEncoder::new("biz-secret");
    let jwt = encoder
        .encode(&JwtClaims::new("user_9527", 4_000_000_000).with_user_id(9527))
        .unwrap();
    let claims = encoder.decode(&jwt).unwrap();
    let user_id = claims.sub.clone();

    // ── 3. state-facade：会话建立（JWT 主体 → Session） ──
    let session = Session::new(format!("sess_{user_id}"), MemorySessionStore::new());
    session.set("uid", json!(9527));
    session.set("role", json!("vip"));
    assert_eq!(session.get("uid").unwrap().as_i64().unwrap(), 9527);

    // ── 4. pay-facade：下单 ──
    let provider = MemoryPayProvider::new();
    let order = PayOrder::new()
        .out_trade_no("E2E202608030001")
        .total_amount(19900)
        .subject("鲜视达超值礼包")
        .notify_url("https://example.com/notify");
    assert!(order.validate().is_ok());
    let pay_result = provider.pay(order).unwrap();
    assert_eq!(pay_result.total_amount, 19900);

    // ── 5. cache-facade：订单状态缓存 ──
    let cache = Cache::new();
    cache.register_default(MemoryCacheDriver::new());
    cache
        .set("order:status:E2E202608030001", "paid", None)
        .unwrap();
    let status: String = cache.get("order:status:E2E202608030001").unwrap().unwrap();
    assert_eq!(status, "paid");

    // ── 6. state-facade：支付事件通知（事件总线） ──
    let dispatcher = EventDispatcher::new();
    let total = Arc::new(AtomicI64::new(0));
    let total_clone = total.clone();
    dispatcher.listen(
        "PaymentSuccess",
        Arc::new(ClosureListener::new(move |params: &Value| {
            let amount = params["amount"].as_i64().unwrap_or(0);
            total_clone.fetch_add(amount, Ordering::SeqCst);
            Ok(Value::Null)
        })),
        false,
    );
    dispatcher
        .trigger(
            "PaymentSuccess",
            &json!({"order_no": "E2E202608030001", "amount": 19900}),
            false,
        )
        .unwrap();
    assert_eq!(
        total.load(Ordering::SeqCst),
        19900,
        "P9-FACADE-04: 事件应累计支付金额"
    );

    // ── 7. orm-facade：对账查询（参数化绑定） ──
    let built = Query::select()
        .columns(&["id", "out_trade_no", "total_amount"])
        .from("orders")
        .where_eq("out_trade_no", "E2E202608030001".into())
        .where_eq("status", "paid".into())
        .build_with_params(DbType::MySQL);
    assert!(built.sql.contains('?'));
    assert_eq!(built.params.len(), 2);

    // ── 8. http-facade：响应组装（成功 + 失败路径） ──
    let ok_resp = ApiResponse::success(
        json!({
            "trade_no": pay_result.trade_no,
            "paid_amount": pay_result.total_amount,
            "order_status": status,
        }),
        "支付成功",
    );
    let body = ok_resp.to_json_string();
    assert!(body.contains("\"code\":1") && body.contains("19900"));

    let err_resp = BaseException::not_login("登录态已过期");
    assert_eq!(err_resp.to_json()["code"], json!(-1));

    // 会话残留校验（链路完整性收尾）
    assert_eq!(session.get("role").unwrap(), json!("vip"));
}

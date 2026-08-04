//! 向后兼容路径测试（P9-COMPAT 系列）
//!
//! sz-rust-core 拆包为 7 个 facade 后，保留 `sz_rust_core::<module>::*` 旧路径。
//! 本测试验证所有旧路径仍可编译并正确转发到 facade 实现（规则 14/18 支撑）。
//!
//! 覆盖路径：
//! - `sz_rust_core::cache` → sz-rust-cache-facade
//! - `sz_rust_core::state` → sz-rust-state-facade
//! - `sz_rust_core::infra` → sz-rust-infra-facade
//! - `sz_rust_core::auth` → sz-rust-auth-facade
//! - `sz_rust_core::pay` → sz-rust-pay-facade
//! - `sz_rust_core::http` → sz-rust-http-facade
//! - `sz_rust_core::orm` → sz-rust-orm-facade
//! - 顶层类型再导出（ApiResponse / Query / Cache 等）

// ── 编译期验证：旧路径可用（use 即编译期断言） ──

use sz_rust_core::cache::Cache as LegacyCache;
use sz_rust_core::cache::MemoryCacheDriver as LegacyMemoryCacheDriver;
use sz_rust_core::http::ApiResponse as LegacyApiResponse;
use sz_rust_core::http::BaseException as LegacyBaseException;
use sz_rust_core::infra::static_files::is_path_safe as legacy_is_path_safe;
use sz_rust_core::orm::{DbType as LegacyDbType, Query as LegacyQuery, Value as LegacyValue};
use sz_rust_core::pay::{
    PayConfig as LegacyPayConfig, PayOrder as LegacyPayOrder, PayPlatform as LegacyPayPlatform,
};
use sz_rust_core::response::ApiResponse as OldResponsePathApiResponse;
use sz_rust_core::state::session::{
    MemorySessionStore as LegacyMemorySessionStore, Session as LegacySession,
};

/// P9-COMPAT-01：cache 旧路径（sz_rust_core::cache）与 facade 实现等价
#[test]
fn legacy_cache_path_forwards_to_facade() {
    let cache = LegacyCache::new();
    cache.register_default(LegacyMemoryCacheDriver::new());
    cache.set("legacy:key", "v1", None).unwrap();
    let v: String = cache.get("legacy:key").unwrap().unwrap();
    assert_eq!(v, "v1", "P9-COMPAT-01: 旧路径 cache 应正常工作");
}

/// P9-COMPAT-02：http / response 旧路径（sz_rust_core::http + sz_rust_core::response）
#[test]
fn legacy_http_and_response_paths() {
    let resp = LegacyApiResponse::success(serde_json::json!({"ok": true}), "兼容测试");
    assert!(resp.to_json_string().contains("\"code\":1"));

    // response 模块下的 ApiResponse（拆包前旧路径）— 与 http 路径等价
    let resp2 = OldResponsePathApiResponse::success(serde_json::json!({"ok": true}), "兼容测试");
    assert_eq!(resp2.to_json_string(), resp.to_json_string());

    let err = LegacyBaseException::not_login("请先登录");
    assert_eq!(err.to_json()["code"], serde_json::json!(-1));
}

/// P9-COMPAT-03：orm 旧路径（sz_rust_core::orm）参数化查询
#[test]
fn legacy_orm_path_builds_parameterized_query() {
    let built = LegacyQuery::select()
        .columns(&["id", "name"])
        .from("users")
        .where_eq("id", 1i64.into())
        .build_with_params(LegacyDbType::MySQL);
    assert!(built.sql.contains('?'));
    assert_eq!(built.params.len(), 1);
    // Value 类型可正常构造
    let v: LegacyValue = 42i64.into();
    assert!(matches!(v, LegacyValue::I64(42)));
}

/// P9-COMPAT-04：pay 旧路径（sz_rust_core::pay）+ 脱敏
#[test]
fn legacy_pay_path_with_redaction() {
    let config = LegacyPayConfig::new(LegacyPayPlatform::Alipay, "app_legacy_001")
        .with_merchant_private_key("MIILEGACYKEY1234567890")
        .with_platform_public_key("MIIPUBLICKEY0987654321")
        .with_notify_url("https://example.com/notify");
    assert!(config.validate().is_ok());

    let debug_str = format!("{config:?}");
    assert!(
        !debug_str.contains("MIILEGACYKEY1234567890"),
        "P9-COMPAT-04: 旧路径配置同样脱敏"
    );
    assert!(debug_str.contains("<redacted>"));

    let order = LegacyPayOrder::new()
        .out_trade_no("COMPAT20260803")
        .total_amount(100)
        .subject("兼容测试");
    assert!(order.validate().is_ok());
}

/// P9-COMPAT-05：state 旧路径（sz_rust_core::state）会话 + 路径安全转发
#[test]
fn legacy_state_and_infra_paths() {
    let session = LegacySession::new("sess_compat", LegacyMemorySessionStore::new());
    session.set("uid", serde_json::json!(7));
    assert_eq!(session.get("uid").unwrap().as_i64().unwrap(), 7);

    // infra 旧路径（sz_rust_core::infra）→ is_path_safe 行为一致
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("a.txt");
    std::fs::write(&file, b"x").unwrap();
    assert!(legacy_is_path_safe(&file, root.path()));
    assert!(!legacy_is_path_safe(
        &root.path().join("..").join("b.txt"),
        root.path()
    ));
}

/// P9-COMPAT-06：新旧路径类型同一性（同类型才能互传，编译期强校验）
#[test]
fn legacy_and_facade_types_are_identical() {
    // 若旧路径类型 != facade 类型，以下赋值无法编译
    let legacy_cache = LegacyCache::new();
    let facade_cache: sz_rust_cache_facade::Cache = legacy_cache;
    let _ = facade_cache;

    let legacy_query = LegacyQuery::select();
    let facade_query: sz_rust_orm_facade::SelectQuery = legacy_query;
    let _ = facade_query;

    let legacy_api = LegacyApiResponse::success(serde_json::json!({}), "");
    let facade_api: sz_rust_http_facade::ApiResponse = legacy_api;
    let _ = facade_api;
}

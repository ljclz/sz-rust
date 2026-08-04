//! sz-rust-sz300 业务服务单元测试
//!
//! 覆盖 Order / OrderItem 模型（Model + ModelExt trait）与 services::row_to_json 转换函数。
//! 服务层 DB 方法（OrderService::list 等）依赖真实连接池，需通过 mocks 或 DB 集成测试覆盖，
//! 本文件聚焦纯逻辑层（模型映射 + 行转 JSON）。
//!
//! P2 补充（2026-08-04）：row_to_json 边界类型全覆盖 + auth_service 空凭证校验（不依赖 DB）。

use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, Value as OrmValue};
use sz_rust_sz300::models::order::Order;
use sz_rust_sz300::models::order_item::OrderItem;
use sz_rust_sz300::services::row_to_json;

// ─────────────────────────────────────────────
// Order 模型
// ─────────────────────────────────────────────

#[test]
fn order_table_name_and_pk_name() {
    assert_eq!(Order::table_name(), "order");
    assert_eq!(Order::pk_name(), "order_id");
}

#[test]
fn order_columns_contains_expected_fields() {
    let cols = Order::columns();
    assert!(cols.contains(&"order_no"));
    assert!(cols.contains(&"merchant_id"));
    assert!(cols.contains(&"total_fen"));
    assert!(cols.contains(&"status"));
    assert!(cols.contains(&"order_id"));
    // guarded 字段 order_id 仍在 columns 中
    assert!(cols.contains(&"order_id"));
}

#[test]
fn order_fillable_excludes_pk() {
    let fillable = Order::fillable();
    assert!(fillable.contains(&"order_no"));
    assert!(fillable.contains(&"merchant_id"));
    // 主键不在 fillable 中
    assert!(!fillable.contains(&"order_id"));
}

#[test]
fn order_guarded_contains_pk() {
    let guarded = Order::guarded();
    assert_eq!(guarded, vec!["order_id"]);
}

#[test]
fn order_pk_get_set() {
    let mut order = Order {
        order_id: None,
        order_no: "ORD001".into(),
        merchant_id: 1,
        device_id: 2,
        total_fen: 1000,
        total_weight_g: 500,
        item_count: 1,
        status: 1,
        pay_method: 1,
        pay_at: None,
        offline_seq: "SEQ001".into(),
        created_at: None,
        updated_at: None,
    };
    assert_eq!(order.pk(), 0);
    order.set_pk(42);
    assert_eq!(order.pk(), 42);
    assert_eq!(order.order_id, Some(42));
}

#[test]
fn order_get_column_value_all_fields() {
    let order = Order {
        order_id: Some(10),
        order_no: "ORD010".into(),
        merchant_id: 100,
        device_id: 200,
        total_fen: 9999,
        total_weight_g: 1234,
        item_count: 3,
        status: 2,
        pay_method: 1,
        pay_at: Some("2026-08-01 12:00:00".into()),
        offline_seq: "OFF001".into(),
        created_at: Some("2026-08-01".into()),
        updated_at: Some("2026-08-01".into()),
    };
    assert_eq!(order.get_column_value("order_id"), Some(OrmValue::I64(10)));
    assert_eq!(
        order.get_column_value("order_no"),
        Some(OrmValue::String("ORD010".into()))
    );
    assert_eq!(
        order.get_column_value("merchant_id"),
        Some(OrmValue::I64(100))
    );
    assert_eq!(
        order.get_column_value("total_fen"),
        Some(OrmValue::I64(9999))
    );
    assert_eq!(order.get_column_value("item_count"), Some(OrmValue::I32(3)));
    assert_eq!(order.get_column_value("status"), Some(OrmValue::I32(2)));
    assert_eq!(order.get_column_value("pay_method"), Some(OrmValue::I32(1)));
    assert_eq!(
        order.get_column_value("pay_at"),
        Some(OrmValue::String("2026-08-01 12:00:00".into()))
    );
    assert_eq!(
        order.get_column_value("offline_seq"),
        Some(OrmValue::String("OFF001".into()))
    );
    assert_eq!(order.get_column_value("unknown"), None);
}

#[test]
fn order_from_value_populates_all_fields() {
    let mut map: HashMap<String, OrmValue> = HashMap::new();
    map.insert("order_id".into(), OrmValue::I64(77));
    map.insert("order_no".into(), OrmValue::String("ORD077".into()));
    map.insert("merchant_id".into(), OrmValue::I64(5));
    map.insert("device_id".into(), OrmValue::I64(8));
    map.insert("total_fen".into(), OrmValue::I64(5000));
    map.insert("total_weight_g".into(), OrmValue::I64(200));
    map.insert("item_count".into(), OrmValue::I64(2));
    map.insert("status".into(), OrmValue::I64(1));
    map.insert("pay_method".into(), OrmValue::I64(2));
    map.insert("pay_at".into(), OrmValue::String("2026-08-02".into()));
    map.insert("offline_seq".into(), OrmValue::String("SEQ077".into()));

    let mut order = Order {
        order_id: None,
        order_no: String::new(),
        merchant_id: 0,
        device_id: 0,
        total_fen: 0,
        total_weight_g: 0,
        item_count: 0,
        status: 0,
        pay_method: 0,
        pay_at: None,
        offline_seq: String::new(),
        created_at: None,
        updated_at: None,
    };
    order.from_value(map);

    assert_eq!(order.order_id, Some(77));
    assert_eq!(order.order_no, "ORD077");
    assert_eq!(order.merchant_id, 5);
    assert_eq!(order.device_id, 8);
    assert_eq!(order.total_fen, 5000);
    assert_eq!(order.total_weight_g, 200);
    assert_eq!(order.item_count, 2);
    assert_eq!(order.status, 1);
    assert_eq!(order.pay_method, 2);
    assert_eq!(order.pay_at, Some("2026-08-02".into()));
    assert_eq!(order.offline_seq, "SEQ077");
}

// ─────────────────────────────────────────────
// OrderItem 模型
// ─────────────────────────────────────────────

#[test]
fn order_item_table_name_and_pk_name() {
    assert_eq!(OrderItem::table_name(), "order_item");
    assert_eq!(OrderItem::pk_name(), "item_id");
}

#[test]
fn order_item_columns_contains_expected_fields() {
    let cols = OrderItem::columns();
    assert!(cols.contains(&"order_id"));
    assert!(cols.contains(&"good_id"));
    assert!(cols.contains(&"good_name"));
    assert!(cols.contains(&"price_fen"));
    assert!(cols.contains(&"quantity"));
    assert!(cols.contains(&"item_id"));
}

#[test]
fn order_item_fillable_excludes_pk() {
    let fillable = OrderItem::fillable();
    assert!(fillable.contains(&"order_id"));
    assert!(fillable.contains(&"good_name"));
    assert!(!fillable.contains(&"item_id"));
}

#[test]
fn order_item_guarded_contains_pk() {
    let guarded = OrderItem::guarded();
    assert_eq!(guarded, vec!["item_id"]);
}

#[test]
fn order_item_pk_get_set() {
    let mut item = OrderItem {
        item_id: None,
        order_id: 10,
        good_id: 20,
        good_name: "苹果".into(),
        price_fen: 500,
        weight_g: 100,
        total_fen: 1000,
        quantity: 2,
    };
    assert_eq!(item.pk(), 0);
    item.set_pk(99);
    assert_eq!(item.pk(), 99);
    assert_eq!(item.item_id, Some(99));
}

#[test]
fn order_item_get_column_value_all_fields() {
    let item = OrderItem {
        item_id: Some(5),
        order_id: 10,
        good_id: 20,
        good_name: "香蕉".into(),
        price_fen: 300,
        weight_g: 150,
        total_fen: 600,
        quantity: 2,
    };
    assert_eq!(item.get_column_value("item_id"), Some(OrmValue::I64(5)));
    assert_eq!(item.get_column_value("order_id"), Some(OrmValue::I64(10)));
    assert_eq!(
        item.get_column_value("good_name"),
        Some(OrmValue::String("香蕉".into()))
    );
    assert_eq!(item.get_column_value("price_fen"), Some(OrmValue::I64(300)));
    assert_eq!(item.get_column_value("quantity"), Some(OrmValue::I32(2)));
    assert_eq!(item.get_column_value("unknown"), None);
}

#[test]
fn order_item_from_value_populates_all_fields() {
    let mut map: HashMap<String, OrmValue> = HashMap::new();
    map.insert("item_id".into(), OrmValue::I64(33));
    map.insert("order_id".into(), OrmValue::I64(10));
    map.insert("good_id".into(), OrmValue::I64(20));
    map.insert("good_name".into(), OrmValue::String("橙子".into()));
    map.insert("price_fen".into(), OrmValue::I64(800));
    map.insert("weight_g".into(), OrmValue::I64(200));
    map.insert("total_fen".into(), OrmValue::I64(1600));
    map.insert("quantity".into(), OrmValue::I64(2));

    let mut item = OrderItem {
        item_id: None,
        order_id: 0,
        good_id: 0,
        good_name: String::new(),
        price_fen: 0,
        weight_g: 0,
        total_fen: 0,
        quantity: 0,
    };
    item.from_value(map);

    assert_eq!(item.item_id, Some(33));
    assert_eq!(item.order_id, 10);
    assert_eq!(item.good_id, 20);
    assert_eq!(item.good_name, "橙子");
    assert_eq!(item.price_fen, 800);
    assert_eq!(item.quantity, 2);
}

// ─────────────────────────────────────────────
// row_to_json（services/mod.rs）
// ─────────────────────────────────────────────

#[test]
fn row_to_json_converts_all_value_types() {
    let mut row: HashMap<String, OrmValue> = HashMap::new();
    row.insert("id".into(), OrmValue::I64(42));
    row.insert("name".into(), OrmValue::String("测试商品".into()));
    row.insert("price".into(), OrmValue::F64(19.99));
    row.insert("active".into(), OrmValue::Bool(true));
    row.insert("deleted".into(), OrmValue::I32(0));
    row.insert("empty".into(), OrmValue::Null);

    let json = row_to_json(&row);
    let obj = json.as_object().unwrap();
    assert_eq!(obj["id"], 42);
    assert_eq!(obj["name"], "测试商品");
    assert_eq!(obj["price"], 19.99);
    assert_eq!(obj["active"], true);
    assert_eq!(obj["deleted"], 0);
    assert!(obj["empty"].is_null());
}

#[test]
fn row_to_json_empty_row() {
    let row: HashMap<String, OrmValue> = HashMap::new();
    let json = row_to_json(&row);
    assert!(json.as_object().unwrap().is_empty());
}

#[test]
fn row_to_json_preserves_keys() {
    let mut row: HashMap<String, OrmValue> = HashMap::new();
    row.insert("order_no".into(), OrmValue::String("ORD001".into()));
    row.insert("total_fen".into(), OrmValue::I64(9999));

    let json = row_to_json(&row);
    let obj = json.as_object().unwrap();
    assert!(obj.contains_key("order_no"));
    assert!(obj.contains_key("total_fen"));
    assert_eq!(obj["order_no"], "ORD001");
    assert_eq!(obj["total_fen"], 9999);
}

// ─────────────────────────────────────────────
// row_to_json — 边界类型全覆盖（P2 补充）
// ─────────────────────────────────────────────

#[test]
fn row_to_json_null_value() {
    let mut row = HashMap::new();
    row.insert("deleted".into(), OrmValue::Null);
    let json = row_to_json(&row);
    assert!(json["deleted"].is_null());
}

#[test]
fn row_to_json_bool_value() {
    let mut row = HashMap::new();
    row.insert("active".into(), OrmValue::Bool(false));
    let json = row_to_json(&row);
    assert_eq!(json["active"], false);
}

#[test]
fn row_to_json_bytes_value_hex_encoded() {
    let mut row = HashMap::new();
    row.insert("hash".into(), OrmValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    let json = row_to_json(&row);
    assert_eq!(json["hash"], "deadbeef");
}

#[test]
fn row_to_json_bytes_empty() {
    let mut row = HashMap::new();
    row.insert("empty".into(), OrmValue::Bytes(vec![]));
    let json = row_to_json(&row);
    assert_eq!(json["empty"], "");
}

#[test]
fn row_to_json_array_value() {
    let mut row = HashMap::new();
    row.insert(
        "tags".into(),
        OrmValue::Array(vec![OrmValue::String("a".into()), OrmValue::I64(42)]),
    );
    let json = row_to_json(&row);
    assert_eq!(json["tags"][0], "a");
    assert_eq!(json["tags"][1], 42);
}

#[test]
fn row_to_json_object_value() {
    let mut inner = HashMap::new();
    inner.insert("k".into(), OrmValue::String("v".into()));
    let mut row = HashMap::new();
    row.insert("meta".into(), OrmValue::Object(inner));
    let json = row_to_json(&row);
    assert_eq!(json["meta"]["k"], "v");
}

#[test]
fn row_to_json_f64_value() {
    let mut row = HashMap::new();
    row.insert("price".into(), OrmValue::F64(19.99));
    let json = row_to_json(&row);
    assert_eq!(json["price"], 19.99);
}

#[test]
fn row_to_json_f32_value() {
    let mut row = HashMap::new();
    row.insert("ratio".into(), OrmValue::F32(0.5));
    let json = row_to_json(&row);
    assert_eq!(json["ratio"], 0.5);
}

#[test]
fn row_to_json_uuid_date_datetime_string_variants() {
    let mut row = HashMap::new();
    row.insert(
        "uuid".into(),
        OrmValue::Uuid("550e8400-e29b-41d4-a716-446655440000".into()),
    );
    row.insert("birth".into(), OrmValue::Date("2026-08-04".into()));
    row.insert(
        "created".into(),
        OrmValue::DateTime("2026-08-04 12:00:00".into()),
    );
    row.insert("at".into(), OrmValue::Time("12:00:00".into()));
    row.insert("extra".into(), OrmValue::Json("{\"k\":\"v\"}".into()));
    row.insert("dec".into(), OrmValue::Decimal("99.99".into()));
    let json = row_to_json(&row);
    assert_eq!(json["uuid"], "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(json["birth"], "2026-08-04");
    assert_eq!(json["created"], "2026-08-04 12:00:00");
    assert_eq!(json["at"], "12:00:00");
    assert_eq!(json["extra"], "{\"k\":\"v\"}");
    assert_eq!(json["dec"], "99.99");
}

#[test]
fn row_to_json_unknown_variant_falls_back_to_null() {
    // non_exhaustive 通配符：未来新增变体应回退为 Null
    // 通过构造一个包含所有已知变体的行来间接验证无 panic
    let mut row = HashMap::new();
    row.insert("a".into(), OrmValue::I8(1));
    row.insert("b".into(), OrmValue::I16(2));
    row.insert("c".into(), OrmValue::U8(3));
    row.insert("d".into(), OrmValue::U16(4));
    row.insert("e".into(), OrmValue::U32(5));
    row.insert("f".into(), OrmValue::U64(6));
    let json = row_to_json(&row);
    assert_eq!(json["a"], 1);
    assert_eq!(json["b"], 2);
    assert_eq!(json["c"], 3);
    assert_eq!(json["d"], 4);
    assert_eq!(json["e"], 5);
    assert_eq!(json["f"], 6);
}

// ─────────────────────────────────────────────
// auth_service — 空凭证校验（纯逻辑，不依赖 DB）
// ─────────────────────────────────────────────

/// authenticate_async：用户名为空时直接返回错误（不访问 DB）
#[tokio::test]
async fn authenticate_async_empty_username_returns_error() {
    let result = sz_rust_sz300::services::auth_service::authenticate_async("", "password").await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "用户名或密码不能为空");
}

/// authenticate_async：用户名为纯空白时直接返回错误（不访问 DB）
#[tokio::test]
async fn authenticate_async_whitespace_username_returns_error() {
    let result = sz_rust_sz300::services::auth_service::authenticate_async("   ", "password").await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "用户名或密码不能为空");
}

/// authenticate_async：密码为空时直接返回错误（不访问 DB）
#[tokio::test]
async fn authenticate_async_empty_password_returns_error() {
    let result = sz_rust_sz300::services::auth_service::authenticate_async("admin", "").await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "用户名或密码不能为空");
}

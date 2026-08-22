//! 服务层模块聚合
//!
//! 2026-07-26：提取公共 `row_to_json` 函数至本模块，消除 product/merchant/order/device
//! 四个 service 中的重复定义（DRY）。
//!
//! 2026-07-30：优化 `row_to_json` 性能 — 引入 `value_to_json_ref` 引用版本，
//! 避免 `v.clone()` 整体克隆 Value。对于 Copy 类型（Bool/I32/I64 等数字）零拷贝，
//! 对于 String 类型仍需 clone（但省去了整体 Value clone 的开销）。

use std::collections::HashMap;

use sz_rust_core::orm::Value;

/// 认证服务模块（对齐 PHP AuthService）
pub mod auth_service;
/// 设备服务模块（封装设备 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod device_service;
/// 文件服务模块（对齐 PHP FileService）
pub mod file_service;
/// 健康检查服务模块（封装 DB 探活逻辑，2026-07-26 新增 — 修复控制器分层违反）
pub mod health_service;
/// 商户服务模块（封装商户 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod merchant_service;
/// MQTT 监听服务模块
pub mod mqtt_listener;
/// MQTT 业务服务模块
pub mod mqtt_service;
/// 订单服务模块（封装订单 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod order_service;
/// 商品服务模块（封装商品 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod product_service;

/// 将 DB 行（`HashMap<String, Value>`）转换为 JSON 对象（供控制器使用）
///
/// ## 参数
///
/// - `row`：DB 行数据（字段名 → 字段值）
///
/// ## 返回
///
/// `serde_json::Value::Object`，字段顺序保持 HashMap 迭代顺序。
///
/// ## 性能优化（2026-07-30）
///
/// 使用 `value_to_json_ref`（引用版本）替代 `value_to_json(v.clone())`，
/// 避免 `v.clone()` 整体克隆 Value 对象。对于数字类型字段（Copy 类型）零拷贝，
/// 对于 String 类型仍需 clone 字符串内容（但省去了 Value enum 包装的开销）。
///
/// ## 重复消除说明
///
/// 2026-07-26 之前，`product_service` / `merchant_service` / `order_service` /
/// `device_service` 各自定义了完全相同的 `row_to_json`，违反 DRY 原则。
/// 现统一提取至本模块，子模块通过 `super::row_to_json` 或 `crate::services::row_to_json` 引用。
pub fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json_ref(v));
    }
    serde_json::Value::Object(obj)
}

/// 将 `&Value` 转换为 `serde_json::Value`（引用版本，避免整体 clone）
///
/// 与 `sz_orm_core::model::value_to_json(v: Value)` 的区别：
/// - 原版接收 `Value`（所有权），调用方需 `v.clone()`
/// - 本版接收 `&Value`，对 Copy 类型（数字/布尔）零拷贝，对 String 类型仅 clone 字符串
///
/// ## 性能对比
///
/// | 值类型 | 原版 clone 次数 | 本版 clone 次数 |
/// |--------|----------------|----------------|
/// | Null/Bool/I32/I64 等数字 | 1（v.clone()） | 0（Copy） |
/// | String/Decimal/Uuid/Date 等 | 1（v.clone()） | 1（s.clone()） |
/// | Bytes | 1（v.clone()） | 1（b.clone()） |
/// | Array | 1 + 递归 | 递归（无整体 clone） |
/// | Object | 1 + 递归 | 递归（无整体 clone） |
fn value_to_json_ref(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::I8(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::I16(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::I32(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::I64(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U8(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U16(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U32(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U64(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::F32(n) => serde_json::Number::from_f64(*n as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::F64(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        // String 类变体：需 clone 字符串内容（无法避免，serde_json 需要 String 所有权）
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Decimal(s) => serde_json::Value::String(s.clone()),
        Value::Uuid(s) => serde_json::Value::String(s.clone()),
        Value::Date(s) => serde_json::Value::String(s.clone()),
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Time(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => serde_json::Value::String(s.clone()),
        // Bytes：转十六进制字符串（对齐 sz-orm-core value_to_json 逻辑）
        Value::Bytes(b) => {
            const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";
            let mut s = String::with_capacity(b.len() * 2);
            for byte in b {
                s.push(HEX_LOWER[(byte >> 4) as usize] as char);
                s.push(HEX_LOWER[(byte & 0x0f) as usize] as char);
            }
            serde_json::Value::String(s)
        }
        // Array：递归转换，无需整体 clone
        Value::Array(arr) => serde_json::Value::Array(arr.iter().map(value_to_json_ref).collect()),
        // Object：递归转换，无需整体 clone
        Value::Object(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                obj.insert(k.clone(), value_to_json_ref(v));
            }
            serde_json::Value::Object(obj)
        }
        // non_exhaustive 通配符：未来新增变体回退为 Null（对齐 sz-orm-core 行为）
        _ => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn row_to_json_empty_map() {
        let row: HashMap<String, Value> = HashMap::new();
        let json = row_to_json(&row);
        assert!(json.is_object());
        assert_eq!(json.as_object().unwrap().len(), 0);
    }

    #[test]
    fn row_to_json_null_value() {
        let mut row = HashMap::new();
        row.insert("k".into(), Value::Null);
        let json = row_to_json(&row);
        assert_eq!(json["k"], serde_json::Value::Null);
    }

    #[test]
    fn row_to_json_bool_value() {
        let mut row = HashMap::new();
        row.insert("flag".into(), Value::Bool(true));
        let json = row_to_json(&row);
        assert_eq!(json["flag"], serde_json::Value::Bool(true));
    }

    #[test]
    fn row_to_json_i32_i64_values() {
        let mut row = HashMap::new();
        row.insert("i32".into(), Value::I32(42));
        row.insert("i64".into(), Value::I64(9_000_000_000));
        row.insert("u32".into(), Value::U32(100));
        row.insert("u64".into(), Value::U64(18_000_000_000));
        let json = row_to_json(&row);
        assert_eq!(json["i32"], 42);
        assert_eq!(json["i64"].as_i64().unwrap(), 9_000_000_000_i64);
        assert_eq!(json["u32"], 100);
        assert_eq!(json["u64"].as_i64().unwrap(), 18_000_000_000_i64);
    }

    #[test]
    fn row_to_json_f64_value() {
        let mut row = HashMap::new();
        row.insert("f".into(), Value::F64(3.5));
        let json = row_to_json(&row);
        assert_eq!(json["f"].as_f64().unwrap(), 3.5);
    }

    #[test]
    fn row_to_json_string_value() {
        let mut row = HashMap::new();
        row.insert("s".into(), Value::String("hello".into()));
        let json = row_to_json(&row);
        assert_eq!(json["s"], "hello");
    }

    #[test]
    fn row_to_json_string_variants() {
        let mut row = HashMap::new();
        row.insert("decimal".into(), Value::Decimal("1.23".into()));
        row.insert("uuid".into(), Value::Uuid("abc".into()));
        row.insert("date".into(), Value::Date("2026-01-01".into()));
        row.insert(
            "datetime".into(),
            Value::DateTime("2026-01-01T00:00:00".into()),
        );
        row.insert("time".into(), Value::Time("00:00:00".into()));
        row.insert("json".into(), Value::Json("{}".into()));
        let json = row_to_json(&row);
        assert_eq!(json["decimal"], "1.23");
        assert_eq!(json["uuid"], "abc");
        assert_eq!(json["date"], "2026-01-01");
    }

    #[test]
    fn row_to_json_bytes_value() {
        let mut row = HashMap::new();
        row.insert("b".into(), Value::Bytes(vec![0xFF, 0x00, 0xAB]));
        let json = row_to_json(&row);
        assert_eq!(json["b"], "ff00ab");
    }

    #[test]
    fn row_to_json_array_value() {
        let mut row = HashMap::new();
        row.insert(
            "arr".into(),
            Value::Array(vec![Value::I32(1), Value::I32(2)]),
        );
        let json = row_to_json(&row);
        assert_eq!(json["arr"], serde_json::json!([1, 2]));
    }

    #[test]
    fn row_to_json_object_value() {
        let mut row = HashMap::new();
        let mut inner = HashMap::new();
        inner.insert("key".into(), Value::String("val".into()));
        row.insert("obj".into(), Value::Object(inner));
        let json = row_to_json(&row);
        assert_eq!(json["obj"]["key"], "val");
    }

    #[test]
    fn row_to_json_multiple_fields() {
        let mut row = HashMap::new();
        row.insert("id".into(), Value::I64(1));
        row.insert("name".into(), Value::String("test".into()));
        row.insert("active".into(), Value::Bool(true));
        let json = row_to_json(&row);
        assert_eq!(json["id"], 1);
        assert_eq!(json["name"], "test");
        assert_eq!(json["active"], true);
        assert_eq!(json.as_object().unwrap().len(), 3);
    }
}

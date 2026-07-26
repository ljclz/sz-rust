//! 控制器公共辅助函数（2026-07-26 新增）
//!
//! 提取控制器间重复的模板代码，减少 CRUD 重复（修复 Brooks-Lint R3 警告）。
//! 当前提供：分页参数解析（page/page_size）。

use serde_json::Value as JsonValue;
use sz_rust_core::orm::Value;
use std::collections::HashMap;

/// 解析分页参数（page/page_size），返回 (page, page_size)
///
/// 统一约定：
/// - page：默认 1，最小 1
/// - page_size：默认值由调用方指定（product=15, device=20 等），clamp 到 [1, 100]
pub fn parse_pagination(data: &JsonValue, default_page_size: i64) -> (i64, i64) {
    let page = data
        .get("page")
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        .max(1);
    let page_size = data
        .get("page_size")
        .and_then(|v| v.as_i64())
        .unwrap_or(default_page_size)
        .clamp(1, 100);
    (page, page_size)
}

/// 按白名单从 data 提取字段，转换为 sz_rust_core::orm::Value 类型映射。
///
/// 用于控制器 update 方法，从请求数据中按白名单提取可更新字段。
/// 转换规则：
/// - Null -> 跳过（不更新该字段）
/// - Bool -> Value::I32(0/1)（对齐 MySQL TINYINT(1) 布尔语义）
/// - Number(i64) -> Value::I64
/// - Number(f64) -> Value::F64（无法转为 i64 的浮点数）
/// - String -> Value::String
/// - 其他类型（Array/Object）-> 跳过
pub fn extract_fields_by_whitelist(data: &JsonValue, allowed_keys: &[&str]) -> HashMap<String, Value> {
    let mut fields: HashMap<String, Value> = HashMap::new();
    for key in allowed_keys {
        if let Some(val) = data.get(*key) {
            let orm_val = match val {
                serde_json::Value::Null => continue,
                serde_json::Value::Bool(b) => Value::I32(if *b { 1 } else { 0 }),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Value::I64(i)
                    } else if let Some(f) = n.as_f64() {
                        Value::F64(f)
                    } else {
                        continue;
                    }
                }
                serde_json::Value::String(s) => Value::String(s.clone()),
                _ => continue,
            };
            fields.insert((*key).to_string(), orm_val);
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_pagination_defaults() {
        let data = json!({});
        let (page, page_size) = parse_pagination(&data, 15);
        assert_eq!(page, 1);
        assert_eq!(page_size, 15);
    }

    #[test]
    fn test_parse_pagination_custom_values() {
        let data = json!({"page": 3, "page_size": 25});
        let (page, page_size) = parse_pagination(&data, 15);
        assert_eq!(page, 3);
        assert_eq!(page_size, 25);
    }

    #[test]
    fn test_parse_pagination_clamp_min() {
        let data = json!({"page": 0, "page_size": 0});
        let (page, page_size) = parse_pagination(&data, 15);
        assert_eq!(page, 1);
        assert_eq!(page_size, 1);
    }

    #[test]
    fn test_parse_pagination_clamp_max() {
        let data = json!({"page": -5, "page_size": 200});
        let (page, page_size) = parse_pagination(&data, 15);
        assert_eq!(page, 1);
        assert_eq!(page_size, 100);
    }

    #[test]
    fn test_parse_pagination_different_defaults() {
        let data = json!({});
        assert_eq!(parse_pagination(&data, 20).1, 20);
        assert_eq!(parse_pagination(&data, 15).1, 15);
        assert_eq!(parse_pagination(&data, 50).1, 50);
    }

    // ===== extract_fields_by_whitelist 测试 =====

    #[test]
    fn test_extract_fields_i64() {
        let data = json!({"price": 100, "stock": 50});
        let fields = extract_fields_by_whitelist(&data, &["price", "stock"]);
        assert_eq!(fields.len(), 2);
        assert!(matches!(fields.get("price"), Some(Value::I64(100))));
        assert!(matches!(fields.get("stock"), Some(Value::I64(50))));
    }

    #[test]
    fn test_extract_fields_string() {
        let data = json!({"name": "apple", "barcode": "123456"});
        let fields = extract_fields_by_whitelist(&data, &["name", "barcode"]);
        assert_eq!(fields.len(), 2);
        if let Some(Value::String(s)) = fields.get("name") {
            assert_eq!(s, "apple");
        } else { panic!("name should be String"); }
    }

    #[test]
    fn test_extract_fields_bool_to_i32() {
        let data = json!({"active": true, "deleted": false});
        let fields = extract_fields_by_whitelist(&data, &["active", "deleted"]);
        assert!(matches!(fields.get("active"), Some(Value::I32(1))));
        assert!(matches!(fields.get("deleted"), Some(Value::I32(0))));
    }

    #[test]
    fn test_extract_fields_null_skipped() {
        let data = json!({"name": null, "price": 100});
        let fields = extract_fields_by_whitelist(&data, &["name", "price"]);
        assert_eq!(fields.len(), 1);
        assert!(!fields.contains_key("name"));
    }

    #[test]
    fn test_extract_fields_whitelist_filter() {
        let data = json!({"name": "apple", "secret": "hidden", "price": 100});
        let fields = extract_fields_by_whitelist(&data, &["name", "price"]);
        assert_eq!(fields.len(), 2);
        assert!(!fields.contains_key("secret"));
    }

    #[test]
    fn test_extract_fields_missing_key_skipped() {
        let data = json!({"name": "apple"});
        let fields = extract_fields_by_whitelist(&data, &["name", "price", "stock"]);
        assert_eq!(fields.len(), 1);
        assert!(fields.contains_key("name"));
    }

    #[test]
    fn test_extract_fields_array_skipped() {
        let data = json!({"items": [1, 2, 3], "name": "apple"});
        let fields = extract_fields_by_whitelist(&data, &["items", "name"]);
        assert_eq!(fields.len(), 1);
        assert!(!fields.contains_key("items"));
    }

    #[test]
    fn test_extract_fields_empty_whitelist() {
        let data = json!({"name": "apple", "price": 100});
        let fields = extract_fields_by_whitelist(&data, &[]);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_extract_fields_f64() {
        let data = json!({"weight": 1.5});
        let fields = extract_fields_by_whitelist(&data, &["weight"]);
        assert_eq!(fields.len(), 1);
        if let Some(Value::F64(f)) = fields.get("weight") {
            assert!((f - 1.5).abs() < 0.001);
        } else { panic!("weight should be F64"); }
    }
}
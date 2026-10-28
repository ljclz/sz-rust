// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 控制器公共辅助函数（2026-07-26 新增）
//!
//! 提取控制器间重复的模板代码，减少 CRUD 重复（修复 Brooks-Lint R3 警告）。
//! 当前提供：分页参数解析（page/page_size）、数据归属过滤（IDOR 修复）。

use crate::middleware::auth_middleware::CurrentUser;
use axum::body::Body;
use axum::http::Request;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use sz_rust_core::orm::Value;

/// 数据归属过滤（2026-10-28 IDOR 修复）
///
/// 返回 `(允许查询的 merchant_id, 是否放行)`：
/// - 管理员：返回请求参数中的 merchant_id（可查看全部/指定商户）
/// - 普通用户：返回自己的 merchant_id（忽略请求参数，防止水平越权）
/// - 无法获取用户上下文：返回 `(None, false)`，调用方应拒绝请求
pub fn effective_merchant_scope(
    req: &Request<Body>,
    requested: Option<i64>,
) -> (Option<i64>, bool) {
    effective_merchant_scope_from_user(CurrentUser::from_request(req).as_ref(), requested)
}

/// 数据归属过滤（基于已提取的用户上下文；供控制器在消费请求前调用）
pub fn effective_merchant_scope_from_user(
    user: Option<&CurrentUser>,
    requested: Option<i64>,
) -> (Option<i64>, bool) {
    match user {
        Some(u) if u.is_admin => (requested, true),
        Some(u) => (u.merchant_id, true),
        None => (None, false),
    }
}

/// 校验单个资源是否属于当前用户（IDOR 修复）
///
/// 管理员可访问任意资源；普通用户仅允许访问自己商户的资源。
/// `resource_merchant_id` 为资源行中的 merchant_id（可能为 None）。
pub fn can_access_merchant_resource(
    req: &Request<Body>,
    resource_merchant_id: Option<i64>,
) -> bool {
    can_access_merchant_resource_from_user(
        CurrentUser::from_request(req).as_ref(),
        resource_merchant_id,
    )
}

/// 资源归属校验（基于已提取的用户上下文）
pub fn can_access_merchant_resource_from_user(
    user: Option<&CurrentUser>,
    resource_merchant_id: Option<i64>,
) -> bool {
    match user {
        Some(u) if u.is_admin => true,
        Some(u) => u.merchant_id.is_some() && u.merchant_id == resource_merchant_id,
        None => false,
    }
}

/// 是否允许创建商户（2026-10-28 越权修复）
///
/// 创建商户会新增租户与市场归属，属于管理动作，仅管理员可执行。
/// 普通用户只能维护自己已有商户的数据（见 [`can_access_merchant_resource_from_user`]）。
pub fn can_create_merchant_from_user(user: Option<&CurrentUser>) -> bool {
    matches!(user, Some(u) if u.is_admin)
}

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
pub fn extract_fields_by_whitelist(
    data: &JsonValue,
    allowed_keys: &[&str],
) -> HashMap<String, Value> {
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
        } else {
            panic!("name should be String");
        }
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
        } else {
            panic!("weight should be F64");
        }
    }

    fn user(is_admin: bool, merchant_id: Option<i64>) -> CurrentUser {
        CurrentUser {
            user_id: 1,
            username: "tester".to_string(),
            merchant_id,
            is_admin,
        }
    }

    #[test]
    fn test_create_merchant_requires_admin() {
        assert!(can_create_merchant_from_user(Some(&user(true, None))));
        assert!(!can_create_merchant_from_user(Some(&user(false, Some(9)))));
        assert!(!can_create_merchant_from_user(None));
    }

    #[test]
    fn test_access_scope_admin_vs_user() {
        let admin = user(true, None);
        let normal = user(false, Some(9));
        assert_eq!(
            effective_merchant_scope_from_user(Some(&admin), Some(42)),
            (Some(42), true)
        );
        assert_eq!(
            effective_merchant_scope_from_user(Some(&normal), Some(42)),
            (Some(9), true)
        );
        assert_eq!(
            effective_merchant_scope_from_user(None, Some(42)),
            (None, false)
        );
        assert!(!can_access_merchant_resource_from_user(
            Some(&normal),
            Some(42)
        ));
        assert!(can_access_merchant_resource_from_user(
            Some(&normal),
            Some(9)
        ));
    }
}

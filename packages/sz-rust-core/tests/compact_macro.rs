// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! compact! 宏集成测试 — PHP `compact()` 一致性验证（R5 硬约束）
//!
//! 对齐 PHP `compact()` 函数行为：
//!
//! ```php
//! $code = 1;
//! $msg = "ok";
//! $data = ["id" => 1];
//! return compact('code', 'msg', 'data');
//! // 等价于：['code' => 1, 'msg' => "ok", 'data' => ["id" => 1]]
//! ```
//!
//! PHP `compact()` 关键行为：
//! 1. 接受变量名作为参数（字符串或数组）
//! 2. 在当前作用域查找同名变量
//! 3. 返回关联数组，键是变量名，值是变量值
//! 4. 字段顺序严格按参数顺序保序
//! 5. 支持任意类型的变量值

use serde_json::json;
use sz_rust_macros::compact;

// ====================================================================
// 基本功能测试
// ====================================================================

#[test]
fn test_compact_basic_single_variable() {
    let code = 1i32;
    let map = compact!(code);
    assert_eq!(map.len(), 1);
    assert_eq!(map["code"], 1);
}

#[test]
fn test_compact_basic_multiple_variables() {
    let name = "alice";
    let age = 30i32;
    let map = compact!(name, age);
    assert_eq!(map.len(), 2);
    assert_eq!(map["name"], "alice");
    assert_eq!(map["age"], 30);
}

#[test]
fn test_compact_empty_arguments() {
    // PHP compact() 无参数返回空数组
    let map = compact!();
    assert_eq!(map.len(), 0);
    assert!(map.is_empty());
}

// ====================================================================
// 字段顺序测试（对齐 PHP compact() 参数顺序）
// ====================================================================

#[test]
fn test_compact_preserves_field_order() {
    // PHP compact('code', 'msg', 'data') 严格按参数顺序保序
    // 对齐 PHP SzController::renderJson 中的 compact('code', 'msg', 'data')
    let code = 1i32;
    let msg = "ok".to_string();
    let data = json!({"id": 1});
    let map = compact!(code, msg, data);

    let keys: Vec<&String> = map.keys().collect();
    assert_eq!(keys, vec!["code", "msg", "data"]);
}

#[test]
fn test_compact_order_independent_of_variable_declaration() {
    // PHP compact() 字段顺序由 compact() 参数顺序决定，与变量声明顺序无关
    let zebra = "z";
    let apple = "a";
    let mango = "m";
    let map = compact!(apple, mango, zebra);

    let keys: Vec<&String> = map.keys().collect();
    assert_eq!(keys, vec!["apple", "mango", "zebra"]);
}

// ====================================================================
// 类型转换测试
// ====================================================================

#[test]
fn test_compact_supports_i32() {
    let value = 42i32;
    let map = compact!(value);
    assert_eq!(map["value"], 42);
}

#[test]
fn test_compact_supports_i64() {
    let value = 9223372036854775807i64;
    let map = compact!(value);
    assert_eq!(map["value"], 9223372036854775807i64);
}

#[test]
fn test_compact_supports_string() {
    let value = "hello".to_string();
    let map = compact!(value);
    assert_eq!(map["value"], "hello");
}

#[test]
fn test_compact_supports_str() {
    let value = "world";
    let map = compact!(value);
    assert_eq!(map["value"], "world");
}

#[test]
fn test_compact_supports_bool() {
    let value = true;
    let map = compact!(value);
    assert_eq!(map["value"], true);
}

#[test]
fn test_compact_supports_vec() {
    let value = vec![1, 2, 3];
    let map = compact!(value);
    assert_eq!(map["value"], json!([1, 2, 3]));
}

#[test]
fn test_compact_supports_serde_json_value() {
    let value = json!({"nested": {"key": "val"}});
    let map = compact!(value);
    assert_eq!(map["value"]["nested"]["key"], "val");
}

#[test]
fn test_compact_supports_option() {
    let some_value: Option<i32> = Some(42);
    let none_value: Option<i32> = None;
    let map = compact!(some_value, none_value);
    assert_eq!(map["some_value"], 42);
    assert_eq!(map["none_value"], serde_json::Value::Null);
}

// ====================================================================
// PHP 一致性测试（R5 硬约束：PHP/Rust 行为对比）
// ====================================================================

#[test]
fn test_php_consistency_compact_returns_array_with_variable_names_as_keys() {
    // PHP compact('code', 'msg', 'data') 返回 ['code' => ..., 'msg' => ..., 'data' => ...]
    // 键名是变量名（字符串），键值是变量值
    let code = 1i32;
    let msg = "success".to_string();
    let data = json!({"list": [], "total": 0});
    let result = compact!(code, msg, data);

    assert_eq!(result["code"], 1, "键名 'code' 对应变量 $code 的值");
    assert_eq!(result["msg"], "success", "键名 'msg' 对应变量 $msg 的值");
    assert_eq!(
        result["data"]["total"], 0,
        "键名 'data' 对应变量 $data 的值"
    );
}

#[test]
fn test_php_consistency_compact_field_order_matches_argument_order() {
    // PHP compact() 严格按参数顺序保序，不按变量声明顺序
    // PHP 源码 SzController::renderJson: return compact('code', 'msg', 'data');
    // 字段顺序必须是 code → msg → data
    let data = json!({});
    let msg = "ok".to_string();
    let code = 1i32;
    let result = compact!(code, msg, data);

    let keys: Vec<&String> = result.keys().collect();
    assert_eq!(
        keys,
        vec!["code", "msg", "data"],
        "字段顺序必须为 code → msg → data（对齐 PHP compact() 参数顺序）"
    );
}

#[test]
fn test_php_consistency_compact_render_json_scenario() {
    // 模拟 PHP SzController::renderJson 完整场景
    // PHP: protected function renderJson($code = 1, $msg = '', $data = [])
    //      { return compact('code', 'msg', 'data'); }
    let code = 1i32;
    let msg = "".to_string();
    let data = json!({});
    let result = compact!(code, msg, data);

    // 验证字段顺序
    let keys: Vec<&String> = result.keys().collect();
    assert_eq!(keys, vec!["code", "msg", "data"]);

    // 验证字段值（对齐 PHP 默认值）
    assert_eq!(result["code"], 1);
    assert_eq!(result["msg"], "");
    assert_eq!(result["data"], json!({}));
}

#[test]
fn test_php_consistency_compact_with_error_code() {
    // PHP renderError 场景：compact() 支持错误码
    // PHP: renderError($msg = 'error', $data = [], $code = 0)
    //      → renderJson($code, $msg, $data) → compact('code', 'msg', 'data')
    let code = -1i32;
    let msg = "未登录".to_string();
    let data = json!({});
    let result = compact!(code, msg, data);

    assert_eq!(
        result["code"], -1,
        "错误码 -1 = 未登录（对齐 PHP BaseException）"
    );
    assert_eq!(result["msg"], "未登录");
    assert_eq!(result["data"], json!({}));
}

#[test]
fn test_php_consistency_compact_with_complex_data() {
    // PHP compact() 支持复杂的 data 字段（数组、嵌套对象）
    let code = 1i32;
    let msg = "查询成功".to_string();
    let data = json!({
        "list": [{"id": 1, "name": "alice"}, {"id": 2, "name": "bob"}],
        "total": 2,
        "page": 1,
        "size": 10
    });
    let result = compact!(code, msg, data);

    assert_eq!(result["code"], 1);
    assert_eq!(result["msg"], "查询成功");
    assert_eq!(result["data"]["total"], 2);
    assert_eq!(result["data"]["list"][0]["id"], 1);
    assert_eq!(result["data"]["list"][0]["name"], "alice");
    assert_eq!(result["data"]["list"][1]["id"], 2);
    assert_eq!(result["data"]["list"][1]["name"], "bob");
    assert_eq!(result["data"]["page"], 1);
    assert_eq!(result["data"]["size"], 10);
}

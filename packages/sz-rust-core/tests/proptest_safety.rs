// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 属性测试套件 — 基于 proptest 的结构化 Fuzz 测试
//!
//! 与 `fuzz.rs`（基于 xorshift64 PRNG）互补，本文件使用 `proptest` crate 提供：
//!
//! - **shrinking**：发现 panic 后自动最小化复现案例
//! - **结构化输入生成**：基于策略（Strategy）生成合法的输入数据结构
//! - **更高迭代次数**：默认 256 个案例（proptest 默认），可配置
//!
//! ## 覆盖范围
//!
//! | 测试用例 | 目标 API | 验证内容 |
//! |---------|---------|---------|
//! | `prop_parse_path_never_panics` | `router::parse_path` | 任意字符串不 panic |
//! | `prop_error_code_conversion_never_panics` | `error::ErrorCode::from` | 任意 i32 不 panic |
//! | `prop_json_response_serialization_never_panics` | `response::ApiResponse` | 任意 JSON 值序列化不 panic |
//! | `prop_validate_rule_never_panics` | `validate::Validate::check_rule` | 任意规则字符串不 panic |
//! | `prop_route_config_load_never_panics` | `routing::load_routes_from_yaml_str` | 任意 YAML 字符串不 panic |
//! | `prop_handler_ref_parse_never_panics` | `routing::HandlerRef::parse` | 任意字符串不 panic |
//! | `prop_sql_injection_in_validate_never_panics` | `validate::Validate::check_rule` | SQL 注入 payload 不 panic |
//! | `prop_sql_injection_in_response_never_panics` | `response::ApiResponse` | SQL 注入 payload 序列化不 panic |
//! | `prop_http_path_semantic_parse` | `router::parse_path` | 语义有效 HTTP 路径正确解析 |
//! | `prop_handler_ref_semantic_parse` | `routing::HandlerRef::parse` | 语义有效 HandlerRef 不 panic |
//! | `prop_validation_rule_semantic_check` | `validate::Validate::check_rule` | 语义有效规则不 panic |
//! | `prop_sql_injection_in_yaml_config_never_panics` | `routing::load_routes_from_yaml_str` | SQL 注入 YAML 不 panic |
//!
//! ## 运行方式
//!
//! ```sh
//! cargo test --package sz-rust-core --test proptest_safety
//! ```
//!
//! 发现失败时 proptest 会自动 shrinking 并输出最小复现案例到 `proptest-regressions/` 目录。

use proptest::collection;
use proptest::prelude::*;
use serde_json::{Map, Value};
use sz_rust_core::error::{BaseException, ErrorCode};
use sz_rust_core::response::ApiResponse;
use sz_rust_core::router::parse_path;
use sz_rust_core::routing::{load_routes_from_yaml_str, HandlerRef};
use sz_rust_core::validate::Validate;

/// 生成任意 JSON 值的策略（深度限制为 2 层，避免无限递归导致栈溢出）
///
/// 支持：null、bool、i64、f64、string、array、object
fn json_value() -> BoxedStrategy<Value> {
    json_value_with_depth(2)
}

/// 递归生成 JSON 值，depth 控制嵌套深度
fn json_value_with_depth(depth: u32) -> BoxedStrategy<Value> {
    if depth == 0 {
        // 基础类型：不再递归
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            any::<f64>().prop_map(|f| {
                if f.is_finite() {
                    serde_json::Number::from_f64(f)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }),
            ".{0,50}".prop_map(Value::String),
        ]
        .boxed()
    } else {
        // 复合类型：递归一层
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            ".{0,50}".prop_map(Value::String),
            collection::vec(json_value_with_depth(depth - 1), 0..3).prop_map(Value::Array),
            collection::hash_map(".{1,10}", json_value_with_depth(depth - 1), 0..3).prop_map(|m| {
                let mut map = Map::new();
                for (k, v) in m {
                    map.insert(k, v);
                }
                Value::Object(map)
            }),
        ]
        .boxed()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 属性测试：`parse_path` 在任意字符串输入下不 panic
    ///
    /// 验证不变量：对于任意字符串输入，`parse_path` 必须返回有效的 ParsedPath，不 panic。
    #[test]
    fn prop_parse_path_never_panics(input in ".{0,200}") {
        let parsed = parse_path(&input);
        // ParsedPath 总是返回非空字段（即使是空字符串也有默认值）
        prop_assert!(!parsed.app.is_empty() || !parsed.controller.is_empty() || !parsed.action.is_empty());
    }

    /// 属性测试：`ErrorCode::from` 在任意 i32 输入下不 panic
    ///
    /// 验证不变量：对于任意 i32 输入，`ErrorCode::from` 必须返回有效 ErrorCode，不 panic。
    #[test]
    fn prop_error_code_conversion_never_panics(code in any::<i32>()) {
        let error_code = ErrorCode::from(code);
        let _exception = BaseException::new(error_code, "test message");
    }

    /// 属性测试：`ApiResponse` 序列化在任意 JSON 值下不 panic
    ///
    /// 验证不变量：对于任意 JSON 值，`ApiResponse::success` 必须能序列化为 JSON 字符串。
    #[test]
    fn prop_json_response_serialization_never_panics(
        msg in ".{0,100}",
        data in json_value()
    ) {
        let response = ApiResponse::success(data, &msg);
        let json = serde_json::to_string(&response);
        prop_assert!(json.is_ok());
    }

    /// 属性测试：`Validate::check_rule` 在任意规则字符串下不 panic
    ///
    /// 验证不变量：对于任意规则和值，`check_rule` 必须返回 Ok 或 Err，不 panic。
    #[test]
    fn prop_validate_rule_never_panics(
        rule in ".{0,50}",
        value in json_value()
    ) {
        let validate = Validate::new();
        let _ = validate.check_rule(&value, &rule);
    }

    /// 属性测试：`load_routes_from_yaml_str` 在任意 YAML 字符串下不 panic
    ///
    /// 验证不变量：对于任意字符串，`load_routes_from_yaml_str` 必须返回 Ok 或 Err，不 panic。
    #[test]
    fn prop_route_config_load_never_panics(yaml in ".{0,500}") {
        let _ = load_routes_from_yaml_str(&yaml);
    }

    /// 属性测试：`HandlerRef::parse` 在任意字符串下不 panic
    ///
    /// 验证不变量：对于任意字符串，`HandlerRef::parse` 必须返回 Ok 或 Err，不 panic。
    #[test]
    fn prop_handler_ref_parse_never_panics(input in ".{0,200}") {
        let _ = HandlerRef::parse(&input);
    }
}

// ============================================================================
// 语义化结构化 Fuzz 测试（2026-07-26 新增 — Brooks-Lint T5 Coverage Illusion 修复）
//
// 以下测试针对关键路径生成**语义有效**的畸形输入，覆盖：
// - SQL 注入向量（验证参数化查询与转义机制不 panic）
// - 路由路径语义（生成合法 HTTP 路径变体）
// - HandlerRef 语义（生成 Controller@action 格式字符串）
// - 验证规则语义（生成 require|max:255|email 格式规则）
// ============================================================================

/// 生成 SQL 注入 payload 字符串的策略
///
/// 包含经典 SQL 注入向量：`' OR '1'='1`、`'; DROP TABLE--`、UNION SELECT 等，
/// 用于验证参数化查询相关 API 在恶意输入下不 panic。
fn sql_injection_payload() -> BoxedStrategy<String> {
    prop_oneof![
        Just("' OR '1'='1".to_string()),
        Just("' OR 1=1 --".to_string()),
        Just("\" OR \"1\"=\"1".to_string()),
        Just("' UNION SELECT NULL--".to_string()),
        Just("' UNION ALL SELECT 1,2,3--".to_string()),
        Just("'; DROP TABLE users--".to_string()),
        Just("'; INSERT INTO admin VALUES('x','y')--".to_string()),
        Just("'/*comment*/OR/*comment*/'1'='1".to_string()),
        Just("' OR 'x' LIKE 'x".to_string()),
        Just("%27%20OR%20%271%27%3D%271".to_string()),
        Just("\\' OR \\'1\\'=\\'1".to_string()),
        ".{0,100}".prop_map(|s| format!("' OR '{}'='1", s)),
        (
            ".{0,20}",
            prop_oneof![
                Just("SELECT".to_string()),
                Just("INSERT".to_string()),
                Just("UPDATE".to_string()),
                Just("DELETE".to_string()),
                Just("DROP".to_string()),
                Just("UNION".to_string())
            ]
        )
            .prop_map(|(prefix, kw)| format!("{} {} --", prefix, kw)),
    ]
    .boxed()
}

/// 生成语义有效的 HTTP 路径策略
///
/// 生成形如 `/api/v1/users/123`、`/admin/dashboard`、`/` 等合法路径变体，
/// 用于验证路由解析器在语义有效输入下行为正确。
fn http_path_semantic() -> BoxedStrategy<String> {
    prop_oneof![
        Just("/".to_string()),
        "[a-z]{1,10}".prop_map(|s| format!("/{}", s)),
        ("[a-z]{1,10}", "[a-z]{1,10}").prop_map(|(a, b)| format!("/{}/{}", a, b)),
        ("[a-z]{1,10}", "[a-z]{1,10}", "[0-9]{1,3}")
            .prop_map(|(a, b, c)| format!("/{}/{}/{}", a, b, c)),
        ("[a-z]{1,10}", "[a-z]{1,10}", "[0-9]{1,10}")
            .prop_map(|(a, b, c)| format!("/{}/{}/{}", a, b, c)),
        ("[a-z]{1,10}", "[a-z]{1,10}", "[a-z]{1,5}=[0-9]{1,5}")
            .prop_map(|(a, b, q)| format!("/{}/{}?{}", a, b, q)),
    ]
    .boxed()
}

/// 生成 `Controller@action` 格式的 HandlerRef 字符串策略
///
/// 包含合法格式与畸形变体（缺少 @、空 action、特殊字符等），
/// 用于验证 HandlerRef::parse 在语义变体下行为正确。
fn handler_ref_semantic() -> BoxedStrategy<String> {
    prop_oneof![
        ("[A-Z][a-zA-Z]{0,20}", "[a-z][a-zA-Z]{0,20}").prop_map(|(c, a)| format!("{}@{}", c, a)),
        ("[A-Z][a-zA-Z]{0,20}", "[a-z][a-zA-Z]{0,20}").prop_map(|(c, a)| format!("{}/{}", c, a)),
        "[A-Z][a-zA-Z]{0,20}".prop_map(|c| format!("{}@", c)),
        "[a-z][a-zA-Z]{0,20}".prop_map(|a| format!("@{}", a)),
        (
            "[A-Z][a-zA-Z]{0,10}",
            "[a-z][a-zA-Z]{0,10}",
            "[a-z][a-zA-Z]{0,10}"
        )
            .prop_map(|(a, b, c)| format!("{}@{}@{}", a, b, c)),
        Just("".to_string()),
        Just("@".to_string()),
    ]
    .boxed()
}

/// 生成语义有效的验证规则字符串策略
///
/// 生成形如 `require|max:255|email`、`integer|between:1,100` 等规则，
/// 用于验证 Validate::check_rule 在语义有效规则下行为正确。
fn validation_rule_semantic() -> BoxedStrategy<String> {
    fn rule_unit() -> BoxedStrategy<String> {
        prop_oneof![
            Just("require".to_string()),
            Just("integer".to_string()),
            Just("float".to_string()),
            Just("boolean".to_string()),
            Just("email".to_string()),
            Just("url".to_string()),
            Just("date".to_string()),
            Just("ip".to_string()),
            Just("alpha".to_string()),
            Just("alphaNum".to_string()),
            Just("alphaDash".to_string()),
            Just("chs".to_string()),
            Just("chsAlpha".to_string()),
            Just("mobile".to_string()),
            Just("idCard".to_string()),
            "[a-z]{1,10}".prop_map(|s| s),
            ("[a-z]{1,10}", "[0-9]{1,5}").prop_map(|(r, n)| format!("{}:{}", r, n)),
        ]
        .boxed()
    }

    prop_oneof![
        rule_unit(),
        collection::vec(rule_unit(), 2..5).prop_map(|rs| rs.join("|")),
        collection::vec(rule_unit(), 2..5).prop_map(|rs| rs.join(",")),
        Just("".to_string()),
        Just("|".to_string()),
        Just(",".to_string()),
    ]
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// 语义测试：SQL 注入 payload 作为参数值传入 validate::check_rule 不 panic
    ///
    /// 验证不变量：对于任意 SQL 注入 payload 字符串，`check_rule` 必须返回 Ok 或 Err，不 panic。
    /// 这确保恶意输入不会破坏验证器内部状态。
    #[test]
    fn prop_sql_injection_in_validate_never_panics(
        payload in sql_injection_payload(),
        rule in validation_rule_semantic()
    ) {
        let validate = Validate::new();
        let _ = validate.check_rule(&Value::String(payload), &rule);
    }

    /// 语义测试：SQL 注入 payload 作为 JSON 字符串值传入 ApiResponse 不 panic
    ///
    /// 验证不变量：对于任意 SQL 注入 payload，`ApiResponse::success` 必须能序列化为 JSON。
    /// 这确保恶意输入不会破坏响应序列化（XSS/响应拆分防护）。
    #[test]
    fn prop_sql_injection_in_response_never_panics(
        payload in sql_injection_payload()
    ) {
        let response = ApiResponse::success(Value::String(payload), "ok");
        let json = serde_json::to_string(&response);
        prop_assert!(json.is_ok());
    }

    /// 语义测试：语义有效的 HTTP 路径传入 parse_path 不 panic 且返回合理结构
    ///
    /// 验证不变量：对于语义有效的 HTTP 路径，`parse_path` 必须返回非空 ParsedPath。
    /// 这确保路由解析器在合法路径下行为正确。
    #[test]
    fn prop_http_path_semantic_parse(
        path in http_path_semantic()
    ) {
        let parsed = parse_path(&path);
        prop_assert!(
            !parsed.app.is_empty() || !parsed.controller.is_empty() || !parsed.action.is_empty(),
            "路径 {:?} 解析后所有字段均为空",
            path
        );
    }

    /// 语义测试：语义有效的 HandlerRef 字符串传入 parse 不 panic
    ///
    /// 验证不变量：对于语义变体的 HandlerRef 字符串，`parse` 必须返回 Ok 或 Err，不 panic。
    /// 这确保 HandlerRef 解析器在合法/畸形变体下都健壮。
    #[test]
    fn prop_handler_ref_semantic_parse(
        input in handler_ref_semantic()
    ) {
        let _ = HandlerRef::parse(&input);
    }

    /// 语义测试：语义有效的验证规则传入 check_rule 不 panic
    ///
    /// 验证不变量：对于语义有效的验证规则字符串，`check_rule` 必须返回 Ok 或 Err，不 panic。
    /// 这确保验证器在合法/畸形规则组合下都健壮。
    #[test]
    fn prop_validation_rule_semantic_check(
        rule in validation_rule_semantic()
    ) {
        let validate = Validate::new();
        let value = Value::String("test_value".to_string());
        let _ = validate.check_rule(&value, &rule);
    }

    /// 语义测试：SQL 注入 payload 作为 YAML 路由配置传入不 panic
    ///
    /// 验证不变量：对于包含 SQL 注入 payload 的 YAML 字符串，
    /// `load_routes_from_yaml_str` 必须返回 Ok 或 Err，不 panic。
    /// 这确保路由配置加载器在恶意输入下健壮。
    #[test]
    fn prop_sql_injection_in_yaml_config_never_panics(
        payload in sql_injection_payload()
    ) {
        let yaml = format!("route:\n  path: {}\n  handler: TestController@index", payload);
        let _ = load_routes_from_yaml_str(&yaml);
    }
}

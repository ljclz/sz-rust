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

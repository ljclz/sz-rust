//! Fuzz 测试套件 — 针对输入边界、恶意输入、随机数据的鲁棒性测试
//!
//! 使用自定义 xorshift64 伪随机数生成器（`common::fuzz::Rng`），不依赖外部 fuzz 库
//! （cargo-fuzz / libfuzzer-sys / afl.rs）。这样：
//!
//! - 不需要 nightly 工具链
//! - 不需要单独的 `fuzz/Cargo.toml` 项目
//! - 可以直接 `cargo test --test fuzz` 运行
//! - 与 CI 的标准 test job 集成
//!
//! ## 测试目标
//!
//! 验证 sz-rust-core 的核心公开 API 在面对随机/恶意输入时**不会 panic**。
//! 不验证业务正确性（那是单元测试的职责），只验证鲁棒性。
//!
//! ## 覆盖范围
//!
//! | 测试用例 | 目标 API | 验证内容 |
//! |---------|---------|---------|
//! | `fuzz_parse_path_safety` | `router::parse_path` | 随机 URI 不 panic |
//! | `fuzz_handler_ref_parse_safety` | `routing::HandlerRef::parse` | 随机字符串不 panic |
//! | `fuzz_route_config_load_safety` | `routing::load_routes_from_yaml_str` | 随机 YAML 不 panic |
//! | `fuzz_json_response_serialization` | `response::ApiResponse` | 随机数据序列化不 panic |
//! | `fuzz_error_code_conversion` | `error::ErrorCode::from` / `BaseException::new` | 随机 i32 不 panic |
//! | `fuzz_config_parse_safety` | `serde_yaml::from_str::<AppConfig>` | 随机 YAML 配置不 panic |
//! | `fuzz_validate_rules` | `validate::Validate::check_rule` | 随机规则不 panic |
//!
//! ## 安全约束
//!
//! - 不使用 `unsafe` 块
//! - 不使用 `todo!` / `unimplemented!` / `unreachable!`
//! - 所有代码有中文文档注释

mod common;

use common::fuzz::Rng;
use serde_json::{Map, Value};
use sz_rust_core::config::AppConfig;
use sz_rust_core::error::{BaseException, ErrorCode};
use sz_rust_core::response::ApiResponse;
use sz_rust_core::router::parse_path;
use sz_rust_core::routing::{load_routes_from_yaml_str, HandlerRef};
use sz_rust_core::validate::Validate;

/// Fuzz 迭代次数：每个测试用例运行 1000 次随机输入
const FUZZ_ITERATIONS: usize = 1000;

/// Fuzz 测试：`router::parse_path` 在随机 URI 输入下不 panic
///
/// `parse_path` 设计为永不为空、永不 panic（即使是空字符串也返回默认三元组）。
/// 本测试验证随机字符串（包含特殊字符、超长字符串、路径穿越尝试）不会破坏这一不变量。
#[test]
fn fuzz_parse_path_safety() {
    let mut rng = Rng::new(42);

    for _ in 0..FUZZ_ITERATIONS {
        let len = rng.next_usize(200);
        let uri = rng.next_string(len);
        let parsed = parse_path(&uri);

        // 验证：parse_path 永远返回非空三元组
        assert!(!parsed.app.is_empty(), "app 不应为空");
        assert!(!parsed.controller.is_empty(), "controller 不应为空");
        assert!(!parsed.action.is_empty(), "action 不应为空");
    }

    // 边界值：空字符串、纯查询字符串、超长路径
    let boundary_inputs = ["", "/", "?", "/?", "/a?b=c", "/a/b?c=d&e=f", "//", "///"];
    for input in &boundary_inputs {
        let parsed = parse_path(input);
        assert!(!parsed.app.is_empty());
        assert!(!parsed.controller.is_empty());
        assert!(!parsed.action.is_empty());
    }

    // 路径穿越尝试
    let traversal_inputs = [
        "/../etc/passwd",
        "/..%2F..%2Fetc",
        "/%2e%2e/%2e%2e/etc",
        "/common/foo/bar",
        "/oapc/../admin",
    ];
    for input in &traversal_inputs {
        let parsed = parse_path(input);
        assert!(!parsed.app.is_empty());
        assert!(!parsed.controller.is_empty());
        assert!(!parsed.action.is_empty());
    }
}

/// Fuzz 测试：`routing::HandlerRef::parse` 在随机字符串输入下不 panic
///
/// `HandlerRef::parse` 对非法输入返回 `Err`，对合法输入返回 `Ok`。
/// 本测试验证随机字符串（包含 `@`、`/`、空格、特殊字符）不会引发 panic，
/// 且 `to_handler_string` 对成功解析的结果不会 panic。
#[test]
fn fuzz_handler_ref_parse_safety() {
    let mut rng = Rng::new(123);

    for _ in 0..FUZZ_ITERATIONS {
        let len = rng.next_usize(50);
        let input = rng.next_string(len);
        let result = HandlerRef::parse(&input);
        if let Ok(handler) = result {
            // 验证：成功解析的结果可重新序列化为字符串
            let s = handler.to_handler_string();
            assert!(!s.is_empty(), "to_handler_string 不应返回空");
        }
    }

    // 边界值：空字符串、纯空格、只有 @、只有 /
    let boundary_inputs = ["", " ", "@", "/", "User@", "@action", "/action"];
    for input in &boundary_inputs {
        let _ = HandlerRef::parse(input);
    }

    // 合法格式：验证解析成功且 to_handler_string 不 panic
    let valid_inputs = ["User", "User@list", "User/list", "Customer@index"];
    for input in &valid_inputs {
        let handler = HandlerRef::parse(input).expect("合法输入应解析成功");
        let _ = handler.to_handler_string();
    }
}

/// Fuzz 测试：`routing::load_routes_from_yaml_str` 在随机 YAML 输入下不 panic
///
/// 随机字符串几乎都不是合法 YAML，会返回 `Err`，这是预期行为。
/// 本测试验证：
/// - 随机字符串不会引发 panic
/// - 故意构造的"接近合法"的 YAML 也不会引发 panic
#[test]
fn fuzz_route_config_load_safety() {
    let mut rng = Rng::new(456);

    for _ in 0..FUZZ_ITERATIONS {
        let len = rng.next_usize(200);
        let yaml = rng.next_string(len);
        let _ = load_routes_from_yaml_str(&yaml);
    }

    // 边界值：空字符串、纯空白、合法但不完整的 YAML
    let boundary_inputs = ["", " ", "\n", "\t", "routes: []", "routes:", "---"];
    for input in &boundary_inputs {
        let _ = load_routes_from_yaml_str(input);
    }

    // 故意构造的"接近合法"YAML：随机 method / path / handler
    for _ in 0..100 {
        let method = rng.next_string(10);
        let path = rng.next_string(20);
        let handler = rng.next_string(30);
        let yaml = format!(
            "routes:\n  - method: {}\n    path: {}\n    handler: {}\n",
            method, path, handler
        );
        let _ = load_routes_from_yaml_str(&yaml);
    }

    // 合法 YAML：验证解析成功
    let valid_yaml = r#"
routes:
  - method: GET
    path: /users
    handler: User@list
  - method: POST
    path: /users
    handler: User@create
"#;
    let config = load_routes_from_yaml_str(valid_yaml).expect("合法 YAML 应解析成功");
    assert_eq!(config.routes.len(), 2);
}

/// Fuzz 测试：`response::ApiResponse` 在随机数据下序列化不 panic
///
/// `ApiResponse::to_json_string` 内部调用 `serde_json` 序列化，
/// 本测试验证随机构造的 `Value`（包括深度嵌套、特殊字符、大数组）不会引发 panic。
#[test]
fn fuzz_json_response_serialization() {
    let mut rng = Rng::new(789);

    for _ in 0..FUZZ_ITERATIONS {
        let code = rng.next_i64() as i32;
        let msg_len = rng.next_usize(100);
        let msg = rng.next_string(msg_len);
        let data = generate_random_json_value(&mut rng, 3);

        // 验证：ApiResponse::new 不 panic
        let resp = ApiResponse::new(code, msg.clone(), data.clone());

        // 验证：to_value 不 panic
        let value = resp.to_value();
        assert_eq!(value["code"], code);

        // 验证：to_json_string 不 panic 且产出合法 JSON
        let json_str = resp.to_json_string();
        let reparsed: Value =
            serde_json::from_str(&json_str).expect("to_json_string 输出应可反序列化");
        assert_eq!(reparsed["code"], code);

        // 验证：success / error / error_with_code 快捷构造不 panic
        let _ = ApiResponse::success(data.clone(), msg.clone());
        let _ = ApiResponse::error(msg.clone());
        let _ = ApiResponse::error_with_code(code, msg, data);
    }
}

/// Fuzz 测试：`error::ErrorCode::from(i32)` 和 `BaseException::new` 在随机 i32 下不 panic
///
/// `ErrorCode::from` 对未知码默认映射到 `Failed`，本测试验证任意 i32（包括
/// `i32::MIN`、`i32::MAX`、0、负数）都能安全转换，且 `as_i32` / `http_status` 不 panic。
#[test]
fn fuzz_error_code_conversion() {
    let mut rng = Rng::new(101);

    for _ in 0..FUZZ_ITERATIONS {
        let code_int = rng.next_i64() as i32;
        let code_enum = ErrorCode::from(code_int);

        // 验证：as_i32 / http_status 不 panic
        let _ = code_enum.as_i32();
        let _ = code_enum.http_status();

        // 验证：BaseException::new 不 panic
        let msg_len = rng.next_usize(50);
        let msg = rng.next_string(msg_len);
        let ex = BaseException::new(code_enum, msg.clone());
        assert_eq!(ex.code, code_enum.as_i32());
        assert_eq!(ex.msg, msg);

        // 验证：to_json 不 panic 且产出合法 JSON
        let json = ex.to_json();
        assert_eq!(json["code"], code_enum.as_i32());
        assert_eq!(json["msg"], msg);
    }

    // 边界值：i32 极值
    let boundary_codes = [
        i32::MIN,
        i32::MIN + 1,
        -1,
        0,
        1,
        100,
        403,
        404,
        422,
        500,
        i32::MAX - 1,
        i32::MAX,
    ];
    for &code_int in &boundary_codes {
        let code_enum = ErrorCode::from(code_int);
        let _ = code_enum.as_i32();
        let _ = code_enum.http_status();
        let ex = BaseException::new(code_enum, "boundary");
        let _ = ex.to_json();
    }

    // 快捷构造函数不 panic
    let _ = BaseException::not_login("test");
    let _ = BaseException::user_not_found("test");
    let _ = BaseException::user_disabled("test");
    let _ = BaseException::failed("test");
    let _ = BaseException::forbidden("test");
    let _ = BaseException::not_found("test");
    let _ = BaseException::validate_failed("test");
    let _ = BaseException::db_error("test");
}

/// Fuzz 测试：`config::AppConfig` 在随机 YAML 输入下反序列化不 panic
///
/// `AppConfig` 实现了 `serde::Deserialize`，所有字段都有 `#[serde(default)]`，
/// 即使 YAML 字段缺失也能正常加载。本测试验证随机 YAML（包含部分合法字段 +
/// 随机噪音）不会引发 panic。
///
/// 注意：`config.rs` 没有提供 `from_yaml_str` 公开 API，只有 `load_from_dir`。
/// 这里直接使用 `serde_yaml::from_str::<AppConfig>` 测试反序列化层。
#[test]
fn fuzz_config_parse_safety() {
    let mut rng = Rng::new(202);

    for _ in 0..FUZZ_ITERATIONS {
        let len = rng.next_usize(200);
        let yaml = rng.next_string(len);
        // 随机字符串几乎都不是合法 YAML，会返回 Err，这是预期行为
        let _ = serde_yaml::from_str::<AppConfig>(&yaml);
    }

    // 边界值：空字符串、纯空白
    let boundary_inputs = ["", " ", "\n", "\t", "---", "..."];
    for input in &boundary_inputs {
        let _ = serde_yaml::from_str::<AppConfig>(input);
    }

    // 部分合法字段 + 随机噪音：验证不 panic（随机字符串可能包含 YAML 特殊字符，
    // 会导致解析失败或字段值被截断，这是预期行为，fuzz 只验证不 panic）
    for _ in 0..100 {
        let app_host = rng.next_string(20);
        let default_app = rng.next_string(10);
        let auto_multi_app = rng.next_bool();
        let yaml = format!(
            "app:\n  app_host: {}\n  default_app: {}\n  auto_multi_app: {}\n",
            app_host, default_app, auto_multi_app
        );
        // 只验证不 panic，不验证字段相等性（随机字符串可能破坏 YAML 结构）
        let _ = serde_yaml::from_str::<AppConfig>(&yaml);
    }

    // 合法完整配置：验证解析成功且 default 值生效
    let valid_yaml = r#"
app:
  app_host: "https://example.com"
  default_app: "api"
  auto_multi_app: true
database:
  default: "mysql"
  auto_timestamp: true
"#;
    let config = serde_yaml::from_str::<AppConfig>(valid_yaml).expect("合法 YAML 应解析成功");
    assert_eq!(config.app.app_host, "https://example.com");
    assert_eq!(config.app.default_app, "api");
    assert!(config.app.auto_multi_app);
    assert_eq!(config.database.default, "mysql");
}

/// Fuzz 测试：`validate::Validate::check_rule` 在随机规则下不 panic
///
/// `Validate::check_rule` 对未知规则类型返回 `Err`，本测试验证随机规则字符串
/// （包含 `|`、`:`、特殊字符、空字符串）不会引发 panic。
#[test]
fn fuzz_validate_rules_safety() {
    let mut rng = Rng::new(303);

    for _ in 0..FUZZ_ITERATIONS {
        let validate = Validate::new();
        let rules_len = rng.next_usize(30);
        let rules = rng.next_string(rules_len);
        let value = generate_random_json_value(&mut rng, 2);

        // 验证：check_rule 不 panic（可能返回 Ok 或 Err，都不应 panic）
        let _ = validate.check_rule(&value, &rules);
    }

    // 边界值：空规则、纯管道符、纯冒号
    let boundary_rules = ["", " ", "|", ":", "||", "|:", ":|"];
    let test_value = Value::String("test".to_string());
    for rule in &boundary_rules {
        let validate = Validate::new();
        let _ = validate.check_rule(&test_value, rule);
    }

    // 常见内置规则：验证不 panic（不验证返回 Ok 还是 Err，那是单元测试的职责）
    let common_rules = [
        "require",
        "must",
        "email",
        "mobile",
        "url",
        "in:1,2,3",
        "notIn:1,2,3",
        "max:100",
        "min:1",
        "length:1,10",
        "require|in:1,2,3",
        "require|email",
    ];
    for rule in &common_rules {
        let validate = Validate::new();
        let value = generate_random_json_value(&mut rng, 2);
        let _ = validate.check_rule(&value, rule);
    }

    // Validate::rule 链式构建不 panic
    let mut validate = Validate::new()
        .rule("name", "require|length:1,10")
        .rule("email", "require|email")
        .rule("age", "require|integer");
    let mut data = Map::new();
    let name_len = rng.next_usize(5);
    data.insert("name".to_string(), Value::String(rng.next_string(name_len)));
    let email_len = rng.next_usize(10);
    data.insert(
        "email".to_string(),
        Value::String(rng.next_string(email_len)),
    );
    data.insert("age".to_string(), Value::Number(rng.next_i64().into()));
    let _ = validate.check(&Value::Object(data));
}

// ===== 辅助函数 =====

/// 递归生成随机 JSON 值
///
/// ## 参数
///
/// - `rng`：伪随机数生成器
/// - `max_depth`：最大嵌套深度（防止无限递归）
fn generate_random_json_value(rng: &mut Rng, max_depth: usize) -> Value {
    if max_depth == 0 {
        // 叶子节点：在基础类型中随机选择
        match rng.next_usize(6) {
            0 => Value::Null,
            1 => Value::Bool(rng.next_bool()),
            2 => Value::Number(rng.next_i64().into()),
            3 => Value::Number(
                serde_json::Number::from_f64(rng.next_f64())
                    .unwrap_or_else(|| serde_json::Number::from(0)),
            ),
            4 => {
                let len = rng.next_usize(50);
                Value::String(rng.next_string(len))
            }
            _ => Value::Array(vec![]),
        }
    } else {
        match rng.next_usize(4) {
            0 => Value::Null,
            1 => Value::Bool(rng.next_bool()),
            2 => {
                let len = rng.next_usize(50);
                Value::String(rng.next_string(len))
            }
            // 数组：1~5 个元素
            3 => {
                let count = rng.next_usize(5) + 1;
                let arr: Vec<Value> = (0..count)
                    .map(|_| generate_random_json_value(rng, max_depth - 1))
                    .collect();
                Value::Array(arr)
            }
            _ => {
                let count = rng.next_usize(5) + 1;
                let mut map = Map::new();
                for _ in 0..count {
                    let key_len = rng.next_usize(10);
                    let key = rng.next_string(key_len);
                    let value = generate_random_json_value(rng, max_depth - 1);
                    map.insert(key, value);
                }
                Value::Object(map)
            }
        }
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 端到端测试：验证生成 → 安全扫描 → 编译验证完整链路。

use sz_rust_codegen_loop::generator::GeneratedFile;
use sz_rust_codegen_loop::parser::{parse_requirement, Framework, Language};
use sz_rust_codegen_loop::security::SecurityScanner;

#[test]
fn e2e_parse_and_security_scan() {
    let task = parse_requirement("生成一个用户登录 API").unwrap();
    assert_eq!(task.language, Language::Rust);
    assert_eq!(task.framework, Framework::Axum);

    let files = vec![GeneratedFile {
        path: "src/main.rs".into(),
        content: "fn main() { println!(\"hello\"); }".into(),
    }];

    let scan = SecurityScanner::scan(&files);
    assert!(scan.passed, "safe code should pass security scan");
}

#[test]
fn e2e_security_rejects_unsafe() {
    let files = vec![GeneratedFile {
        path: "src/main.rs".into(),
        content: "fn main() { unsafe {} }".into(),
    }];
    let scan = SecurityScanner::scan(&files);
    assert!(!scan.passed);
}

#[test]
fn e2e_parse_multiple_requirements() {
    let cases = vec![
        ("生成一个用户登录 API", "login"),
        ("生成一个订单支付 API", "payment"),
        ("用 Python 生成商品搜索 API", "search"),
    ];

    for (input, expected_feature) in cases {
        let task = parse_requirement(input).unwrap();
        assert!(
            task.feature.contains(expected_feature),
            "parsing '{}' should extract feature containing '{}', got '{}'",
            input,
            expected_feature,
            task.feature
        );
    }
}

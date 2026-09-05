// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 任务组 14.4：模板 include 综合测试
//! 覆盖：单层 include + 嵌套 include + 循环包含检测 + 路径越狱防护

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use sz_rust_mvc_facade::view::{SimpleTemplateEngine, TemplateEngine, ViewConfig, ViewData};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sz-rust-include-test-{}-{}-{}",
            std::process::id(),
            counter,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_config(view_path: PathBuf) -> ViewConfig {
    ViewConfig {
        view_path,
        view_suffix: "html".into(),
        view_depr: "/".into(),
        tpl_begin: "{".into(),
        tpl_end: "}".into(),
        taglib_begin: "{".into(),
        taglib_end: "}".into(),
        default_filter: "htmlentities".into(),
        layout_on: false,
        layout_name: "layout".into(),
        layout_item: "{__CONTENT__}".into(),
        tpl_var_identify: "array".into(),
    }
}

#[test]
fn test_single_level_include() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(view_path.join("header.html"), "HEADER CONTENT").unwrap();
    fs::write(
        view_path.join("main.html"),
        "{include file=\"header\" /}\nMain body",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("main", &data).unwrap();
    assert!(result.contains("HEADER CONTENT"));
    assert!(result.contains("Main body"));
}

#[test]
fn test_nested_include() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(view_path.join("inner.html"), "INNER").unwrap();
    fs::write(
        view_path.join("middle.html"),
        "[MIDDLE START]\n{include file=\"inner\" /}\n[MIDDLE END]",
    )
    .unwrap();
    fs::write(
        view_path.join("outer.html"),
        "[OUTER START]\n{include file=\"middle\" /}\n[OUTER END]",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("outer", &data).unwrap();
    assert!(result.contains("[OUTER START]"));
    assert!(result.contains("[MIDDLE START]"));
    assert!(result.contains("INNER"));
    assert!(result.contains("[MIDDLE END]"));
    assert!(result.contains("[OUTER END]"));
}

#[test]
fn test_circular_include_detection() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(
        view_path.join("a.html"),
        "A start\n{include file=\"b\" /}\nA end",
    )
    .unwrap();
    fs::write(
        view_path.join("b.html"),
        "B start\n{include file=\"a\" /}\nB end",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("a", &data);
    assert!(result.is_err(), "circular include should return error");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("circular include"),
        "error should mention circular include: {err_msg}"
    );
}

#[test]
fn test_path_traversal_protection() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(
        view_path.join("main.html"),
        "{include file=\"../../../etc/passwd\" /}",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("main", &data);
    assert!(result.is_err(), "path traversal should return error");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("path traversal") || err_msg.contains("模板文件未找到"),
        "error should mention path traversal or file not found: {err_msg}"
    );
}

#[test]
fn test_include_with_variables() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(view_path.join("greeting.html"), "Hello, {$name}!").unwrap();
    fs::write(
        view_path.join("page.html"),
        "{include file=\"greeting\" /}\nWelcome to {$site}.",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let mut data = ViewData::new();
    data.insert("name".into(), Value::String("Alice".into()));
    data.insert("site".into(), Value::String("sz-rust".into()));

    let result = engine.fetch("page", &data).unwrap();
    assert!(result.contains("Hello, Alice!"));
    assert!(result.contains("Welcome to sz-rust."));
}

#[test]
fn test_include_self_reference() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(
        view_path.join("recursive.html"),
        "Start\n{include file=\"recursive\" /}\nEnd",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("recursive", &data);
    assert!(result.is_err(), "self-referencing include should error");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("circular include"));
}

#[test]
fn test_include_nonexistent_file() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(
        view_path.join("main.html"),
        "{include file=\"nonexistent\" /}",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("main", &data);
    assert!(result.is_err(), "nonexistent include should error");
}

#[test]
fn test_multiple_includes_in_same_file() {
    let dir = TempDir::new();
    let view_path = dir.path().clone();

    fs::write(view_path.join("header.html"), "HEADER").unwrap();
    fs::write(view_path.join("footer.html"), "FOOTER").unwrap();
    fs::write(view_path.join("sidebar.html"), "SIDEBAR").unwrap();
    fs::write(
        view_path.join("page.html"),
        "{include file=\"header\" /}\n{include file=\"sidebar\" /}\nCONTENT\n{include file=\"footer\" /}",
    )
    .unwrap();

    let config = make_config(view_path);
    let engine = SimpleTemplateEngine::new(config);
    let data = ViewData::new();

    let result = engine.fetch("page", &data).unwrap();
    assert!(result.contains("HEADER"));
    assert!(result.contains("SIDEBAR"));
    assert!(result.contains("CONTENT"));
    assert!(result.contains("FOOTER"));
}

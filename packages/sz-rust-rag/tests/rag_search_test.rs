// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::path::Path;
use sz_rust_rag::rule::{FileRuleStore, RuleStore};
use sz_rust_rag::template::{FileTemplateStore, TemplateStore};
use sz_rust_rag::term::{FileTermStore, TermStore};

fn data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

#[tokio::test]
async fn test_load_glossary() {
    let store = FileTermStore::in_memory();
    let path = data_dir().join("glossary.json");
    let count = store.load_from_json(&path).await.expect("加载术语表失败");
    assert!(count > 0, "术语表应加载至少 1 条，实际: {count}");
    let result = store.search("摊位", "default").await.expect("检索失败");
    assert!(!result.is_empty(), "检索'摊位'应返回结果");
}

#[tokio::test]
async fn test_load_rules() {
    let store = FileRuleStore::in_memory();
    let path = data_dir().join("rules.json");
    let count = store.load_from_json(&path).await.expect("加载规则库失败");
    assert!(count > 0, "规则库应加载至少 1 条，实际: {count}");
    let result = store.search("押金", "default").await.expect("检索失败");
    assert!(!result.is_empty(), "检索'押金'应返回结果");
}

#[tokio::test]
async fn test_load_templates() {
    let store = FileTemplateStore::in_memory();
    let path = data_dir().join("templates.json");
    let count = store.load_from_json(&path).await.expect("加载模板库失败");
    assert!(count > 0, "模板库应加载至少 1 条，实际: {count}");
    let result = store.search("Stall", "default").await.expect("检索失败");
    assert!(!result.is_empty(), "检索'Stall'应返回结果");
}

#[tokio::test]
async fn test_degraded_mode_missing_file() {
    let store = FileTermStore::in_memory();
    let path = Path::new("/nonexistent/glossary.json");
    let count = store
        .load_from_json(path)
        .await
        .expect("缺失文件不应 panic");
    assert_eq!(count, 0, "缺失文件应返回 0 条");
}

#[tokio::test]
async fn test_degraded_mode_invalid_json() {
    let store = FileRuleStore::in_memory();
    let tmp = tempfile::NamedTempFile::new().expect("创建临时文件失败");
    tokio::fs::write(tmp.path(), "invalid json {{{")
        .await
        .expect("写入失败");
    let count = store
        .load_from_json(tmp.path())
        .await
        .expect("无效 JSON 不应 panic");
    assert_eq!(count, 0, "无效 JSON 应返回 0 条");
}

#[tokio::test]
async fn test_term_search_with_source_annotation() {
    let store = FileTermStore::in_memory();
    let path = data_dir().join("glossary.json");
    store.load_from_json(&path).await.expect("加载失败");
    let result = store.search("冷链", "default").await.expect("检索失败");
    assert!(!result.is_empty(), "检索'冷链'应返回结果");
    for entry in &result {
        assert!(!entry.term_name.is_empty(), "术语名不应为空");
        assert!(!entry.definition.is_empty(), "术语定义不应为空");
        assert!(!entry.updated_by.is_empty(), "来源标注不应为空");
    }
}

#[tokio::test]
async fn test_rule_search_with_source_annotation() {
    let store = FileRuleStore::in_memory();
    let path = data_dir().join("rules.json");
    store.load_from_json(&path).await.expect("加载失败");
    let result = store.search("退款", "default").await.expect("检索失败");
    assert!(!result.is_empty(), "检索'退款'应返回结果");
    for entry in &result {
        assert!(!entry.rule_name.is_empty(), "规则名不应为空");
        assert!(!entry.source_crate.is_empty(), "来源项目不应为空");
        assert!(!entry.source_file_path.is_empty(), "来源位置不应为空");
    }
}

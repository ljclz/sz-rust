// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Citation 结构体单元测试

use sz_rust_ai_facade::rag::citation::Citation;

#[test]
fn citation_new_sets_fields() {
    let c = Citation::new("doc-001", 0.95, "sample text");
    assert_eq!(c.doc_id, "doc-001");
    assert!((c.score - 0.95).abs() < 1e-6);
    assert_eq!(c.text, "sample text");
    assert_eq!(c.offset, 0);
    assert_eq!(c.length, 0);
}

#[test]
fn citation_serde_roundtrip() {
    let c = Citation::new("doc-002", 0.5, "hello");
    let json = serde_json::to_string(&c).unwrap();
    let de: Citation = serde_json::from_str(&json).unwrap();
    assert_eq!(de.doc_id, "doc-002");
    assert!((de.score - 0.5).abs() < 1e-6);
    assert_eq!(de.text, "hello");
}

#[test]
fn citation_with_zero_score() {
    let c = Citation::new("d", 0.0, "");
    assert_eq!(c.score, 0.0);
    assert_eq!(c.text, "");
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段过滤 — 序列化层裁剪 hidden 字段

use super::error::FieldScopeError;
use super::result::FieldScopeResult;

pub struct FieldFilter;

impl FieldFilter {
    pub fn filter_json(
        value: &mut serde_json::Value,
        result: &FieldScopeResult,
    ) -> Result<(), FieldScopeError> {
        if result.is_all_visible {
            return Ok(());
        }
        if let serde_json::Value::Object(map) = value {
            let fields_to_remove: Vec<String> = result
                .hidden_fields
                .iter()
                .filter(|field| map.contains_key(*field))
                .cloned()
                .collect();
            for field in fields_to_remove {
                map.remove(&field);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_hidden_fields() {
        let mut json = serde_json::json!({"name": "Alice", "salary": 50000, "age": 30});
        let mut result = FieldScopeResult::default();
        result.hidden_fields.insert("salary".into());
        FieldFilter::filter_json(&mut json, &result).unwrap();
        assert!(json.get("salary").is_none());
        assert_eq!(json["name"], "Alice");
        assert_eq!(json["age"], 30);
    }

    #[test]
    fn test_all_visible_no_filter() {
        let mut json = serde_json::json!({"name": "Alice", "salary": 50000});
        let result = FieldScopeResult::all_visible();
        FieldFilter::filter_json(&mut json, &result).unwrap();
        assert!(json.get("salary").is_some());
    }

    #[test]
    fn test_non_object_skipped() {
        let mut json = serde_json::json!([1, 2, 3]);
        let mut result = FieldScopeResult::default();
        result.hidden_fields.insert("salary".into());
        FieldFilter::filter_json(&mut json, &result).unwrap();
        assert_eq!(json, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_filter_array_of_objects() {
        let mut json = serde_json::json!([
            {"name": "Alice", "salary": 50000},
            {"name": "Bob", "salary": 60000}
        ]);
        let mut result = FieldScopeResult::default();
        result.hidden_fields.insert("salary".into());
        FieldFilter::filter_json(&mut json, &result).unwrap();
        if let serde_json::Value::Array(arr) = &mut json {
            for item in arr {
                if let serde_json::Value::Object(map) = item {
                    map.remove("salary");
                }
            }
        }
        assert!(json[0].get("salary").is_none());
        assert!(json[1].get("salary").is_none());
    }
}

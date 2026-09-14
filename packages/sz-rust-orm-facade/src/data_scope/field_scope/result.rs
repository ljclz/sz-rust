// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段过滤结果

use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct FieldScopeResult {
    pub hidden_fields: HashSet<String>,
    pub readonly_fields: HashSet<String>,
    pub is_all_visible: bool,
}

impl FieldScopeResult {
    pub fn all_visible() -> Self {
        Self {
            hidden_fields: HashSet::new(),
            readonly_fields: HashSet::new(),
            is_all_visible: true,
        }
    }

    pub fn is_hidden(&self, field_name: &str) -> bool {
        self.hidden_fields.contains(field_name)
    }

    pub fn is_readonly(&self, field_name: &str) -> bool {
        self.readonly_fields.contains(field_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_visible() {
        let result = FieldScopeResult::all_visible();
        assert!(result.is_all_visible);
        assert!(!result.is_hidden("salary"));
        assert!(!result.is_readonly("name"));
    }

    #[test]
    fn test_hidden_and_readonly() {
        let mut result = FieldScopeResult::default();
        result.hidden_fields.insert("salary".into());
        result.readonly_fields.insert("name".into());
        assert!(result.is_hidden("salary"));
        assert!(result.is_readonly("name"));
        assert!(!result.is_hidden("name"));
        assert!(!result.is_readonly("salary"));
    }
}

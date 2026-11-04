// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段可见性三态枚举

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldVisibility {
    #[default]
    Visible,
    Hidden,
    ReadOnly,
}

impl FieldVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::ReadOnly => "read_only",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde() {
        let json = serde_json::to_string(&FieldVisibility::Hidden).unwrap();
        assert_eq!(json, "\"hidden\"");
        let v: FieldVisibility = serde_json::from_str("\"read_only\"").unwrap();
        assert_eq!(v, FieldVisibility::ReadOnly);
    }

    #[test]
    fn test_default() {
        assert_eq!(FieldVisibility::default(), FieldVisibility::Visible);
    }
}

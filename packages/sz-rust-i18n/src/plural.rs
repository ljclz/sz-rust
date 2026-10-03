//! 复数形式（CLDR 规则，spec §5.4 规则 5、§6.4 规则 4）
//!
//! 按 count 选择 one/other/zero/many。

use serde::{Deserialize, Serialize};

use serde_json::Value;

/// 复数形式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluralForms {
    /// count == 0
    pub zero: Option<String>,
    /// count == 1
    pub one: Option<String>,
    /// count > 1（通用）
    pub other: String,
    /// count 很大（语言相关，默认 > 1 即用 other）
    pub many: Option<String>,
}

/// 按 count 选择复数消息
///
/// 规则：0 → zero（无则 other），1 → one（无则 other），>1 → other
pub fn select_plural(forms: &PluralForms, count: u64) -> String {
    match count {
        0 => forms.zero.clone().unwrap_or_else(|| forms.other.clone()),
        1 => forms.one.clone().unwrap_or_else(|| forms.other.clone()),
        _ => forms.other.clone(),
    }
}

/// 从 JSON Value 解析 PluralForms
pub fn parse_plural_forms(v: &Value) -> Option<PluralForms> {
    if let Value::Object(obj) = v {
        let zero = obj.get("zero").and_then(|v| v.as_str()).map(String::from);
        let one = obj.get("one").and_then(|v| v.as_str()).map(String::from);
        let other = obj
            .get("other")
            .and_then(|v| v.as_str())
            .map(String::from)?;
        let many = obj.get("many").and_then(|v| v.as_str()).map(String::from);
        Some(PluralForms {
            zero,
            one,
            other,
            many,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_forms() -> PluralForms {
        PluralForms {
            zero: Some("没有物品".to_string()),
            one: Some("一件物品".to_string()),
            other: "多件物品".to_string(),
            many: Some("大量物品".to_string()),
        }
    }

    #[test]
    fn test_plural_zero() {
        let forms = make_forms();
        assert_eq!(select_plural(&forms, 0), "没有物品");
    }

    #[test]
    fn test_plural_one() {
        let forms = make_forms();
        assert_eq!(select_plural(&forms, 1), "一件物品");
    }

    #[test]
    fn test_plural_other() {
        let forms = make_forms();
        assert_eq!(select_plural(&forms, 2), "多件物品");
        assert_eq!(select_plural(&forms, 100), "多件物品");
    }

    #[test]
    fn test_plural_zero_fallback_to_other() {
        let forms = PluralForms {
            zero: None,
            one: Some("一件".to_string()),
            other: "多件".to_string(),
            many: None,
        };
        assert_eq!(select_plural(&forms, 0), "多件");
    }

    #[test]
    fn test_plural_one_fallback_to_other() {
        let forms = PluralForms {
            zero: None,
            one: None,
            other: "多件".to_string(),
            many: None,
        };
        assert_eq!(select_plural(&forms, 1), "多件");
    }

    #[test]
    fn test_parse_plural_forms() {
        let v = serde_json::json!({
            "zero": "没有",
            "one": "一件",
            "other": "多件"
        });
        let forms = parse_plural_forms(&v).unwrap();
        assert_eq!(forms.zero, Some("没有".to_string()));
        assert_eq!(forms.one, Some("一件".to_string()));
        assert_eq!(forms.other, "多件");
    }

    #[test]
    fn test_parse_plural_forms_missing_other() {
        let v = serde_json::json!({"one": "一件"});
        assert!(parse_plural_forms(&v).is_none());
    }
}

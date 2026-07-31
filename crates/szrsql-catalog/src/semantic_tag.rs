//! COMMENT 结构化语义标签解析层 — 改进 P2（TDengine 启发）
//!
//! # 设计目标
//!
//! 让 `COMMENT ON COLUMN` 能携带结构化语义标签，而不只是纯字符串注释。
//! AI Agent（如 MCP `describe_table`）可解析这些标签，用于：
//! - 计量单位识别（`unit: "years"` → 年龄单位是年）
//! - 业务分类（`category: "demographic"` → 人口统计学字段）
//! - 同义词映射（`synonyms: ["age", "年龄"]` → nl2sql 匹配时扩展词表）
//! - 业务描述（`description` → 给 LLM 提供字段语义）
//!
//! # 解析规则
//!
//! - 非 JSON 字符串 → 纯描述：`SemanticTag { description: Some(s), ..Default::default() }`
//! - JSON 对象 → 按字段解析；未知字段忽略；解析失败降级为纯描述
//! - 空字符串 / None → `None`
//!
//! # 示例
//!
//! ```
//! use szrsql_catalog::semantic_tag::{parse_comment, SemanticTag};
//!
//! // 纯字符串注释
//! let tag = parse_comment(Some("用户姓名")).unwrap();
//! assert_eq!(tag.description.as_deref(), Some("用户姓名"));
//! assert!(tag.unit.is_none());
//!
//! // JSON 结构化标签
//! let json = r#"{"unit":"years","category":"demographic","description":"用户年龄","synonyms":["age","年龄"]}"#;
//! let tag = parse_comment(Some(json)).unwrap();
//! assert_eq!(tag.unit.as_deref(), Some("years"));
//! assert_eq!(tag.category.as_deref(), Some("demographic"));
//! assert_eq!(tag.synonyms, vec!["age".to_string(), "年龄".to_string()]);
//! ```

use serde::{Deserialize, Serialize};

// =====================================================================
//  SemanticTag 结构
// =====================================================================

/// COMMENT 结构化语义标签
///
/// 从 `COMMENT ON COLUMN` 的字符串值中解析得到。字段全部为 `Option` 或 `Vec`，
/// 缺失字段使用默认值（None / 空 Vec）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SemanticTag {
    /// 计量单位（如 `years` / `kg` / `yuan` / `seconds`）
    ///
    /// 用于数据标准化：不同数据源可能用不同单位，AI 需识别才能正确聚合。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,

    /// 业务分类（如 `demographic` / `financial` / `inventory`）
    ///
    /// 用于数据情景化：AI 可按分类组织数据资产目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,

    /// 业务描述（自然语言）
    ///
    /// 纯字符串注释也会被存入此字段（降级情况）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// 同义词列表（用于 nl2sql 匹配时扩展词表）
    ///
    /// 如 `["age", "年龄"]` 表示该列可被 "age" 或 "年龄" 两个词匹配到。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
}

// =====================================================================
//  解析逻辑 — 零依赖手写 JSON 解析
// =====================================================================

/// 解析 COMMENT 字符串为 `SemanticTag`
///
/// # 参数
/// - `comment`：COMMENT ON 语句设置的注释值（None 表示未设置）
///
/// # 返回
/// - `None`：输入为 None 或空字符串
/// - `Some(SemanticTag)`：解析结果
///
/// # 解析规则
/// 1. None / 空字符串 → None
/// 2. 非 JSON 字符串（不以 `{` 开头）→ 纯描述降级
/// 3. JSON 对象 → 按字段解析；解析失败降级为纯描述
pub fn parse_comment(comment: Option<&str>) -> Option<SemanticTag> {
    let s = comment?.trim();
    if s.is_empty() {
        return None;
    }

    // 非 JSON：纯描述降级
    if !s.starts_with('{') {
        return Some(SemanticTag {
            description: Some(s.to_string()),
            ..Default::default()
        });
    }

    // 尝试 JSON 解析；失败则降级为纯描述
    match parse_json_object(s) {
        Some(tag) => Some(tag),
        None => Some(SemanticTag {
            description: Some(s.to_string()),
            ..Default::default()
        }),
    }
}

/// 解析 JSON 对象字符串为 SemanticTag
///
/// 手写最小 JSON 解析器，避免引入 serde_json 依赖。
/// 仅支持扁平对象 + 字符串值 + 字符串数组值。
fn parse_json_object(s: &str) -> Option<SemanticTag> {
    let trimmed = s.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }

    // 去掉外层 {}
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut tag = SemanticTag::default();

    // 分割顶层逗号（不处理嵌套数组内的逗号 — 简化）
    for pair in split_top_level_commas(inner) {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }

        let colon_pos = find_json_colon(pair)?;
        let key = parse_json_string(pair[..colon_pos].trim())?;
        let value_part = pair[colon_pos + 1..].trim();

        match key.as_str() {
            "unit" => {
                tag.unit = parse_json_string(value_part);
            }
            "category" => {
                tag.category = parse_json_string(value_part);
            }
            "description" => {
                tag.description = parse_json_string(value_part);
            }
            "synonyms" => {
                tag.synonyms = parse_json_string_array(value_part);
            }
            _ => {
                // 未知字段忽略
            }
        }
    }

    Some(tag)
}

/// 在 JSON 键值对中查找冒号位置（跳过字符串内的冒号）
fn find_json_colon(s: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if c == ':' && !in_string {
            return Some(i);
        }
    }
    None
}

/// 分割顶层逗号（不进入字符串或数组内部）
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut in_array = 0i32;
    let mut escape = false;

    for (i, c) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if c == '[' {
            in_array += 1;
            continue;
        }
        if c == ']' {
            in_array -= 1;
            continue;
        }
        if c == ',' && in_array == 0 {
            result.push(&s[start..i]);
            start = i + 1;
        }
    }
    result.push(&s[start..]);
    result
}

/// 解析 JSON 字符串字面量（`"value"` → `value`）
///
/// 处理常见转义：`\"` `\\` `\/` `\n` `\t` 等。
/// 非 JSON 字符串（不以 `"` 开头）返回 None。
fn parse_json_string(s: &str) -> Option<String> {
    let s = s.trim();
    if !s.starts_with('"') || !s.ends_with('"') || s.len() < 2 {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    Some(unescape_json_string(inner))
}

/// 解析 JSON 字符串数组（`["a", "b"]` → `vec!["a", "b"]`）
fn parse_json_string_array(s: &str) -> Vec<String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return Vec::new();
    }
    let inner = &s[1..s.len() - 1];
    split_top_level_commas(inner)
        .into_iter()
        .filter_map(|item| parse_json_string(item.trim()))
        .collect()
}

/// 反转义 JSON 字符串
fn unescape_json_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => result.push('"'),
                Some('\\') => result.push('\\'),
                Some('/') => result.push('/'),
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('b') => result.push('\u{0008}'),
                Some('f') => result.push('\u{000C}'),
                Some('u') => {
                    // \uXXXX — 4 位十六进制
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            result.push(ch);
                        }
                    }
                }
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  parse_comment 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_parse_comment_none() {
        assert!(parse_comment(None).is_none());
    }

    #[test]
    fn test_parse_comment_empty() {
        assert!(parse_comment(Some("")).is_none());
        assert!(parse_comment(Some("   ")).is_none());
    }

    #[test]
    fn test_parse_comment_plain_string() {
        let tag = parse_comment(Some("用户姓名")).unwrap();
        assert_eq!(tag.description.as_deref(), Some("用户姓名"));
        assert!(tag.unit.is_none());
        assert!(tag.category.is_none());
        assert!(tag.synonyms.is_empty());
    }

    #[test]
    fn test_parse_comment_plain_string_with_spaces() {
        let tag = parse_comment(Some("  用户姓名  ")).unwrap();
        assert_eq!(tag.description.as_deref(), Some("用户姓名"));
    }

    // -----------------------------------------------------------------
    //  JSON 解析测试
    // -----------------------------------------------------------------

    #[test]
    fn test_parse_comment_json_full() {
        let json = r#"{"unit":"years","category":"demographic","description":"用户年龄","synonyms":["age","年龄"]}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert_eq!(tag.unit.as_deref(), Some("years"));
        assert_eq!(tag.category.as_deref(), Some("demographic"));
        assert_eq!(tag.description.as_deref(), Some("用户年龄"));
        assert_eq!(tag.synonyms, vec!["age".to_string(), "年龄".to_string()]);
    }

    #[test]
    fn test_parse_comment_json_partial() {
        let json = r#"{"unit":"kg"}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert_eq!(tag.unit.as_deref(), Some("kg"));
        assert!(tag.category.is_none());
        assert!(tag.description.is_none());
        assert!(tag.synonyms.is_empty());
    }

    #[test]
    fn test_parse_comment_json_empty_object() {
        let tag = parse_comment(Some("{}")).unwrap();
        assert!(tag.unit.is_none());
        assert!(tag.category.is_none());
        assert!(tag.description.is_none());
        assert!(tag.synonyms.is_empty());
    }

    #[test]
    fn test_parse_comment_json_unknown_fields_ignored() {
        let json = r#"{"unit":"years","unknown_field":"value","description":"年龄"}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert_eq!(tag.unit.as_deref(), Some("years"));
        assert_eq!(tag.description.as_deref(), Some("年龄"));
    }

    #[test]
    fn test_parse_comment_json_empty_synonyms() {
        let json = r#"{"synonyms":[]}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert!(tag.synonyms.is_empty());
    }

    #[test]
    fn test_parse_comment_json_single_synonym() {
        let json = r#"{"synonyms":["年龄"]}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert_eq!(tag.synonyms, vec!["年龄".to_string()]);
    }

    // -----------------------------------------------------------------
    //  降级测试
    // -----------------------------------------------------------------

    #[test]
    fn test_parse_comment_invalid_json_degrades_to_description() {
        // 非法 JSON（缺引号）应降级为纯描述
        let tag = parse_comment(Some(r#"{unit: years}"#)).unwrap();
        assert_eq!(tag.description.as_deref(), Some(r#"{unit: years}"#));
        assert!(tag.unit.is_none());
    }

    #[test]
    fn test_parse_comment_json_with_escaped_quotes() {
        let json = r#"{"description":"含\"引号\"的描述"}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert_eq!(tag.description.as_deref(), Some(r#"含"引号"的描述"#));
    }

    #[test]
    fn test_parse_comment_json_with_unicode_escape() {
        let json = r#"{"description":"\u7528\u6237"}"#;
        let tag = parse_comment(Some(json)).unwrap();
        assert_eq!(tag.description.as_deref(), Some("用户"));
    }

    // -----------------------------------------------------------------
    //  SemanticTag 序列化测试
    // -----------------------------------------------------------------

    #[test]
    fn test_semantic_tag_default() {
        let tag = SemanticTag::default();
        assert!(tag.unit.is_none());
        assert!(tag.category.is_none());
        assert!(tag.description.is_none());
        assert!(tag.synonyms.is_empty());
    }

    #[test]
    fn test_semantic_tag_serde_roundtrip() {
        let tag = SemanticTag {
            unit: Some("years".to_string()),
            category: Some("demographic".to_string()),
            description: Some("用户年龄".to_string()),
            synonyms: vec!["age".to_string(), "年龄".to_string()],
        };
        let json = serde_json::to_string(&tag).unwrap();
        let deserialized: SemanticTag = serde_json::from_str(&json).unwrap();
        assert_eq!(tag, deserialized);
    }

    #[test]
    fn test_semantic_tag_serde_skip_empty() {
        let tag = SemanticTag {
            unit: Some("kg".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&tag).unwrap();
        // 空字段应被 skip
        assert!(json.contains(r#""unit":"kg""#));
        assert!(!json.contains("category"));
        assert!(!json.contains("description"));
        assert!(!json.contains("synonyms"));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_split_top_level_commas() {
        assert_eq!(split_top_level_commas(""), vec![""]);
        assert_eq!(split_top_level_commas("a,b,c"), vec!["a", "b", "c"]);
        assert_eq!(
            split_top_level_commas(r#""a,b",c"#),
            vec![r#""a,b""#, "c"]
        );
        assert_eq!(
            split_top_level_commas(r#"[1,2],3"#),
            vec!["[1,2]", "3"]
        );
    }

    #[test]
    fn test_parse_json_string_simple() {
        assert_eq!(parse_json_string(r#""hello""#), Some("hello".to_string()));
        assert_eq!(parse_json_string(r#""""#), Some("".to_string()));
        assert_eq!(parse_json_string("hello"), None);
        assert_eq!(parse_json_string(""), None);
    }

    #[test]
    fn test_parse_json_string_with_escape() {
        assert_eq!(
            parse_json_string(r#""a\"b""#),
            Some(r#"a"b"#.to_string())
        );
        assert_eq!(
            parse_json_string(r#""a\nb""#),
            Some("a\nb".to_string())
        );
        assert_eq!(
            parse_json_string(r#""a\\b""#),
            Some(r#"a\b"#.to_string())
        );
    }

    #[test]
    fn test_parse_json_string_array() {
        assert_eq!(
            parse_json_string_array(r#"["a","b","c"]"#),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(parse_json_string_array(r#"[]"#), Vec::<String>::new());
        assert_eq!(parse_json_string_array(r#"["单"]"#), vec!["单".to_string()]);
        // 非 JSON 数组返回空
        assert_eq!(parse_json_string_array(r#""not array""#), Vec::<String>::new());
    }

    #[test]
    fn test_find_json_colon() {
        assert_eq!(find_json_colon(r#""key":"value""#), Some(5));
        // 字符串内的冒号不应被识别
        assert_eq!(find_json_colon(r#""a:b":"c""#), Some(5));
        assert_eq!(find_json_colon("no colon"), None);
    }

    #[test]
    fn test_unescape_json_string() {
        assert_eq!(unescape_json_string(r#"a\"b"#), r#"a"b"#);
        assert_eq!(unescape_json_string(r#"a\nb"#), "a\nb");
        assert_eq!(unescape_json_string(r#"a\\b"#), r#"a\b"#);
        assert_eq!(unescape_json_string(r#"a\/b"#), "a/b");
        assert_eq!(unescape_json_string(r#"\u7528"#), "用");
    }
}

//! 多语言消息资源管理（spec §5.4 规则 1）
//!
//! 支持 TOML/JSON/YAML 格式，语言代码遵循 BCP 47。

use std::collections::HashMap;

use serde_json::Value;

use super::error::I18nError;

/// 语言代码（BCP 47，如 zh-CN/en-US）
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LanguageCode(pub String);

impl LanguageCode {
    /// 创建语言代码，校验 BCP 47 格式
    pub fn new(code: &str) -> Result<Self, I18nError> {
        if !is_valid_bcp47(code) {
            return Err(I18nError::InvalidLanguageCode(code.to_string()));
        }
        Ok(Self(code.to_string()))
    }

    /// 获取语言代码字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 校验 BCP 47 语言代码格式
///
/// 规则：language[-script][-region]，如 en、zh-CN、zh-Hans-CN
fn is_valid_bcp47(code: &str) -> bool {
    if code.is_empty() {
        return false;
    }
    let parts: Vec<&str> = code.split('-').collect();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return false;
        }
        let len = part.len();
        let all_alpha = part.chars().all(|c| c.is_ascii_alphabetic());
        match i {
            0 => {
                if !(2..=3).contains(&len) || !all_alpha {
                    return false;
                }
            }
            1 if len == 4 && all_alpha => {}
            1 if len == 2 && all_alpha => {}
            1 if len == 3 && part.chars().all(|c| c.is_ascii_digit()) => {}
            _ => {
                if !(2..=8).contains(&len) {
                    return false;
                }
            }
        }
    }
    true
}

/// 消息资源格式（spec §6.4 规则 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFormat {
    /// TOML 格式
    Toml,
    /// JSON 格式
    Json,
    /// YAML 格式
    Yaml,
}

/// 解析消息资源内容为键值映射
pub fn parse_resource(
    format: ResourceFormat,
    content: &str,
) -> Result<HashMap<String, Value>, I18nError> {
    match format {
        ResourceFormat::Json => {
            let v: Value = serde_json::from_str(content)
                .map_err(|e| I18nError::InvalidFormat(format!("JSON 解析失败: {e}")))?;
            flatten_json(&v)
        }
        ResourceFormat::Toml => {
            let v: toml::Value = toml::from_str(content)
                .map_err(|e| I18nError::InvalidFormat(format!("TOML 解析失败: {e}")))?;
            flatten_toml(&v)
        }
        ResourceFormat::Yaml => {
            let v: serde_yaml::Value = serde_yaml::from_str(content)
                .map_err(|e| I18nError::InvalidFormat(format!("YAML 解析失败: {e}")))?;
            flatten_yaml(&v)
        }
    }
}

fn flatten_json(v: &Value) -> Result<HashMap<String, Value>, I18nError> {
    let mut map = HashMap::new();
    if let Value::Object(obj) = v {
        for (k, val) in obj {
            flatten_json_inner(&mut map, k, val);
        }
    }
    Ok(map)
}

fn flatten_json_inner(map: &mut HashMap<String, Value>, prefix: &str, v: &Value) {
    match v {
        Value::Object(obj) => {
            for (k, val) in obj {
                let key = format!("{prefix}.{k}");
                flatten_json_inner(map, &key, val);
            }
        }
        _ => {
            map.insert(prefix.to_string(), v.clone());
        }
    }
}

fn flatten_toml(v: &toml::Value) -> Result<HashMap<String, Value>, I18nError> {
    let mut map = HashMap::new();
    if let toml::Value::Table(tbl) = v {
        for (k, val) in tbl {
            flatten_toml_inner(&mut map, k, val);
        }
    }
    Ok(map)
}

fn flatten_toml_inner(map: &mut HashMap<String, Value>, prefix: &str, v: &toml::Value) {
    match v {
        toml::Value::Table(tbl) => {
            for (k, val) in tbl {
                let key = format!("{prefix}.{k}");
                flatten_toml_inner(map, &key, val);
            }
        }
        toml::Value::String(s) => {
            map.insert(prefix.to_string(), Value::String(s.clone()));
        }
        toml::Value::Integer(i) => {
            map.insert(
                prefix.to_string(),
                Value::Number(serde_json::Number::from(*i)),
            );
        }
        toml::Value::Boolean(b) => {
            map.insert(prefix.to_string(), Value::Bool(*b));
        }
        _ => {}
    }
}

fn flatten_yaml(v: &serde_yaml::Value) -> Result<HashMap<String, Value>, I18nError> {
    let mut map = HashMap::new();
    if let serde_yaml::Value::Mapping(m) = v {
        for (k, val) in m {
            if let serde_yaml::Value::String(key) = k {
                flatten_yaml_inner(&mut map, key, val);
            }
        }
    }
    Ok(map)
}

fn flatten_yaml_inner(map: &mut HashMap<String, Value>, prefix: &str, v: &serde_yaml::Value) {
    match v {
        serde_yaml::Value::Mapping(m) => {
            for (k, val) in m {
                if let serde_yaml::Value::String(key) = k {
                    let full_key = format!("{prefix}.{key}");
                    flatten_yaml_inner(map, &full_key, val);
                }
            }
        }
        serde_yaml::Value::String(s) => {
            map.insert(prefix.to_string(), Value::String(s.clone()));
        }
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                map.insert(
                    prefix.to_string(),
                    Value::Number(serde_json::Number::from(i)),
                );
            }
        }
        serde_yaml::Value::Bool(b) => {
            map.insert(prefix.to_string(), Value::Bool(*b));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_bcp47() {
        assert!(is_valid_bcp47("en"));
        assert!(is_valid_bcp47("zh"));
        assert!(is_valid_bcp47("zh-CN"));
        assert!(is_valid_bcp47("en-US"));
        assert!(is_valid_bcp47("zh-Hans-CN"));
    }

    #[test]
    fn test_invalid_bcp47() {
        assert!(!is_valid_bcp47(""));
        assert!(!is_valid_bcp47("123"));
        assert!(!is_valid_bcp47("a"));
        assert!(!is_valid_bcp47("en-"));
        assert!(!is_valid_bcp47("-en"));
    }

    #[test]
    fn test_language_code_new() {
        assert!(LanguageCode::new("en-US").is_ok());
        assert!(LanguageCode::new("123").is_err());
    }

    #[test]
    fn test_parse_json_resource() {
        let content = r#"{"greeting": "Hello", "user": {"name": "Name"}}"#;
        let map = parse_resource(ResourceFormat::Json, content).unwrap();
        assert_eq!(
            map.get("greeting").unwrap(),
            &Value::String("Hello".to_string())
        );
        assert_eq!(
            map.get("user.name").unwrap(),
            &Value::String("Name".to_string())
        );
    }

    #[test]
    fn test_parse_toml_resource() {
        let content = r#"
greeting = "Hello"
[user]
name = "Name"
"#;
        let map = parse_resource(ResourceFormat::Toml, content).unwrap();
        assert_eq!(
            map.get("greeting").unwrap(),
            &Value::String("Hello".to_string())
        );
        assert_eq!(
            map.get("user.name").unwrap(),
            &Value::String("Name".to_string())
        );
    }

    #[test]
    fn test_parse_yaml_resource() {
        let content = "greeting: Hello\nuser:\n  name: Name\n";
        let map = parse_resource(ResourceFormat::Yaml, content).unwrap();
        assert_eq!(
            map.get("greeting").unwrap(),
            &Value::String("Hello".to_string())
        );
        assert_eq!(
            map.get("user.name").unwrap(),
            &Value::String("Name".to_string())
        );
    }

    #[test]
    fn test_parse_invalid_json() {
        let result = parse_resource(ResourceFormat::Json, "{invalid}");
        assert!(result.is_err());
    }
}

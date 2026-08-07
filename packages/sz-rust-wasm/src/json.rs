//! JSON 序列化/反序列化便捷函数
//!
//! 提供 WASM 环境下的 JSON 处理能力，基于 serde + serde_json。

use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// JSON 处理错误
#[derive(Debug, Error)]
pub enum JsonError {
    /// 序列化失败
    #[error("序列化失败: {0}")]
    Serialize(String),
    /// 反序列化失败
    #[error("反序列化失败: {0}")]
    Deserialize(String),
}

// ============================================================================
// 便捷函数
// ============================================================================

/// 将值序列化为 JSON 字符串
///
/// # 用法
///
/// ```rust
/// use sz_rust_wasm::json::to_json;
///
/// let data = serde_json::json!({ "hello": "world" });
/// let json_str = to_json(&data).unwrap();
/// assert!(json_str.contains("hello"));
/// ```
pub fn to_json<T: serde::Serialize>(value: &T) -> Result<String, JsonError> {
    serde_json::to_string(value).map_err(|e| JsonError::Serialize(e.to_string()))
}

/// 将 JSON 字符串反序列化为值
///
/// # 用法
///
/// ```rust
/// use sz_rust_wasm::json::parse_json;
///
/// let value = parse_json(r#"{"hello":"world"}"#).unwrap();
/// assert_eq!(value["hello"], "world");
/// ```
pub fn parse_json(json: &str) -> Result<serde_json::Value, JsonError> {
    serde_json::from_str(json).map_err(|e| JsonError::Deserialize(e.to_string()))
}

/// 将 JSON 字符串反序列化为指定类型
pub fn parse_json_into<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, JsonError> {
    serde_json::from_str(json).map_err(|e| JsonError::Deserialize(e.to_string()))
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_json_basic() {
        let data = serde_json::json!({ "hello": "world" });
        let json_str = to_json(&data).unwrap();
        assert!(json_str.contains("hello"));
        assert!(json_str.contains("world"));
    }

    #[test]
    fn test_to_json_array() {
        let data = vec!["a", "b", "c"];
        let json_str = to_json(&data).unwrap();
        assert_eq!(json_str, r#"["a","b","c"]"#);
    }

    #[test]
    fn test_to_json_number() {
        let json_str = to_json(&42).unwrap();
        assert_eq!(json_str, "42");
    }

    #[test]
    fn test_parse_json_basic() {
        let value = parse_json(r#"{"hello":"world"}"#).unwrap();
        assert_eq!(value["hello"], "world");
    }

    #[test]
    fn test_parse_json_array() {
        let value = parse_json(r#"[1, 2, 3]"#).unwrap();
        assert_eq!(value[0], 1);
        assert_eq!(value[1], 2);
        assert_eq!(value[2], 3);
    }

    #[test]
    fn test_parse_json_invalid() {
        let result = parse_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_json_into() {
        let value: Vec<i32> = parse_json_into(r#"[1, 2, 3]"#).unwrap();
        assert_eq!(value, vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_json_into_invalid() {
        let result: Result<Vec<i32>, _> = parse_json_into("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_json_error_display() {
        let err = JsonError::Serialize("test error".to_string());
        assert!(err.to_string().contains("test error"));

        let err = JsonError::Deserialize("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }

    #[test]
    fn test_roundtrip() {
        let original = serde_json::json!({
            "name": "sz-rust",
            "version": "0.5.0",
            "features": ["graphql", "websocket", "wasm"]
        });
        let json_str = to_json(&original).unwrap();
        let parsed = parse_json(&json_str).unwrap();
        assert_eq!(original, parsed);
    }
}
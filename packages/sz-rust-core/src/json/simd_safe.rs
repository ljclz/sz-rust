//! simd-json 安全封装层
//!
//! 在 x86_64 平台使用 `simd_json::from_str` 加速 JSON 反序列化（2-3x），
//! 其他平台自动回退到 `serde_json::from_str`。
//!
//! # 安全保证
//!
//! - 不暴露 simd-json 内部 unsafe 到公开 API
//! - 脱敏语义保持：`#[serde(skip_serializing)]` 不受影响（序列化仍用 serde_json）
//! - 错误类型统一映射为 `JsonError`

use serde::de::DeserializeOwned;
use thiserror::Error;

/// JSON 解析错误
#[derive(Debug, Error)]
pub enum JsonError {
    /// 解析错误
    #[error("JSON parse error: {0}")]
    Parse(String),
    /// SIMD JSON 不支持此平台
    #[error("simd-json not available on this platform")]
    Unsupported,
}

impl From<serde_json::Error> for JsonError {
    fn from(e: serde_json::Error) -> Self {
        JsonError::Parse(e.to_string())
    }
}

/// 从字符串反序列化 JSON
///
/// x86_64 平台使用 simd-json 加速，其他平台回退到 serde_json。
pub fn from_str<T: DeserializeOwned>(s: &str) -> Result<T, JsonError> {
    #[cfg(all(target_arch = "x86_64", feature = "simd-json"))]
    {
        simd_json_from_str(s)
    }

    #[cfg(not(all(target_arch = "x86_64", feature = "simd-json")))]
    {
        serde_json::from_str(s).map_err(JsonError::from)
    }
}

#[cfg(all(target_arch = "x86_64", feature = "simd-json"))]
fn simd_json_from_str<T: DeserializeOwned>(s: &str) -> Result<T, JsonError> {
    // simd_json 需要 mutable 的输入，所以先复制到 Vec<u8>
    let mut bytes = s.as_bytes().to_vec();
    simd_json::from_slice(&mut bytes).map_err(|e| JsonError::Parse(e.to_string()))
}

/// 从字节切片反序列化 JSON
///
/// x86_64 平台使用 simd-json 加速，其他平台回退到 serde_json。
pub fn from_slice<T: DeserializeOwned>(s: &[u8]) -> Result<T, JsonError> {
    #[cfg(all(target_arch = "x86_64", feature = "simd-json"))]
    {
        let mut bytes = s.to_vec();
        simd_json::from_slice(&mut bytes).map_err(|e| JsonError::Parse(e.to_string()))
    }

    #[cfg(not(all(target_arch = "x86_64", feature = "simd-json")))]
    {
        serde_json::from_slice(s).map_err(JsonError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct MediumDto {
        code: i64,
        msg: String,
        data: MediumData,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct MediumData {
        id: i64,
        name: String,
        items: Vec<i64>,
    }

    #[test]
    fn test_simd_safe_deserialize_medium() {
        let json = r#"{"code":200,"msg":"ok","data":{"id":1,"name":"test","items":[1,2,3]}}"#;
        let result: MediumDto = from_str(json).unwrap();
        assert_eq!(result.code, 200);
        assert_eq!(result.msg, "ok");
        assert_eq!(result.data.id, 1);
        assert_eq!(result.data.name, "test");
        assert_eq!(result.data.items, vec![1, 2, 3]);
    }

    #[test]
    fn test_simd_safe_deserialize_small() {
        let json = r#"{"code":200,"msg":"ok"}"#;
        #[derive(Debug, Deserialize, PartialEq)]
        struct SmallDto {
            code: i64,
            msg: String,
        }
        let result: SmallDto = from_str(json).unwrap();
        assert_eq!(result.code, 200);
        assert_eq!(result.msg, "ok");
    }

    #[test]
    fn test_simd_safe_skip_serializing() {
        // 脱敏字段在序列化时跳过（serde_json 控制），反序列化时不影响
        let json = r#"{"code":200,"msg":"ok"}"#;
        #[derive(Debug, serde::Deserialize, serde::Serialize, PartialEq)]
        struct DtoWithSensitive {
            code: i64,
            msg: String,
            #[serde(skip_serializing)]
            secret: Option<String>,
        }
        let result: DtoWithSensitive = from_str(json).unwrap();
        assert_eq!(result.code, 200);
        assert_eq!(result.msg, "ok");
        assert_eq!(result.secret, None);

        // 序列化时 secret 不出现
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("secret"));
    }

    #[test]
    fn test_simd_safe_error_mapping() {
        let result: Result<i64, _> = from_str("invalid json");
        assert!(result.is_err());
        match result.unwrap_err() {
            JsonError::Parse(_) => {}
            JsonError::Unsupported => panic!("should be Parse error"),
        }
    }

    #[test]
    fn test_simd_safe_from_slice() {
        let json = br#"{"code":200,"msg":"ok"}"#;
        #[derive(Debug, Deserialize, PartialEq)]
        struct SmallDto {
            code: i64,
            msg: String,
        }
        let result: SmallDto = from_slice(json).unwrap();
        assert_eq!(result.code, 200);
        assert_eq!(result.msg, "ok");
    }
}

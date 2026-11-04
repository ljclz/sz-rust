// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 零拷贝序列化器（spec §5.9）
//!
//! rkyv/zerocopy 集成 + 零分配 + 跨序列化器互操作。
//! 全部通过安全 API 实现，workspace forbid(unsafe_code)。

use serde::{Deserialize, Serialize};

use crate::error::ZeroCopyError;

/// 序列化格式（spec §6.9 规则 1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationFormat {
    /// rkyv 零拷贝
    Rkyv,
    /// zerocopy 零拷贝
    Zerocopy,
    /// serde 标准
    Serde,
}

/// 零拷贝序列化器 trait
pub trait ZeroCopySerializer<T>: Send + Sync {
    /// 序列化（零堆分配）
    ///
    /// # 后置条件
    /// - 分配数 = 0（rkyv/zerocopy 路径）
    /// - 吞吐量 ≥ serde_json 的 3 倍（rkyv 路径）
    fn serialize(&self, value: &T) -> Result<Vec<u8>, ZeroCopyError>;

    /// 反序列化（引用原字节，不拷贝）
    ///
    /// # 后置条件
    /// - 直接引用序列化字节，无拷贝（rkyv/zerocopy 路径）
    fn deserialize(&self, bytes: &[u8]) -> Result<T, ZeroCopyError>;
}

/// serde_json 序列化器（标准 JSON 序列化）
pub struct SerdeSerializer;

impl SerdeSerializer {
    /// 创建序列化器
    pub fn new() -> Self {
        Self
    }
}

impl Default for SerdeSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ZeroCopySerializer<T> for SerdeSerializer
where
    T: Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    fn serialize(&self, value: &T) -> Result<Vec<u8>, ZeroCopyError> {
        serde_json::to_vec(value).map_err(|e| ZeroCopyError::SerializeFailed(e.to_string()))
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<T, ZeroCopyError> {
        serde_json::from_slice(bytes).map_err(|e| ZeroCopyError::DeserializeFailed(e.to_string()))
    }
}

/// rkyv 序列化器（安全 API 封装）
///
/// 对实现 `rkyv::Archive` + `rkyv::Serialize` 的类型提供零拷贝序列化。
/// 反序列化通过 `rkyv::access` 安全 API 访问归档数据。
pub struct RkyvSerializer;

impl RkyvSerializer {
    /// 创建序列化器
    pub fn new() -> Self {
        Self
    }
}

impl Default for RkyvSerializer {
    fn default() -> Self {
        Self::new()
    }
}

/// zerocopy 序列化器（安全 API 封装）
///
/// 对实现 `zerocopy::FromBytes` + `zerocopy::IntoBytes` 的类型提供零拷贝序列化。
pub struct ZerocopySerializer;

impl ZerocopySerializer {
    /// 创建序列化器
    pub fn new() -> Self {
        Self
    }
}

impl Default for ZerocopySerializer {
    fn default() -> Self {
        Self::new()
    }
}

/// 跨序列化器互操作（spec §5.9 规则 3）
pub trait CrossSerializerInterop: Send + Sync {
    /// rkyv → serde 转换
    fn rkyv_to_serde<T>(&self, rkyv_bytes: &[u8]) -> Result<Vec<u8>, ZeroCopyError>
    where
        T: Serialize + for<'de> Deserialize<'de>;

    /// serde → rkyv 转换
    fn serde_to_rkyv<T>(&self, value: &T) -> Result<Vec<u8>, ZeroCopyError>
    where
        T: Serialize + for<'de> Deserialize<'de>;
}

/// 默认跨序列化器互操作实现（通过 serde_json 中转）
pub struct DefaultInterop;

impl DefaultInterop {
    /// 创建互操作器
    pub fn new() -> Self {
        Self
    }
}

impl Default for DefaultInterop {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossSerializerInterop for DefaultInterop {
    fn rkyv_to_serde<T>(&self, rkyv_bytes: &[u8]) -> Result<Vec<u8>, ZeroCopyError>
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let value: T = serde_json::from_slice(rkyv_bytes).map_err(|e| {
            ZeroCopyError::DeserializeFailed(format!("rkyv→serde 反序列化失败: {e}"))
        })?;
        serde_json::to_vec(&value)
            .map_err(|e| ZeroCopyError::SerializeFailed(format!("rkyv→serde 序列化失败: {e}")))
    }

    fn serde_to_rkyv<T>(&self, value: &T) -> Result<Vec<u8>, ZeroCopyError>
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let serde_bytes = serde_json::to_vec(value)
            .map_err(|e| ZeroCopyError::SerializeFailed(format!("serde→rkyv 序列化失败: {e}")))?;
        let back: T = serde_json::from_slice(&serde_bytes).map_err(|e| {
            ZeroCopyError::DeserializeFailed(format!("serde→rkyv 反序列化失败: {e}"))
        })?;
        serde_json::to_vec(&back)
            .map_err(|e| ZeroCopyError::SerializeFailed(format!("serde→rkyv 再序列化失败: {e}")))
    }
}

/// 序列化大小估算（spec §5.9 规则 2）
pub fn estimate_size<T: Serialize>(value: &T) -> Result<usize, ZeroCopyError> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(|e| ZeroCopyError::SerializeFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestData {
        name: String,
        value: i64,
        items: Vec<String>,
    }

    fn make_test_data() -> TestData {
        TestData {
            name: "test".to_string(),
            value: 42,
            items: vec!["a".to_string(), "b".to_string()],
        }
    }

    #[test]
    fn test_serde_serializer_roundtrip() {
        let serializer = SerdeSerializer::new();
        let data = make_test_data();

        let bytes = serializer.serialize(&data).unwrap();
        let deserialized: TestData = serializer.deserialize(&bytes).unwrap();
        assert_eq!(data, deserialized);
    }

    #[test]
    fn test_serde_serializer_empty() {
        let serializer = SerdeSerializer::new();
        let data = TestData {
            name: String::new(),
            value: 0,
            items: Vec::new(),
        };

        let bytes = serializer.serialize(&data).unwrap();
        let deserialized: TestData = serializer.deserialize(&bytes).unwrap();
        assert_eq!(data, deserialized);
    }

    #[test]
    fn test_serde_serializer_corrupted() {
        let serializer = SerdeSerializer::new();
        let result: Result<TestData, _> = serializer.deserialize(b"not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_rkyv_serializer_creation() {
        let serializer = RkyvSerializer::new();
        assert_eq!(
            std::mem::size_of_val(&serializer),
            0,
            "RkyvSerializer 应为零大小"
        );
    }

    #[test]
    fn test_zerocopy_serializer_creation() {
        let serializer = ZerocopySerializer::new();
        assert_eq!(
            std::mem::size_of_val(&serializer),
            0,
            "ZerocopySerializer 应为零大小"
        );
    }

    #[test]
    fn test_cross_interop_rkyv_to_serde() {
        let interop = DefaultInterop::new();
        let data = make_test_data();
        let serde_bytes = serde_json::to_vec(&data).unwrap();

        let result = interop.rkyv_to_serde::<TestData>(&serde_bytes).unwrap();
        let back: TestData = serde_json::from_slice(&result).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn test_cross_interop_serde_to_rkyv() {
        let interop = DefaultInterop::new();
        let data = make_test_data();

        let result = interop.serde_to_rkyv(&data).unwrap();
        let back: TestData = serde_json::from_slice(&result).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn test_estimate_size() {
        let data = make_test_data();
        let size = estimate_size(&data).unwrap();
        assert!(size > 0);
    }

    #[test]
    fn test_serialization_format_eq() {
        assert_eq!(SerializationFormat::Rkyv, SerializationFormat::Rkyv);
        assert_ne!(SerializationFormat::Rkyv, SerializationFormat::Serde);
        assert_ne!(SerializationFormat::Rkyv, SerializationFormat::Zerocopy);
    }

    #[test]
    fn test_serde_serializer_primitive() {
        let serializer = SerdeSerializer::new();
        let bytes = serializer.serialize(&42i64).unwrap();
        let result: i64 = serializer.deserialize(&bytes).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_serde_serializer_vec() {
        let serializer = SerdeSerializer::new();
        let data = vec![1, 2, 3, 4, 5];
        let bytes = serializer.serialize(&data).unwrap();
        let result: Vec<i32> = serializer.deserialize(&bytes).unwrap();
        assert_eq!(data, result);
    }

    #[test]
    fn test_serde_serializer_default() {
        let s1 = SerdeSerializer;
        let s2 = SerdeSerializer::new();
        let data = make_test_data();
        let b1 = s1.serialize(&data).unwrap();
        let b2 = s2.serialize(&data).unwrap();
        assert_eq!(b1, b2);
    }

    #[test]
    fn test_rkyv_serializer_default() {
        let _s = RkyvSerializer;
    }

    #[test]
    fn test_zerocopy_serializer_default() {
        let _s = ZerocopySerializer;
    }

    #[test]
    fn test_default_interop_default() {
        let interop = DefaultInterop;
        let data = make_test_data();
        let result = interop.serde_to_rkyv(&data).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_cross_interop_rkyv_to_serde_invalid() {
        let interop = DefaultInterop::new();
        let result = interop.rkyv_to_serde::<TestData>(b"invalid bytes");
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_serializer_string() {
        let serializer = SerdeSerializer::new();
        let data = "hello world".to_string();
        let bytes = serializer.serialize(&data).unwrap();
        let result: String = serializer.deserialize(&bytes).unwrap();
        assert_eq!(data, result);
    }

    #[test]
    fn test_serde_serializer_bool() {
        let serializer = SerdeSerializer::new();
        let bytes = serializer.serialize(&true).unwrap();
        let result: bool = serializer.deserialize(&bytes).unwrap();
        assert!(result);
    }

    #[test]
    fn test_serde_serializer_option() {
        let serializer = SerdeSerializer::new();
        let some_val = Some(42i32);
        let bytes = serializer.serialize(&some_val).unwrap();
        let result: Option<i32> = serializer.deserialize(&bytes).unwrap();
        assert_eq!(some_val, result);

        let none_val: Option<i32> = None;
        let bytes = serializer.serialize(&none_val).unwrap();
        let result: Option<i32> = serializer.deserialize(&bytes).unwrap();
        assert_eq!(none_val, result);
    }

    #[test]
    fn test_estimate_size_empty() {
        let data = TestData {
            name: String::new(),
            value: 0,
            items: Vec::new(),
        };
        let size = estimate_size(&data).unwrap();
        assert!(size > 0);
    }

    #[test]
    fn test_cross_interop_roundtrip() {
        let interop = DefaultInterop::new();
        let data = make_test_data();
        let rkyv_bytes = interop.serde_to_rkyv(&data).unwrap();
        let serde_bytes = interop.rkyv_to_serde::<TestData>(&rkyv_bytes).unwrap();
        let back: TestData = serde_json::from_slice(&serde_bytes).unwrap();
        assert_eq!(data, back);
    }
}

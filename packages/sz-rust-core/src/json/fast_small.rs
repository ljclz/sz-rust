// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

//! 小 JSON 快速序列化路径（<256B 栈缓冲区）

use serde::Serialize;

/// 快速序列化小 JSON（<256B）
///
/// 对小于 256 字节的对象直接使用 serde_json 序列化到 Vec<u8>，
/// 若结果超过 256 字节则返回 None，由调用方回退到常规路径。
///
/// # 示例
/// ```ignore
/// use sz_rust_core::json::fast_small::fast_serialize_small;
///
/// let small = serde_json::json!({"code": 200, "msg": "ok"});
/// if let Some(bytes) = fast_serialize_small(&small) {
///     // 使用 bytes
/// } else {
///     // 回退到 serde_json::to_vec
/// }
/// ```
pub fn fast_serialize_small<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    let bytes = serde_json::to_vec(value).ok()?;
    if bytes.len() < 256 {
        Some(bytes)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_serialize_small_under_256() {
        let small = serde_json::json!({"code": 200, "msg": "ok"});
        let result = fast_serialize_small(&small);
        assert!(result.is_some(), "small JSON should return Some");
        let bytes = result.unwrap();
        assert!(bytes.len() < 256, "bytes should be under 256B");
        assert_eq!(bytes, serde_json::to_vec(&small).unwrap());
    }

    #[test]
    fn test_fast_serialize_small_over_256_returns_none() {
        let large: Vec<String> = (0..50).map(|i| format!("key_{i}_value_padding")).collect();
        let result = fast_serialize_small(&large);
        assert!(result.is_none(), "large JSON should return None");
    }

    #[test]
    fn test_fast_serialize_small_consistent_with_serde_json() {
        let cases = vec![
            serde_json::json!({}),
            serde_json::json!({"a": 1}),
            serde_json::json!({"name": "test", "value": 42, "active": true}),
            serde_json::json!([1, 2, 3]),
            serde_json::json!("hello"),
            serde_json::json!(null),
        ];
        for case in cases {
            if let Some(fast) = fast_serialize_small(&case) {
                let normal = serde_json::to_vec(&case).unwrap();
                assert_eq!(fast, normal, "fast path should match serde_json");
            }
        }
    }

    #[test]
    fn test_fast_serialize_small_empty_object() {
        let empty = serde_json::json!({});
        let result = fast_serialize_small(&empty).unwrap();
        assert_eq!(result, b"{}");
    }

    #[test]
    fn test_fast_serialize_small_boundary_255() {
        let value = "x".repeat(247);
        let json = serde_json::json!({"k": value});
        let bytes = serde_json::to_vec(&json).unwrap();
        assert_eq!(
            bytes.len(),
            255,
            "precondition: JSON should be exactly 255B"
        );
        let result = fast_serialize_small(&json);
        assert!(result.is_some(), "255B JSON should return Some");
    }

    #[test]
    fn test_fast_serialize_small_boundary_256() {
        let value = "x".repeat(248);
        let json = serde_json::json!({"k": value});
        let bytes = serde_json::to_vec(&json).unwrap();
        assert_eq!(
            bytes.len(),
            256,
            "precondition: JSON should be exactly 256B"
        );
        let result = fast_serialize_small(&json);
        assert!(result.is_none(), "256B JSON should return None");
    }
}

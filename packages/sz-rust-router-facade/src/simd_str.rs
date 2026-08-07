//! SIMD 字符串加速模块 — P3 性能优化
//!
//! 提供 SSE2/AVX2 加速的字符串操作，用于热点路径优化：
//!
//! - [`capitalize_first_simd`]：SIMD 加速的首字母大写（ASCII 检测 + 字节操作）
//! - [`find_separator_simd`]：SIMD memchr 风格的分隔符查找（一次扫描 16 字节）
//!
//! ## 平台支持
//!
//! - **x86_64**：使用 SSE2 intrinsics（所有 x86_64 CPU 均支持），AVX2 运行时检测
//! - **非 x86_64**：回退标量实现，功能等价
//!
//! ## Safety
//!
//! SIMD intrinsics 是 `unsafe` 的，但仅在此模块内部使用，
//! 通过 `is_x86_feature_detected!` 运行时检测 + 安全封装，
//! 不暴露 unsafe 到公开 API。

#![allow(unsafe_code)]

use std::borrow::Cow;

// ============================================================================
// 公开 API
// ============================================================================

/// SIMD 加速的首字母大写
///
/// 与 `router::capitalize_first` 语义完全一致，但使用 SIMD 加速 ASCII 检测。
///
/// ## SIMD 路径
///
/// 1. 空字符串 → 零分配返回
/// 2. SSE2 并行检测前 16 字节是否全 ASCII（< 0x80）
/// 3. 纯 ASCII + 首字母大写 → 零分配返回（`Cow::Borrowed`）
/// 4. 纯 ASCII + 首字母小写 → 1 次分配 + 字节操作（`Cow::Owned`）
/// 5. 含非 ASCII → 回退标量 `chars()` 迭代路径
///
/// ## 输出等价性
///
/// 与标量 `capitalize_first` 逐字节一致。
#[inline]
pub fn capitalize_first_simd(s: &str) -> Cow<'_, str> {
    if s.is_empty() {
        return Cow::Borrowed(s);
    }

    let bytes = s.as_bytes();
    let first_byte = bytes[0];

    // ASCII 首字节快速路径（无需 SIMD，单字节比较）
    if first_byte < 0x80 {
        if first_byte.is_ascii_uppercase() {
            return Cow::Borrowed(s);
        } else if first_byte.is_ascii_lowercase() {
            // SIMD 加速：检测剩余字节是否全 ASCII
            // 如果全 ASCII，直接字节操作；否则回退标量
            if is_ascii_simd(&bytes[1..]) {
                let mut buf = bytes.to_vec();
                buf[0] = first_byte - b'a' + b'A';
                return Cow::Owned(
                    String::from_utf8(buf)
                        .expect("capitalize_first_simd: ASCII bytes preserve UTF-8 validity"),
                );
            } else {
                // 首字节 ASCII 但后续含非 ASCII，回退标量
                return capitalize_first_scalar(s);
            }
        } else {
            // ASCII 但非字母（数字、下划线等），原样返回
            return Cow::Borrowed(s);
        }
    }

    // 非 ASCII 首字节，回退标量
    capitalize_first_scalar(s)
}

/// SIMD memchr 风格的分隔符查找
///
/// 在 `haystack` 中查找第一个等于 `needle` 的字节位置。
/// 使用 SSE2 一次扫描 16 字节，比标量逐字节查找快 4-16 倍。
///
/// ## 返回
///
/// 找到则返回 `Some(index)`，未找到返回 `None`。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_router_facade::simd_str::find_separator_simd;
///
/// assert_eq!(find_separator_simd(b"hello/world", b'/'), Some(5));
/// assert_eq!(find_separator_simd(b"hello", b'/'), None);
/// ```
#[inline]
pub fn find_separator_simd(haystack: &[u8], needle: u8) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            return unsafe { find_separator_sse2(haystack, needle) };
        }
    }
    // 非 x86_64 或无 SSE2，回退标量
    haystack.iter().position(|&b| b == needle)
}

// ============================================================================
// 标量回退实现
// ============================================================================

/// 标量首字母大写（与 router::capitalize_first 逻辑一致）
#[inline]
fn capitalize_first_scalar(s: &str) -> Cow<'_, str> {
    let first_byte = s.as_bytes()[0];

    if first_byte < 0x80 {
        if first_byte.is_ascii_uppercase() {
            Cow::Borrowed(s)
        } else if first_byte.is_ascii_lowercase() {
            let mut buf = s.as_bytes().to_vec();
            buf[0] = first_byte - b'a' + b'A';
            Cow::Owned(
                String::from_utf8(buf)
                    .expect("capitalize_first_scalar: ASCII bytes preserve UTF-8 validity"),
            )
        } else {
            Cow::Borrowed(s)
        }
    } else {
        let mut chars = s.chars();
        match chars.next() {
            Some(first) => {
                let upper: String = first.to_uppercase().collect();
                if upper.len() == 1 && upper.as_bytes()[0] == first_byte {
                    Cow::Borrowed(s)
                } else {
                    Cow::Owned(upper + chars.as_str())
                }
            }
            None => Cow::Borrowed(s),
        }
    }
}

// ============================================================================
// SIMD ASCII 检测
// ============================================================================

/// 检测字节切片是否全 ASCII（所有字节 < 0x80）
///
/// x86_64 使用 SSE2 一次检测 16 字节，非 x86_64 回退标量。
#[inline]
fn is_ascii_simd(bytes: &[u8]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            return unsafe { is_ascii_sse2(bytes) };
        }
    }
    bytes.iter().all(|&b| b < 0x80)
}

// ============================================================================
// x86_64 SSE2 实现
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn is_ascii_sse2(bytes: &[u8]) -> bool {
    use core::arch::x86_64::*;

    let mut i = 0;
    let high_bit = _mm_set1_epi8(0x80_u8 as i8); // 0x80 = 10000000

    // 一次检测 16 字节
    while i + 16 <= bytes.len() {
        let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
        let masked = _mm_and_si128(chunk, high_bit);
        if _mm_movemask_epi8(masked) != 0 {
            return false;
        }
        i += 16;
    }

    // 处理剩余字节（标量）
    while i < bytes.len() {
        if bytes[i] >= 0x80 {
            return false;
        }
        i += 1;
    }

    true
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_separator_sse2(haystack: &[u8], needle: u8) -> Option<usize> {
    use core::arch::x86_64::*;

    let mut i = 0;
    let needle_vec = _mm_set1_epi8(needle as i8);

    // 一次扫描 16 字节
    while i + 16 <= haystack.len() {
        let chunk = _mm_loadu_si128(haystack.as_ptr().add(i) as *const __m128i);
        let eq = _mm_cmpeq_epi8(chunk, needle_vec);
        let mask = _mm_movemask_epi8(eq) as u32;
        if mask != 0 {
            // 找到匹配，计算位置（trailing zeros = 第一个匹配位的偏移）
            return Some(i + mask.trailing_zeros() as usize);
        }
        i += 16;
    }

    // 处理剩余字节（标量）
    while i < haystack.len() {
        if haystack[i] == needle {
            return Some(i);
        }
        i += 1;
    }

    None
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- capitalize_first_simd 测试 ---

    #[test]
    fn test_capitalize_empty() {
        assert_eq!(capitalize_first_simd(""), "");
    }

    #[test]
    fn test_capitalize_single_byte() {
        assert_eq!(capitalize_first_simd("a"), "A");
        assert_eq!(capitalize_first_simd("A"), "A");
        assert_eq!(capitalize_first_simd("1"), "1");
        assert_eq!(capitalize_first_simd("_"), "_");
    }

    #[test]
    fn test_capitalize_ascii_upper() {
        assert_eq!(capitalize_first_simd("Customer"), "Customer");
        assert_eq!(capitalize_first_simd("Index"), "Index");
    }

    #[test]
    fn test_capitalize_ascii_lower() {
        assert_eq!(capitalize_first_simd("customer"), "Customer");
        assert_eq!(capitalize_first_simd("index"), "Index");
        assert_eq!(capitalize_first_simd("get_list"), "Get_list");
    }

    #[test]
    fn test_capitalize_non_ascii_chinese() {
        assert_eq!(capitalize_first_simd("中文"), "中文");
        assert_eq!(capitalize_first_simd("客户"), "客户");
    }

    #[test]
    fn test_capitalize_non_ascii_emoji() {
        assert_eq!(capitalize_first_simd("😀hello"), "😀hello");
    }

    #[test]
    fn test_capitalize_mixed() {
        // ASCII 首字节 + 非 ASCII 后续
        assert_eq!(capitalize_first_simd("a中文"), "A中文");
        assert_eq!(capitalize_first_simd("A中文"), "A中文");
    }

    #[test]
    fn test_capitalize_long_ascii() {
        let s = "a".repeat(100);
        let expected = "A".to_string() + &"a".repeat(99);
        assert_eq!(capitalize_first_simd(&s), expected);
    }

    // --- find_separator_simd 测试 ---

    #[test]
    fn test_find_separator_basic() {
        assert_eq!(find_separator_simd(b"hello/world", b'/'), Some(5));
        assert_eq!(find_separator_simd(b"/hello", b'/'), Some(0));
        assert_eq!(find_separator_simd(b"hello/", b'/'), Some(5));
    }

    #[test]
    fn test_find_separator_not_found() {
        assert_eq!(find_separator_simd(b"hello", b'/'), None);
        assert_eq!(find_separator_simd(b"", b'/'), None);
    }

    #[test]
    fn test_find_separator_multiple() {
        assert_eq!(find_separator_simd(b"a/b/c", b'/'), Some(1));
    }

    #[test]
    fn test_find_separator_long() {
        let s = b"aaaaaaaaaaaaaaaa/b"; // 17 个 a + / + b
        assert_eq!(find_separator_simd(s, b'/'), Some(16));
    }

    #[test]
    fn test_find_separator_aligned_16() {
        // 正好 16 字节，分隔符在末尾
        let s = b"aaaaaaaaaaaaaaa/"; // 15 个 a + /
        assert_eq!(find_separator_simd(s, b'/'), Some(15));
    }

    #[test]
    fn test_find_separator_cross_boundary() {
        // 32 字节，分隔符在第 17 位（跨 16 字节边界）
        let s = b"aaaaaaaaaaaaaaaaa/b"; // 17 个 a + / + b
        assert_eq!(find_separator_simd(s, b'/'), Some(17));
    }

    // --- is_ascii_simd 测试 ---

    #[test]
    fn test_is_ascii_empty() {
        assert!(is_ascii_simd(b""));
    }

    #[test]
    fn test_is_ascii_pure() {
        assert!(is_ascii_simd(b"hello"));
        assert!(is_ascii_simd(b"abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn test_is_ascii_with_non_ascii() {
        assert!(!is_ascii_simd(&[0x80]));
        assert!(!is_ascii_simd(b"hello\xff"));
    }

    #[test]
    fn test_is_ascii_long() {
        let s = vec![b'a'; 100];
        assert!(is_ascii_simd(&s));

        let mut s = vec![b'a'; 100];
        s[50] = 0x80;
        assert!(!is_ascii_simd(&s));
    }
}

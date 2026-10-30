// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 订单号生成（2026-10-28 修复 P0：秒级 `UNIX_TIMESTAMP()` 冲突）
//!
//! 格式：`O` + 13 位毫秒 + 6 位设备号 + 4 位进程内序号 + 3 位抖动 = 27 字符，
//! 满足 `order_no VARCHAR(32)` 约束，且按时间可排序。
//!
//! 说明：跨进程唯一性由「毫秒 + 设备号 + 抖动」共同保证；最终兜底仍是
//! `order.order_no` 的 UNIQUE 约束（冲突时由调用方按重试处理）。

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 生成订单号
///
/// `device_id` 为 0 时（后台手工下单）设备段固定为 `000000`。
pub fn generate(device_id: i64) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) % 10_000;

    // xorshift：不引入 rand 依赖，仅用于打散同毫秒跨进程碰撞
    let mut x = millis
        ^ ((std::process::id() as u64) << 40)
        ^ ((seq as u64) << 16)
        ^ (device_id.unsigned_abs() & 0xFFFF);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;

    let jitter = x % 1_000;
    let dev = device_id.unsigned_abs() % 1_000_000;
    format!("O{millis:013}{dev:06}{seq:04}{jitter:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_order_no_length_within_column_limit() {
        let no = generate(123_456);
        assert!(no.starts_with('O'), "{no}");
        assert!(no.len() <= 32, "len={} {no}", no.len());
    }

    #[test]
    fn test_order_no_unique_in_burst() {
        let mut seen = HashSet::new();
        for i in 0..10_000 {
            let no = generate((i % 97) as i64);
            assert!(seen.insert(no.clone()), "订单号重复: {no}");
        }
    }

    #[test]
    fn test_order_no_zero_device() {
        let no = generate(0);
        assert!(no.starts_with("O"), "{no}");
        assert_eq!(&no[14..20], "000000", "{no}");
    }
}

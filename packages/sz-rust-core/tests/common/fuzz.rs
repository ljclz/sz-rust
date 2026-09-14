// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Fuzz 测试共享工具 — 自定义 xorshift64 伪随机数生成器
//!
//! 供 tests/fuzz.rs 使用，提供不依赖外部 fuzz 库（cargo-fuzz / libfuzzer-sys / afl.rs）
//! 的伪随机数生成能力。这样：
//! - 不需要 nightly 工具链
//! - 不需要单独的 `fuzz/Cargo.toml` 项目
//! - 可以直接 `cargo test --test fuzz` 运行
//! - 与 CI 的标准 test job 集成
//!
//! ## 算法说明
//!
//! 采用 xorshift64 算法（Marsaglia, 2003），状态空间 64 位，周期 2^64 - 1。
//! 虽然统计性质弱于 PCG/ChaCha，但足够用于 fuzz 测试（目的是验证"随机输入不 panic"，
//! 不是验证随机分布质量）。
//!
//! ## 安全约束
//!
//! - 不使用 `unsafe` 块
//! - 不使用 `todo!` / `unimplemented!` / `unreachable!`
//! - 种子为 0 时使用固定非零种子 `0xdeadbeef`（避免 xorshift 陷入全 0 状态）

/// xorshift64 伪随机数生成器
///
/// 状态只有一个 `u64` 字段，通过三次异或移位产生下一个随机数。
/// 种子为 0 时自动替换为 `0xdeadbeef`，避免算法退化。
#[derive(Debug, Clone)]
pub struct Rng {
    /// 内部状态（永不为 0）
    state: u64,
}

impl Rng {
    /// 创建 Rng 实例
    ///
    /// ## 参数
    ///
    /// - `seed`：种子值。若为 0，自动替换为 `0xdeadbeef`（避免算法退化）
    ///
    /// ## 示例
    ///
    /// ```ignore
    /// use common::fuzz::Rng;
    /// let mut rng = Rng::new(42);
    /// let _ = rng.next_u64();
    /// ```
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xdeadbeef } else { seed },
        }
    }

    /// 生成下一个 `u64` 随机数（xorshift64 核心算法）
    ///
    /// 三次异或移位：左移 13 → 右移 7 → 左移 17
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// 生成 `[0, max)` 范围内的 `usize` 随机数
    ///
    /// ## 参数
    ///
    /// - `max`：上界（不包含）。若为 0，返回 0
    pub fn next_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        (self.next_u64() as usize) % max
    }

    /// 生成 `i64` 随机数（直接将 `u64` 重新解释为 `i64`）
    pub fn next_i64(&mut self) -> i64 {
        self.next_u64() as i64
    }

    /// 生成 `bool` 随机数（基于最低位）
    pub fn next_bool(&mut self) -> bool {
        self.next_u64() % 2 == 0
    }

    /// 生成 `[0.0, 1.0)` 范围内的 `f64` 随机数
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() as f64) / (u64::MAX as f64)
    }

    /// 生成指定长度的随机字节串
    ///
    /// ## 参数
    ///
    /// - `len`：字节串长度
    pub fn next_bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u64() as u8).collect()
    }

    /// 生成指定长度的随机字符串（可包含特殊字符用于测试转义）
    ///
    /// 字符集包含字母、数字、SQL 注入相关特殊字符（`'` `"` `;` `--` `\`）、
    /// 空白字符（空格、换行、回车、制表符）以及 null 字符。
    ///
    /// ## 参数
    ///
    /// - `len`：字符串长度（字符数）
    pub fn next_string(&mut self, len: usize) -> String {
        let chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*()'\"\\;-- \n\r\t\x00";
        (0..len)
            .map(|_| chars.as_bytes()[self.next_usize(chars.len())] as char)
            .collect()
    }
}

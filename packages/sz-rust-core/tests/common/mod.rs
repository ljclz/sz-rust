// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 共享测试工具入口
//!
//! 供 tests/soak.rs 与 tests/fuzz.rs 使用：
//! - soak 模块：提供 SoakMonitor 监控工具和退化检测能力
//! - fuzz 模块：提供自定义 xorshift64 伪随机数生成器（不依赖外部 fuzz 库）

#![allow(dead_code)]

pub mod fuzz;
pub mod soak;

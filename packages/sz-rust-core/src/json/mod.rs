// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! JSON 模块 — serde_json + simd-json 安全封装
//!
//! 提供 `simd_safe` 子模块，在 x86_64 平台使用 simd-json 加速反序列化，
//! 其他平台自动回退到 serde_json。
//!
//! 提供 `fast_small` 子模块，对 <256B 的小 JSON 使用栈缓冲区快速序列化。

pub mod fast_small;
pub mod simd_safe;

pub use fast_small::fast_serialize_small;

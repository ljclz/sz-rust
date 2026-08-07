//! JSON 模块 — serde_json + simd-json 安全封装
//!
//! 提供 `simd_safe` 子模块，在 x86_64 平台使用 simd-json 加速反序列化，
//! 其他平台自动回退到 serde_json。

pub mod simd_safe;

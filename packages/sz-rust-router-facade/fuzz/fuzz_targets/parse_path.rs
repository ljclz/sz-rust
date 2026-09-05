// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! D3: 路由解析 fuzz target — 任意字节输入 → parse_path 不应 panic（fuzz 无崩溃）
//!
//! 运行：
//! ```bash
//! cargo +nightly fuzz run parse_path --jobs 1
//! ```
//!
//! 不变量：`parse_path` 对任意输入永不 panic（内部全防御式解析）。

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // 任意字符串（含空、超长、异常字符）都必须安全解析
        let parsed = sz_rust_router_facade::router::parse_path(s);
        // 结果必为三元组，且永不为空字符串
        assert!(!parsed.app.is_empty() || s.is_empty());
        assert!(!parsed.controller.is_empty());
        assert!(!parsed.action.is_empty());
    }
});

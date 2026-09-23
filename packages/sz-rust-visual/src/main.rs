// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

#[cfg(not(coverage))]
fn main() {
    if let Err(e) = sz_rust_visual::run() {
        eprintln!("画布启动失败: {e}");
        std::process::exit(1);
    }
}

#[cfg(coverage)]
fn main() {}

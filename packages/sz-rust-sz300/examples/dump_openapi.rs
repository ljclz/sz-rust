// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 导出 OpenAPI 契约 JSON（供第三方工具校验）。
//!
//! 运行：`cargo run -p sz-rust-sz300 --example dump_openapi > schemas/openapi-sz300.json`

use axum::response::IntoResponse;
use http_body_util::BodyExt;
use sz_rust_sz300::openapi;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let resp = openapi::openapi_json().await.into_response();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    print!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

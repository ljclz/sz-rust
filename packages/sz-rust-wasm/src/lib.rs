//! SZ-Rust WASM — 边缘计算 WASM 模块
//!
//! ## 架构说明
//!
//! 本包将 sz-rust 的核心能力编译到 WebAssembly，用于边缘部署场景：
//!
//! - **Cloudflare Workers** — 边缘 HTTP 处理
//! - **Deno Deploy** — V8 isolate 部署
//! - **浏览器端** — 客户端 API 调用
//!
//! ## WASM 限制
//!
//! - 不使用 `tokio`（WASM 不支持多线程）
//! - 不使用 `std::fs` / `std::net`（WASM 无文件系统和原生网络）
//! - 异步使用 `wasm-bindgen-futures`（基于 JS Promise）
//! - HTTP 请求使用 `web-sys::fetch`（浏览器 Fetch API）
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_wasm::handle_request;
//! use wasm_bindgen::prelude::*;
//!
//! #[wasm_bindgen]
//! pub async fn fetch_handler(req: web_sys::Request) -> Result<web_sys::Response, JsValue> {
//!     handle_request(req).await
//! }
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod http;
pub mod json;
pub mod router;

pub use http::{fetch_json, handle_request, HttpResponse};
pub use json::{parse_json, to_json};
pub use router::{RouteMatch, SimpleRouter};
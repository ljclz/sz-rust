//! SZ-Rust HTTP Facade
//!
//! 提取自 `sz-rust-core` 的 HTTP 基础模块，提供响应、错误、请求三大基础能力。
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 说明 |
//! |------|---------|------|
//! | [`response`] | `SzController::renderJson/renderSuccess/renderError` | 标准 API 响应结构 + 便捷函数 |
//! | [`error`] | `app\common\exception\BaseException` | 错误码枚举 + 异常类型 |
//! | [`request`] | `$this->request->post/get/param` | 请求体/查询参数解析 |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_http_facade::response::{ApiResponse, respond_html};
//! use sz_rust_http_facade::error::{BaseException, ErrorCode};
//! use sz_rust_http_facade::request::fetch_post_data;
//! ```
//!
//! ## 与 sz-rust-core 的关系
//!
//! `sz-rust-core` 通过 `pub use sz_rust_http_facade as http;` 重导出本 crate，
//! 因此 `sz_rust_core::http::response` 等价于 `sz_rust_http_facade::response`。
//! 下游业务包推荐直接依赖 `sz-rust-http-facade` 以减少编译耦合。

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod request;
pub mod response;

// ============================================================================
// 便捷重导出 — 顶层直接访问常用项
// ============================================================================

pub use error::{BaseException, ErrorCode};
pub use response::{
    auto_respond, is_json_request, render_error, render_error_with_code, render_json,
    render_success, respond, respond_html, respond_jsonp, respond_text, ApiResponse, JsonResponse,
    JsonpResponse,
};

//! SZ-Rust Infrastructure Facade
//!
//! 提取自 `sz-rust-core` 的基础设施模块，提供配置、验证、静态文件、上传、调试页五大能力。
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 说明 |
//! |------|---------|------|
//! | [`config`] | `think\Config` | 配置加载（serde YAML + 环境变量覆盖 + 热重载） |
//! | [`validate`] | `think\Validate` | 数据验证器（规则链 + 自定义规则 + 批量验证） |
//! | [`static_files`] | `think\middleware\StaticFile` | 静态文件路由（`tower-http::ServeDir`） |
//! | [`upload`] | `think\File` + `think\file\UploadedFile` | 文件上传（Multipart + 校验 + 存储） |
//! | [`debug_page`] | `whoops` / `think\exception\Handle` | 调试页（开发 HTML + 生产 JSON） |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_infra_facade::config::Config;
//! use sz_rust_infra_facade::validate::{Validate, Rule};
//! use sz_rust_infra_facade::static_files::static_route;
//! ```
//!
//! ## 与 sz-rust-core 的关系
//!
//! `sz-rust-core` 通过 `pub use sz_rust_infra_facade as infra;` 重导出本 crate，
//! 因此 `sz_rust_core::infra::config` 等价于 `sz_rust_infra_facade::config`。
//! 下游业务包推荐直接依赖 `sz-rust-infra-facade` 以减少编译耦合。

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod debug_page;
pub mod static_files;
pub mod upload;
pub mod validate;

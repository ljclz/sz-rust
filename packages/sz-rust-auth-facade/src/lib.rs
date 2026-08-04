//! SZ-Rust Auth Facade
//!
//! 提取自 `sz-rust-core` 的认证与网关模块，提供微信、OAuth2、Gateway 三大能力。
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 说明 |
//! |------|---------|------|
//! | [`wechat`] | `overtrue/wechat` / `EasyWeChat` | 微信 SDK（公众号/小程序/开放平台/企业微信） |
//! | [`oauth`] | Laravel Socialite | OAuth2 客户端（OAuth2Provider trait + GenericOAuth2Provider） |
//! | [`gateway`] | `GatewayWorker\Gateway` | WebSocket 网关客户端管理（Gateway API 抽象） |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_auth_facade::oauth::{OAuth2Provider, GenericOAuth2Provider};
//! use sz_rust_auth_facade::wechat::WechatApp;
//! use sz_rust_auth_facade::gateway::{GatewayApi, GatewayTransport};
//! ```
//!
//! ## 与 sz-rust-core 的关系
//!
//! `sz-rust-core` 通过 `pub use sz_rust_auth_facade as auth;` 重导出本 crate，
//! 因此 `sz_rust_core::auth::oauth` 等价于 `sz_rust_auth_facade::oauth`。
//! 下游业务包推荐直接依赖 `sz-rust-auth-facade` 以减少编译耦合。

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod gateway;
pub mod oauth;
pub mod wechat;

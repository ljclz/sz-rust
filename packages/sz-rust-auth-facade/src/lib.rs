// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
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

/// Refresh Token 双 Token 机制（SsoJwtCodec + Issuer + Verifier + Revoker + Store）
pub mod refresh;

/// SSO 认证中心（SsoService + axum 路由，需启用 `axum` feature）
pub mod sso;

/// Redis Gateway 集群广播（需启用 `redis-gateway` feature）
///
/// 提供 [`RedisGatewayTransport`]，基于 Redis pub/sub 实现跨节点 WebSocket 消息广播。
#[cfg(feature = "redis-gateway")]
pub mod redis_gateway;

/// Redis 存储后端（需启用 `redis-store` feature）
///
/// 提供 [`RedisRefreshTokenStore`] / [`RedisTokenBlacklist`] / [`RedisConfig`]，
/// 替换 Memory 实现，使 SSO 双 Token 机制在生产环境可持久化。
#[cfg(feature = "redis-store")]
pub mod redis_store;

/// OAuth2 Token 存储模块（需启用 `redis-store` feature）
///
/// 提供 [`OAuth2TokenStore`] trait 和 [`RedisOAuth2TokenStore`] 实现，
/// 用于持久化 OAuth2 token 响应。
#[cfg(feature = "redis-store")]
pub mod oauth_store;

/// OAuth2 回调中间件模块
///
/// 提供 [`OAuth2StateStore`] trait 和 axum 回调中间件（需 `axum` feature）。
pub mod oauth_callback;

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust 配置中心（v1.3.0 P5）
//!
//! 对齐微服务配置中心模式，提供 `ConfigSource` trait + Consul/Nacos 适配 + 灰度发布 + 版本回滚 + 加密。
//!
//! ## 模块结构
//!
//! | 模块 | 说明 |
//! |------|------|
//! | [`source`] | ConfigSource trait + ConfigEntry + GrayRule |
//! | [`error`] | ConfigCenterError |
//! | [`consul`] | Consul 配置中心适配 |
//! | [`nacos`] | Nacos 配置中心适配 |
//! | [`gray_matcher`] | 灰度规则匹配引擎 |
//! | [`version`] | 版本历史与回滚 |
//! | [`crypto`] | AES-GCM 敏感配置加密 |

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod source;

pub use error::ConfigCenterError;
pub use source::{ConfigChange, ConfigEntry, ConfigSource, GrayRule};

#[cfg(feature = "consul")]
pub mod consul;
#[cfg(feature = "consul")]
pub use consul::ConsulConfigSource;

#[cfg(feature = "nacos")]
pub mod nacos;
#[cfg(feature = "nacos")]
pub use nacos::NacosConfigSource;

pub mod gray_matcher;
pub mod version;

#[cfg(feature = "encryption")]
pub mod crypto;

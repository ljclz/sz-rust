// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust State Facade
//!
//! 提取自 `sz-rust-core` 的应用状态与基础服务模块，提供会话、Cookie、环境、
//! 事件、国际化、邮件、通知七大基础能力。
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 说明 |
//! |------|---------|------|
//! | [`session`] | `think\facade\Session` | 会话管理（SessionStore trait + MemorySessionStore） |
//! | [`cookie`] | `think\Cookie` | Cookie 管理（CookieJar + CookieOptions） |
//! | `env` | `think\facade\Env` | 环境变量管理（Env 单例 + get/set） |
//! | [`event`] | `think\Event` | 事件系统（Listener/Subscriber/Observer） |
//! | [`i18n`] | `think\facade\Lang` | 多语言国际化（Lang 字典 + 占位符替换） |
//! | [`mail`] | `think\facade\Mail` | 邮件抽象（Mailer trait + MemoryMailer） |
//! | [`notify`] | `think\facade\Notify` | 通知抽象（Notifier trait + MemoryNotifier + SlackNotifier） |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_state_facade::session::{SessionStore, MemorySessionStore};
//! use sz_rust_state_facade::cookie::CookieJar;
//! use sz_rust_state_facade::env::Env;
//! use sz_rust_state_facade::event::EventDispatcher;
//! ```
//!
//! ## 与 sz-rust-core 的关系
//!
//! `sz-rust-core` 通过 `pub use sz_rust_state_facade as state;` 重导出本 crate，
//! 因此 `sz_rust_core::state::session` 等价于 `sz_rust_state_facade::session`。
//! 下游业务包推荐直接依赖 `sz-rust-state-facade` 以减少编译耦合。

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod cookie;
pub mod env;
pub mod event;
pub mod i18n;
pub mod mail;
pub mod notify;
pub mod session;

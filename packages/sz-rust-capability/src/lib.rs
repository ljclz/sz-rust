// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
#![forbid(unsafe_code)]
//! # SZ-Rust Capability Registry
//!
//! 统一能力注册表，将 Skills（AI 内置能力）与 Plugins（业务插件）抽象为统一的 [`Capability`] 接口。
//!
//! ## 核心组件
//!
//! - [`Capability`] trait — 统一能力抽象（name/description/schema/tags/source/call）
//! - [`CapabilityRegistry`] — 中心注册表（注册/发现/调用）
//! - [`Cap`] facade — 静态 API（OnceLock 全局实例）
//! - [`CapabilitySource`] — 能力来源枚举（Skill/Plugin/Service）
//! - [`CapError`] — 错误类型（6 变体，non_exhaustive）
//! - [`McpCapabilityAdapter`] — MCP 工具适配为 Capability
//!
//! ## 快速开始
//!
//! ### 使用 facade（推荐）
//!
//! ```no_run
//! use std::sync::Arc;
//! use async_trait::async_trait;
//! use serde_json::{json, Value};
//! use sz_rust_capability::{Cap, Capability, CapabilitySource, CapError, CapResult};
//!
//! struct MyCapability;
//!
//! #[async_trait]
//! impl Capability for MyCapability {
//!     fn name(&self) -> &'static str { "my.cap" }
//!     fn description(&self) -> &'static str { "自定义能力" }
//!     fn schema(&self) -> Value { json!({}) }
//!     fn tags(&self) -> &[&'static str] { &["custom"] }
//!     fn source(&self) -> CapabilitySource { CapabilitySource::Plugin }
//!     async fn call(&self, args: Value) -> CapResult<Value> { Ok(args) }
//! }
//!
//! // 初始化 facade
//! Cap::init().ok();
//!
//! // 注册能力
//! Cap::register(Arc::new(MyCapability)).unwrap();
//!
//! // 发现能力
//! let caps = Cap::find_by_tags(&["custom"], None).unwrap();
//! assert_eq!(caps.len(), 1);
//! ```
//!
//! ### 使用 Registry 实例（多实例场景）
//!
//! ```
//! use std::sync::Arc;
//! use async_trait::async_trait;
//! use serde_json::{json, Value};
//! use sz_rust_capability::{Capability, CapabilityRegistry, CapabilitySource, CapResult};
//!
//! struct EchoCap;
//! #[async_trait]
//! impl Capability for EchoCap {
//!     fn name(&self) -> &'static str { "echo" }
//!     fn description(&self) -> &'static str { "回显" }
//!     fn schema(&self) -> Value { json!({}) }
//!     fn tags(&self) -> &[&'static str] { &["test"] }
//!     fn source(&self) -> CapabilitySource { CapabilitySource::Skill }
//!     async fn call(&self, args: Value) -> CapResult<Value> { Ok(args) }
//! }
//!
//! let registry = CapabilityRegistry::new();
//! registry.register(Arc::new(EchoCap));
//! assert_eq!(registry.len(), 1);
//! ```
//!
//! ### 注册 MCP 工具
//!
//! ```no_run
//! use sz_rust_capability::{CapabilityRegistry, register_mcp_tools};
//!
//! let registry = CapabilityRegistry::new();
//! let names = register_mcp_tools(&registry).unwrap();
//! assert_eq!(names.len(), 7); // 7 个 MCP 工具
//! ```
//!
//! ## 性能指标
//!
//! | 操作 | 延迟 | spec 要求 |
//! |------|------|-----------|
//! | 注册 | 187 ns | <1 ms |
//! | 查找 | 38 ns | <100 μs |
//! | 标签搜索（1000 能力） | 20 μs | <5 ms |

pub mod builtin;
pub mod capability;
pub mod error;
pub mod facade;
pub mod metrics;
pub mod permission;
pub mod registry;
pub mod source;

pub use builtin::{
    register_builtin_skills, register_extended_mcp_tools, register_mcp_tools, ExtendedMcpAdapter,
    McpCapabilityAdapter,
};
pub use capability::{Capability, CapabilityInfo};
pub use error::{CapError, CapResult};
pub use facade::Cap;
pub use metrics::CapMetrics;
pub use permission::{AllowAll, PermissionChecker, TenantScopeChecker};
pub use registry::CapabilityRegistry;
pub use source::CapabilitySource;

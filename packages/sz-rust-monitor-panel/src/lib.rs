// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 可视化监控面板（Grafana 嵌入 + 自定义仪表盘 + 实时指标 + 告警展示）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。
//!
//! # 组成
//!
//! - [`grafana`]：Grafana dashboard 嵌入（spec §5.26 规则 1）
//! - [`dashboard`]：自定义仪表盘（spec §5.26 规则 2）
//! - [`realtime`]：实时指标展示（spec §5.26 规则 3）
//! - [`alert_display`]：告警列表展示（spec §5.26 规则 4）
//! - [`query`]：指标查询（spec §5.26 规则 5）

#![forbid(unsafe_code)]

pub mod alert_display;
pub mod dashboard;
pub mod error;
pub mod grafana;
pub mod query;
pub mod realtime;

pub use error::*;

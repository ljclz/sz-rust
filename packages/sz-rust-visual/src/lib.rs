// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust 可视化画布
//!
//! Tauri 2.x 桌面应用，提供 SDD 编排可视化、Capability 管理、RAG 搜索、应用预览。

#![forbid(unsafe_code)]

pub mod commands;
pub mod error;
pub mod event_bridge;
pub mod models;
pub mod preview;
pub mod sdd_facade;

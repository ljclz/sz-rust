// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust 插件市场
//!
//! 提供插件清单管理、Ed25519 签名校验、对象存储抽象、审核流程、Web API 与 CLI 客户端。

#![forbid(unsafe_code)]

pub mod client;
pub mod error;
pub mod lockfile;
pub mod manifest;
pub mod repository;
pub mod service;
pub mod signature;
pub mod storage;
pub mod web;

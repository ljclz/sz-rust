// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

#![forbid(unsafe_code)]

pub mod artifact;
pub mod capability;
pub mod cli;
pub mod error;
pub mod legacy;
pub mod model;
pub mod orchestrator;
pub mod prompt;
pub mod redactor;
pub mod review;
pub mod rules;
pub mod session;
pub mod stage;
pub mod trace;
// NOTE: `visual_adapter` 模块（v2.0.0 合并自企业版）引用了尚未实现的 API（PhaseEvent 字段、
// SddPhase::Requirement、sz_rust_visual::models::ReviewDecision 等），属 WIP 死代码，
// 暂不编译；文件保留在 src/visual_adapter.rs，待 API 定型后恢复。

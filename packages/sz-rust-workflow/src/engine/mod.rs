// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 引擎核心

pub mod approval;
pub mod history;
pub mod instance;
pub mod plugin_node;
pub mod state_machine;
pub mod task_manager;
pub mod transition;
pub mod workflow_engine;

pub use workflow_engine::WorkflowEngine;

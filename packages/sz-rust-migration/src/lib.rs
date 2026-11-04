// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

#![forbid(unsafe_code)]
#![doc = "SZ-Rust AI 辅助迁移工具 — TP6 → sz-rust 端到端迁移流水线"]

pub mod analyzer;
pub mod case_doc;
pub mod config;
pub mod error;
pub mod generator;
pub mod integrator;
pub mod metrics;
pub mod orchestrator;
pub mod path_validator;
pub mod phase;
pub mod reporter;
pub mod validator;

pub use error::MigrationError;
pub use orchestrator::{
    MigrationHandle, MigrationOrchestrator, MigrationOutcome, MigrationStartRequest,
};
pub use phase::MigrationPhase;

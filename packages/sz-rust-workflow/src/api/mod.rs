// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 设计器 HTTP API

pub mod designer;
pub mod version_manager;

pub use designer::DesignerApi;
pub use version_manager::{DefinitionSummary, VersionManager};

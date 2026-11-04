// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub mod code_writer;
pub mod mapping_table;
pub mod task_builder;

use std::path::PathBuf;

use crate::analyzer::Tp6AnalysisReport;
use crate::config::MigrationConfig;
use crate::error::MigrationResult;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationResult {
    pub generated_files: Vec<PathBuf>,
    pub failed_interfaces: Vec<String>,
    pub total_retries: u32,
}

pub struct SzRustGenerator;

impl SzRustGenerator {
    pub fn new() -> Self {
        Self
    }

    pub async fn generate(
        &self,
        analysis: &Tp6AnalysisReport,
        config: &MigrationConfig,
        target_path: &std::path::Path,
    ) -> MigrationResult<GenerationResult> {
        let _ = (analysis, config, target_path);
        Ok(GenerationResult {
            generated_files: Vec::new(),
            failed_interfaces: Vec::new(),
            total_retries: 0,
        })
    }
}

impl Default for SzRustGenerator {
    fn default() -> Self {
        Self::new()
    }
}

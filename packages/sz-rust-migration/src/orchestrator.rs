// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::analyzer::{Tp6AnalysisInput, Tp6Analyzer};
use crate::config::MigrationConfig;
use crate::error::MigrationResult;
use crate::generator::SzRustGenerator;
use crate::metrics::MigrationMetrics;
use crate::phase::{MigrationPhase, MigrationStatus};
use crate::reporter::MigrationReporter;
use crate::validator::{HttpRequestSample, IncrementalValidator};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationStartRequest {
    pub tp6_project_path: PathBuf,
    pub config_path: PathBuf,
    pub output_dir: PathBuf,
    pub validation_samples: Vec<HttpRequestSample>,
    pub tenant_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationOutcome {
    pub migration_id: String,
    pub status: MigrationStatus,
    pub analysis_report_path: Option<PathBuf>,
    pub validation_report_path: Option<PathBuf>,
    pub migration_report_path: Option<PathBuf>,
    pub case_doc_path: Option<PathBuf>,
    pub coverage_percent: f64,
}

pub struct MigrationHandle {
    inner: Arc<Mutex<MigrationState>>,
}

struct MigrationState {
    migration_id: String,
    status: MigrationStatus,
}

impl MigrationHandle {
    pub async fn status(&self) -> MigrationStatus {
        self.inner.lock().await.status.clone()
    }

    pub async fn migration_id(&self) -> String {
        self.inner.lock().await.migration_id.clone()
    }
}

pub struct MigrationOrchestrator {
    metrics: Arc<MigrationMetrics>,
}

impl MigrationOrchestrator {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(MigrationMetrics::new()),
        }
    }

    pub async fn start(&self, request: MigrationStartRequest) -> MigrationResult<MigrationHandle> {
        let migration_id = uuid::Uuid::new_v4().to_string();
        tracing::info!(
            migration_id = %migration_id,
            "Starting migration for tenant {}",
            request.tenant_id
        );
        let state = MigrationState {
            migration_id: migration_id.clone(),
            status: MigrationStatus::Running {
                current_phase: MigrationPhase::Analysis,
            },
        };
        let handle = MigrationHandle {
            inner: Arc::new(Mutex::new(state)),
        };
        let metrics = self.metrics.clone();
        let config = MigrationConfig::load(&request.config_path).await?;
        let req = request;
        let id = migration_id.clone();
        tokio::spawn(async move {
            let _ = run_migration(id, req, config, metrics).await;
        });
        Ok(handle)
    }
}

impl Default for MigrationOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_migration(
    migration_id: String,
    request: MigrationStartRequest,
    config: MigrationConfig,
    metrics: Arc<MigrationMetrics>,
) -> MigrationResult<MigrationOutcome> {
    tokio::fs::create_dir_all(&request.output_dir).await?;

    let analysis_start = std::time::Instant::now();
    let analyzer = Tp6Analyzer::new();
    let analysis_input = Tp6AnalysisInput {
        project_path: request.tp6_project_path.clone(),
        exclude_paths: config.exclude_paths.clone(),
        llm_assist: false,
        trace_id: migration_id.clone(),
    };
    let analysis_report = analyzer.analyze(&analysis_input).await?;
    metrics.observe_analysis_duration(analysis_start.elapsed().as_secs_f64());

    let analysis_path = request.output_dir.join("analysis_report.json");
    crate::analyzer::report::AnalysisReportWriter::write_json(&analysis_report, &analysis_path)
        .await?;

    let target_path = if config.target_crate.path.is_empty() {
        request.output_dir.join("generated")
    } else {
        PathBuf::from(&config.target_crate.path)
    };
    tokio::fs::create_dir_all(&target_path).await?;
    let generator = SzRustGenerator::new();
    let generation_result = generator
        .generate(&analysis_report, &config, &target_path)
        .await?;

    let validator = IncrementalValidator::new(config.validation.clone());
    let validation_report = validator
        .validate(&analysis_report, &request.validation_samples)
        .await?;

    let validation_path = request.output_dir.join("validation_report.json");
    crate::validator::report::ValidationReportWriter::write_json(
        &validation_report,
        &validation_path,
    )
    .await?;

    let migration_report = MigrationReporter::build(
        &migration_id,
        &analysis_report,
        &generation_result,
        &validation_report,
    );
    metrics.set_coverage_percent(migration_report.coverage_percent);

    let report_path = request.output_dir.join("migration_report.md");
    MigrationReporter::write_markdown(&migration_report, &report_path).await?;

    let case_doc_path = request.output_dir.join("migration_case.md");
    let _ = crate::case_doc::CaseDocumentGenerator::generate(
        &analysis_report,
        &validation_report,
        &case_doc_path,
    )
    .await;

    Ok(MigrationOutcome {
        migration_id,
        status: MigrationStatus::Completed,
        analysis_report_path: Some(analysis_path),
        validation_report_path: Some(validation_path),
        migration_report_path: Some(report_path),
        case_doc_path: Some(case_doc_path),
        coverage_percent: migration_report.coverage_percent,
    })
}

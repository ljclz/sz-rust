// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub mod http_comparator;
pub mod report;
pub mod response_comparator;

use crate::analyzer::Tp6AnalysisReport;
use crate::config::ValidationConfig;
use crate::error::MigrationResult;
use crate::phase::HttpMethod;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HttpRequestSample {
    pub method: HttpMethod,
    pub path: String,
    pub body: Option<serde_json::Value>,
    pub headers: std::collections::HashMap<String, String>,
}

impl HttpRequestSample {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            body: None,
            headers: std::collections::HashMap::new(),
        }
    }

    pub fn post(path: impl Into<String>, body: serde_json::Value) -> Self {
        Self {
            method: HttpMethod::Post,
            path: path.into(),
            body: Some(body),
            headers: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationReport {
    pub total_interfaces: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub results: Vec<InterfaceValidationResult>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterfaceValidationResult {
    pub interface_id: String,
    pub request: HttpRequestSample,
    pub tp6_status: Option<u16>,
    pub sz_rust_status: Option<u16>,
    pub is_consistent: bool,
    pub diff: Option<String>,
}

pub struct IncrementalValidator {
    config: ValidationConfig,
}

impl IncrementalValidator {
    pub fn new(config: ValidationConfig) -> Self {
        Self { config }
    }

    pub async fn validate(
        &self,
        analysis: &Tp6AnalysisReport,
        samples: &[HttpRequestSample],
    ) -> MigrationResult<ValidationReport> {
        let _ = analysis;
        let mut results = Vec::new();
        for sample in samples {
            let tp6_resp = http_comparator::HttpComparator::send(
                &self.config.tp6_service_url,
                sample,
                self.config.timeout_secs,
            )
            .await;
            let sz_resp = http_comparator::HttpComparator::send(
                &self.config.sz_rust_service_url,
                sample,
                self.config.timeout_secs,
            )
            .await;
            let result = response_comparator::ResponseComparator::compare(
                sample.path.clone(),
                sample.clone(),
                tp6_resp.as_ref().ok(),
                sz_resp.as_ref().ok(),
            );
            results.push(result);
        }
        let passed = results.iter().filter(|r| r.is_consistent).count() as u32;
        let failed = results.iter().filter(|r| !r.is_consistent).count() as u32;
        Ok(ValidationReport {
            total_interfaces: results.len() as u32,
            passed,
            failed,
            skipped: 0,
            results,
        })
    }
}

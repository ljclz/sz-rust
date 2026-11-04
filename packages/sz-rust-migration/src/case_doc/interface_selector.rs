// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::validator::ValidationReport;

pub struct InterfaceSelector;

impl InterfaceSelector {
    pub fn select(report: &ValidationReport, min_count: usize) -> Vec<String> {
        let mut selected: Vec<String> = report
            .results
            .iter()
            .filter(|r| r.is_consistent)
            .take(min_count)
            .map(|r| r.interface_id.clone())
            .collect();
        if selected.len() < min_count {
            for result in &report.results {
                if !result.is_consistent {
                    selected.push(result.interface_id.clone());
                    if selected.len() >= min_count {
                        break;
                    }
                }
            }
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::validator::{HttpRequestSample, InterfaceValidationResult};

    #[test]
    fn select_from_passed() {
        let report = ValidationReport {
            total_interfaces: 3,
            passed: 2,
            failed: 1,
            skipped: 0,
            results: vec![
                InterfaceValidationResult {
                    interface_id: "a".into(),
                    request: HttpRequestSample::get("/a"),
                    tp6_status: Some(200),
                    sz_rust_status: Some(200),
                    is_consistent: true,
                    diff: None,
                },
                InterfaceValidationResult {
                    interface_id: "b".into(),
                    request: HttpRequestSample::get("/b"),
                    tp6_status: Some(200),
                    sz_rust_status: Some(200),
                    is_consistent: true,
                    diff: None,
                },
                InterfaceValidationResult {
                    interface_id: "c".into(),
                    request: HttpRequestSample::get("/c"),
                    tp6_status: Some(200),
                    sz_rust_status: Some(404),
                    is_consistent: false,
                    diff: Some("status mismatch".into()),
                },
            ],
        };
        let selected = InterfaceSelector::select(&report, 2);
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&"a".to_string()));
        assert!(selected.contains(&"b".to_string()));
    }
}

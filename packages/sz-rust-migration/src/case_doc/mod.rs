// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub mod interface_selector;
pub mod template;

use std::path::Path;

use crate::analyzer::Tp6AnalysisReport;
use crate::error::MigrationResult;
use crate::validator::ValidationReport;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseDocument {
    pub title: String,
    pub description: String,
    pub interfaces: Vec<CaseInterface>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseInterface {
    pub interface_id: String,
    pub tp6_code: String,
    pub sz_rust_code: String,
    pub notes: String,
}

pub struct CaseDocumentGenerator;

impl CaseDocumentGenerator {
    pub async fn generate(
        analysis: &Tp6AnalysisReport,
        validation: &ValidationReport,
        output_path: &Path,
    ) -> MigrationResult<CaseDocument> {
        let selected = interface_selector::InterfaceSelector::select(validation, 3);
        let mut interfaces = Vec::new();
        for iface_id in &selected {
            let tp6_code = analysis
                .routes
                .iter()
                .find(|r| format!("{}::{}", r.controller, r.action) == *iface_id)
                .map(|r| {
                    format!(
                        "Route::{}('{}', {}::{});",
                        r.method, r.path_pattern, r.controller, r.action
                    )
                })
                .unwrap_or_default();
            interfaces.push(CaseInterface {
                interface_id: iface_id.clone(),
                tp6_code,
                sz_rust_code: template::TemplateGenerator::generate_handler_template(iface_id),
                notes: String::new(),
            });
        }
        let doc = CaseDocument {
            title: "TP6 → sz-rust 迁移案例".into(),
            description: format!("基于 {} 个典型接口的迁移对比", selected.len()),
            interfaces,
        };
        let mut md = String::new();
        md.push_str(&format!("# {}\n\n{}\n\n", doc.title, doc.description));
        for iface in &doc.interfaces {
            md.push_str(&format!("## {}\n\n", iface.interface_id));
            md.push_str(&format!(
                "### TP6 代码\n\n```php\n{}\n```\n\n",
                iface.tp6_code
            ));
            md.push_str(&format!(
                "### sz-rust 代码\n\n```rust\n{}\n```\n\n",
                iface.sz_rust_code
            ));
        }
        tokio::fs::write(output_path, md).await?;
        Ok(doc)
    }
}

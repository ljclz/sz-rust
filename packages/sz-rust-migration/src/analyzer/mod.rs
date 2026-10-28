// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub mod complexity;
pub mod controller_parser;
pub mod dependency_graph;
pub mod middleware_parser;
pub mod model_parser;
pub mod report;
pub mod route_parser;
pub mod version_detector;

use std::path::PathBuf;

use crate::error::MigrationResult;
use crate::phase::ComplexitySummary;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6AnalysisInput {
    pub project_path: PathBuf,
    pub exclude_paths: Vec<PathBuf>,
    pub llm_assist: bool,
    pub trace_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Tp6AnalysisReport {
    pub version: String,
    pub routes: Vec<crate::analyzer::route_parser::Tp6Route>,
    pub models: Vec<crate::analyzer::model_parser::Tp6Model>,
    pub controllers: Vec<crate::analyzer::controller_parser::Tp6Controller>,
    pub middlewares: Vec<crate::analyzer::middleware_parser::Tp6Middleware>,
    pub dependency_graph: crate::analyzer::dependency_graph::DependencyGraph,
    pub complexity_summary: ComplexitySummary,
}

pub struct Tp6Analyzer;

impl Tp6Analyzer {
    pub fn new() -> Self {
        Self
    }

    pub async fn analyze(&self, input: &Tp6AnalysisInput) -> MigrationResult<Tp6AnalysisReport> {
        let version = crate::analyzer::version_detector::Tp6VersionDetector::new()
            .detect(&input.project_path)
            .await?;
        let routes = crate::analyzer::route_parser::Tp6RouteParser::new()
            .parse(&input.project_path.join("route"))
            .await?;
        let models = crate::analyzer::model_parser::Tp6ModelParser::new()
            .parse(&input.project_path.join("app").join("model"))
            .await?;
        let controllers = crate::analyzer::controller_parser::Tp6ControllerParser::new()
            .parse(&input.project_path.join("app").join("controller"))
            .await?;
        let middlewares = crate::analyzer::middleware_parser::Tp6MiddlewareParser::new()
            .parse(&input.project_path.join("app").join("middleware"))
            .await?;
        let dependency_graph = crate::analyzer::dependency_graph::DependencyGraphBuilder::build(
            &controllers,
            &models,
            &middlewares,
        );
        let complexity_summary =
            crate::analyzer::complexity::ComplexityAssessor::assess(&routes, &controllers, &models);
        Ok(Tp6AnalysisReport {
            version: format!("{}.{}.{}", version.major, version.minor, version.patch),
            routes,
            models,
            controllers,
            middlewares,
            dependency_graph,
            complexity_summary,
        })
    }
}

impl Default for Tp6Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

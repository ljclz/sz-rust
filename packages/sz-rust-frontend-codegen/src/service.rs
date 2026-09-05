// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 核心服务编排

use std::path::{Path, PathBuf};

use crate::config::{Framework, GenerationConfig};
use crate::error::FrontendCodegenError;
use crate::file_writer::FileWriter;
use crate::generators::api_client::ApiClientGenerator;
use crate::generators::permission::PermissionGenerator;
use crate::generators::react::ReactComponentGenerator;
use crate::generators::route::RouteGenerator;
use crate::generators::vue::VueComponentGenerator;
use crate::metadata::ModelMetadata;
use crate::model_parser::ModelParser;
use crate::report::{Failure, GenerationReport};
use crate::template_engine::CodegenTemplateEngine;
use crate::ui_adapter::UiAdapter;

/// 代码生成服务
pub struct CodegenService;

impl CodegenService {
    /// 创建新服务
    pub fn new() -> Self {
        Self
    }

    /// 执行生成流水线
    pub async fn generate(
        &self,
        config: GenerationConfig,
    ) -> Result<GenerationReport, FrontendCodegenError> {
        let started_at = chrono::Utc::now();
        let mut report = GenerationReport::new();
        report.started_at = started_at;

        if config.models.is_empty() {
            return Err(FrontendCodegenError::MissingModel);
        }

        let builtin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates");
        let engine =
            CodegenTemplateEngine::init(&builtin_dir, config.template_dir.as_deref()).await?;

        let all_models = ModelParser::parse_dir(&config.model_dir).await?;
        let target_models: Vec<ModelMetadata> = all_models
            .into_iter()
            .filter(|m| config.models.iter().any(|name| name == &m.name))
            .collect();

        if target_models.is_empty() {
            return Err(FrontendCodegenError::MissingModel);
        }

        let mut all_files: Vec<(PathBuf, String)> = Vec::new();

        for model in &target_models {
            let ui_adapted = UiAdapter::adapt(model, config.ui_library);
            let gen_result = match config.framework {
                Framework::Vue => {
                    VueComponentGenerator::new(&engine)
                        .generate(model, &ui_adapted, &config)
                        .await
                }
                Framework::React => {
                    ReactComponentGenerator::new(&engine)
                        .generate(model, &config)
                        .await
                }
            };
            match gen_result {
                Ok(files) => {
                    for f in files {
                        all_files.push((f.path.clone(), String::new()));
                        report.generated_files.push(f);
                    }
                }
                Err(e) => {
                    report.failures.push(Failure {
                        code: e.error_code().to_string(),
                        message: e.to_string(),
                        source_model: Some(model.name.clone()),
                        source_template: None,
                    });
                }
            }
        }

        let write_result =
            FileWriter::write_batch(all_files, &config.output_dir, config.override_strategy)
                .await?;
        report.skipped_files.extend(write_result.skipped);
        for (path, msg) in write_result.failed {
            report.failures.push(Failure {
                code: "FE_CODEGEN_FILE_WRITE_ERROR".to_string(),
                message: msg,
                source_model: None,
                source_template: None,
            });
            let _ = path;
        }

        report.finished_at = chrono::Utc::now();
        report.duration_ms = (report.finished_at - report.started_at).num_milliseconds() as u64;
        Ok(report)
    }
}

impl Default for CodegenService {
    fn default() -> Self {
        Self::new()
    }
}

/// 仅解析模型不生成
pub async fn parse_models(
    model_dir: &Path,
    models: &[String],
) -> Result<Vec<ModelMetadata>, FrontendCodegenError> {
    let all = ModelParser::parse_dir(model_dir).await?;
    Ok(all
        .into_iter()
        .filter(|m| models.iter().any(|name| name == &m.name))
        .collect())
}

/// 模板定义
#[derive(Debug, Clone)]
pub struct TemplateDefinition {
    /// 模板名
    pub name: String,
    /// 路径
    pub path: PathBuf,
    /// 是否自定义
    pub is_custom: bool,
}

/// 校验模板语法
pub async fn validate_templates(
    template_dir: &Path,
) -> Result<Vec<TemplateDefinition>, FrontendCodegenError> {
    let mut defs = Vec::new();
    let mut entries = tokio::fs::read_dir(template_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "tera") {
            let content = tokio::fs::read_to_string(&path).await?;
            tera::Tera::one_off(&content, &tera::Context::new(), true).map_err(|e| {
                FrontendCodegenError::TemplateSyntaxError(format!("{}: {e}", path.display()))
            })?;
            defs.push(TemplateDefinition {
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                path,
                is_custom: true,
            });
        }
    }
    Ok(defs)
}

#[allow(unused)]
fn _unused() {
    let _ = (
        RouteGenerator::new,
        PermissionGenerator::new,
        ApiClientGenerator::new,
    );
}

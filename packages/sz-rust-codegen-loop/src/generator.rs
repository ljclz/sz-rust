// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! AI 代码生成器：调用 LLM 生成代码，复用多模型路由。

use crate::error::CodegenError;
use crate::parser::CodegenTask;
use std::sync::Arc;
use sz_rust_ai_facade::llm::{ChatMessage, ModelRegistry, Role, RoutingStrategy};

/// 生成的文件。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// 生成器配置。
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    pub model_capability: String,
    pub strategy: RoutingStrategy,
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            model_capability: "codegen".into(),
            strategy: RoutingStrategy::Capability,
            max_tokens: 4096,
            temperature: 0.7,
        }
    }
}

/// AI 代码生成器。
pub struct Generator {
    registry: Arc<ModelRegistry>,
    config: GeneratorConfig,
}

impl Generator {
    /// 创建生成器。
    pub fn new(registry: Arc<ModelRegistry>, config: GeneratorConfig) -> Self {
        Self { registry, config }
    }

    /// 根据代码生成任务生成代码。
    ///
    /// 路由到具备 `codegen` 能力的模型（如 KAT-Coder-Pro-V2.5）。
    pub async fn generate(&self, task: &CodegenTask) -> Result<Vec<GeneratedFile>, CodegenError> {
        let candidates = self
            .registry
            .query_by_capability(&self.config.model_capability);
        if candidates.is_empty() {
            return Err(CodegenError::Generation(format!(
                "no model with capability '{}'",
                self.config.model_capability
            )));
        }

        let provider = &candidates[0].provider;
        let prompt = build_prompt(task);
        let req = sz_rust_ai_facade::llm::ChatRequest {
            model: candidates[0].name.clone(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "You are a code generator. Output only valid code.".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: prompt.into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
            ],
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(self.config.temperature),
            tools: None,
            stream: false,
        };

        let completion = provider.chat_completion(req).await?;
        let response = completion
            .choices
            .first()
            .map(|c| c.message.content.text_or_empty().to_string())
            .unwrap_or_default();

        parse_generated_files(&response)
    }
}

fn build_prompt(task: &CodegenTask) -> String {
    let mut prompt = format!(
        "Generate {} code using {} framework.\nFeature: {}\n",
        task.language, task.framework, task.feature
    );
    if !task.constraints.is_empty() {
        prompt.push_str(&format!("Constraints: {}\n", task.constraints.join(", ")));
    }
    prompt.push_str(&format!("\nRequirement: {}\n", task.raw_requirement));
    prompt.push_str("\nOutput format: ```path:file_path\ncontent\n```");
    prompt
}

fn parse_generated_files(response: &str) -> Result<Vec<GeneratedFile>, CodegenError> {
    let mut files = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_content = String::new();
    let mut in_block = false;

    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```path:") {
            in_block = true;
            current_path = Some(rest.trim().to_string());
            current_content.clear();
        } else if trimmed == "```" && in_block {
            if let Some(path) = current_path.take() {
                files.push(GeneratedFile {
                    path,
                    content: std::mem::take(&mut current_content),
                });
            }
            in_block = false;
        } else if in_block {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    if files.is_empty() {
        files.push(GeneratedFile {
            path: "generated.rs".into(),
            content: response.to_string(),
        });
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_generated_files_from_blocks() {
        let response =
            "```path:src/main.rs\nfn main() {}\n```\n```path:src/lib.rs\npub fn lib() {}\n```";
        let files = parse_generated_files(response).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/main.rs");
        assert!(files[0].content.contains("fn main()"));
        assert_eq!(files[1].path, "src/lib.rs");
    }

    #[test]
    fn parse_generated_files_fallback() {
        let response = "just some code without blocks";
        let files = parse_generated_files(response).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "generated.rs");
    }

    #[test]
    fn build_prompt_includes_feature() {
        let task = CodegenTask {
            language: crate::parser::Language::Rust,
            framework: crate::parser::Framework::Axum,
            feature: "login_api".into(),
            constraints: vec!["async".into()],
            raw_requirement: "生成登录 API".into(),
        };
        let prompt = build_prompt(&task);
        assert!(prompt.contains("login_api"));
        assert!(prompt.contains("async"));
    }

    #[test]
    fn generator_config_default() {
        let config = GeneratorConfig::default();
        assert_eq!(config.model_capability, "codegen");
        assert_eq!(config.strategy, RoutingStrategy::Capability);
        assert_eq!(config.max_tokens, 4096);
    }
}

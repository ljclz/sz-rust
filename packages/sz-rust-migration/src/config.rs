// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::MigrationError;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationConfig {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub model: ModelConfig,
    #[serde(default)]
    pub mapping: MappingConfig,
    #[serde(default)]
    pub exclude_paths: Vec<PathBuf>,
    #[serde(default)]
    pub target_crate: TargetCrateConfig,
    #[serde(default)]
    pub validation: ValidationConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            model: ModelConfig::default(),
            mapping: MappingConfig::default(),
            exclude_paths: Vec::new(),
            target_crate: TargetCrateConfig::default(),
            validation: ValidationConfig::default(),
            agent: AgentConfig::default(),
        }
    }
}

impl MigrationConfig {
    pub async fn load(path: &Path) -> Result<Self, MigrationError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| MigrationError::ConfigInvalid(format!("read config failed: {e}")))?;
        let config: Self = toml::from_str(&content).map_err(MigrationError::Toml)?;
        if config.version != "1.0.0" {
            tracing::warn!(
                "migration config version = {}, expected 1.0.0",
                config.version
            );
        }
        Ok(config)
    }

    pub fn validate(&self) -> Result<Vec<ConfigWarning>, MigrationError> {
        let mut warnings = Vec::new();
        if self.validation.tp6_service_url.is_empty() {
            warnings.push(ConfigWarning {
                field: "validation.tp6_service_url".into(),
                message: "TP6 service URL is empty, validation phase will be skipped".into(),
            });
        }
        if self.validation.sz_rust_service_url.is_empty() {
            warnings.push(ConfigWarning {
                field: "validation.sz_rust_service_url".into(),
                message: "sz-rust service URL is empty, validation phase will be skipped".into(),
            });
        }
        if self.target_crate.path.is_empty() {
            warnings.push(ConfigWarning {
                field: "target_crate.path".into(),
                message: "target crate path is empty, generation will use default".into(),
            });
        }
        Ok(warnings)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelConfig {
    #[serde(default = "default_analysis_model")]
    pub analysis_model: String,
    #[serde(default = "default_generation_model")]
    pub generation_model: String,
    #[serde(default = "default_validation_model")]
    pub validation_model: String,
    #[serde(default)]
    pub fallback_model: Option<String>,
}

fn default_analysis_model() -> String {
    "deepseek-chat".into()
}
fn default_generation_model() -> String {
    "deepseek-coder".into()
}
fn default_validation_model() -> String {
    "deepseek-chat".into()
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            analysis_model: default_analysis_model(),
            generation_model: default_generation_model(),
            validation_model: default_validation_model(),
            fallback_model: None,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MappingConfig {
    #[serde(default)]
    pub route_mapping: HashMap<String, String>,
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    #[serde(default)]
    pub middleware_mapping: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TargetCrateConfig {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub allow_overwrite: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationConfig {
    #[serde(default = "default_tp6_url")]
    pub tp6_service_url: String,
    #[serde(default = "default_sz_rust_url")]
    pub sz_rust_service_url: String,
    #[serde(default = "default_validation_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_validation_retries")]
    pub max_retries: u32,
}

fn default_tp6_url() -> String {
    "http://localhost:8000".into()
}
fn default_sz_rust_url() -> String {
    "http://localhost:3000".into()
}
fn default_validation_timeout() -> u64 {
    10
}
fn default_validation_retries() -> u32 {
    3
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            tp6_service_url: default_tp6_url(),
            sz_rust_service_url: default_sz_rust_url(),
            timeout_secs: default_validation_timeout(),
            max_retries: default_validation_retries(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_agent_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_compile_timeout")]
    pub compile_timeout_secs: u64,
    #[serde(default = "default_test_timeout")]
    pub test_timeout_secs: u64,
}

fn default_agent_max_retries() -> u32 {
    5
}
fn default_compile_timeout() -> u64 {
    120
}
fn default_test_timeout() -> u64 {
    300
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_retries: default_agent_max_retries(),
            compile_timeout_secs: default_compile_timeout(),
            test_timeout_secs: default_test_timeout(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigWarning {
    pub field: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = MigrationConfig::default();
        assert_eq!(config.version, "1.0.0");
        assert_eq!(config.validation.timeout_secs, 10);
        assert_eq!(config.validation.max_retries, 3);
        assert_eq!(config.agent.max_retries, 5);
        assert_eq!(config.model.analysis_model, "deepseek-chat");
        assert_eq!(config.model.generation_model, "deepseek-coder");
    }

    #[test]
    fn validate_empty_urls_warns() {
        let mut config = MigrationConfig::default();
        config.validation.tp6_service_url = String::new();
        config.validation.sz_rust_service_url = String::new();
        config.target_crate.path = String::new();
        let warnings = config.validate().unwrap();
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn validate_normal_config_no_warnings() {
        let mut config = MigrationConfig::default();
        config.target_crate.path = "packages/generated".into();
        let warnings = config.validate().unwrap();
        assert!(warnings.is_empty());
    }

    #[tokio::test]
    async fn load_valid_toml() {
        let toml_content = r#"
version = "1.0.0"

[model]
analysis_model = "gpt-4"
generation_model = "gpt-4"

[validation]
tp6_service_url = "http://tp6:8000"
sz_rust_service_url = "http://sz:3000"
timeout_secs = 30
"#;
        let temp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(temp.path(), toml_content).await.unwrap();
        let config = MigrationConfig::load(temp.path()).await.unwrap();
        assert_eq!(config.model.analysis_model, "gpt-4");
        assert_eq!(config.validation.timeout_secs, 30);
    }

    #[tokio::test]
    async fn load_missing_version_defaults() {
        let toml_content = "";
        let temp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(temp.path(), toml_content).await.unwrap();
        let config = MigrationConfig::load(temp.path()).await.unwrap();
        assert_eq!(config.version, "1.0.0");
    }
}

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 生成配置

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::FrontendCodegenError;

/// 前端框架
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    /// Vue 3
    #[default]
    Vue,
    /// React 18
    React,
}

/// UI 组件库
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiLibrary {
    /// Element Plus
    #[default]
    ElementPlus,
    /// Ant Design Vue
    AntDesignVue,
}

/// 覆盖策略
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverrideStrategy {
    /// 跳过已存在文件
    #[default]
    Skip,
    /// 强制覆盖
    Overwrite,
    /// 合并
    Merge,
}

/// 生成配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    /// 要生成的模型名列表
    pub models: Vec<String>,
    /// 模型目录
    pub model_dir: PathBuf,
    /// 前端框架
    pub framework: Framework,
    /// UI 组件库
    pub ui_library: UiLibrary,
    /// 输出目录
    pub output_dir: PathBuf,
    /// 自定义模板目录
    pub template_dir: Option<PathBuf>,
    /// 覆盖策略
    pub override_strategy: OverrideStrategy,
    /// 是否生成测试骨架
    pub with_tests: bool,
    /// 是否生成请求拦截器
    pub with_interceptors: bool,
    /// 是否懒加载路由
    pub lazy_load: bool,
    /// 强制覆盖非空输出目录
    pub force: bool,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            models: Vec::new(),
            model_dir: PathBuf::from("src/model/"),
            framework: Framework::default(),
            ui_library: UiLibrary::default(),
            output_dir: PathBuf::from("./frontend/"),
            template_dir: None,
            override_strategy: OverrideStrategy::default(),
            with_tests: false,
            with_interceptors: false,
            lazy_load: true,
            force: false,
        }
    }
}

/// 从 `.codegen.toml` 加载配置
pub async fn load_config_file(path: &Path) -> Result<GenerationConfig, FrontendCodegenError> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| FrontendCodegenError::ConfigParseError(format!("读取配置文件失败: {e}")))?;
    let config: GenerationConfig = toml::from_str(&content)
        .map_err(|e| FrontendCodegenError::ConfigParseError(format!("解析配置文件失败: {e}")))?;
    Ok(config)
}

/// 合并配置：CLI 参数覆盖配置文件值（CLI 非默认值优先）
pub fn merge_config(file_config: GenerationConfig, cli_args: GenerationConfig) -> GenerationConfig {
    GenerationConfig {
        models: if cli_args.models.is_empty() {
            file_config.models
        } else {
            cli_args.models
        },
        model_dir: if cli_args.model_dir == GenerationConfig::default().model_dir {
            file_config.model_dir
        } else {
            cli_args.model_dir
        },
        framework: if cli_args.framework == GenerationConfig::default().framework {
            file_config.framework
        } else {
            cli_args.framework
        },
        ui_library: if cli_args.ui_library == GenerationConfig::default().ui_library {
            file_config.ui_library
        } else {
            cli_args.ui_library
        },
        output_dir: if cli_args.output_dir == GenerationConfig::default().output_dir {
            file_config.output_dir
        } else {
            cli_args.output_dir
        },
        template_dir: cli_args.template_dir.or(file_config.template_dir),
        override_strategy: if cli_args.override_strategy
            == GenerationConfig::default().override_strategy
        {
            file_config.override_strategy
        } else {
            cli_args.override_strategy
        },
        with_tests: cli_args.with_tests || file_config.with_tests,
        with_interceptors: cli_args.with_interceptors || file_config.with_interceptors,
        lazy_load: cli_args.lazy_load && file_config.lazy_load,
        force: cli_args.force || file_config.force,
    }
}

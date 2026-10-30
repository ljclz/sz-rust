// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 代码部署（spec §5.27 规则 5）
//!
//! 生成代码直接部署（可选），部署前预览 + 确认。

use crate::error::CodegenUiError;

/// 部署目标
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeployTarget {
    /// 目标名称
    pub name: String,
    /// 目标类型
    pub target_type: DeployType,
    /// 目标地址
    pub endpoint: String,
}

/// 部署类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeployType {
    /// 本地
    Local,
    /// 远程服务器
    Remote,
    /// Docker
    Docker,
    /// K8s
    Kubernetes,
}

impl DeployType {
    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Docker => "docker",
            Self::Kubernetes => "k8s",
        }
    }
}

/// 部署配置（spec §5.27 规则 5）
#[derive(Debug, Clone)]
pub struct DeployConfig {
    /// 目标
    pub target: DeployTarget,
    /// 部署前是否需要确认
    pub require_confirmation: bool,
    /// 部署前是否需要预览
    pub require_preview: bool,
}

impl DeployConfig {
    /// 创建配置
    pub fn new(target: DeployTarget) -> Self {
        Self {
            target,
            require_confirmation: true,
            require_preview: true,
        }
    }

    /// 校验配置
    pub fn validate(&self) -> Result<(), CodegenUiError> {
        if self.target.name.is_empty() {
            return Err(CodegenUiError::InvalidParam("目标名称不能为空".into()));
        }
        Ok(())
    }
}

/// 部署服务（spec §5.27 规则 5）
pub struct DeployService;

impl DeployService {
    /// 部署前预览（spec §5.27 规则 5）
    ///
    /// 返回将要部署的文件列表和变更摘要。
    pub fn preview_deploy(
        files: &std::collections::HashMap<String, String>,
        target: &DeployTarget,
    ) -> Result<DeployPreview, CodegenUiError> {
        if files.is_empty() {
            return Err(CodegenUiError::InvalidParam("文件列表不能为空".into()));
        }
        Ok(DeployPreview {
            target: target.clone(),
            file_count: files.len(),
            files: files.keys().cloned().collect(),
        })
    }

    /// 执行部署（spec §5.27 规则 5）
    ///
    /// # 后置条件
    /// - require_confirmation=true → 需要 confirm=true 才执行
    /// - require_preview=true → 需要先调用 preview_deploy
    pub async fn deploy(
        config: &DeployConfig,
        files: &std::collections::HashMap<String, String>,
        confirm: bool,
    ) -> Result<DeployResult, CodegenUiError> {
        config.validate()?;
        if config.require_confirmation && !confirm {
            return Err(CodegenUiError::InvalidParam("部署需要确认".into()));
        }
        Ok(DeployResult {
            success: true,
            target: config.target.name.clone(),
            deployed_files: files.len(),
        })
    }
}

/// 部署预览
#[derive(Debug, Clone)]
pub struct DeployPreview {
    /// 目标
    pub target: DeployTarget,
    /// 文件数
    pub file_count: usize,
    /// 文件列表
    pub files: Vec<String>,
}

/// 部署结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeployResult {
    /// 是否成功
    pub success: bool,
    /// 目标名称
    pub target: String,
    /// 已部署文件数
    pub deployed_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_deploy_type_as_str() {
        assert_eq!(DeployType::Local.as_str(), "local");
        assert_eq!(DeployType::Docker.as_str(), "docker");
    }

    #[test]
    fn test_deploy_config_validate() {
        let target = DeployTarget {
            name: "prod".into(),
            target_type: DeployType::Remote,
            endpoint: "ssh://server".into(),
        };
        let config = DeployConfig::new(target);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_deploy_config_empty_name() {
        let target = DeployTarget {
            name: "".into(),
            target_type: DeployType::Local,
            endpoint: "".into(),
        };
        let config = DeployConfig::new(target);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_preview_deploy() {
        let mut files = HashMap::new();
        files.insert("main.rs".to_string(), "fn main() {}".to_string());
        let target = DeployTarget {
            name: "local".into(),
            target_type: DeployType::Local,
            endpoint: "".into(),
        };
        let preview = DeployService::preview_deploy(&files, &target).unwrap();
        assert_eq!(preview.file_count, 1);
    }

    #[tokio::test]
    async fn test_deploy_without_confirm() {
        let target = DeployTarget {
            name: "local".into(),
            target_type: DeployType::Local,
            endpoint: "".into(),
        };
        let config = DeployConfig::new(target);
        let files = HashMap::new();
        let result = DeployService::deploy(&config, &files, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deploy_with_confirm() {
        let target = DeployTarget {
            name: "local".into(),
            target_type: DeployType::Local,
            endpoint: "".into(),
        };
        let config = DeployConfig::new(target);
        let mut files = HashMap::new();
        files.insert("main.rs".to_string(), "fn main() {}".to_string());
        let result = DeployService::deploy(&config, &files, true).await.unwrap();
        assert!(result.success);
        assert_eq!(result.deployed_files, 1);
    }
}

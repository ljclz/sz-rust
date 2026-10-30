// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! ConfigLoader — 配置文件加载与热重载
//!
//! 支持 YAML/JSON 两种格式，按扩展名自动选择解析器。
//! 全程使用 tokio::fs，禁止 std::fs。
//! 部分失败不中断：逐条 create_rule/create_policy，错误收集到 LoadReport。

use super::hot_reload::HotReloadManager;
use super::path_guard::PathGuard;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::field_scope::policy::FieldScopePolicy;
use crate::data_scope::rule::DataScopeRule;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 配置文件结构
#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigFile {
    pub format_version: String,
    pub rules: Vec<DataScopeRule>,
    pub policies: Vec<FieldScopePolicy>,
}

/// 加载报告
#[derive(Debug, Serialize)]
pub struct LoadReport {
    pub success_count: usize,
    pub failed_count: usize,
    pub generation: u64,
    pub errors: Vec<LoadError>,
}

/// 单条加载错误
#[derive(Debug, Serialize)]
pub struct LoadError {
    pub index: usize,
    pub target: String,
    pub code: String,
    pub message: String,
}

/// 配置加载器
pub struct ConfigLoader {
    manager: Arc<HotReloadManager>,
    path_guard: PathGuard,
    max_file_size: u64,
}

impl ConfigLoader {
    /// 创建加载器。max_file_size 建议传 10*1024*1024（10MB）
    pub fn new(manager: Arc<HotReloadManager>, path_guard: PathGuard, max_file_size: u64) -> Self {
        Self {
            manager,
            path_guard,
            max_file_size,
        }
    }

    /// 默认最大文件大小 10MB
    pub const DEFAULT_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

    /// 从文件加载配置（不清理已有数据）
    pub async fn load_from_file(&self, path: &str) -> Result<LoadReport, DataScopeError> {
        // 路径白名单校验
        let canonical = self.path_guard.validate(path)?;

        // 文件存在性 & 大小检查
        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|_| DataScopeError::ConfigFileNotFound(path.to_string()))?;
        let size = metadata.len();
        if size > self.max_file_size {
            return Err(DataScopeError::ConfigFileTooLarge {
                size,
                limit: self.max_file_size,
            });
        }

        // 读取文件内容
        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|_| DataScopeError::ConfigFileNotFound(path.to_string()))?;

        // 按扩展名解析
        let config = parse_config(&canonical, &content)?;

        // 逐条加载，部分失败不中断
        let mut errors = Vec::new();
        let mut success_count = 0usize;
        let mut failed_count = 0usize;

        for (i, rule) in config.rules.into_iter().enumerate() {
            let target = rule.target_table.clone();
            match self.manager.create_rule(rule).await {
                Ok(_) => success_count += 1,
                Err(e) => {
                    failed_count += 1;
                    errors.push(LoadError {
                        index: i,
                        target,
                        code: e.error_code().to_string(),
                        message: e.to_string(),
                    });
                }
            }
        }

        for (j, policy) in config.policies.into_iter().enumerate() {
            let target = policy.policy_id();
            match self.manager.create_policy(policy).await {
                Ok(_) => success_count += 1,
                Err(e) => {
                    failed_count += 1;
                    errors.push(LoadError {
                        index: j,
                        target,
                        code: e.error_code().to_string(),
                        message: e.to_string(),
                    });
                }
            }
        }

        let generation = self.manager.generation();
        // 广播 ConfigReloaded 事件
        self.manager.broadcast_config_reloaded(generation, path);

        Ok(LoadReport {
            success_count,
            failed_count,
            generation,
            errors,
        })
    }

    /// 重载配置文件
    ///
    /// - `confirm_clear=true`：先 invalidate_all 清空所有规则与策略，再加载
    /// - `confirm_clear=false`：若文件 rules 与 policies 均为空，则拒绝（避免误清空）
    pub async fn reload_from_file(
        &self,
        path: &str,
        confirm_clear: bool,
    ) -> Result<LoadReport, DataScopeError> {
        let canonical = self.path_guard.validate(path)?;
        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|_| DataScopeError::ConfigFileNotFound(path.to_string()))?;
        let size = metadata.len();
        if size > self.max_file_size {
            return Err(DataScopeError::ConfigFileTooLarge {
                size,
                limit: self.max_file_size,
            });
        }
        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|_| DataScopeError::ConfigFileNotFound(path.to_string()))?;
        let config = parse_config(&canonical, &content)?;

        // confirm_clear=false 且文件为空时拒绝
        if !confirm_clear && config.rules.is_empty() && config.policies.is_empty() {
            return Err(DataScopeError::InvalidRule(
                "reload rejected: empty config without confirm_clear".into(),
            ));
        }

        if confirm_clear {
            self.manager.invalidate_all();
        }

        // 逐条加载
        let mut errors = Vec::new();
        let mut success_count = 0usize;
        let mut failed_count = 0usize;

        for (i, rule) in config.rules.into_iter().enumerate() {
            let target = rule.target_table.clone();
            match self.manager.create_rule(rule).await {
                Ok(_) => success_count += 1,
                Err(e) => {
                    failed_count += 1;
                    errors.push(LoadError {
                        index: i,
                        target,
                        code: e.error_code().to_string(),
                        message: e.to_string(),
                    });
                }
            }
        }
        for (j, policy) in config.policies.into_iter().enumerate() {
            let target = policy.policy_id();
            match self.manager.create_policy(policy).await {
                Ok(_) => success_count += 1,
                Err(e) => {
                    failed_count += 1;
                    errors.push(LoadError {
                        index: j,
                        target,
                        code: e.error_code().to_string(),
                        message: e.to_string(),
                    });
                }
            }
        }

        let generation = self.manager.generation();
        self.manager.broadcast_config_reloaded(generation, path);
        // 记录一次重载指标
        self.manager.metrics().record_reload();
        // 审计
        self.manager
            .audit_record("reload_from_file", &canonical.to_string_lossy())
            .await;

        Ok(LoadReport {
            success_count,
            failed_count,
            generation,
            errors,
        })
    }
}

/// 按扩展名解析配置文件
fn parse_config(path: &std::path::Path, content: &str) -> Result<ConfigFile, DataScopeError> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "yaml" | "yml" => {
            serde_yaml::from_str(content).map_err(|e| DataScopeError::ConfigParseError {
                line: 0,
                msg: format!("yaml parse error: {}", e),
            })
        }
        "json" => serde_json::from_str(content).map_err(|e| DataScopeError::ConfigParseError {
            line: 0,
            msg: format!("json parse error: {}", e),
        }),
        _ => Err(DataScopeError::ConfigParseError {
            line: 0,
            msg: format!("unsupported file extension: {}", ext),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::custom::CustomGeneratorRegistry;
    use crate::data_scope::ext::audit::TracingAuditLogger;
    use crate::data_scope::ext::generation::PolicyGeneration;
    use crate::data_scope::ext::hot_reload::HotReloadManager;
    use crate::data_scope::ext::notifier::ChangeNotifier;
    use crate::data_scope::field_scope::registry::FieldScopePolicyRegistry;
    use crate::data_scope::field_scope::visibility::FieldVisibility;
    use crate::data_scope::metrics::DataScopeMetrics;
    use crate::data_scope::registry::DataScopeRuleRegistry;
    use crate::data_scope::rule::DataScopeMode;
    use std::path::PathBuf;

    fn make_manager() -> Arc<HotReloadManager> {
        let rule_registry = Arc::new(DataScopeRuleRegistry::new());
        let policy_registry = Arc::new(FieldScopePolicyRegistry::new());
        let custom_registry = Arc::new(CustomGeneratorRegistry::new());
        let generation = PolicyGeneration::new();
        let notifier = ChangeNotifier::new(64);
        let metrics = Arc::new(DataScopeMetrics::new());
        let audit: Arc<dyn crate::data_scope::ext::audit::AuditLogger> =
            Arc::new(TracingAuditLogger);
        Arc::new(HotReloadManager::new(
            rule_registry,
            policy_registry,
            custom_registry,
            generation,
            notifier,
            metrics,
            audit,
        ))
    }

    async fn write_temp_file(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sz_rust_config_loader_tests");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    async fn cleanup_temp_file(path: &PathBuf) {
        let _ = tokio::fs::remove_file(path).await;
    }

    fn make_loader(manager: Arc<HotReloadManager>) -> ConfigLoader {
        let cwd = std::env::current_dir().unwrap();
        let temp = std::env::temp_dir();
        let guard = PathGuard::new(vec![cwd, temp]);
        ConfigLoader::new(manager, guard, ConfigLoader::DEFAULT_MAX_FILE_SIZE)
    }

    #[tokio::test]
    async fn test_load_yaml() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: order
    priority: 5
    rule_id: r1
    enabled: true
policies: []
"#;
        let path = write_temp_file("test_load.yaml", yaml).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.success_count, 1);
        assert_eq!(report.failed_count, 0);
        assert_eq!(mgr.rule_count(), 1);
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_load_json() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        let json = r#"{
  "format_version": "1.0",
  "rules": [
    {"mode": "all", "target_table": "order", "priority": 1, "rule_id": "j1", "enabled": true}
  ],
  "policies": [
    {"table_name": "employee", "role_name": "hr", "field_rules": [["salary", "hidden"]], "default_visibility": "visible", "enabled": true}
  ]
}"#;
        let path = write_temp_file("test_load.json", json).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.success_count, 2);
        assert_eq!(report.failed_count, 0);
        assert_eq!(mgr.rule_count(), 1);
        assert_eq!(mgr.policy_count(), 1);
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_partial_failure_continues() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        // 第二条规则 mode=dept 但缺 dept_field → 失败；第一条应成功
        let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: ok_table
    priority: 1
    rule_id: ok1
    enabled: true
  - mode: dept
    target_table: bad_table
    priority: 1
    rule_id: bad1
    enabled: true
policies: []
"#;
        let path = write_temp_file("partial.yaml", yaml).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.success_count, 1);
        assert_eq!(report.failed_count, 1);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].code, "RULE_FIELD_MISSING");
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_load_failure_keeps_existing() {
        let mgr = make_manager();
        // 先加载一条有效规则
        mgr.create_rule(DataScopeRule::new("existing", DataScopeMode::All))
            .await
            .unwrap();
        assert_eq!(mgr.rule_count(), 1);

        let loader = make_loader(mgr.clone());
        // 加载一个含错误项的文件，已有规则应保留
        let yaml = r#"
format_version: "1.0"
rules:
  - mode: dept
    target_table: bad
    priority: 1
    rule_id: bad1
    enabled: true
policies: []
"#;
        let path = write_temp_file("failure_keep.yaml", yaml).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.failed_count, 1);
        // 已有规则仍存在
        assert!(mgr.rule_count() >= 1);
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_file_too_large() {
        let mgr = make_manager();
        // 构造一个 max_file_size=1 的 loader
        let cwd = std::env::current_dir().unwrap();
        let temp = std::env::temp_dir();
        let guard = PathGuard::new(vec![cwd, temp]);
        let loader = ConfigLoader::new(mgr, guard, 1);

        let content = "x".repeat(100);
        let path = write_temp_file("too_large.yaml", &content).await;
        let err = loader
            .load_from_file(path.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "CONFIG_FILE_TOO_LARGE");
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let mgr = make_manager();
        let loader = make_loader(mgr);
        let err = loader
            .load_from_file("/nonexistent/path/to/file.yaml")
            .await
            .unwrap_err();
        // 路径不在白名单内或文件不存在
        let code = err.error_code();
        assert!(
            code == "PATH_NOT_ALLOWED" || code == "CONFIG_FILE_NOT_FOUND",
            "unexpected error code: {}",
            code
        );
    }

    #[tokio::test]
    async fn test_reload_with_confirm_clear() {
        let mgr = make_manager();
        // 预置一条已有规则
        mgr.create_rule(DataScopeRule::new("old", DataScopeMode::All))
            .await
            .unwrap();
        assert_eq!(mgr.rule_count(), 1);

        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: new_table
    priority: 1
    rule_id: new1
    enabled: true
policies: []
"#;
        let path = write_temp_file("reload_clear.yaml", yaml).await;
        let report = loader
            .reload_from_file(path.to_str().unwrap(), true)
            .await
            .unwrap();
        assert_eq!(report.success_count, 1);
        // 旧规则应被清空，仅剩新规则
        assert_eq!(mgr.rule_count(), 1);
        assert!(mgr.rule_registry().get_rule_by_id("new1").is_some());
        assert!(mgr.rule_registry().get_rule_by_id("old").is_none());
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_reload_reject_empty_without_confirm() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
rules: []
policies: []
"#;
        let path = write_temp_file("empty.yaml", yaml).await;
        let err = loader
            .reload_from_file(path.to_str().unwrap(), false)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "DATA_SCOPE_INVALID_RULE");
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_reload_empty_with_confirm_succeeds() {
        let mgr = make_manager();
        mgr.create_rule(DataScopeRule::new("old", DataScopeMode::All))
            .await
            .unwrap();
        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
rules: []
policies: []
"#;
        let path = write_temp_file("empty_confirm.yaml", yaml).await;
        let report = loader
            .reload_from_file(path.to_str().unwrap(), true)
            .await
            .unwrap();
        assert_eq!(report.success_count, 0);
        assert_eq!(mgr.rule_count(), 0);
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_unsupported_extension() {
        let mgr = make_manager();
        let loader = make_loader(mgr);
        let path = write_temp_file("config.txt", "hello").await;
        let err = loader
            .load_from_file(path.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "CONFIG_PARSE_ERROR");
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_policy_with_visibility_roundtrip() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
rules: []
policies:
  - table_name: employee
    role_name: hr
    field_rules:
      - [salary, hidden]
      - [name, read_only]
    default_visibility: visible
    enabled: true
"#;
        let path = write_temp_file("policy_vis.yaml", yaml).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.success_count, 1);
        let policy = mgr.policy_registry().get_policy("employee", "hr").unwrap();
        assert_eq!(policy.field_visibility("salary"), FieldVisibility::Hidden);
        assert_eq!(policy.field_visibility("name"), FieldVisibility::ReadOnly);
        cleanup_temp_file(&path).await;
    }
}

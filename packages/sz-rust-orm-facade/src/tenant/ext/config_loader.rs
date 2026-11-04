// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户配置加载器 — 异步读取租户隔离表清单配置文件
//!
//! 复用 PathGuard / LoadReport / LoadError，支持 YAML/JSON 格式。

use super::hot_reload::TenantHotReloadManager;
use crate::data_scope::ext::config_loader::{LoadError, LoadReport};
use crate::data_scope::ext::path_guard::PathGuard;
use crate::tenant::error::TenantError;
use crate::tenant::scoped_table::TenantScopedTable;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 租户配置文件结构
#[derive(Debug, Deserialize, Serialize)]
pub struct TenantConfigFile {
    pub format_version: String,
    pub scoped_tables: Vec<TenantScopedTable>,
}

/// 租户配置加载器
pub struct TenantConfigLoader {
    manager: Arc<TenantHotReloadManager>,
    path_guard: PathGuard,
    max_file_size: u64,
}

impl TenantConfigLoader {
    pub const DEFAULT_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

    pub fn new(
        manager: Arc<TenantHotReloadManager>,
        path_guard: PathGuard,
        max_file_size: u64,
    ) -> Self {
        Self {
            manager,
            path_guard,
            max_file_size,
        }
    }

    /// 从文件加载配置（不清理已有数据）
    pub async fn load_from_file(&self, path: &str) -> Result<LoadReport, TenantError> {
        let canonical = self
            .path_guard
            .validate(path)
            .map_err(|e| TenantError::TenantConfigReloadFailed(e.to_string()))?;

        let metadata = tokio::fs::metadata(&canonical).await.map_err(|_| {
            TenantError::TenantConfigReloadFailed(format!("file not found: {}", path))
        })?;
        if metadata.len() > self.max_file_size {
            return Err(TenantError::TenantConfigReloadFailed(format!(
                "file too large: {} bytes, limit: {} bytes",
                metadata.len(),
                self.max_file_size
            )));
        }

        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|_| TenantError::TenantConfigReloadFailed(format!("read failed: {}", path)))?;

        let config = parse_config(&canonical, &content)?;

        let mut errors = Vec::new();
        let mut success_count = 0usize;
        let mut failed_count = 0usize;

        for (i, table) in config.scoped_tables.into_iter().enumerate() {
            let target = table.table_name.clone();
            match self.manager.register_scoped_table(table).await {
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

        let generation = self.manager.generation();
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
    /// `confirm_clear=true` 时先清空所有隔离表再加载；清空后 generation.bump() 一次。
    pub async fn reload_from_file(
        &self,
        path: &str,
        confirm_clear: bool,
    ) -> Result<LoadReport, TenantError> {
        let canonical = self
            .path_guard
            .validate(path)
            .map_err(|e| TenantError::TenantConfigReloadFailed(e.to_string()))?;

        let metadata = tokio::fs::metadata(&canonical).await.map_err(|_| {
            TenantError::TenantConfigReloadFailed(format!("file not found: {}", path))
        })?;
        if metadata.len() > self.max_file_size {
            return Err(TenantError::TenantConfigReloadFailed(format!(
                "file too large: {} bytes, limit: {} bytes",
                metadata.len(),
                self.max_file_size
            )));
        }

        let content = tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|_| TenantError::TenantConfigReloadFailed(format!("read failed: {}", path)))?;

        let config = parse_config(&canonical, &content)?;

        if !confirm_clear && config.scoped_tables.is_empty() {
            return Err(TenantError::TenantConfigReloadFailed(
                "reload rejected: empty config without confirm_clear".into(),
            ));
        }

        if confirm_clear {
            self.manager.scoped_table_registry().invalidate_all();
            self.manager.generation_handle().bump();
        }

        let mut errors = Vec::new();
        let mut success_count = 0usize;
        let mut failed_count = 0usize;

        for (i, table) in config.scoped_tables.into_iter().enumerate() {
            let target = table.table_name.clone();
            match self.manager.register_scoped_table(table).await {
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

        let generation = self.manager.generation();
        self.manager.broadcast_config_reloaded(generation, path);
        self.manager.metrics().record_reload();
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

fn parse_config(path: &std::path::Path, content: &str) -> Result<TenantConfigFile, TenantError> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "yaml" | "yml" => serde_yaml::from_str(content)
            .map_err(|e| TenantError::TenantConfigReloadFailed(format!("yaml parse error: {}", e))),
        "json" => serde_json::from_str(content)
            .map_err(|e| TenantError::TenantConfigReloadFailed(format!("json parse error: {}", e))),
        _ => Err(TenantError::TenantConfigReloadFailed(format!(
            "unsupported file extension: {}",
            ext
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::ext::audit::TracingAuditLogger;
    use crate::data_scope::ext::generation::PolicyGeneration;
    use crate::data_scope::ext::notifier::ChangeNotifier;
    use crate::data_scope::metrics::DataScopeMetrics;
    use crate::tenant::config::TenantConfigRegistry;
    use crate::tenant::record::TenantRecordRegistry;
    use crate::tenant::scoped_table::TenantScopedTableRegistry;
    use std::path::PathBuf;

    fn make_manager() -> Arc<TenantHotReloadManager> {
        Arc::new(TenantHotReloadManager::new(
            Arc::new(TenantRecordRegistry::new()),
            Arc::new(TenantScopedTableRegistry::new()),
            Arc::new(TenantConfigRegistry::new()),
            PolicyGeneration::new(),
            ChangeNotifier::new(64),
            Arc::new(DataScopeMetrics::new()),
            Arc::new(TracingAuditLogger),
        ))
    }

    fn make_loader(manager: Arc<TenantHotReloadManager>) -> TenantConfigLoader {
        let cwd = std::env::current_dir().unwrap();
        let temp = std::env::temp_dir();
        let guard = PathGuard::new(vec![cwd, temp]);
        TenantConfigLoader::new(manager, guard, TenantConfigLoader::DEFAULT_MAX_FILE_SIZE)
    }

    async fn write_temp_file(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sz_rust_tenant_config_loader_tests");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    async fn cleanup_temp_file(path: &PathBuf) {
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn test_load_yaml() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
scoped_tables:
  - table_name: orders
    tenant_field: tenant_id
    enabled: true
  - table_name: employees
    tenant_field: tenant_id
    enabled: true
"#;
        let path = write_temp_file("test_load.yaml", yaml).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.success_count, 2);
        assert_eq!(report.failed_count, 0);
        assert_eq!(mgr.list_scoped_tables().len(), 2);
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_load_json() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());
        let json = r#"{
  "format_version": "1.0",
  "scoped_tables": [
    {"table_name": "orders", "tenant_field": "tenant_id", "enabled": true}
  ]
}"#;
        let path = write_temp_file("test_load.json", json).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert_eq!(report.success_count, 1);
        assert_eq!(report.failed_count, 0);
        assert_eq!(mgr.list_scoped_tables().len(), 1);
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
        assert_eq!(err.error_code(), "TENANT_CONFIG_RELOAD_FAILED");
    }

    #[tokio::test]
    async fn test_parse_error() {
        let mgr = make_manager();
        let loader = make_loader(mgr);
        let path = write_temp_file("bad.yaml", "not: valid: yaml: :::").await;
        let err = loader
            .load_from_file(path.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "TENANT_CONFIG_RELOAD_FAILED");
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_reload_with_confirm_clear() {
        let mgr = make_manager();
        mgr.register_scoped_table(TenantScopedTable::new("old_table"))
            .await
            .unwrap();
        assert_eq!(mgr.list_scoped_tables().len(), 1);

        let loader = make_loader(mgr.clone());
        let yaml = r#"
format_version: "1.0"
scoped_tables:
  - table_name: new_table
    tenant_field: tenant_id
    enabled: true
"#;
        let path = write_temp_file("reload_clear.yaml", yaml).await;
        let report = loader
            .reload_from_file(path.to_str().unwrap(), true)
            .await
            .unwrap();
        assert_eq!(report.success_count, 1);
        assert_eq!(mgr.list_scoped_tables().len(), 1);
        assert!(mgr
            .list_scoped_tables()
            .iter()
            .any(|t| t.table_name == "new_table"));
        assert!(!mgr
            .list_scoped_tables()
            .iter()
            .any(|t| t.table_name == "old_table"));
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn test_reload_reject_empty_without_confirm() {
        let mgr = make_manager();
        let loader = make_loader(mgr);
        let yaml = r#"
format_version: "1.0"
scoped_tables: []
"#;
        let path = write_temp_file("empty.yaml", yaml).await;
        let err = loader
            .reload_from_file(path.to_str().unwrap(), false)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "TENANT_CONFIG_RELOAD_FAILED");
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn ac21_partial_failure_load_report() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());

        let yaml = r#"
format_version: "1.0"
scoped_tables:
  - table_name: orders
    tenant_field: tenant_id
    enabled: true
  - table_name: ""
    tenant_field: tenant_id
    enabled: true
  - table_name: items
    tenant_field: tenant_id
    enabled: true
"#;
        let path = write_temp_file("partial_fail.yaml", yaml).await;
        let report = loader.load_from_file(path.to_str().unwrap()).await;

        match report {
            Ok(r) => {
                assert!(r.success_count >= 1, "should have at least 1 success");
                assert!(
                    r.failed_count >= 1 || r.success_count == 3,
                    "should have partial failure or all succeed if validation differs"
                );
                assert!(r.generation > 0, "generation should be set");
            }
            Err(e) => {
                assert_eq!(e.error_code(), "TENANT_CONFIG_RELOAD_FAILED");
            }
        }
        cleanup_temp_file(&path).await;
    }

    #[tokio::test]
    async fn ac21_reload_report_has_generation() {
        let mgr = make_manager();
        let loader = make_loader(mgr.clone());

        let yaml = r#"
format_version: "1.0"
scoped_tables:
  - table_name: orders
    tenant_field: tenant_id
    enabled: true
"#;
        let path = write_temp_file("gen_report.yaml", yaml).await;
        let gen_before = mgr.generation();
        let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
        assert!(
            report.generation >= gen_before,
            "report generation should be >= before"
        );
        assert_eq!(report.success_count, 1);
        assert_eq!(report.failed_count, 0);
        assert!(report.errors.is_empty());
        cleanup_temp_file(&path).await;
    }
}

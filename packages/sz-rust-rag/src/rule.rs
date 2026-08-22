//! 业务规则库。

use crate::error::{RagError, RagResult};
use crate::store::{FileVersionedStore, VersionedStore};
use crate::term::fuzzy_match;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// 规则条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuleEntry {
    pub rule_name: String,
    pub rule_text: String,
    pub source_crate: String,
    pub source_file_path: String,
    pub source_line_start: u32,
    pub source_line_end: u32,
    pub applicable_scene: Option<String>,
    pub acceptance_criteria: Option<String>,
    pub version: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: String,
}

/// 规则存储 trait。
#[async_trait]
pub trait RuleStore: Send + Sync {
    async fn add(&self, entry: RuleEntry, tenant: &str) -> RagResult<RuleEntry>;
    async fn update(&self, name: &str, entry: RuleEntry, tenant: &str) -> RagResult<RuleEntry>;
    async fn delete(&self, name: &str, tenant: &str) -> RagResult<()>;
    async fn get(&self, name: &str, tenant: &str) -> RagResult<Option<RuleEntry>>;
    async fn search(&self, keyword: &str, tenant: &str) -> RagResult<Vec<RuleEntry>>;
    async fn history(&self, name: &str, tenant: &str) -> RagResult<Vec<RuleEntry>>;
    async fn validate_source(&self, entry: &RuleEntry, workspace_root: &Path) -> RagResult<()>;
}

/// 基于文件版本化存储的规则库实现。
pub struct FileRuleStore {
    store: Arc<FileVersionedStore<RuleEntry>>,
}

impl FileRuleStore {
    pub async fn new(file_path: &Path) -> RagResult<Self> {
        Ok(Self {
            store: Arc::new(FileVersionedStore::load(file_path).await?),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(FileVersionedStore::new_in_memory()),
        }
    }

    /// 从 rules.json 加载业务规则库，加载失败时降级为空表不阻断启动。
    pub async fn load_from_json(&self, path: &Path) -> RagResult<usize> {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("规则库加载失败，降级为空表: {e}");
                return Ok(0);
            }
        };
        let rules_json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("规则库 JSON 解析失败，降级为空表: {e}");
                return Ok(0);
            }
        };
        let mut count = 0;
        if let Some(rules) = rules_json.get("rules").and_then(|v| v.as_array()) {
            for rule in rules {
                let name = rule.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let desc = rule
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let src = rule
                    .get("source_project")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let loc = rule
                    .get("source_location")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let entry = RuleEntry {
                    rule_name: name.to_string(),
                    rule_text: desc.to_string(),
                    source_crate: src.to_string(),
                    source_file_path: loc.to_string(),
                    source_line_start: 0,
                    source_line_end: 0,
                    applicable_scene: rule
                        .get("severity")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    acceptance_criteria: rule
                        .get("condition")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    version: 1,
                    updated_at: chrono::Utc::now(),
                    updated_by: "rules.json".to_string(),
                };
                let _ = self
                    .store
                    .add(&entry.rule_name, entry.clone(), "default", "system")
                    .await;
                count += 1;
            }
        }
        tracing::info!("规则库加载完成: {count} 条");
        Ok(count)
    }
}

#[async_trait]
impl RuleStore for FileRuleStore {
    async fn add(&self, entry: RuleEntry, tenant: &str) -> RagResult<RuleEntry> {
        self.store
            .add(&entry.rule_name, entry.clone(), tenant, &entry.updated_by)
            .await
    }

    async fn update(&self, name: &str, entry: RuleEntry, tenant: &str) -> RagResult<RuleEntry> {
        self.store
            .update(name, entry.clone(), tenant, &entry.updated_by)
            .await
    }

    async fn delete(&self, name: &str, tenant: &str) -> RagResult<()> {
        self.store.delete(name, tenant).await
    }

    async fn get(&self, name: &str, tenant: &str) -> RagResult<Option<RuleEntry>> {
        self.store.get(name, tenant).await
    }

    async fn search(&self, keyword: &str, tenant: &str) -> RagResult<Vec<RuleEntry>> {
        let all = self.store.list(tenant).await?;
        let kw = keyword.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|e| {
                let name = e.rule_name.to_lowercase();
                let text = e.rule_text.to_lowercase();
                kw.contains(&name)
                    || name.contains(&kw)
                    || text.contains(&kw)
                    || fuzzy_match(&kw, &name)
                    || fuzzy_match(&kw, &text)
                    || e.applicable_scene
                        .as_ref()
                        .map(|s| {
                            let sc = s.to_lowercase();
                            kw.contains(&sc) || sc.contains(&kw) || fuzzy_match(&kw, &sc)
                        })
                        .unwrap_or(false)
            })
            .collect())
    }

    async fn history(&self, name: &str, tenant: &str) -> RagResult<Vec<RuleEntry>> {
        self.store.history(name, tenant).await
    }

    async fn validate_source(&self, entry: &RuleEntry, workspace_root: &Path) -> RagResult<()> {
        let file_path = workspace_root
            .join("packages")
            .join(&entry.source_crate)
            .join(&entry.source_file_path);
        if !tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
            return Err(RagError::Internal(format!(
                "source file not found: {}",
                file_path.display()
            )));
        }
        let content = tokio::fs::read_to_string(&file_path).await?;
        let line_count = content.lines().count() as u32;
        if entry.source_line_start > line_count || entry.source_line_end > line_count {
            return Err(RagError::Internal(format!(
                "source line out of range: {}-{} (file has {} lines)",
                entry.source_line_start, entry.source_line_end, line_count
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str) -> RuleEntry {
        RuleEntry {
            rule_name: name.into(),
            rule_text: "WHERE 必须参数化绑定".into(),
            source_crate: "sz-rust-core".into(),
            source_file_path: "src/lib.rs".into(),
            source_line_start: 1,
            source_line_end: 10,
            applicable_scene: Some("repository".into()),
            acceptance_criteria: Some("无拼接 SQL".into()),
            version: 1,
            updated_at: chrono::Utc::now(),
            updated_by: "tester".into(),
        }
    }

    #[tokio::test]
    async fn add_get_search() {
        let store = FileRuleStore::in_memory();
        store
            .add(make_entry("sql-injection-guard"), "t")
            .await
            .unwrap();
        let got = store.get("sql-injection-guard", "t").await.unwrap();
        assert!(got.is_some());

        let results = store.search("WHERE", "t").await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn update_history() {
        let store = FileRuleStore::in_memory();
        store.add(make_entry("R1"), "t").await.unwrap();
        let mut e2 = make_entry("R1");
        e2.rule_text = "updated rule".into();
        store.update("R1", e2, "t").await.unwrap();
        assert_eq!(store.history("R1", "t").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn validate_source_nonexistent() {
        let store = FileRuleStore::in_memory();
        let entry = make_entry("R1");
        let result = store
            .validate_source(&entry, Path::new("/nonexistent"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn new_from_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = FileRuleStore::new(tmp.path()).await.unwrap();
        store.add(make_entry("R1"), "t").await.unwrap();
        let got = store.get("R1", "t").await.unwrap();
        assert!(got.is_some());
    }

    #[tokio::test]
    async fn delete_rule() {
        let store = FileRuleStore::in_memory();
        store.add(make_entry("R1"), "t").await.unwrap();
        assert!(store.get("R1", "t").await.unwrap().is_some());
        store.delete("R1", "t").await.unwrap();
        assert!(store.get("R1", "t").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn load_from_json_missing_file() {
        let store = FileRuleStore::in_memory();
        let count = store
            .load_from_json(Path::new("/nonexistent/rules.json"))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn load_from_json_invalid_json() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(tmp.path(), "invalid json").await.unwrap();
        let store = FileRuleStore::in_memory();
        let count = store.load_from_json(tmp.path()).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn load_from_json_valid() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let json = r#"{"rules": [{"name": "R1", "description": "desc", "source_project": "p", "source_location": "loc", "severity": "high", "condition": "cond"}]}"#;
        tokio::fs::write(tmp.path(), json).await.unwrap();
        let store = FileRuleStore::in_memory();
        let count = store.load_from_json(tmp.path()).await.unwrap();
        assert_eq!(count, 1);
        let results = store.search("R1", "default").await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn search_by_applicable_scene() {
        let store = FileRuleStore::in_memory();
        store.add(make_entry("R1"), "t").await.unwrap();
        let results = store.search("repository", "t").await.unwrap();
        assert!(!results.is_empty());
    }
}

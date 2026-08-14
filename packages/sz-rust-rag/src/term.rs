//! 行业术语库。

use crate::error::RagResult;
use crate::store::{FileVersionedStore, VersionedStore};
use async_trait::async_trait;
use std::sync::Arc;

pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.len() < 2 || target.len() < 2 {
        return false;
    }
    let q_chars: Vec<char> = query.chars().collect();
    let t_chars: Vec<char> = target.chars().collect();
    for i in 0..t_chars.len().saturating_sub(1) {
        let bigram: String = t_chars[i..i + 2].iter().collect();
        if query.contains(&bigram) {
            return true;
        }
    }
    for i in 0..q_chars.len().saturating_sub(1) {
        let bigram: String = q_chars[i..i + 2].iter().collect();
        if target.contains(&bigram) {
            return true;
        }
    }
    false
}

/// 术语条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TermEntry {
    pub term_name: String,
    pub definition: String,
    pub aliases: Vec<String>,
    pub confusable_with: Vec<String>,
    pub version: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: String,
}

/// 术语存储 trait。
#[async_trait]
pub trait TermStore: Send + Sync {
    async fn add(&self, entry: TermEntry, tenant: &str) -> RagResult<TermEntry>;
    async fn update(&self, name: &str, entry: TermEntry, tenant: &str) -> RagResult<TermEntry>;
    async fn delete(&self, name: &str, tenant: &str) -> RagResult<()>;
    async fn get(&self, name: &str, tenant: &str) -> RagResult<Option<TermEntry>>;
    async fn search(&self, keyword: &str, tenant: &str) -> RagResult<Vec<TermEntry>>;
    async fn history(&self, name: &str, tenant: &str) -> RagResult<Vec<TermEntry>>;
    async fn references(&self, name: &str, tenant: &str) -> RagResult<Vec<String>>;
}

/// 基于文件版本化存储的术语库实现。
pub struct FileTermStore {
    store: Arc<FileVersionedStore<TermEntry>>,
}

impl FileTermStore {
    pub async fn new(file_path: &std::path::Path) -> RagResult<Self> {
        Ok(Self {
            store: Arc::new(FileVersionedStore::load(file_path).await?),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(FileVersionedStore::new_in_memory()),
        }
    }

    /// 从 glossary.json 加载术语表，加载失败时降级为空表不阻断启动。
    pub async fn load_from_json(&self, path: &std::path::Path) -> RagResult<usize> {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("术语表加载失败，降级为空表: {e}");
                return Ok(0);
            }
        };
        let glossary: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("术语表 JSON 解析失败，降级为空表: {e}");
                return Ok(0);
            }
        };
        let mut count = 0;
        if let Some(categories) = glossary.get("categories").and_then(|v| v.as_object()) {
            for (_cat_key, cat_val) in categories {
                if let Some(terms) = cat_val.get("terms").and_then(|v| v.as_array()) {
                    for term in terms {
                        let name = term.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let def = term
                            .get("definition")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let entry = TermEntry {
                            term_name: name.to_string(),
                            definition: def.to_string(),
                            aliases: vec![],
                            confusable_with: vec![],
                            version: 1,
                            updated_at: chrono::Utc::now(),
                            updated_by: "glossary.json".to_string(),
                        };
                        let _ = self
                            .store
                            .add(&entry.term_name, entry.clone(), "default", "system")
                            .await;
                        count += 1;
                    }
                }
            }
        }
        tracing::info!("术语表加载完成: {count} 条");
        Ok(count)
    }
}

#[async_trait]
impl TermStore for FileTermStore {
    async fn add(&self, entry: TermEntry, tenant: &str) -> RagResult<TermEntry> {
        self.store
            .add(&entry.term_name, entry.clone(), tenant, &entry.updated_by)
            .await
    }

    async fn update(&self, name: &str, entry: TermEntry, tenant: &str) -> RagResult<TermEntry> {
        self.store
            .update(name, entry.clone(), tenant, &entry.updated_by)
            .await
    }

    async fn delete(&self, name: &str, tenant: &str) -> RagResult<()> {
        self.store.delete(name, tenant).await
    }

    async fn get(&self, name: &str, tenant: &str) -> RagResult<Option<TermEntry>> {
        self.store.get(name, tenant).await
    }

    async fn search(&self, keyword: &str, tenant: &str) -> RagResult<Vec<TermEntry>> {
        let all = self.store.list(tenant).await?;
        let kw = keyword.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|e| {
                let name = e.term_name.to_lowercase();
                let def = e.definition.to_lowercase();
                kw.contains(&name)
                    || name.contains(&kw)
                    || def.contains(&kw)
                    || fuzzy_match(&kw, &name)
                    || fuzzy_match(&kw, &def)
                    || e.aliases.iter().any(|a| {
                        let al = a.to_lowercase();
                        kw.contains(&al) || al.contains(&kw) || fuzzy_match(&kw, &al)
                    })
            })
            .collect())
    }

    async fn history(&self, name: &str, tenant: &str) -> RagResult<Vec<TermEntry>> {
        self.store.history(name, tenant).await
    }

    async fn references(&self, name: &str, tenant: &str) -> RagResult<Vec<String>> {
        let _ = (name, tenant);
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str) -> TermEntry {
        TermEntry {
            term_name: name.into(),
            definition: "test definition".into(),
            aliases: vec!["alias1".into()],
            confusable_with: vec![],
            version: 1,
            updated_at: chrono::Utc::now(),
            updated_by: "tester".into(),
        }
    }

    #[tokio::test]
    async fn add_get() {
        let store = FileTermStore::in_memory();
        store.add(make_entry("SKU"), "t").await.unwrap();
        let got = store.get("SKU", "t").await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().term_name, "SKU");
    }

    #[tokio::test]
    async fn search_by_keyword() {
        let store = FileTermStore::in_memory();
        store.add(make_entry("SKU"), "t").await.unwrap();
        store.add(make_entry("冷链"), "t").await.unwrap();
        let results = store.search("sku", "t").await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn update_and_history() {
        let store = FileTermStore::in_memory();
        store.add(make_entry("SKU"), "t").await.unwrap();
        let mut e2 = make_entry("SKU");
        e2.definition = "updated".into();
        store.update("SKU", e2, "t").await.unwrap();
        let history = store.history("SKU", "t").await.unwrap();
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn delete() {
        let store = FileTermStore::in_memory();
        store.add(make_entry("SKU"), "t").await.unwrap();
        store.delete("SKU", "t").await.unwrap();
        assert!(store.get("SKU", "t").await.unwrap().is_none());
    }
}

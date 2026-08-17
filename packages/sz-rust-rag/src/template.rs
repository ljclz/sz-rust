//! 数据模型模板库。

use crate::error::RagResult;
use crate::store::{FileVersionedStore, VersionedStore};
use crate::term::fuzzy_match;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// 模板字段。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateField {
    pub field_name: String,
    pub business_meaning: String,
    pub constraint: Option<String>,
}

/// 数据模型模板。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelTemplate {
    pub object_name: String,
    pub fields: Vec<TemplateField>,
    pub version: u64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by: String,
}

/// 模板存储 trait。
#[async_trait]
pub trait TemplateStore: Send + Sync {
    async fn add(&self, template: ModelTemplate, tenant: &str) -> RagResult<ModelTemplate>;
    async fn update(
        &self,
        name: &str,
        template: ModelTemplate,
        tenant: &str,
    ) -> RagResult<ModelTemplate>;
    async fn delete(&self, name: &str, tenant: &str) -> RagResult<()>;
    async fn get(&self, name: &str, tenant: &str) -> RagResult<Option<ModelTemplate>>;
    async fn search(&self, keyword: &str, tenant: &str) -> RagResult<Vec<ModelTemplate>>;
    async fn history(&self, name: &str, tenant: &str) -> RagResult<Vec<ModelTemplate>>;
}

/// 基于文件版本化存储的模板库实现。
pub struct FileTemplateStore {
    store: Arc<FileVersionedStore<ModelTemplate>>,
}

impl FileTemplateStore {
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

    /// 从 templates.json 加载数据模型模板库，加载失败时降级为空表不阻断启动。
    pub async fn load_from_json(&self, path: &Path) -> RagResult<usize> {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("模板库加载失败，降级为空表: {e}");
                return Ok(0);
            }
        };
        let templates_json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("模板库 JSON 解析失败，降级为空表: {e}");
                return Ok(0);
            }
        };
        let mut count = 0;
        if let Some(templates) = templates_json.get("templates").and_then(|v| v.as_array()) {
            for tmpl in templates {
                let name = tmpl
                    .get("entity_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut fields = Vec::new();
                if let Some(fields_arr) = tmpl.get("fields").and_then(|v| v.as_array()) {
                    for f in fields_arr {
                        let fname = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let ftype = f.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let comment = f.get("comment").and_then(|v| v.as_str()).unwrap_or("");
                        fields.push(TemplateField {
                            field_name: fname.to_string(),
                            business_meaning: if comment.is_empty() {
                                ftype.to_string()
                            } else {
                                comment.to_string()
                            },
                            constraint: f.get("nullable").and_then(|v| v.as_bool()).map(|b| {
                                if b {
                                    "nullable".to_string()
                                } else {
                                    "not_null".to_string()
                                }
                            }),
                        });
                    }
                }
                let entry = ModelTemplate {
                    object_name: name.to_string(),
                    fields,
                    version: 1,
                    updated_at: chrono::Utc::now(),
                    updated_by: "templates.json".to_string(),
                };
                let _ = self
                    .store
                    .add(&entry.object_name, entry.clone(), "default", "system")
                    .await;
                count += 1;
            }
        }
        tracing::info!("模板库加载完成: {count} 条");
        Ok(count)
    }
}

#[async_trait]
impl TemplateStore for FileTemplateStore {
    async fn add(&self, template: ModelTemplate, tenant: &str) -> RagResult<ModelTemplate> {
        self.store
            .add(
                &template.object_name,
                template.clone(),
                tenant,
                &template.updated_by,
            )
            .await
    }

    async fn update(
        &self,
        name: &str,
        template: ModelTemplate,
        tenant: &str,
    ) -> RagResult<ModelTemplate> {
        self.store
            .update(name, template.clone(), tenant, &template.updated_by)
            .await
    }

    async fn delete(&self, name: &str, tenant: &str) -> RagResult<()> {
        self.store.delete(name, tenant).await
    }

    async fn get(&self, name: &str, tenant: &str) -> RagResult<Option<ModelTemplate>> {
        self.store.get(name, tenant).await
    }

    async fn search(&self, keyword: &str, tenant: &str) -> RagResult<Vec<ModelTemplate>> {
        let all = self.store.list(tenant).await?;
        let kw = keyword.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|t| {
                let name = t.object_name.to_lowercase();
                kw.contains(&name)
                    || name.contains(&kw)
                    || fuzzy_match(&kw, &name)
                    || t.fields.iter().any(|f| {
                        let fn_ = f.field_name.to_lowercase();
                        let bm = f.business_meaning.to_lowercase();
                        kw.contains(&fn_)
                            || fn_.contains(&kw)
                            || bm.contains(&kw)
                            || fuzzy_match(&kw, &bm)
                    })
            })
            .collect())
    }

    async fn history(&self, name: &str, tenant: &str) -> RagResult<Vec<ModelTemplate>> {
        self.store.history(name, tenant).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_template(name: &str) -> ModelTemplate {
        ModelTemplate {
            object_name: name.into(),
            fields: vec![TemplateField {
                field_name: "sku_code".into(),
                business_meaning: "商品 SKU 编码".into(),
                constraint: Some("non-empty".into()),
            }],
            version: 1,
            updated_at: chrono::Utc::now(),
            updated_by: "tester".into(),
        }
    }

    #[tokio::test]
    async fn add_get_search() {
        let store = FileTemplateStore::in_memory();
        store.add(make_template("Product"), "t").await.unwrap();
        let got = store.get("Product", "t").await.unwrap();
        assert!(got.is_some());

        let results = store.search("sku", "t").await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn update_and_delete() {
        let store = FileTemplateStore::in_memory();
        store.add(make_template("Order"), "t").await.unwrap();
        let mut t2 = make_template("Order");
        t2.fields.push(TemplateField {
            field_name: "amount".into(),
            business_meaning: "订单金额".into(),
            constraint: None,
        });
        store.update("Order", t2, "t").await.unwrap();
        let got = store.get("Order", "t").await.unwrap().unwrap();
        assert_eq!(got.fields.len(), 2);

        store.delete("Order", "t").await.unwrap();
        assert!(store.get("Order", "t").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn new_from_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = FileTemplateStore::new(tmp.path()).await.unwrap();
        store.add(make_template("T1"), "t").await.unwrap();
        assert!(store.get("T1", "t").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn history() {
        let store = FileTemplateStore::in_memory();
        store.add(make_template("T1"), "t").await.unwrap();
        let mut t2 = make_template("T1");
        t2.fields.push(TemplateField {
            field_name: "x".into(),
            business_meaning: "y".into(),
            constraint: None,
        });
        store.update("T1", t2, "t").await.unwrap();
        let history = store.history("T1", "t").await.unwrap();
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn load_from_json_missing_file() {
        let store = FileTemplateStore::in_memory();
        let count = store
            .load_from_json(std::path::Path::new("/nonexistent/templates.json"))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn load_from_json_invalid_json() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(tmp.path(), "invalid json").await.unwrap();
        let store = FileTemplateStore::in_memory();
        let count = store.load_from_json(tmp.path()).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn load_from_json_valid() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let json = r#"{"templates": [{"entity_name": "Product", "fields": [{"name": "sku", "type": "string", "comment": "商品编码", "nullable": false}, {"name": "price", "type": "f64", "nullable": true}]}]}"#;
        tokio::fs::write(tmp.path(), json).await.unwrap();
        let store = FileTemplateStore::in_memory();
        let count = store.load_from_json(tmp.path()).await.unwrap();
        assert_eq!(count, 1);
        let results = store.search("product", "default").await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn search_by_field_name() {
        let store = FileTemplateStore::in_memory();
        store.add(make_template("Product"), "t").await.unwrap();
        let results = store.search("sku_code", "t").await.unwrap();
        assert!(!results.is_empty());
    }
}

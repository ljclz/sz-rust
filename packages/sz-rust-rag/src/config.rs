// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! RAG 配置加载与热更新。

use crate::error::RagResult;
use crate::warning::RagWarningCode;
use arc_swap::ArcSwap;
use notify::Watcher;
use std::sync::Arc;
use tracing::warn;

/// RAG 配置。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagConfig {
    pub version: String,
    pub embedding_model: String,
    pub vector_dimensions: usize,
    pub similarity_metric: sz_rust_ai_facade::embedding::SimilarityMetric,
    pub default_topk: usize,
    pub max_topk: usize,
    pub default_token_budget: u32,
    pub low_recall_threshold: f32,
    pub embedding_concurrency: usize,
    pub chunk_max_chars: usize,
    pub embedding_max_retries: u32,
    pub knowledge_dir: String,
    pub audit_log_path: String,
    pub vectorization_journal_path: String,
}

impl RagConfig {
    /// 从 config/rag.toml 异步加载，缺失字段填默认值并告警。
    pub async fn load(path: &std::path::Path) -> RagResult<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let raw: toml::Value = toml::from_str(&content)?;

        let get_or_warn = |key: &str, default: &str| -> String {
            match raw.get(key) {
                Some(toml::Value::String(s)) => s.clone(),
                _ => {
                    warn!(
                        code = RagWarningCode::ConfigFieldMissing.as_str(),
                        field = key,
                        "config field missing, using default"
                    );
                    default.to_string()
                }
            }
        };

        let version = get_or_warn("version", "1.0.0");
        let embedding_model = get_or_warn("embedding_model", "text-embedding-3-small");
        let knowledge_dir = get_or_warn("knowledge_dir", "knowledge");
        let audit_log_path = get_or_warn("audit_log_path", "logs/rag-audit.jsonl");
        let vectorization_journal_path =
            get_or_warn("vectorization_journal_path", "logs/rag-journal.jsonl");

        let vector_dimensions = raw
            .get("vector_dimensions")
            .and_then(|v| v.as_integer())
            .map(|v| v as usize)
            .unwrap_or_else(|| {
                warn!(
                    code = RagWarningCode::ConfigFieldMissing.as_str(),
                    "vector_dimensions default 1536"
                );
                1536
            });
        let default_topk = raw
            .get("default_topk")
            .and_then(|v| v.as_integer())
            .map(|v| v as usize)
            .unwrap_or(5);
        let max_topk = raw
            .get("max_topk")
            .and_then(|v| v.as_integer())
            .map(|v| v as usize)
            .unwrap_or(20);
        let default_token_budget = raw
            .get("default_token_budget")
            .and_then(|v| v.as_integer())
            .map(|v| v as u32)
            .unwrap_or(4096);
        let low_recall_threshold = raw
            .get("low_recall_threshold")
            .and_then(|v| v.as_float())
            .map(|v| v as f32)
            .unwrap_or(0.65);
        let embedding_concurrency = raw
            .get("embedding_concurrency")
            .and_then(|v| v.as_integer())
            .map(|v| v as usize)
            .unwrap_or(4);
        let chunk_max_chars = raw
            .get("chunk_max_chars")
            .and_then(|v| v.as_integer())
            .map(|v| v as usize)
            .unwrap_or(1200);
        let embedding_max_retries = raw
            .get("embedding_max_retries")
            .and_then(|v| v.as_integer())
            .map(|v| v as u32)
            .unwrap_or(3);

        let similarity_metric = raw
            .get("similarity_metric")
            .and_then(|v| v.as_str())
            .map(parse_similarity_metric)
            .unwrap_or(sz_rust_ai_facade::embedding::SimilarityMetric::Cosine);

        Ok(Self {
            version,
            embedding_model,
            vector_dimensions,
            similarity_metric,
            default_topk,
            max_topk,
            default_token_budget,
            low_recall_threshold,
            embedding_concurrency,
            chunk_max_chars,
            embedding_max_retries,
            knowledge_dir,
            audit_log_path,
            vectorization_journal_path,
        })
    }

    /// 生产默认配置（用于测试）。
    pub fn for_testing() -> Self {
        Self {
            version: "test".into(),
            embedding_model: "test-model".into(),
            vector_dimensions: 8,
            similarity_metric: sz_rust_ai_facade::embedding::SimilarityMetric::Cosine,
            default_topk: 3,
            max_topk: 10,
            default_token_budget: 2048,
            low_recall_threshold: 0.5,
            embedding_concurrency: 2,
            chunk_max_chars: 800,
            embedding_max_retries: 1,
            knowledge_dir: "knowledge".into(),
            audit_log_path: "logs/rag-audit.jsonl".into(),
            vectorization_journal_path: "logs/rag-journal.jsonl".into(),
        }
    }
}

fn parse_similarity_metric(s: &str) -> sz_rust_ai_facade::embedding::SimilarityMetric {
    match s.to_lowercase().as_str() {
        "dot" | "dotproduct" => sz_rust_ai_facade::embedding::SimilarityMetric::Dot,
        "l2" | "euclidean" => sz_rust_ai_facade::embedding::SimilarityMetric::L2,
        _ => sz_rust_ai_facade::embedding::SimilarityMetric::Cosine,
    }
}

/// 配置热更新 watcher（notify + arc-swap）。
pub struct RagConfigWatcher {
    config: Arc<ArcSwap<RagConfig>>,
    _watcher: notify::RecommendedWatcher,
}

impl RagConfigWatcher {
    /// 启动文件监听并返回 watcher。
    pub async fn start(path: &std::path::Path) -> RagResult<Arc<Self>> {
        let initial = RagConfig::load(path).await?;
        let config = Arc::new(ArcSwap::from_pointee(initial));

        let config_clone = config.clone();
        let path_buf = path.to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if res.is_ok() {
                let path_clone = path_buf.clone();
                let config_clone = config_clone.clone();
                tokio::spawn(async move {
                    if let Ok(new_cfg) = RagConfig::load(&path_clone).await {
                        config_clone.store(Arc::new(new_cfg));
                        tracing::info!("rag config hot-reloaded");
                    }
                });
            }
        })?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                watcher.watch(parent, notify::RecursiveMode::NonRecursive)?;
            }
        }

        Ok(Arc::new(Self {
            config,
            _watcher: watcher,
        }))
    }

    /// 获取当前配置快照（无锁读）。
    pub fn current(&self) -> Arc<RagConfig> {
        self.config.load_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_default_config() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(tmp.path(), "").await.unwrap();
        let cfg = RagConfig::load(tmp.path()).await.unwrap();
        assert_eq!(cfg.vector_dimensions, 1536);
        assert_eq!(cfg.default_topk, 5);
    }

    #[tokio::test]
    async fn load_full_config() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let toml_content = r#"
version = "2.0.0"
embedding_model = "bge-large"
vector_dimensions = 1024
default_topk = 8
max_topk = 32
default_token_budget = 8192
low_recall_threshold = 0.7
embedding_concurrency = 8
chunk_max_chars = 1500
embedding_max_retries = 5
knowledge_dir = "/data/knowledge"
audit_log_path = "/data/logs/audit.jsonl"
vectorization_journal_path = "/data/logs/journal.jsonl"
"#;
        tokio::fs::write(tmp.path(), toml_content).await.unwrap();
        let cfg = RagConfig::load(tmp.path()).await.unwrap();
        assert_eq!(cfg.version, "2.0.0");
        assert_eq!(cfg.vector_dimensions, 1024);
        assert_eq!(cfg.default_topk, 8);
    }

    #[test]
    fn for_testing_sane() {
        let cfg = RagConfig::for_testing();
        assert!(cfg.max_topk >= cfg.default_topk);
        assert!(cfg.low_recall_threshold > 0.0 && cfg.low_recall_threshold < 1.0);
    }

    #[tokio::test]
    async fn load_with_similarity_metric() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let toml_content = r#"
similarity_metric = "dot"
"#;
        tokio::fs::write(tmp.path(), toml_content).await.unwrap();
        let cfg = RagConfig::load(tmp.path()).await.unwrap();
        assert!(matches!(
            cfg.similarity_metric,
            sz_rust_ai_facade::embedding::SimilarityMetric::Dot
        ));
    }

    #[tokio::test]
    async fn load_similarity_metric_l2() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let toml_content = r#"
similarity_metric = "euclidean"
"#;
        tokio::fs::write(tmp.path(), toml_content).await.unwrap();
        let cfg = RagConfig::load(tmp.path()).await.unwrap();
        assert!(matches!(
            cfg.similarity_metric,
            sz_rust_ai_facade::embedding::SimilarityMetric::L2
        ));
    }

    #[tokio::test]
    async fn load_similarity_metric_unknown_defaults_cosine() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let toml_content = r#"
similarity_metric = "unknown_metric"
"#;
        tokio::fs::write(tmp.path(), toml_content).await.unwrap();
        let cfg = RagConfig::load(tmp.path()).await.unwrap();
        assert!(matches!(
            cfg.similarity_metric,
            sz_rust_ai_facade::embedding::SimilarityMetric::Cosine
        ));
    }

    // 注：RagConfigWatcher::start 在 Windows 上存在 notify 后台线程清理 panic 问题，
    // 因此不在此处测试。该功能在集成测试中验证。

    #[tokio::test]
    async fn load_nonexistent_path_fails() {
        let result = RagConfig::load(std::path::Path::new("/nonexistent/config.toml")).await;
        assert!(result.is_err());
    }
}

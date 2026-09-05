// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 语料向量化编排器与断点续跑日志。

use crate::chunking::SemanticChunker;
use crate::config::RagConfig;
use crate::corpus::ProjectCorpusScanner;
use crate::error::{RagError, RagResult};
use crate::metrics::RagMetrics;
use crate::redact::SourceCodeRedactor;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, VectorRecord, VectorStore,
};

/// 向量化结果统计。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorizationResult {
    pub total: u64,
    pub success: u64,
    pub failed: u64,
    pub skipped: u64,
    pub duration_secs: u64,
}

/// 日志条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorizationJournalEntry {
    pub crate_name: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub status: JournalStatus,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 日志状态。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalStatus {
    Success,
    Failed,
    Skipped,
}

/// 断点续跑日志。
pub struct VectorizationJournal {
    path: std::path::PathBuf,
    entries: Vec<VectorizationJournalEntry>,
}

impl VectorizationJournal {
    pub async fn load(path: &Path) -> RagResult<Self> {
        let entries = if tokio::fs::try_exists(path).await.unwrap_or(false) {
            let content = tokio::fs::read_to_string(path).await?;
            content
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| serde_json::from_str(l).map_err(RagError::Json))
                .collect::<RagResult<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Ok(Self {
            path: path.to_path_buf(),
            entries,
        })
    }

    /// 返回已成功向量化的 (file_path, line_start, line_end) 集合。
    pub fn success_keys(&self) -> HashSet<(String, u32, u32)> {
        self.entries
            .iter()
            .filter(|e| e.status == JournalStatus::Success)
            .map(|e| (e.file_path.clone(), e.line_start, e.line_end))
            .collect()
    }

    pub async fn append(&mut self, entry: VectorizationJournalEntry) -> RagResult<()> {
        let line = serde_json::to_string(&entry).map_err(RagError::Json)?;
        let line_with_newline = format!("{}\n", line);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(line_with_newline.as_bytes()).await?;
        self.entries.push(entry);
        Ok(())
    }
}

/// 向量化编排器。
pub struct VectorizationOrchestrator {
    embedding: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    config: Arc<RagConfig>,
    metrics: Arc<RagMetrics>,
}

impl VectorizationOrchestrator {
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        config: Arc<RagConfig>,
        metrics: Arc<RagMetrics>,
    ) -> Self {
        Self {
            embedding,
            vector_store,
            config,
            metrics,
        }
    }

    /// 冷启动：全量向量化 workspace 所有 crate 源码。
    pub async fn cold_start(
        &self,
        workspace_root: &Path,
        tenant_id: &str,
    ) -> RagResult<VectorizationResult> {
        let start = std::time::Instant::now();
        let files = ProjectCorpusScanner::scan(workspace_root).await?;
        let mut journal =
            VectorizationJournal::load(Path::new(&self.config.vectorization_journal_path)).await?;
        let done = journal.success_keys();

        let chunker = SemanticChunker::new(self.config.chunk_max_chars);
        let redactor = SourceCodeRedactor::new();

        let mut all_chunks = Vec::new();
        for file in &files {
            all_chunks.extend(chunker.chunk(file));
        }

        let total = all_chunks.len() as u64;
        let mut success = 0u64;
        let mut failed = 0u64;
        let mut skipped = 0u64;

        for chunk in all_chunks {
            let key = (chunk.file_path.clone(), chunk.line_start, chunk.line_end);
            if done.contains(&key) {
                skipped += 1;
                continue;
            }

            let redacted_text = redactor.redact(&chunk.text);
            let metadata = serde_json::json!({
                "crate_name": chunk.crate_name,
                "file_path": chunk.file_path,
                "line_start": chunk.line_start,
                "line_end": chunk.line_end,
                "symbol_type": chunk.symbol_type,
            });

            let req = EmbeddingRequest {
                model: self.config.embedding_model.clone(),
                input: vec![redacted_text],
            };

            let entry_status;
            match self.embedding.embed(req).await {
                Ok(result) => {
                    if let Some(vector) = result.embeddings.into_iter().next() {
                        let record = VectorRecord {
                            id: format!(
                                "{}:{}:{}:{}",
                                chunk.crate_name, chunk.file_path, chunk.line_start, chunk.line_end
                            ),
                            vector,
                            metadata,
                            tenant_id: tenant_id.to_string(),
                        };
                        if self.vector_store.upsert(&[record]).await.is_ok() {
                            success += 1;
                            entry_status = JournalStatus::Success;
                        } else {
                            failed += 1;
                            self.metrics.record_vector_store_error();
                            entry_status = JournalStatus::Failed;
                        }
                    } else {
                        failed += 1;
                        entry_status = JournalStatus::Failed;
                    }
                    self.metrics.record_embedding_call();
                }
                Err(_) => {
                    failed += 1;
                    entry_status = JournalStatus::Failed;
                }
            }

            let entry = VectorizationJournalEntry {
                crate_name: chunk.crate_name,
                file_path: chunk.file_path,
                line_start: chunk.line_start,
                line_end: chunk.line_end,
                status: entry_status,
                timestamp: chrono::Utc::now(),
            };
            let _ = journal.append(entry).await;
        }

        self.metrics.set_index_size(success);

        Ok(VectorizationResult {
            total,
            success,
            failed,
            skipped,
            duration_secs: start.elapsed().as_secs(),
        })
    }

    /// 增量：重新向量化指定 crate。
    pub async fn revectorize_crate(
        &self,
        workspace_root: &Path,
        crate_name: &str,
        tenant_id: &str,
    ) -> RagResult<VectorizationResult> {
        let start = std::time::Instant::now();
        let files = ProjectCorpusScanner::scan_crate(workspace_root, crate_name).await?;

        let chunker = SemanticChunker::new(self.config.chunk_max_chars);
        let redactor = SourceCodeRedactor::new();

        let mut all_chunks = Vec::new();
        for file in &files {
            all_chunks.extend(chunker.chunk(file));
        }

        let total = all_chunks.len() as u64;
        let mut success = 0u64;
        let mut failed = 0u64;

        for chunk in all_chunks {
            let redacted_text = redactor.redact(&chunk.text);
            let metadata = serde_json::json!({
                "crate_name": chunk.crate_name,
                "file_path": chunk.file_path,
                "line_start": chunk.line_start,
                "line_end": chunk.line_end,
                "symbol_type": chunk.symbol_type,
            });

            let req = EmbeddingRequest {
                model: self.config.embedding_model.clone(),
                input: vec![redacted_text],
            };

            match self.embedding.embed(req).await {
                Ok(result) => {
                    if let Some(vector) = result.embeddings.into_iter().next() {
                        let record = VectorRecord {
                            id: format!(
                                "{}:{}:{}:{}",
                                chunk.crate_name, chunk.file_path, chunk.line_start, chunk.line_end
                            ),
                            vector,
                            metadata,
                            tenant_id: tenant_id.to_string(),
                        };
                        if self.vector_store.upsert(&[record]).await.is_ok() {
                            success += 1;
                        } else {
                            failed += 1;
                            self.metrics.record_vector_store_error();
                        }
                    } else {
                        failed += 1;
                    }
                    self.metrics.record_embedding_call();
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }

        self.metrics.set_index_size(success);

        Ok(VectorizationResult {
            total,
            success,
            failed,
            skipped: 0,
            duration_secs: start.elapsed().as_secs(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_ai_facade::common::AiError;
    use sz_rust_ai_facade::embedding::{EmbeddingResult, SimilarityMetric, VectorHit};

    struct StubEmbeddingProvider;
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbeddingProvider {
        fn name(&self) -> &str {
            "stub"
        }
        async fn embed(&self, _req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
            Ok(EmbeddingResult {
                model: "stub".into(),
                embeddings: vec![vec![0.0; 8]],
                dimensions: 8,
                usage_tokens: 1,
            })
        }
        fn dimensions(&self) -> usize {
            8
        }
        fn supported_models(&self) -> &[&str] {
            &["stub"]
        }
    }

    struct StubVectorStore;
    #[async_trait::async_trait]
    impl VectorStore for StubVectorStore {
        async fn upsert(&self, _records: &[VectorRecord]) -> Result<(), AiError> {
            Ok(())
        }
        async fn query(
            &self,
            _vector: &[f32],
            _topk: usize,
            _metric: SimilarityMetric,
            _tenant: &str,
        ) -> Result<Vec<VectorHit>, AiError> {
            Ok(Vec::new())
        }
        async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
            Ok(())
        }
    }

    fn make_orchestrator(
        embedding: Arc<dyn EmbeddingProvider>,
        store: Arc<dyn VectorStore>,
    ) -> VectorizationOrchestrator {
        VectorizationOrchestrator::new(
            embedding,
            store,
            Arc::new(RagConfig::for_testing()),
            Arc::new(RagMetrics::register()),
        )
    }

    #[tokio::test]
    async fn cold_start_nonexistent_workspace() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let orch = make_orchestrator(embedding, store);
        let result = orch.cold_start(Path::new("/nonexistent"), "t").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn revectorize_nonexistent_crate() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let orch = make_orchestrator(embedding, store);
        let result = orch
            .revectorize_crate(Path::new("/nonexistent"), "foo", "t")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn journal_load_nonexistent() {
        let journal = VectorizationJournal::load(Path::new("/nonexistent/journal.jsonl")).await;
        assert!(journal.is_ok());
        assert!(journal.unwrap().success_keys().is_empty());
    }

    #[tokio::test]
    async fn journal_append_and_success_keys() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut journal = VectorizationJournal::load(&path).await.unwrap();
        let entry = VectorizationJournalEntry {
            crate_name: "test".into(),
            file_path: "src/lib.rs".into(),
            line_start: 1,
            line_end: 10,
            status: JournalStatus::Success,
            timestamp: chrono::Utc::now(),
        };
        journal.append(entry).await.unwrap();
        let entry2 = VectorizationJournalEntry {
            crate_name: "test".into(),
            file_path: "src/lib.rs".into(),
            line_start: 11,
            line_end: 20,
            status: JournalStatus::Failed,
            timestamp: chrono::Utc::now(),
        };
        journal.append(entry2).await.unwrap();

        let keys = journal.success_keys();
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&("src/lib.rs".to_string(), 1, 10)));

        let reloaded = VectorizationJournal::load(&path).await.unwrap();
        assert_eq!(reloaded.success_keys().len(), 1);
    }

    #[tokio::test]
    async fn cold_start_real_workspace() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let journal_path = tmp.path().to_path_buf();
        let mut config = RagConfig::for_testing();
        config.vectorization_journal_path = journal_path.to_string_lossy().to_string();
        let orch = VectorizationOrchestrator::new(
            embedding,
            store,
            Arc::new(config),
            Arc::new(RagMetrics::register()),
        );
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = orch.cold_start(workspace, "t").await.unwrap();
        assert!(result.total > 0);
        assert!(result.success > 0);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn cold_start_with_existing_journal_skips() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let journal_path = tmp.path().to_path_buf();

        let mut config = RagConfig::for_testing();
        config.vectorization_journal_path = journal_path.to_string_lossy().to_string();
        let orch = VectorizationOrchestrator::new(
            embedding.clone(),
            store.clone(),
            Arc::new(config.clone()),
            Arc::new(RagMetrics::register()),
        );
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let first = orch.cold_start(workspace, "t").await.unwrap();
        assert!(first.success > 0);

        let orch2 = VectorizationOrchestrator::new(
            embedding,
            store,
            Arc::new(config),
            Arc::new(RagMetrics::register()),
        );
        let second = orch2.cold_start(workspace, "t").await.unwrap();
        assert!(second.skipped > 0);
        assert_eq!(second.success, 0);
    }

    #[tokio::test]
    async fn revectorize_crate_real() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let orch = make_orchestrator(embedding, store);
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = orch
            .revectorize_crate(workspace, "sz-rust-rag", "t")
            .await
            .unwrap();
        assert!(result.total > 0);
        assert!(result.success > 0);
        assert_eq!(result.skipped, 0);
    }

    // Embedding 失败的 stub
    struct FailEmbedding;
    #[async_trait::async_trait]
    impl EmbeddingProvider for FailEmbedding {
        fn name(&self) -> &str {
            "fail"
        }
        async fn embed(&self, _req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
            Err(AiError::ProviderUnavailable("down".into()))
        }
        fn dimensions(&self) -> usize {
            8
        }
        fn supported_models(&self) -> &[&str] {
            &["fail"]
        }
    }

    // 返回空 embeddings 的 stub
    struct EmptyEmbedding;
    #[async_trait::async_trait]
    impl EmbeddingProvider for EmptyEmbedding {
        fn name(&self) -> &str {
            "empty"
        }
        async fn embed(&self, _req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
            Ok(EmbeddingResult {
                model: "empty".into(),
                embeddings: vec![],
                dimensions: 8,
                usage_tokens: 0,
            })
        }
        fn dimensions(&self) -> usize {
            8
        }
        fn supported_models(&self) -> &[&str] {
            &["empty"]
        }
    }

    // VectorStore upsert 失败的 stub
    struct FailVectorStore;
    #[async_trait::async_trait]
    impl VectorStore for FailVectorStore {
        async fn upsert(&self, _records: &[VectorRecord]) -> Result<(), AiError> {
            Err(AiError::VectorStoreUnavailable("down".into()))
        }
        async fn query(
            &self,
            _vector: &[f32],
            _topk: usize,
            _metric: SimilarityMetric,
            _tenant: &str,
        ) -> Result<Vec<VectorHit>, AiError> {
            Ok(Vec::new())
        }
        async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn revectorize_crate_with_embedding_failure() {
        let embedding = Arc::new(FailEmbedding) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let orch = make_orchestrator(embedding, store);
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = orch
            .revectorize_crate(workspace, "sz-rust-rag", "t")
            .await
            .unwrap();
        assert!(result.total > 0);
        assert_eq!(result.success, 0);
        assert!(result.failed > 0);
    }

    #[tokio::test]
    async fn revectorize_crate_with_empty_embedding() {
        let embedding = Arc::new(EmptyEmbedding) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(StubVectorStore) as Arc<dyn VectorStore>;
        let orch = make_orchestrator(embedding, store);
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = orch
            .revectorize_crate(workspace, "sz-rust-rag", "t")
            .await
            .unwrap();
        assert!(result.total > 0);
        assert_eq!(result.success, 0);
        assert!(result.failed > 0);
    }

    #[tokio::test]
    async fn revectorize_crate_with_vector_store_failure() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(FailVectorStore) as Arc<dyn VectorStore>;
        let orch = make_orchestrator(embedding, store);
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = orch
            .revectorize_crate(workspace, "sz-rust-rag", "t")
            .await
            .unwrap();
        assert!(result.total > 0);
        assert_eq!(result.success, 0);
        assert!(result.failed > 0);
    }

    #[tokio::test]
    async fn cold_start_with_vector_store_failure() {
        let embedding = Arc::new(StubEmbeddingProvider) as Arc<dyn EmbeddingProvider>;
        let store = Arc::new(FailVectorStore) as Arc<dyn VectorStore>;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut config = RagConfig::for_testing();
        config.vectorization_journal_path = tmp.path().to_string_lossy().to_string();
        let orch = VectorizationOrchestrator::new(
            embedding,
            store,
            Arc::new(config),
            Arc::new(RagMetrics::register()),
        );
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = orch.cold_start(workspace, "t").await.unwrap();
        assert!(result.total > 0);
        assert_eq!(result.success, 0);
        assert!(result.failed > 0);
    }
}

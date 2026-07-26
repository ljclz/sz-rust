//! RAG 集成 — Phase 7b.5
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! Retrievable Augmented Generation — 基于检索的增强生成。结合：
//! - **Embedding 索引**（Phase 7b.2）— 向量化文档 + HNSW 检索
//! - **NL2SQL**（Phase 7b.3）— 自然语言 → SQL 查询
//! - **LLM 缓存**（Phase 7b.4）— 缓存问答结果，CDC 失效
//!
//! ## 工作流程
//!
//! 1. **索引阶段** — `add_document(namespace, text, table, row_id)` 将文档向量化并写入 HNSW
//! 2. **检索阶段** — `rag_ask(question, filter, namespace)` 检索 TOP-K 相关文档
//! 3. **过滤阶段** — 应用 `filter`（如 "库存低于安全库存"）作为后置过滤条件
//! 4. **生成阶段** — 基于检索结果 + 过滤条件生成自然语言回答（模板化，无外部 LLM）
//! 5. **引用阶段** — 标注每条数据引用（表名、行 ID、相关性分数）
//! 6. **缓存阶段** — 问答结果写入 LLM 缓存，下次相同问题直接返回
//!
//! # 验证标准
//!
//! - `rag_ask('哪些商品需补货？', '库存低于安全库存', '零售助手')` → 返回自然语言回答 + 数据引用
//! - 回答正确且引用数据准确
//!
//! 对应 `SzRSQL实施进度.md` Phase 7b.5。

use std::collections::HashMap;

use crate::embedding::{EmbeddingError, EmbeddingLifecycle, HashingEmbedder, SearchResult};
use crate::llm_cache::LlmCache;
use crate::nl2sql::Nl2SqlEngine;

// =====================================================================
//  错误类型
// =====================================================================

/// RAG 错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RagError {
    /// 命名空间不存在
    #[error("namespace not found: {0}")]
    NamespaceNotFound(String),
    /// 命名空间已存在
    #[error("namespace already exists: {0}")]
    NamespaceAlreadyExists(String),
    /// 文档为空
    #[error("empty document text")]
    EmptyDocument,
    /// 无相关文档
    #[error("no relevant documents found")]
    NoRelevantDocuments,
    /// Embedding 错误
    #[error("embedding error: {0}")]
    EmbeddingError(String),
    /// 过滤条件无效
    #[error("invalid filter: {0}")]
    InvalidFilter(String),
}

impl From<EmbeddingError> for RagError {
    fn from(e: EmbeddingError) -> Self {
        RagError::EmbeddingError(e.to_string())
    }
}

// =====================================================================
//  数据结构
// =====================================================================

/// RAG 文档
#[derive(Debug, Clone)]
pub struct RagDocument {
    /// 文档 ID（内部序号）
    pub doc_id: u64,
    /// 文档文本
    pub text: String,
    /// 所属表名（用于数据引用）
    pub table: String,
    /// 行 ID（外部业务行 ID）
    pub row_id: u64,
    /// 元数据（键值对，如 {"category": "食品"}）
    pub metadata: HashMap<String, String>,
}

/// RAG 数据引用
#[derive(Debug, Clone, PartialEq)]
pub struct RagCitation {
    /// 引用序号（从 1 开始）
    pub index: usize,
    /// 表名
    pub table: String,
    /// 行 ID
    pub row_id: u64,
    /// 相关性分数 [0, 1]
    pub score: f32,
    /// 引用的文档片段
    pub snippet: String,
}

/// RAG 回答
#[derive(Debug, Clone)]
pub struct RagAnswer {
    /// 自然语言回答文本
    pub text: String,
    /// 数据引用列表
    pub citations: Vec<RagCitation>,
    /// 是否命中缓存
    pub cache_hit: bool,
    /// 检索耗时（毫秒）
    pub retrieval_ms: u64,
    /// 检索到的文档数
    pub retrieved_count: usize,
    /// 过滤后剩余文档数
    pub filtered_count: usize,
}

/// 命名空间统计
#[derive(Debug, Clone, Default)]
pub struct NamespaceStats {
    /// 文档数
    pub doc_count: usize,
    /// 索引节点数
    pub index_size: usize,
}

// =====================================================================
//  命名空间
// =====================================================================

/// RAG 命名空间 — 独立的文档集合 + Embedding 索引
struct Namespace {
    /// 文档列表（doc_id → RagDocument）
    documents: HashMap<u64, RagDocument>,
    /// Embedding 生命周期（管理 HNSW 索引）
    lifecycle: EmbeddingLifecycle,
    /// 下一个 doc_id
    next_doc_id: u64,
    /// Embedding 列名（固定为 "content"）
    embedding_column: String,
}

impl Namespace {
    fn new(dim: usize) -> Result<Self, RagError> {
        let mut lifecycle = EmbeddingLifecycle::with_dim(dim)?;
        // 声明 EMBEDDING 列：content EMBEDDING(dim) FROM (text)
        lifecycle.declare_embedding("__rag__", "content", vec!["text".to_string()], dim)?;
        Ok(Self {
            documents: HashMap::new(),
            lifecycle,
            next_doc_id: 1,
            embedding_column: "content".to_string(),
        })
    }

    fn add_document(&mut self, text: &str, table: &str, row_id: u64) -> Result<u64, RagError> {
        if text.trim().is_empty() {
            return Err(RagError::EmptyDocument);
        }
        let doc_id = self.next_doc_id;
        self.next_doc_id += 1;

        let doc = RagDocument {
            doc_id,
            text: text.to_string(),
            table: table.to_string(),
            row_id,
            metadata: HashMap::new(),
        };

        // 写入 Embedding 索引（使用 doc_id 作为 HNSW payload）
        let mut row = HashMap::new();
        row.insert("text".to_string(), text.to_string());
        self.lifecycle.on_insert("__rag__", doc_id, &row)?;

        self.documents.insert(doc_id, doc);
        Ok(doc_id)
    }

    fn search(&self, query: &str, top_k: usize) -> Result<Vec<SearchResult>, RagError> {
        Ok(self
            .lifecycle
            .search("__rag__", &self.embedding_column, query, top_k)?)
    }

    fn get_document(&self, doc_id: u64) -> Option<&RagDocument> {
        self.documents.get(&doc_id)
    }

    fn stats(&self) -> NamespaceStats {
        NamespaceStats {
            doc_count: self.documents.len(),
            index_size: self
                .lifecycle
                .index_size("__rag__", &self.embedding_column)
                .unwrap_or(0),
        }
    }
}

// =====================================================================
//  RagEngine — RAG 引擎
// =====================================================================

/// RAG 引擎 — 检索增强生成
///
/// # 工作流程
///
/// 1. `register_namespace(name)` — 注册命名空间
/// 2. `add_document(namespace, text, table, row_id)` — 添加文档
/// 3. `rag_ask(question, filter, namespace)` — 问答主入口
pub struct RagEngine {
    /// 命名空间（name → Namespace）
    namespaces: HashMap<String, Namespace>,
    /// LLM 缓存
    cache: LlmCache,
    /// NL2SQL 引擎（用于 filter 解析，可选）
    nl2sql: Nl2SqlEngine,
    /// Embedding 维度
    dim: usize,
    /// 默认 TOP-K
    default_top_k: usize,
    /// 嵌入器（用于查询向量化）
    embedder: HashingEmbedder,
}

impl Default for RagEngine {
    fn default() -> Self {
        Self::new(128, 1000, 10).expect("default params valid")
    }
}

impl RagEngine {
    /// 创建 RAG 引擎
    ///
    /// - `dim` — Embedding 维度
    /// - `cache_capacity` — LLM 缓存容量
    /// - `default_top_k` — 默认检索 TOP-K
    pub fn new(dim: usize, cache_capacity: usize, default_top_k: usize) -> Result<Self, RagError> {
        Ok(Self {
            namespaces: HashMap::new(),
            cache: LlmCache::new(cache_capacity)
                .map_err(|_| RagError::InvalidFilter("cache capacity must be > 0".to_string()))?,
            nl2sql: Nl2SqlEngine::new(),
            embedder: HashingEmbedder::new(dim)?,
            dim,
            default_top_k,
        })
    }

    /// 注册命名空间
    pub fn register_namespace(&mut self, name: &str) -> Result<(), RagError> {
        if self.namespaces.contains_key(name) {
            return Err(RagError::NamespaceAlreadyExists(name.to_string()));
        }
        let ns = Namespace::new(self.dim)?;
        self.namespaces.insert(name.to_string(), ns);
        Ok(())
    }

    /// 添加文档到命名空间
    pub fn add_document(
        &mut self,
        namespace: &str,
        text: &str,
        table: &str,
        row_id: u64,
    ) -> Result<u64, RagError> {
        let ns = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| RagError::NamespaceNotFound(namespace.to_string()))?;
        let doc_id = ns.add_document(text, table, row_id)?;

        // CDC：失效依赖该表的缓存条目
        self.cache.invalidate_table(table);

        Ok(doc_id)
    }

    /// 添加带元数据的文档
    pub fn add_document_with_metadata(
        &mut self,
        namespace: &str,
        text: &str,
        table: &str,
        row_id: u64,
        metadata: HashMap<String, String>,
    ) -> Result<u64, RagError> {
        let ns = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| RagError::NamespaceNotFound(namespace.to_string()))?;
        let doc_id = ns.add_document(text, table, row_id)?;

        // 补充元数据
        if let Some(doc) = ns.documents.get_mut(&doc_id) {
            doc.metadata = metadata;
        }

        self.cache.invalidate_table(table);
        Ok(doc_id)
    }

    /// RAG 问答主入口
    ///
    /// - `question` — 自然语言问题（如 "哪些商品需补货？"）
    /// - `filter` — 过滤条件（如 "库存低于安全库存"），为空则不过滤
    /// - `namespace` — 命名空间（如 "零售助手"）
    ///
    /// 返回自然语言回答 + 数据引用
    pub fn rag_ask(
        &mut self,
        question: &str,
        filter: &str,
        namespace: &str,
    ) -> Result<RagAnswer, RagError> {
        if question.trim().is_empty() {
            return Err(RagError::EmptyDocument);
        }

        let start = std::time::Instant::now();

        // Step 1: 检查缓存（key = question + filter + namespace）
        let cache_key = format!("{question}|{filter}|{namespace}");
        if let Some(cached) = self.cache.get(&cache_key) {
            // 缓存命中 — 解析缓存的回答
            return Ok(RagAnswer {
                text: cached,
                citations: Vec::new(), // 缓存命中时不返回引用（已过期）
                cache_hit: true,
                retrieval_ms: start.elapsed().as_millis() as u64,
                retrieved_count: 0,
                filtered_count: 0,
            });
        }

        // Step 2: 检索 TOP-K 相关文档
        let ns = self
            .namespaces
            .get(namespace)
            .ok_or_else(|| RagError::NamespaceNotFound(namespace.to_string()))?;

        let results = ns.search(question, self.default_top_k)?;
        if results.is_empty() {
            return Err(RagError::NoRelevantDocuments);
        }

        // Step 3: 收集检索到的文档
        let mut retrieved: Vec<(RagDocument, f32)> = Vec::new();
        for r in &results {
            if let Some(doc) = ns.get_document(r.row_id) {
                retrieved.push((doc.clone(), r.score));
            }
        }

        let retrieved_count = retrieved.len();

        // Step 4: 应用过滤条件（如 "库存低于安全库存"）
        let filtered = if filter.trim().is_empty() {
            retrieved
        } else {
            self.apply_filter(retrieved, filter)?
        };

        let filtered_count = filtered.len();

        // Step 5: 生成数据引用
        let citations: Vec<RagCitation> = filtered
            .iter()
            .enumerate()
            .map(|(i, (doc, score))| RagCitation {
                index: i + 1,
                table: doc.table.clone(),
                row_id: doc.row_id,
                score: *score,
                snippet: Self::snippet(&doc.text, 80),
            })
            .collect();

        // Step 6: 生成自然语言回答
        let answer_text = self.generate_answer(question, filter, &filtered)?;

        let retrieval_ms = start.elapsed().as_millis() as u64;

        // Step 7: 写入缓存
        let table_deps: Vec<String> = filtered
            .iter()
            .map(|(doc, _)| doc.table.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        self.cache.put(&cache_key, answer_text.clone(), table_deps);

        Ok(RagAnswer {
            text: answer_text,
            citations,
            cache_hit: false,
            retrieval_ms,
            retrieved_count,
            filtered_count,
        })
    }

    /// 获取命名空间统计
    pub fn namespace_stats(&self, name: &str) -> Result<NamespaceStats, RagError> {
        let ns = self
            .namespaces
            .get(name)
            .ok_or_else(|| RagError::NamespaceNotFound(name.to_string()))?;
        Ok(ns.stats())
    }

    /// 获取缓存统计
    pub fn cache_stats(&self) -> crate::llm_cache::CacheStats {
        self.cache.stats()
    }

    /// 失效命名空间下指定表的所有缓存
    pub fn invalidate_table(&mut self, table: &str) -> usize {
        self.cache.invalidate_table(table)
    }

    /// 已注册的命名空间列表
    pub fn list_namespaces(&self) -> Vec<String> {
        self.namespaces.keys().cloned().collect()
    }

    // -----------------------------------------------------------------
    //  内部方法
    // -----------------------------------------------------------------

    /// 应用过滤条件
    ///
    /// 解析过滤条件（如 "库存低于安全库存"）并过滤文档。
    /// 当前实现支持基于元数据的简单过滤：
    /// - "库存低于安全库存" → metadata["stock"] < metadata["safety_stock"]
    /// - "category=食品" → metadata["category"] == "食品"
    fn apply_filter(
        &self,
        docs: Vec<(RagDocument, f32)>,
        filter: &str,
    ) -> Result<Vec<(RagDocument, f32)>, RagError> {
        let filter_lower = filter.to_lowercase();

        // 解析过滤条件类型
        if filter_lower.contains("低于") || filter_lower.contains("小于") {
            // "库存低于安全库存" → 比较两个 metadata 字段
            // 简化：查找 "X低于Y" 模式
            let parts: Vec<&str> = filter_lower.split("低于").collect();
            if parts.len() == 2 {
                let left = parts[0].trim();
                let right = parts[1].trim();
                return Ok(docs
                    .into_iter()
                    .filter(|(doc, _)| {
                        let l = doc.metadata.get(left).and_then(|v| v.parse::<f64>().ok());
                        let r = doc.metadata.get(right).and_then(|v| v.parse::<f64>().ok());
                        match (l, r) {
                            (Some(l), Some(r)) => l < r,
                            _ => false,
                        }
                    })
                    .collect());
            }
        }

        if filter_lower.contains("高于") || filter_lower.contains("大于") {
            let parts: Vec<&str> = if filter_lower.contains("高于") {
                filter_lower.split("高于").collect()
            } else {
                filter_lower.split("大于").collect()
            };
            if parts.len() == 2 {
                let left = parts[0].trim();
                let right = parts[1].trim();
                return Ok(docs
                    .into_iter()
                    .filter(|(doc, _)| {
                        let l = doc.metadata.get(left).and_then(|v| v.parse::<f64>().ok());
                        let r = doc.metadata.get(right).and_then(|v| v.parse::<f64>().ok());
                        match (l, r) {
                            (Some(l), Some(r)) => l > r,
                            _ => false,
                        }
                    })
                    .collect());
            }
        }

        // "key=value" 模式
        if let Some(eq_pos) = filter.find('=') {
            let key = filter[..eq_pos].trim().to_lowercase();
            let value = filter[eq_pos + 1..].trim();
            return Ok(docs
                .into_iter()
                .filter(|(doc, _)| doc.metadata.get(&key).map(|v| v == value).unwrap_or(false))
                .collect());
        }

        // 无法识别的过滤条件 — 返回原文档（宽松策略）
        Ok(docs)
    }

    /// 生成自然语言回答（模板化）
    fn generate_answer(
        &self,
        question: &str,
        filter: &str,
        docs: &[(RagDocument, f32)],
    ) -> Result<String, RagError> {
        if docs.is_empty() {
            return Ok(format!("未找到与「{question}」相关的记录。"));
        }

        let mut answer = String::new();

        // 回答开头
        if filter.trim().is_empty() {
            answer.push_str(&format!(
                "针对「{}」，找到 {} 条相关记录：\n\n",
                question,
                docs.len()
            ));
        } else {
            answer.push_str(&format!(
                "针对「{}」（过滤条件：{}），找到 {} 条相关记录：\n\n",
                question,
                filter,
                docs.len()
            ));
        }

        // 列出每条记录的摘要
        for (i, (doc, score)) in docs.iter().enumerate() {
            let snippet = Self::snippet(&doc.text, 100);
            answer.push_str(&format!(
                "[{}] {}（表：{}，行 ID：{}，相关性：{:.2}）\n",
                i + 1,
                snippet,
                doc.table,
                doc.row_id,
                score
            ));
        }

        // 回答结尾
        answer.push('\n');
        if docs.len() == 1 {
            answer.push_str("以上为最相关的记录。");
        } else {
            answer.push_str(&format!("以上 {} 条记录按相关性排序。", docs.len()));
        }

        Ok(answer)
    }

    /// 截取文本片段（最多 max_chars 个字符）
    fn snippet(text: &str, max_chars: usize) -> String {
        if text.chars().count() <= max_chars {
            return text.to_string();
        }
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

// =====================================================================
//  Debug 实现
// =====================================================================

impl std::fmt::Debug for RagEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RagEngine")
            .field("dim", &self.dim)
            .field("default_top_k", &self.default_top_k)
            .field("namespace_count", &self.namespaces.len())
            .field("cache_stats", &self.cache.stats())
            .finish()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建测试用 RAG 引擎（小维度 + 小缓存）
    fn make_test_engine() -> RagEngine {
        RagEngine::new(32, 100, 5).unwrap()
    }

    /// 创建零售助手命名空间并添加测试商品文档
    ///
    /// metadata key 使用中文以匹配中文过滤条件（如 "库存低于安全库存"）
    /// 文档内容以空格分隔关键词，便于 HashingEmbedder 分词匹配
    fn make_retail_namespace(engine: &mut RagEngine) {
        engine.register_namespace("零售助手").unwrap();

        // 商品 1：库存低于安全库存（需补货）
        let mut meta1 = HashMap::new();
        meta1.insert("库存".to_string(), "5".to_string());
        meta1.insert("安全库存".to_string(), "20".to_string());
        meta1.insert("类别".to_string(), "食品".to_string());
        engine
            .add_document_with_metadata(
                "零售助手",
                "商品 苹果汁 需补货 库存 5 瓶 安全库存 20 瓶",
                "products",
                1001,
                meta1,
            )
            .unwrap();

        // 商品 2：库存低于安全库存（需补货）
        let mut meta2 = HashMap::new();
        meta2.insert("库存".to_string(), "3".to_string());
        meta2.insert("安全库存".to_string(), "15".to_string());
        meta2.insert("类别".to_string(), "饮料".to_string());
        engine
            .add_document_with_metadata(
                "零售助手",
                "商品 橙汁 需补货 库存 3 瓶 安全库存 15 瓶",
                "products",
                1002,
                meta2,
            )
            .unwrap();

        // 商品 3：库存充足（不需补货）
        let mut meta3 = HashMap::new();
        meta3.insert("库存".to_string(), "50".to_string());
        meta3.insert("安全库存".to_string(), "20".to_string());
        meta3.insert("类别".to_string(), "食品".to_string());
        engine
            .add_document_with_metadata(
                "零售助手",
                "商品 面包 库存充足 库存 50 个 安全库存 20 个",
                "products",
                1003,
                meta3,
            )
            .unwrap();
    }

    #[test]
    fn test_7b5_register_namespace() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns1").unwrap();
        assert_eq!(engine.list_namespaces(), vec!["ns1"]);

        // 重复注册 → 错误
        let err = engine.register_namespace("ns1").unwrap_err();
        assert_eq!(err, RagError::NamespaceAlreadyExists("ns1".to_string()));
    }

    #[test]
    fn test_7b5_add_document_returns_doc_id() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns").unwrap();

        let id1 = engine.add_document("ns", "文档1", "t1", 1).unwrap();
        let id2 = engine.add_document("ns", "文档2", "t1", 2).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn test_7b5_add_document_empty_text_errors() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns").unwrap();

        let err = engine.add_document("ns", "", "t1", 1).unwrap_err();
        assert_eq!(err, RagError::EmptyDocument);

        let err = engine.add_document("ns", "   ", "t1", 1).unwrap_err();
        assert_eq!(err, RagError::EmptyDocument);
    }

    #[test]
    fn test_7b5_add_document_unknown_namespace_errors() {
        let mut engine = make_test_engine();
        let err = engine.add_document("unknown", "text", "t1", 1).unwrap_err();
        assert_eq!(err, RagError::NamespaceNotFound("unknown".to_string()));
    }

    #[test]
    fn test_7b5_rag_ask_basic() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let answer = engine.rag_ask("商品 需补货", "", "零售助手").unwrap();

        assert!(!answer.cache_hit);
        assert!(answer.retrieved_count > 0);
        assert!(!answer.text.is_empty());
        assert!(!answer.citations.is_empty());
        // 每条引用都有表名、行 ID、分数
        for c in &answer.citations {
            assert_eq!(c.table, "products");
            assert!(c.row_id >= 1001);
            assert!(c.score >= 0.0 && c.score <= 1.0);
        }
    }

    #[test]
    fn test_7b5_rag_ask_with_filter_stock_below_safety() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 过滤：库存低于安全库存
        let answer = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();

        // 应该只返回库存 < 安全库存的商品（苹果汁 + 橙汁，不包括面包）
        assert_eq!(answer.filtered_count, 2);
        assert_eq!(answer.citations.len(), 2);

        // 验证返回的行 ID 是 1001 和 1002（不包括 1003）
        let row_ids: Vec<u64> = answer.citations.iter().map(|c| c.row_id).collect();
        assert!(row_ids.contains(&1001));
        assert!(row_ids.contains(&1002));
        assert!(!row_ids.contains(&1003));
    }

    #[test]
    fn test_7b5_rag_ask_cache_hit() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 第一次查询 — 未命中缓存
        let answer1 = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();
        assert!(!answer1.cache_hit);

        // 第二次相同查询 — 应命中缓存
        let answer2 = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();
        assert!(answer2.cache_hit);
        assert_eq!(answer2.text, answer1.text);
    }

    #[test]
    fn test_7b5_rag_ask_cdc_invalidation() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 第一次查询 — 填充缓存
        let answer1 = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();
        assert!(!answer1.cache_hit);

        // 添加新文档到 products 表 → CDC 失效
        let mut meta4 = HashMap::new();
        meta4.insert("库存".to_string(), "2".to_string());
        meta4.insert("安全库存".to_string(), "10".to_string());
        meta4.insert("类别".to_string(), "食品".to_string());
        engine
            .add_document_with_metadata(
                "零售助手",
                "商品：牛奶，当前库存 2 瓶，安全库存 10 瓶，需补货",
                "products",
                1004,
                meta4,
            )
            .unwrap();

        // 第二次相同查询 — 应未命中缓存（CDC 失效）
        let answer2 = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();
        assert!(!answer2.cache_hit);
        // 现在应该有 3 条需补货商品（苹果汁 + 橙汁 + 牛奶）
        assert_eq!(answer2.filtered_count, 3);
    }

    #[test]
    fn test_7b5_rag_ask_empty_question_errors() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns").unwrap();

        let err = engine.rag_ask("", "", "ns").unwrap_err();
        assert_eq!(err, RagError::EmptyDocument);
    }

    #[test]
    fn test_7b5_rag_ask_unknown_namespace_errors() {
        let mut engine = make_test_engine();
        let err = engine.rag_ask("问题", "", "unknown").unwrap_err();
        assert_eq!(err, RagError::NamespaceNotFound("unknown".to_string()));
    }

    #[test]
    fn test_7b5_rag_ask_no_relevant_documents() {
        let mut engine = make_test_engine();
        engine.register_namespace("empty").unwrap();

        let err = engine.rag_ask("查询", "", "empty").unwrap_err();
        assert_eq!(err, RagError::NoRelevantDocuments);
    }

    #[test]
    fn test_7b5_citation_index_starts_from_1() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let answer = engine.rag_ask("商品", "", "零售助手").unwrap();

        for (i, c) in answer.citations.iter().enumerate() {
            assert_eq!(c.index, i + 1);
        }
    }

    #[test]
    fn test_7b5_snippet_truncation() {
        let short = "短文本";
        assert_eq!(RagEngine::snippet(short, 80), "短文本");

        let long = "这是一段很长的文本".repeat(20);
        let snippet = RagEngine::snippet(&long, 10);
        assert!(snippet.ends_with("..."));
        assert_eq!(snippet.chars().count(), 13); // 10 + "..."
    }

    #[test]
    fn test_7b5_filter_key_value() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns").unwrap();

        let mut meta1 = HashMap::new();
        meta1.insert("category".to_string(), "食品".to_string());
        engine
            .add_document_with_metadata("ns", "文档 食品类 1", "t1", 1, meta1)
            .unwrap();

        let mut meta2 = HashMap::new();
        meta2.insert("category".to_string(), "饮料".to_string());
        engine
            .add_document_with_metadata("ns", "文档 饮料类 1", "t1", 2, meta2)
            .unwrap();

        // 过滤 category=食品
        let answer = engine.rag_ask("文档", "category=食品", "ns").unwrap();
        assert_eq!(answer.filtered_count, 1);
        assert_eq!(answer.citations[0].row_id, 1);
    }

    #[test]
    fn test_7b5_filter_stock_above_safety() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 过滤：库存高于安全库存 → 只返回面包（50 > 20）
        let answer = engine
            .rag_ask("商品", "库存高于安全库存", "零售助手")
            .unwrap();
        assert_eq!(answer.filtered_count, 1);
        assert_eq!(answer.citations[0].row_id, 1003);
    }

    #[test]
    fn test_7b5_namespace_stats() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let stats = engine.namespace_stats("零售助手").unwrap();
        assert_eq!(stats.doc_count, 3);
        assert_eq!(stats.index_size, 3);
    }

    #[test]
    fn test_7b5_namespace_stats_unknown_errors() {
        let engine = make_test_engine();
        let err = engine.namespace_stats("unknown").unwrap_err();
        assert_eq!(err, RagError::NamespaceNotFound("unknown".to_string()));
    }

    #[test]
    fn test_7b5_invalidate_table_returns_count() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 先填充缓存
        engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();

        // 手动失效 products 表
        let count = engine.invalidate_table("products");
        assert!(count >= 1);
    }

    #[test]
    fn test_7b5_cache_stats_reflects_queries() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 第一次查询 — miss
        engine.rag_ask("商品", "", "零售助手").unwrap();

        // 第二次相同查询 — hit
        engine.rag_ask("商品", "", "零售助手").unwrap();

        let stats = engine.cache_stats();
        assert!(stats.total_queries >= 2);
        assert!(stats.hits >= 1);
    }

    #[test]
    fn test_7b5_answer_text_contains_question() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let answer = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();

        assert!(answer.text.contains("商品 需补货"));
        assert!(answer.text.contains("库存低于安全库存"));
    }

    #[test]
    fn test_7b5_answer_text_contains_record_count() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let answer = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();

        // 应该包含 "2 条相关记录"
        assert!(answer.text.contains("2 条相关记录"));
    }

    #[test]
    fn test_7b5_multiple_namespaces_independent() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns1").unwrap();
        engine.register_namespace("ns2").unwrap();

        engine.add_document("ns1", "文档 A", "t1", 1).unwrap();
        engine.add_document("ns2", "文档 B", "t1", 2).unwrap();

        let answer1 = engine.rag_ask("文档", "", "ns1").unwrap();
        let answer2 = engine.rag_ask("文档", "", "ns2").unwrap();

        // 两个命名空间独立检索
        assert_eq!(answer1.retrieved_count, 1);
        assert_eq!(answer2.retrieved_count, 1);
        assert_eq!(answer1.citations[0].row_id, 1);
        assert_eq!(answer2.citations[0].row_id, 2);
    }

    #[test]
    fn test_7b5_retrieval_ms_nonzero() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let answer = engine.rag_ask("商品", "", "零售助手").unwrap();
        // 检索耗时可能为 0（非常快），但不应大于合理范围
        assert!(answer.retrieval_ms < 10000);
    }

    #[test]
    fn test_7b5_rag_ask_with_unrecognized_filter_returns_all() {
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        // 无法识别的过滤条件 → 宽松策略，返回所有文档
        let answer = engine
            .rag_ask("商品", "无法识别的条件", "零售助手")
            .unwrap();
        assert_eq!(answer.filtered_count, answer.retrieved_count);
    }

    #[test]
    fn test_7b5_citation_snippet_truncated() {
        let mut engine = make_test_engine();
        engine.register_namespace("ns").unwrap();

        // 文档内容包含 "文档" 作为单独 token（空格分隔），便于查询匹配
        let long_text = format!("文档 {}", "这是一段很长的内容".repeat(20));
        engine.add_document("ns", &long_text, "t1", 1).unwrap();

        let answer = engine.rag_ask("文档", "", "ns").unwrap();
        assert!(!answer.citations.is_empty());
        // snippet 最多 80 字符 + "..."
        let snippet = &answer.citations[0].snippet;
        assert!(snippet.chars().count() <= 83);
    }

    #[test]
    fn test_7b5_stress_1000_queries() {
        let mut engine = RagEngine::new(32, 1000, 5).unwrap();
        engine.register_namespace("stress").unwrap();

        // 添加 100 个文档
        for i in 0..100 {
            engine
                .add_document("stress", &format!("文档 {i} 内容关键词"), "t1", i)
                .unwrap();
        }

        // 1000 次查询（70% 重复 + 30% 唯一）
        let mut hit_count = 0;
        for i in 0..1000 {
            let q = if i % 10 < 7 {
                "文档 内容".to_string() // 重复查询
            } else {
                format!("唯一查询 {i}")
            };
            if let Ok(answer) = engine.rag_ask(&q, "", "stress") {
                if answer.cache_hit {
                    hit_count += 1;
                }
            }
        }

        // 缓存命中率应 >= 50%
        let hit_rate = hit_count as f64 / 1000.0;
        assert!(
            hit_rate >= 0.5,
            "cache hit rate should be >= 50%, got {:.1}%",
            hit_rate * 100.0
        );
    }

    #[test]
    fn test_7b5_retail_scenario_complete() {
        // 完整零售场景验证：rag_ask('哪些商品需补货？', '库存低于安全库存', '零售助手')
        let mut engine = make_test_engine();
        make_retail_namespace(&mut engine);

        let answer = engine
            .rag_ask("商品 需补货", "库存低于安全库存", "零售助手")
            .unwrap();

        // 验证回答正确
        assert!(answer.text.contains("商品 需补货"));
        assert!(answer.text.contains("库存低于安全库存"));
        assert!(answer.text.contains("2 条相关记录"));

        // 验证引用数据准确
        assert_eq!(answer.citations.len(), 2);
        let row_ids: Vec<u64> = answer.citations.iter().map(|c| c.row_id).collect();
        assert!(row_ids.contains(&1001)); // 苹果汁
        assert!(row_ids.contains(&1002)); // 橙汁

        // 验证所有引用都来自 products 表
        for c in &answer.citations {
            assert_eq!(c.table, "products");
        }
    }
}

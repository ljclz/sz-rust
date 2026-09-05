// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! BM25 全文检索索引
//!
//! 任务组 12.1：实现内存 BM25 索引，支持增量更新
//! 用于 HybridRetriever 的关键词检索分支

use std::collections::HashMap;

/// BM25 参数
#[derive(Clone, Debug)]
pub struct Bm25Params {
    /// 词频饱和参数，默认 1.2
    pub k1: f32,
    /// 文档长度归一化参数，默认 0.75
    pub b: f32,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// BM25 检索结果
#[derive(Clone, Debug)]
pub struct Bm25Hit {
    /// 文档 ID
    pub doc_id: String,
    /// BM25 分数
    pub score: f32,
}

/// BM25 索引
///
/// 内存中构建 term_freqs + idf，支持增量添加文档。
/// 分词策略：按 Unicode 空白/标点分割，转小写。
pub struct Bm25Index {
    params: Bm25Params,
    /// doc_id -> (token -> tf)
    doc_term_freqs: HashMap<String, HashMap<String, u32>>,
    /// doc_id -> doc_length
    doc_lengths: HashMap<String, usize>,
    /// token -> document frequency（包含该 token 的文档数）
    doc_freq: HashMap<String, usize>,
    /// 总文档数
    num_docs: usize,
    /// 平均文档长度
    avg_doc_len: f32,
}

impl Bm25Index {
    pub fn new() -> Self {
        Self::with_params(Bm25Params::default())
    }

    pub fn with_params(params: Bm25Params) -> Self {
        Self {
            params,
            doc_term_freqs: HashMap::new(),
            doc_lengths: HashMap::new(),
            doc_freq: HashMap::new(),
            num_docs: 0,
            avg_doc_len: 0.0,
        }
    }

    /// 简单分词：按非字母数字字符分割，转小写
    fn tokenize(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .collect()
    }

    /// 添加单个文档（增量更新）
    pub fn add_document(&mut self, doc_id: impl Into<String>, text: impl AsRef<str>) {
        let doc_id = doc_id.into();
        let tokens = Self::tokenize(text.as_ref());

        if self.doc_term_freqs.contains_key(&doc_id) {
            self.remove_document(&doc_id);
        }

        let doc_len = tokens.len();
        let mut term_freq: HashMap<String, u32> = HashMap::new();
        for token in &tokens {
            *term_freq.entry(token.clone()).or_insert(0) += 1;
        }

        for token in term_freq.keys() {
            *self.doc_freq.entry(token.clone()).or_insert(0) += 1;
        }

        self.doc_term_freqs.insert(doc_id.clone(), term_freq);
        self.doc_lengths.insert(doc_id, doc_len);
        self.num_docs += 1;
        self.recompute_avg_doc_len();
    }

    /// 批量添加文档
    pub fn add_documents(&mut self, docs: impl IntoIterator<Item = (String, String)>) {
        for (doc_id, text) in docs {
            self.add_document(doc_id, &text);
        }
    }

    /// 移除文档
    pub fn remove_document(&mut self, doc_id: &str) {
        if let Some(term_freq) = self.doc_term_freqs.remove(doc_id) {
            for token in term_freq.keys() {
                if let Some(count) = self.doc_freq.get_mut(token) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.doc_freq.remove(token);
                    }
                }
            }
            self.doc_lengths.remove(doc_id);
            self.num_docs = self.num_docs.saturating_sub(1);
            self.recompute_avg_doc_len();
        }
    }

    fn recompute_avg_doc_len(&mut self) {
        if self.num_docs == 0 {
            self.avg_doc_len = 0.0;
        } else {
            let total: usize = self.doc_lengths.values().sum();
            self.avg_doc_len = total as f32 / self.num_docs as f32;
        }
    }

    /// 计算 IDF：log((N - df + 0.5) / (df + 0.5) + 1)
    fn idf(&self, df: usize) -> f32 {
        let n = self.num_docs as f32;
        let df = df as f32;
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// 检索 top-k 相关文档
    pub fn search(&self, query: &str, topk: usize) -> Vec<Bm25Hit> {
        if self.num_docs == 0 || topk == 0 {
            return Vec::new();
        }

        let query_tokens = Self::tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let k1 = self.params.k1;
        let b = self.params.b;
        let avgdl = self.avg_doc_len.max(1.0);

        let mut scores: HashMap<String, f32> = HashMap::new();

        for token in &query_tokens {
            let df = match self.doc_freq.get(token) {
                Some(&df) => df,
                None => continue,
            };
            let idf = self.idf(df);

            for (doc_id, term_freq) in &self.doc_term_freqs {
                if let Some(&tf) = term_freq.get(token) {
                    let doc_len = *self.doc_lengths.get(doc_id).unwrap_or(&0) as f32;
                    let tf_component = (tf as f32 * (k1 + 1.0))
                        / (tf as f32 + k1 * (1.0 - b + b * doc_len / avgdl));
                    let score = idf * tf_component;
                    *scores.entry(doc_id.clone()).or_insert(0.0) += score;
                }
            }
        }

        let mut hits: Vec<Bm25Hit> = scores
            .into_iter()
            .map(|(doc_id, score)| Bm25Hit { doc_id, score })
            .collect();

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        hits.into_iter().take(topk).collect()
    }

    /// 文档数量
    pub fn len(&self) -> usize {
        self.num_docs
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.num_docs == 0
    }

    /// 获取文档文本长度
    pub fn doc_length(&self, doc_id: &str) -> Option<usize> {
        self.doc_lengths.get(doc_id).copied()
    }
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bm25_empty_index_search_returns_empty() {
        let index = Bm25Index::new();
        let hits = index.search("query", 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn bm25_add_and_search_single_doc() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "Rust is a systems programming language");

        let hits = index.search("Rust", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "doc1");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn bm25_search_ranks_relevant_docs_higher() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "Rust Rust Rust memory safety");
        index.add_document("doc2", "Python is a scripting language");
        index.add_document("doc3", "Rust and Cargo package manager");

        let hits = index.search("Rust", 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].doc_id, "doc1");
    }

    #[test]
    fn bm25_tokenization_case_insensitive() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "RUST Programming");

        let hits = index.search("rust programming", 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn bm25_incremental_add_documents() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "first document");
        assert_eq!(index.len(), 1);

        index.add_document("doc2", "second document");
        assert_eq!(index.len(), 2);

        index.add_document("doc3", "third document");
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn bm25_remove_document() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "Rust programming");
        index.add_document("doc2", "Python scripting");

        assert_eq!(index.len(), 2);
        index.remove_document("doc1");
        assert_eq!(index.len(), 1);

        let hits = index.search("Rust", 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn bm25_topk_limit() {
        let mut index = Bm25Index::new();
        for i in 0..10 {
            index.add_document(format!("doc{i}"), format!("document number {i}"));
        }

        let hits = index.search("document", 3);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn bm25_query_no_matching_tokens() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "Rust programming language");

        let hits = index.search("Python", 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn bm25_add_documents_batch() {
        let mut index = Bm25Index::new();
        index.add_documents(vec![
            ("doc1".to_string(), "first".to_string()),
            ("doc2".to_string(), "second".to_string()),
            ("doc3".to_string(), "third".to_string()),
        ]);
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn bm25_re_add_document_overwrites() {
        let mut index = Bm25Index::new();
        index.add_document("doc1", "Rust");
        index.add_document("doc1", "Python");

        let hits = index.search("Rust", 5);
        assert!(hits.is_empty(), "re-adding should overwrite old content");

        let hits = index.search("Python", 5);
        assert_eq!(hits.len(), 1);
    }
}

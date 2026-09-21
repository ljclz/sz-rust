// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! RAG 检索增强生成：向量检索 + 上下文组装 + 引用溯源 + 重排序 + 混合检索

pub mod bm25;
pub mod citation;
pub mod hybrid;
pub mod pipeline;
pub mod reranker;

pub use bm25::{Bm25Hit, Bm25Index, Bm25Params};
pub use citation::Citation;
pub use hybrid::{HybridRetriever, HybridRetrieverTrait, RrfParams};
pub use pipeline::{RagPipeline, RagRequest, RagResult, WarningCode};
pub use reranker::{NoopReranker, Reranker, WeightedReranker};

#[cfg(feature = "reranker")]
pub use reranker::CrossEncoderReranker;

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Embedding 与向量存储：EmbeddingProvider trait + VectorStore trait

pub mod batch;
pub mod file_store;
pub mod local;
pub mod memory_store;
pub mod openai;
pub mod provider;
pub mod vector_store;

pub use batch::BatchChunker;
pub use file_store::FileVectorStore;
pub use local::LocalEmbedding;
pub use memory_store::MemoryVectorStore;
pub use openai::OpenAiEmbedding;
pub use provider::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult};
pub use vector_store::{SimilarityMetric, VectorHit, VectorRecord, VectorStore};

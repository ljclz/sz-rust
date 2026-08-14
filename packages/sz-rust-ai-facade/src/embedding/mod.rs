//! Embedding 与向量存储：EmbeddingProvider trait + VectorStore trait

pub mod batch;
pub mod local;
pub mod openai;
pub mod provider;
pub mod vector_store;

pub use batch::BatchChunker;
pub use local::LocalEmbedding;
pub use openai::OpenAiEmbedding;
pub use provider::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult};
pub use vector_store::{SimilarityMetric, VectorHit, VectorRecord, VectorStore};

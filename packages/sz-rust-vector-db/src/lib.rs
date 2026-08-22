//! SZ-Rust Vector DB
//!
//! 专业向量数据库适配器，实现 [`sz_rust_ai_facade::embedding::VectorStore`] trait。
//!
//! ## 模块结构
//!
//! | 模块 | 说明 | Feature |
//! |------|------|---------|
//! | [`qdrant`] | Qdrant HTTP API 适配器 | `qdrant` |
//!
//! ## 设计说明
//!
//! - **复用 trait**：`VectorStore` / `VectorRecord` / `VectorHit` / `SimilarityMetric` 均复用
//!   `sz-rust-ai-facade::embedding` 中的定义，避免重复抽象。
//! - **多租户隔离**：通过 Qdrant payload filter（`tenant_id` 字段）实现，collection 共享。
//! - **ID 映射**：Qdrant point ID 必须为 `uint64` 或 `UUID`，使用 UUID v5 确定性映射
//!   `VectorRecord.id`（字符串）→ UUID，保证 upsert 幂等。
//! - **Feature gate**：`qdrant` 默认不启用，需显式 `--features qdrant`。

#![deny(unsafe_code)]

pub use sz_rust_ai_facade::common::AiError;
pub use sz_rust_ai_facade::embedding::{SimilarityMetric, VectorHit, VectorRecord, VectorStore};

#[cfg(feature = "qdrant")]
pub mod qdrant;

#[cfg(feature = "qdrant")]
pub use qdrant::QdrantVectorStore;

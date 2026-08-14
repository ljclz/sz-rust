#![forbid(unsafe_code)]
//! sz-rust-rag — 行业 RAG 知识库（企业版）。
//!
//! 为 SDD Agent 提供生鲜零售行业知识检索能力，复用 `sz-rust-ai-facade` 的
//! EmbeddingProvider/VectorStore 接口。不负责 LLM 生成，仅提供检索增强。

pub mod audit;
pub mod capability;
pub mod chunking;
pub mod config;
pub mod corpus;
pub mod error;
pub mod facade;
pub mod metrics;
pub mod redact;
pub mod rule;
pub mod search;
pub mod store;
pub mod template;
pub mod term;
pub mod vectorize;
pub mod warning;

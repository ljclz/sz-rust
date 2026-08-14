//! RAG 检索增强生成：向量检索 + 上下文组装 + 引用溯源

pub mod citation;
pub mod pipeline;

pub use citation::Citation;
pub use pipeline::{RagPipeline, RagRequest, RagResult, WarningCode};

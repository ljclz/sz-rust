//! SZ-Rust AI Facade
//!
//! AI 应用开发 facade，提供 LLM / Embedding / RAG / Agent 四大模块的统一抽象。
//!
//! ## 模块结构
//!
//! | 模块 | 说明 |
//! |------|------|
//! | [`common`] | 公共基础层：错误类型 / 审计 HTTP 客户端 / SSE 适配器 / Prometheus 指标 |
//! | [`llm`] | LLM 统一抽象：LlmProvider trait + OpenAI/Claude/Gemini Provider + 路由 + 故障切换 |
//! | [`embedding`] | Embedding 与向量存储：EmbeddingProvider trait + VectorStore trait |
//! | [`rag`] | RAG 检索增强生成：向量检索 + 上下文组装 + 引用溯源 |
//! | [`agent`] | Agent 编排引擎：工具选择循环 + 短期/长期记忆 + 多步推理 |
//! | [`mcp_bridge`] | MCP 工具桥接：将 sz-rust-mcp 7 工具暴露为 Agent 可用工具 |
//!
//! ## 与 sz-rust 生态的关系
//!
//! - 流式响应经 [`sz_rust_http_facade::sse`] 透传，不新建 SSE 通道
//! - 向量存储复用 [`sz_rust_orm_facade`]，缓存复用 [`sz_rust_cache_facade`]
//! - MCP 工具桥接复用 [`sz_rust_mcp`] 现有 7 工具
//! - 可观测性复用 [`sz_rust_observability`] Prometheus 指标注册

#![deny(unsafe_code)]

pub mod agent;
pub mod capability;
pub mod common;
pub mod embedding;
pub mod facade;
pub mod llm;
pub mod mcp_bridge;
pub mod rag;

pub use facade::Ai;

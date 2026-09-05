// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::agent::engine::{Agent, AgentOptions, AgentResult, AgentTask};
use crate::agent::tool::ToolRegistry;
use crate::common::AiError;
use crate::embedding::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult, VectorStore};
use crate::llm::provider::{ChatCompletion, ChatRequest, StreamDelta};
use crate::llm::router::ModelRouter;
use crate::rag::pipeline::{RagPipeline, RagRequest, RagResult};
use futures::stream::BoxStream;
use std::sync::{Arc, OnceLock};

struct AiInstance {
    router: Arc<ModelRouter>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    #[allow(dead_code)]
    vector_store: Option<Arc<dyn VectorStore>>,
    rag: Option<Arc<RagPipeline>>,
    tools: Option<Arc<ToolRegistry>>,
}

static GLOBAL: OnceLock<AiInstance> = OnceLock::new();

pub struct Ai;

impl Ai {
    pub fn init_default(
        router: ModelRouter,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
        vector_store: Option<Arc<dyn VectorStore>>,
        rag: Option<Arc<RagPipeline>>,
        tools: Option<Arc<ToolRegistry>>,
    ) -> Result<(), AiError> {
        GLOBAL
            .set(AiInstance {
                router: Arc::new(router),
                embedding,
                vector_store,
                rag,
                tools,
            })
            .map_err(|_| AiError::Internal("Ai facade already initialized".to_string()))
    }

    fn instance() -> Result<&'static AiInstance, AiError> {
        GLOBAL.get().ok_or_else(|| {
            AiError::Internal(
                "Ai facade not initialized — call Ai::init_default() first".to_string(),
            )
        })
    }

    pub async fn chat(req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let inst = Self::instance()?;
        let provider = inst.router.route(Some(&req.model))?;
        provider.chat_completion(req).await
    }

    pub async fn stream_chat(
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        let inst = Self::instance()?;
        let provider = inst.router.route(Some(&req.model))?;
        provider.stream_completion(req).await
    }

    pub async fn embed(texts: Vec<String>, model: &str) -> Result<EmbeddingResult, AiError> {
        let inst = Self::instance()?;
        let embedding = inst
            .embedding
            .as_ref()
            .ok_or_else(|| AiError::Internal("embedding provider not configured".to_string()))?;
        let req = EmbeddingRequest::new(model, texts);
        embedding.embed(req).await
    }

    pub async fn rag(req: RagRequest) -> Result<RagResult, AiError> {
        let inst = Self::instance()?;
        let rag = inst
            .rag
            .as_ref()
            .ok_or_else(|| AiError::Internal("RAG pipeline not configured".to_string()))?;
        rag.rag(req).await
    }

    pub async fn agent(task: AgentTask, opts: AgentOptions) -> Result<AgentResult, AiError> {
        let inst = Self::instance()?;
        let tools = inst
            .tools
            .as_ref()
            .ok_or_else(|| AiError::Internal("tool registry not configured".to_string()))?;
        let provider = inst.router.route(None)?;
        let mut agent = Agent::new(provider, tools.clone());
        if let Some(ref rag) = inst.rag {
            agent = agent.with_rag_pipeline(rag.clone());
        }
        agent.run(task, opts).await
    }

    pub fn default_model() -> Result<String, AiError> {
        let inst = Self::instance()?;
        Ok(inst.router.default_model())
    }

    pub fn is_initialized() -> bool {
        GLOBAL.get().is_some()
    }
}

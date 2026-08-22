//! 任务组 10.3：Agent citations 端到端测试
//! 验证 Agent 注入 RagPipeline 后，AgentResult.citations 非空且内容正确

mod common;

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use sz_rust_ai_facade::agent::engine::{Agent, AgentOptions, AgentTask};
use sz_rust_ai_facade::agent::tool::ToolRegistry;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
    VectorRecord, VectorStore,
};
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    Usage,
};
use sz_rust_ai_facade::rag::pipeline::RagPipeline;

struct MockEmbedding;

#[async_trait]
impl EmbeddingProvider for MockEmbedding {
    fn name(&self) -> &str {
        "mock-embedding"
    }
    async fn embed(&self, _req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        Ok(EmbeddingResult {
            model: "mock".into(),
            embeddings: vec![vec![0.1, 0.2, 0.3]],
            dimensions: 3,
            usage_tokens: 1,
        })
    }
    fn dimensions(&self) -> usize {
        3
    }
    fn supported_models(&self) -> &[&str] {
        &["mock"]
    }
}

struct MockVectorStore {
    hits: Vec<VectorHit>,
}

#[async_trait]
impl VectorStore for MockVectorStore {
    async fn upsert(&self, _records: &[VectorRecord]) -> Result<(), AiError> {
        Ok(())
    }
    async fn query(
        &self,
        _vec: &[f32],
        topk: usize,
        _metric: SimilarityMetric,
        _tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        Ok(self.hits.iter().take(topk).cloned().collect())
    }
    async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
        Ok(())
    }
}

struct MockLlm;

#[async_trait]
impl LlmProvider for MockLlm {
    fn name(&self) -> &str {
        "mock-llm"
    }
    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "mock".into(),
            model: "mock".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "Answer based on retrieved context".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        })
    }
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("not supported".into()))
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &["mock"]
    }
}

fn build_hits() -> Vec<VectorHit> {
    vec![
        VectorHit {
            id: "doc-rust-001".into(),
            score: 0.95,
            metadata: serde_json::json!({"source": "rust-book", "chapter": 1}),
            text: "Rust guarantees memory safety without garbage collection.".into(),
        },
        VectorHit {
            id: "doc-cargo-002".into(),
            score: 0.88,
            metadata: serde_json::json!({"source": "cargo-guide"}),
            text: "Cargo is the official package manager and build system for Rust.".into(),
        },
        VectorHit {
            id: "doc-tokio-003".into(),
            score: 0.76,
            metadata: serde_json::json!({"source": "tokio-doc"}),
            text: "Tokio is an asynchronous runtime for Rust.".into(),
        },
    ]
}

#[tokio::test]
async fn agent_with_rag_produces_non_empty_citations() {
    let hits = build_hits();
    let pipeline = Arc::new(RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: hits.clone() }),
        Arc::new(MockLlm),
    ));

    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(MockLlm), tools).with_rag_pipeline(pipeline);

    let task = AgentTask::new("What is Rust and its key features?");
    let opts = AgentOptions::new("tenant-test");

    let result = agent
        .run(task, opts)
        .await
        .expect("agent run should succeed");

    assert!(
        !result.citations.is_empty(),
        "citations must be non-empty when RAG pipeline is attached"
    );
    assert_eq!(
        result.citations.len(),
        hits.len(),
        "citations count should match retrieved hits (top 5, but only 3 available)"
    );
}

#[tokio::test]
async fn agent_citations_preserve_doc_id_and_score() {
    let hits = build_hits();
    let pipeline = Arc::new(RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: hits.clone() }),
        Arc::new(MockLlm),
    ));

    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(MockLlm), tools).with_rag_pipeline(pipeline);

    let task = AgentTask::new("Explain Cargo");
    let opts = AgentOptions::new("tenant-test");

    let result = agent.run(task, opts).await.unwrap();

    for (i, citation) in result.citations.iter().enumerate() {
        assert_eq!(
            citation.doc_id, hits[i].id,
            "doc_id must match hit id at index {i}"
        );
        assert_eq!(
            citation.offset, i as u32,
            "offset must be sequential index {i}"
        );
        assert_eq!(
            citation.text, hits[i].text,
            "text must match hit text at index {i}"
        );
        assert!(
            (citation.score - hits[i].score).abs() < 1e-6,
            "score must match hit score at index {i}"
        );
    }
}

#[tokio::test]
async fn agent_without_rag_has_empty_citations() {
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(MockLlm), tools);

    let task = AgentTask::new("Hello");
    let opts = AgentOptions::new("tenant-test");

    let result = agent.run(task, opts).await.unwrap();

    assert!(
        result.citations.is_empty(),
        "citations must be empty when no RAG pipeline is attached"
    );
}

#[tokio::test]
async fn agent_rag_context_injected_into_memory() {
    let hits = build_hits();
    let pipeline = Arc::new(RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: hits.clone() }),
        Arc::new(MockLlm),
    ));

    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(MockLlm), tools).with_rag_pipeline(pipeline);

    let task = AgentTask::new("What is Tokio?");
    let opts = AgentOptions::new("tenant-test");

    let result = agent.run(task, opts).await.unwrap();

    assert!(!result.citations.is_empty());
    assert!(
        !result.final_answer.is_empty(),
        "final answer should be produced"
    );
}

#[tokio::test]
async fn agent_citations_with_empty_vector_store() {
    let pipeline = Arc::new(RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: vec![] }),
        Arc::new(MockLlm),
    ));

    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(MockLlm), tools).with_rag_pipeline(pipeline);

    let task = AgentTask::new("Anything?");
    let opts = AgentOptions::new("tenant-test");

    let result = agent.run(task, opts).await.unwrap();

    assert!(
        result.citations.is_empty(),
        "citations must be empty when vector store returns no hits"
    );
}

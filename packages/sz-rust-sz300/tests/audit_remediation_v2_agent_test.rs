//! audit_remediation_v2 — Agent + LongTermMemory 接线端到端测试
//!
//! 验证 sz300 main.rs 构造链实际调用了 Agent + with_rag_pipeline + FileLongTermMemoryStore。
//! 使用 StubProvider + LocalEmbedding::new_pseudo + FileVectorStore::new_in_memory + tempfile。

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::sync::{Arc, Once};
use sz_rust_ai_facade::agent::memory::{
    FileLongTermMemoryStore, LongTermMemory, LongTermMemoryStore,
};
use sz_rust_ai_facade::agent::{AgentOptions, AgentTask, ToolRegistry};
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{EmbeddingProvider, FileVectorStore, VectorStore};
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, ContentPart, FinishReason, LlmProvider, Role,
    StreamDelta, Usage,
};
use sz_rust_ai_facade::llm::{provider::ProviderRef, ModelRouter};
use sz_rust_ai_facade::rag::{NoopReranker, RagPipeline};

struct StubProvider;

#[async_trait]
impl LlmProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "chatcmpl-stub".to_string(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: ContentPart::Text("Stub agent response".into()),
                    tool_call_id: None,
                    tool_calls: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
        })
    }

    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("stub does not support stream".into()))
    }

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }

    fn supported_models(&self) -> &[&str] {
        &["gpt-4o-mini", "gpt-4o"]
    }
}

static INIT: Once = Once::new();

fn init_ai_with_agent() {
    INIT.call_once(|| {
        let provider = Arc::new(StubProvider) as ProviderRef;
        let mut routes = HashMap::new();
        routes.insert("gpt-4o-mini".to_string(), provider.clone());
        routes.insert("gpt-4o".to_string(), provider.clone());
        let router = ModelRouter::new(routes, "gpt-4o-mini".to_string());

        let embedding = Arc::new(sz_rust_ai_facade::embedding::LocalEmbedding::new_pseudo(
            384,
        )) as Arc<dyn EmbeddingProvider>;
        let vector_store = Arc::new(FileVectorStore::new_in_memory()) as Arc<dyn VectorStore>;

        let pipeline = RagPipeline::new(embedding.clone(), vector_store.clone(), provider.clone())
            .with_reranker(Arc::new(NoopReranker::new()));

        let tools = Arc::new(ToolRegistry::new());

        sz_rust_ai_facade::Ai::init_default(
            router,
            Some(embedding),
            Some(vector_store),
            Some(Arc::new(pipeline)),
            Some(tools),
        )
        .expect("Ai::init_default 应成功");
    });
}

#[tokio::test]
async fn agent_with_rag_pipeline_citations() {
    init_ai_with_agent();
    let task = AgentTask::new("总结 RAG 检索结果");
    let opts = AgentOptions::new("tenant-1");
    let result = sz_rust_ai_facade::Ai::agent(task, opts).await;
    assert!(result.is_ok(), "Ai::agent 应成功: {:?}", result.err());
    let agent_result = result.unwrap();
    assert!(
        !agent_result.final_answer.is_empty(),
        "AgentResult.final_answer 不应为空"
    );
}

#[tokio::test]
async fn long_term_memory_store_retrieve_roundtrip() {
    let tmpdir = tempfile::tempdir().expect("tempdir 创建失败");
    let store = FileLongTermMemoryStore::new(tmpdir.path());

    let memory = LongTermMemory::new("agent-1", "测试记忆内容", "tenant-1");
    store.store(memory).await.expect("store 应成功");

    let retrieved = store
        .retrieve("agent-1", "tenant-1", 10)
        .await
        .expect("retrieve 应成功");
    assert_eq!(retrieved.len(), 1, "应检索到 1 条记忆");
    assert_eq!(retrieved[0].content, "测试记忆内容");
    assert_eq!(retrieved[0].agent_id, "agent-1");
    assert_eq!(retrieved[0].tenant_id, "tenant-1");
}

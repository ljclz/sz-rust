//! audit_remediation_v2 — RagPipeline 接线端到端测试
//!
//! 验证 sz300 main.rs 构造链实际调用了 RagPipeline + with_reranker + with_hybrid_retriever。
//! 使用 StubProvider + LocalEmbedding::new_pseudo + FileVectorStore::new_in_memory，
//! 不依赖外部服务。

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::sync::{Arc, Once};
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{EmbeddingProvider, FileVectorStore, VectorStore};
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, ContentPart, FinishReason, LlmProvider, Role,
    StreamDelta, Usage,
};
use sz_rust_ai_facade::llm::{provider::ProviderRef, ModelRouter};
use sz_rust_ai_facade::rag::{Bm25Index, HybridRetriever, NoopReranker, RagPipeline, RagRequest};

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
                    content: ContentPart::Text("Stub RAG response".into()),
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

fn init_ai_with_rag() {
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

        let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
        let hybrid = Arc::new(HybridRetriever::new(
            embedding.clone(),
            vector_store.clone(),
            bm25,
        ));

        let pipeline = RagPipeline::new(embedding.clone(), vector_store.clone(), provider.clone())
            .with_reranker(Arc::new(NoopReranker::new()))
            .with_hybrid_retriever(hybrid);

        sz_rust_ai_facade::Ai::init_default(
            router,
            Some(embedding),
            Some(vector_store),
            Some(Arc::new(pipeline)),
            None,
        )
        .expect("Ai::init_default 应成功");
    });
}

#[tokio::test]
async fn rag_pipeline_with_reranker_injection() {
    init_ai_with_rag();
    let req = RagRequest::new("测试查询", "tenant-1");
    let result = sz_rust_ai_facade::Ai::rag(req).await;
    assert!(result.is_ok(), "Ai::rag 应成功: {:?}", result.err());
    let rag_result = result.unwrap();
    assert!(!rag_result.content.is_empty(), "RagResult.content 不应为空");
}

#[tokio::test]
async fn rag_pipeline_with_hybrid_injection() {
    init_ai_with_rag();
    let req = RagRequest::new("混合检索查询", "tenant-1");
    let result = sz_rust_ai_facade::Ai::rag(req).await;
    assert!(
        result.is_ok(),
        "Ai::rag（hybrid）应成功: {:?}",
        result.err()
    );
}

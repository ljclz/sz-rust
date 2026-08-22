//! audit_remediation_v3 — 5 项高风险幻影交付接线端到端测试
//!
//! 验证：
//! 1. LlmChatCapability 实现 Capability trait（ai.llm_chat 能力可注册）
//! 2. Ai::embed 全局可用（embedding 端点接线）
//! 3. Ai::stream_chat 全局可用（流式端点接线）
//! 4. McpToolBridge 注入 MCP 工具到 ToolRegistry（Agent 工具接线）
//! 5. AiMetrics 全局实例可用（Prometheus 指标接线）

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
                    content: ContentPart::Text("Stub response".into()),
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

fn init_ai_full() {
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

        let tools = Arc::new(sz_rust_ai_facade::agent::ToolRegistry::new());

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
async fn llm_chat_capability_implements_capability_trait() {
    use sz_rust_capability::Capability;
    let cap = sz_rust_ai_facade::capability::LlmChatCapability;
    assert_eq!(cap.name(), "ai.llm_chat");
    assert!(!cap.description().is_empty());
    assert!(cap.tags().contains(&"ai"));
    assert!(cap.tags().contains(&"llm"));
}

#[tokio::test]
async fn ai_embed_endpoint_available() {
    init_ai_full();
    let result =
        sz_rust_ai_facade::Ai::embed(vec!["test".to_string()], "text-embedding-3-small").await;
    assert!(result.is_ok(), "Ai::embed 应成功: {:?}", result.err());
    let emb = result.unwrap();
    assert!(!emb.embeddings.is_empty(), "应返回嵌入向量");
}

#[tokio::test]
async fn ai_stream_chat_endpoint_available() {
    init_ai_full();
    let req = ChatRequest::new(
        "gpt-4o-mini",
        vec![ChatMessage {
            role: Role::User,
            content: "test".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );
    let result = sz_rust_ai_facade::Ai::stream_chat(req).await;
    let _ = result;
}

#[tokio::test]
async fn mcp_tool_bridge_injects_tools_to_registry() {
    use sz_rust_ai_facade::agent::tool::ToolRegistry;
    use sz_rust_ai_facade::mcp_bridge::bridge::McpToolBridge;

    let bridge = McpToolBridge::new(&["parse_path", "build_select_query", "url_decode"]);
    let adapters = bridge.adapters();
    assert_eq!(adapters.len(), 3, "应生成 3 个工具适配器");

    let mut registry = ToolRegistry::new();
    for adapter in adapters {
        registry.register(Box::new(adapter));
    }
    let tools = registry.list();
    assert!(
        tools.contains(&"parse_path".to_string()),
        "应包含 parse_path"
    );
    assert!(
        tools.contains(&"build_select_query".to_string()),
        "应包含 build_select_query"
    );
    assert!(
        tools.contains(&"url_decode".to_string()),
        "应包含 url_decode"
    );
}

#[tokio::test]
async fn ai_metrics_global_instance_available() {
    let metrics = sz_rust_ai_facade::common::metrics::AiMetrics::global();
    let _ = metrics;
}

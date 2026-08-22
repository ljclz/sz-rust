//! ai-facade 性能基准测试 — 8 类压测场景
//!
//! 对应 spec 5.2.1（8 类压测场景）+ tasks.md T2.1-T2.9
//! 约束：mock 环境，排除网络（spec 5.2.1.9）

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::stream::BoxStream;
use futures::StreamExt;
use sz_rust_ai_facade::agent::engine::{Agent, AgentOptions, AgentTask};
use sz_rust_ai_facade::agent::tool::{Tool, ToolRegistry};
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
    VectorRecord, VectorStore,
};
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    Usage,
};
use sz_rust_ai_facade::llm::truncator::ContextTruncator;
use sz_rust_ai_facade::rag::pipeline::{RagPipeline, RagRequest};

// ============================================================================
// Mock Providers — 无真实网络，纯内存实现
// ============================================================================

struct StubLlm;

#[async_trait]
impl LlmProvider for StubLlm {
    fn name(&self) -> &str {
        "stub"
    }
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "bench-stub".into(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "Benchmark response".into(),
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
        let tokens = vec!["Hello".to_string(), " ".to_string(), "World".to_string()];
        let stream = futures::stream::iter(tokens.into_iter().map(|t| {
            Ok(StreamDelta {
                content_delta: t,
                finish_reason: None,
                tool_call_delta: None,
            })
        }))
        .chain(futures::stream::once(async {
            Ok(StreamDelta {
                content_delta: String::new(),
                finish_reason: Some(FinishReason::Stop),
                tool_call_delta: None,
            })
        }));
        Ok(stream.boxed())
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &["stub-model"]
    }
}

struct MockEmbedding;

#[async_trait]
impl EmbeddingProvider for MockEmbedding {
    fn name(&self) -> &str {
        "mock-embed"
    }
    async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        let count = req.input.len();
        let embeddings = (0..count).map(|_| vec![0.1, 0.2, 0.3]).collect();
        Ok(EmbeddingResult {
            model: req.model,
            embeddings,
            dimensions: 3,
            usage_tokens: count as u32,
        })
    }
    fn dimensions(&self) -> usize {
        3
    }
    fn supported_models(&self) -> &[&str] {
        &["mock-embed"]
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

struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn call(&self, _args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
        Ok(serde_json::json!({"ok": true}))
    }
}

fn make_chat_request() -> ChatRequest {
    ChatRequest::new(
        "stub-model",
        vec![ChatMessage {
            role: Role::User,
            content: "Benchmark query".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    )
}

fn make_vector_hits(n: usize) -> Vec<VectorHit> {
    (0..n)
        .map(|i| VectorHit {
            id: format!("doc{}", i),
            score: 0.9 - i as f32 * 0.01,
            metadata: serde_json::json!({}),
            text: format!("Document {} content for benchmark", i),
        })
        .collect()
}

// ============================================================================
// T2.2 chat 延迟基准 — P99 < 5ms
// ============================================================================

fn chat_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let provider = Arc::new(StubLlm);

    let mut group = c.benchmark_group("chat_latency");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(1000);

    group.bench_function("single_request", |b| {
        b.iter(|| {
            let provider = provider.clone();
            let req = make_chat_request();
            rt.block_on(async move { provider.chat_completion(req).await.unwrap() })
        })
    });

    group.finish();
}

// ============================================================================
// T2.3 流式吞吐量基准 — TTFT < 10ms
// ============================================================================

fn stream_throughput(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let provider = Arc::new(StubLlm);

    let mut group = c.benchmark_group("stream_throughput");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    group.bench_function("500_tokens", |b| {
        b.iter(|| {
            let provider = provider.clone();
            let req = make_chat_request();
            rt.block_on(async move {
                let stream = provider.stream_completion(req).await.unwrap();
                let collected: Vec<_> = stream.collect().await;
                collected
            })
        })
    });

    group.finish();
}

// ============================================================================
// T2.4 Embedding 批量吞吐量基准 — 4 档 batch size
// ============================================================================

fn embedding_batch(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let embedding = Arc::new(MockEmbedding);

    let batch_sizes = [1, 10, 100, 1000];

    let mut group = c.benchmark_group("embedding_batch");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    for &size in &batch_sizes {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let embedding = embedding.clone();
                let texts: Vec<String> = (0..size).map(|i| format!("text {}", i)).collect();
                rt.block_on(async move {
                    let req = EmbeddingRequest::new("mock-embed", texts);
                    embedding.embed(req).await.unwrap()
                })
            })
        });
    }

    group.finish();
}

// ============================================================================
// T2.5 RAG 三段式延迟基准 — retrieve / assemble / generate
// ============================================================================

fn rag_three_stage(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let hits = make_vector_hits(100);
    let pipeline = Arc::new(RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits }),
        Arc::new(StubLlm),
    ));

    let mut group = c.benchmark_group("rag_three_stage");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(200);

    group.bench_function("full_pipeline", |b| {
        b.iter(|| {
            let pipeline = pipeline.clone();
            rt.block_on(async move {
                let req = RagRequest::new("What is Rust?", "tenant-bench");
                pipeline.rag(req).await.unwrap()
            })
        })
    });

    group.finish();
}

// ============================================================================
// T2.6 Agent 多轮延迟基准 — 4 档轮数
// ============================================================================

fn agent_multi_round(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let round_counts = [1u32, 5, 10, 20];

    let mut group = c.benchmark_group("agent_multi_round");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(50);

    for &rounds in &round_counts {
        group.bench_with_input(
            BenchmarkId::from_parameter(rounds),
            &rounds,
            |b, &rounds| {
                b.iter(|| {
                    rt.block_on(async move {
                        let llm = Arc::new(StubLlm);
                        let mut registry = ToolRegistry::new();
                        registry.register(Box::new(NoopTool));
                        let tools = Arc::new(registry);
                        let agent = Agent::new(llm, tools);
                        let task = AgentTask::new("Benchmark task");
                        let mut opts = AgentOptions::new("tenant-bench");
                        opts.max_steps = Some(rounds);
                        opts.allow_tools = vec!["noop".into()];
                        agent.run(task, opts).await.unwrap()
                    })
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// T2.7 并发 QPS 与 P99 基准 — 4 档并发数
// ============================================================================

fn concurrent_qps_p99(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let concurrency_levels = [1u32, 10, 50, 100];

    let mut group = c.benchmark_group("concurrent_qps_p99");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(30);

    for &concurrency in &concurrency_levels {
        group.throughput(Throughput::Elements(concurrency as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &concurrency| {
                let provider = Arc::new(StubLlm);
                b.iter(|| {
                    let provider = provider.clone();
                    rt.block_on(async move {
                        let mut handles = Vec::with_capacity(concurrency as usize);
                        for _ in 0..concurrency {
                            let p = provider.clone();
                            handles.push(tokio::spawn(async move {
                                let req = make_chat_request();
                                p.chat_completion(req).await.unwrap()
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    })
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// T2.8 上下文裁剪耗时基准 — 4 档 context 长度
// ============================================================================

fn context_truncation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let context_sizes = [4_000u32, 16_000, 64_000, 128_000];

    let mut group = c.benchmark_group("context_truncation");
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(500);

    for &size in &context_sizes {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let truncator = ContextTruncator::new(size / 2);
                let messages: Vec<ChatMessage> = (0..size / 100)
                    .map(|i| ChatMessage {
                        role: Role::User,
                        content: format!(
                            "Message {} with some content for truncation benchmark",
                            i
                        )
                        .into(),
                        tool_call_id: None,
                        tool_calls: None,
                    })
                    .collect();
                let counter = StubLlm;
                rt.block_on(
                    async move { truncator.truncate(messages, None, &counter).await.unwrap() },
                )
            })
        });
    }

    group.finish();
}

// ============================================================================
// T2.9 内存占用基准 — 空闲态 vs 满负载态
// ============================================================================

fn memory_rss(c: &mut Criterion) {
    use sysinfo::{ProcessesToUpdate, System};

    let mut group = c.benchmark_group("memory_rss");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(10);

    group.bench_function("idle", |b| {
        b.iter(|| {
            let mut sys = System::new();
            let pid = sysinfo::Pid::from(std::process::id() as usize);
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            sys.process(pid).map(|p| p.memory()).unwrap_or(0)
        })
    });

    let rt = tokio::runtime::Runtime::new().unwrap();
    let provider = Arc::new(StubLlm);

    group.bench_function("load_100_concurrent", |b| {
        b.iter(|| {
            let provider = provider.clone();
            rt.block_on(async move {
                let mut handles = Vec::with_capacity(100);
                for _ in 0..100 {
                    let p = provider.clone();
                    handles.push(tokio::spawn(async move {
                        let req = make_chat_request();
                        p.chat_completion(req).await.unwrap()
                    }));
                }
                for h in handles {
                    h.await.unwrap();
                }
                let mut sys = System::new();
                let pid = sysinfo::Pid::from(std::process::id() as usize);
                sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
                sys.process(pid).map(|p| p.memory()).unwrap_or(0)
            })
        })
    });

    group.finish();
}

// ============================================================================
// Criterion Group & Main
// ============================================================================

criterion_group!(
    benches,
    chat_latency,
    stream_throughput,
    embedding_batch,
    rag_three_stage,
    agent_multi_round,
    concurrent_qps_p99,
    context_truncation,
    memory_rss,
);
criterion_main!(benches);

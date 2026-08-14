> **中文** | [English](README.en.md)

# sz-rust-ai-facade

SZ-Rust AI 原生集成 facade。提供 LLM / Embedding / RAG / Agent 四大模块的统一抽象，让 sz-rust 从通用 Web 框架升级为 AI 应用开发框架。

## 功能

- **LLM 统一抽象**：`LlmProvider` trait + OpenAI / Claude / Gemini 三家 Provider 实现
- **模型路由**：`ModelRouter` 基于 `ArcSwap` 无锁热替换路由表
- **故障切换**：`ProviderFailover` 状态机（Available → Degraded → Cooldown）+ `call_with_failover`
- **上下文裁剪**：`ContextTruncator` 自动裁剪超长上下文（保留 System 消息）
- **Token 计数**：`TokenCounter` 带缓存的无推理计数
- **Embedding**：`EmbeddingProvider` trait + OpenAI Embedding + `BatchChunker` 批量分片
- **向量存储**：`VectorStore` trait + `SimilarityMetric`（Cosine / Dot / L2）
- **RAG 管道**：`RagPipeline` 三段式（retrieve → assemble → generate）+ 引用溯源
- **Agent 引擎**：`Agent` + `AgentExecutor` 工具选择循环 + `TerminationPolicy`
- **MCP 桥接**：`McpToolBridge` 将 sz-rust-mcp 7 工具暴露为 Agent 可用工具
- **Facade 静态 API**：`Ai::chat / stream_chat / embed / rag / agent` + `OnceLock` 全局实例
- **可观测性**：7 个 Prometheus 指标 + `tracing` 结构化日志
- **审计 HTTP 客户端**：`AuditHttpClient` + 令牌桶限流

## 用法

### 基本聊天

```rust
use sz_rust_ai_facade::Ai;
use sz_rust_ai_facade::llm::provider::{ChatRequest, ChatMessage, Role};

// 初始化（通常在应用启动时）
Ai::init_default(router, embedding, vector_store, rag, tools)?;

// 同步聊天
let req = ChatRequest::new("gpt-4o", vec![
    ChatMessage { role: Role::User, content: "你好".into(), tool_call_id: None, tool_calls: None },
]);
let result = Ai::chat(req).await?;
println!("{}", result.choices[0].message.content);
```

### Embedding

```rust
let result = Ai::embed(vec!["hello".into(), "world".into()], "text-embedding-3-small").await?;
assert_eq!(result.embeddings.len(), 2);
```

### RAG 检索增强生成

```rust
use sz_rust_ai_facade::rag::pipeline::RagRequest;

let req = RagRequest::new("什么是 Rust？", "tenant-1");
let result = Ai::rag(req).await?;
println!("{}", result.content);
```

### Agent 编排

```rust
use sz_rust_ai_facade::agent::engine::{AgentTask, AgentOptions};

let task = AgentTask::new("分析这段代码并给出优化建议");
let opts = AgentOptions::new("tenant-1");
let result = Ai::agent(task, opts).await?;
println!("{}", result.final_answer);
```

## 配置

在 `config.toml` 中添加 `[ai]` 段：

```toml
[ai]
default_model = "gpt-4o"

[ai.providers.openai]
api_key = "sk-..."
base_url = "https://api.openai.com/v1"

[ai.providers.claude]
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com"

[ai.routing]
"gpt-4o" = "openai"
"claude-3-opus" = "claude"

[ai.rate_limit]
enabled = true
requests_per_minute = 60

[ai.failover]
threshold = 3
cooldown_ms = 5000
```

## 与现有 facade 的集成关系

| facade | 集成点 |
|--------|--------|
| `http-facade` | 流式响应经 `SseEvent` 透传 |
| `orm-facade` | 向量存储复用 ORM 查询 |
| `cache-facade` | Token 计数缓存 |
| `observability` | Prometheus 指标注册 |
| `mcp` | 7 工具桥接为 Agent 可用工具 |

## PHP `think\facade\Ai` 对齐

本 facade 的静态 API 设计对齐 PHP ThinkPHP 的 `think\facade\Ai`：
- `Ai::chat()` ↔ `Ai::chat()`
- `Ai::streamChat()` ↔ `Ai::streamChat()`
- `Ai::embed()` ↔ `Ai::embed()`
- `Ai::rag()` ↔ `Ai::rag()`
- `Ai::agent()` ↔ `Ai::agent()`

## 依赖

- `sz-rust-orm-facade` / `sz-rust-cache-facade` / `sz-rust-http-facade`
- `sz-rust-mcp` / `sz-rust-observability`
- `reqwest`（HTTP 客户端）/ `arc-swap`（无锁路由）/ `futures`（流式）

## 版本策略

与 `sz-rust-core` 保持同步。
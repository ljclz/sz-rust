# ADR-AI-02: LLM Provider 统一抽象 — 三家标准化

- **状态**: Accepted
- **日期**: 2026-08-10
- **相关代码**: `packages/sz-rust-ai-facade/src/llm/provider.rs`、`src/llm/openai.rs`、`src/llm/claude.rs`、`src/llm/gemini.rs`

## 背景

OpenAI、Claude（Anthropic）、Gemini（Google）三家 LLM API 的请求/响应格式各不相同：
- OpenAI：`/v1/chat/completions`，`choices[].message.content`
- Claude：`/v1/messages`，`content[].text`（content block 数组）
- Gemini：`generateContent`，`candidates[].content.parts[].text`

## 决策

定义 `LlmProvider` trait 统一抽象，三家 Provider 各自实现格式转换：

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError>;
    async fn stream_completion(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError>;
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError>;
    fn supported_models(&self) -> &[&str];
}
```

所有 Provider 返回统一的 `ChatCompletion`（含 `choices[].message.content` / `tool_calls` / `finish_reason` / `usage`）。

## 理由

- **业务解耦**：上层代码不感知具体 Provider，通过 `ModelRouter` 路由
- **可扩展**：新增 Provider 只需实现 trait，不影响上层
- **流式统一**：`StreamDelta` 统一三家流式增量格式

## 影响

契约测试验证三家返回字段集合一致（`tests/llm_contract.rs`）。
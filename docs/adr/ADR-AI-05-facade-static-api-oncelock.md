# ADR-AI-05: Facade 静态 API — OnceLock 全局实例对齐 PHP think\facade

- **状态**: Accepted
- **日期**: 2026-08-10
- **相关代码**: `packages/sz-rust-ai-facade/src/facade.rs`

## 背景

sz-rust 现有 11 个 facade 均采用 `OnceLock` + 静态 API 模式（对齐 PHP ThinkPHP `think\facade`）。AI facade 需保持一致风格。

## 决策

`Ai` 零大小结构体 + `OnceLock<AiInstance>` 全局实例 + 静态方法：

```rust
static GLOBAL: OnceLock<AiInstance> = OnceLock::new();

pub struct Ai;

impl Ai {
    pub fn init_default(...) -> Result<(), AiError> { GLOBAL.set(...).map_err(...) }
    pub async fn chat(req: ChatRequest) -> Result<ChatCompletion, AiError> { ... }
    pub async fn embed(texts: Vec<String>, model: &str) -> Result<EmbeddingResult, AiError> { ... }
    pub async fn rag(req: RagRequest) -> Result<RagResult, AiError> { ... }
    pub async fn agent(task: AgentTask, opts: AgentOptions) -> Result<AgentResult, AiError> { ... }
}
```

## 理由

- **风格一致**：与 `Cache` / `Http` / `Orm` 等 11 个 facade 代码模式统一
- **PHP 对齐**：`Ai::chat()` ↔ `think\facade\Ai::chat()`，降低 PHP 开发者迁移成本
- **线程安全**：`OnceLock` 保证全局唯一初始化，无数据竞争

## 代价

- 全局可变性受限（仅初始化一次），但 AI 配置运行时变更通过 `ModelRouter::apply_update()` 实现

## 影响

端到端测试验证 `Ai::chat/embed/rag/agent` 全链路可用（`tests/facade_e2e.rs`）。
# ADR-AI-04: RAG 三段式管道 — retrieve → assemble → generate

- **状态**: Accepted
- **日期**: 2026-08-10
- **相关代码**: `packages/sz-rust-ai-facade/src/rag/pipeline.rs`

## 背景

RAG（检索增强生成）需要将向量检索、上下文组装、LLM 生成三个阶段串联。设计需支持：
- 三段独立调用（可单独测试/复用）
- 一次性 `rag()` 调用（端到端便捷）
- 预算控制（token 上限截断上下文）
- 引用溯源（citations 字段）

## 决策

`RagPipeline` 持有 embedding + vector_store + llm 三个依赖，暴露三段方法 + 串联方法：

```rust
pub async fn rag(&self, req: RagRequest) -> Result<RagResult, AiError> {
    let hits = self.retrieve(&req.query, req.topk).await?;
    let context = self.assemble(&hits, req.token_budget).await?;
    let result = self.generate(&context, &req.query).await?;
    Ok(result)
}
```

## 理由

- **可测试**：每段可独立 Mock 测试（`tests/rag_pipeline.rs`）
- **可复用**：retrieve 结果可用于前端高亮、assemble 结果可用于调试
- **降级友好**：空检索 → 空 context → LLM 仍生成（无引用回答）

## 影响

`RagResult` 含 `content` / `citations` / `warnings` 三字段，支持引用溯源与降级告警。
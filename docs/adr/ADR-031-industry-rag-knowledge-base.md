# ADR-031: 行业 RAG 知识库三段式管道

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-rag/src/`, `packages/sz-rust-rag/data/`

## 背景

P1-2 缺口：SDD Agent 缺乏行业知识支撑，生成的设计方案和代码缺乏行业最佳实践参考。

## 决策

1. **29+ 项目向量化**：glossary.json（25 术语）+ rules.json（10 规则）+ templates.json（7 模型）
2. **retrieve→assemble→generate 三段式管道**：检索→组装上下文→生成建议
3. **降级策略**：JSON 加载失败返回 Ok(0) 不阻断启动
4. **来源标注**：所有检索结果附来源文件和行号

## 替代方案

- **全文检索**：缺乏语义理解
- **外部 RAG 服务**：依赖外部 API，延迟和成本高

## Bug 定位提示

- `data/glossary.json` — 25 个行业术语定义
- `data/rules.json` — 10 条业务规则
- `data/templates.json` — 7 个数据模型模板
- `src/term.rs` — `load_from_json` 降级安全加载

## 影响

- SDD Agent Design 阶段集成 RAG 检索（search_industry_practices）
- Coding 阶段集成 few-shot 示例（search_few_shot_examples）
- 52 tests passed
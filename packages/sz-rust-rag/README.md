# sz-rust-rag

行业 RAG 知识库 — 为 SDD Agent 提供生鲜零售行业知识检索能力。

## 职责

- 项目源码语料向量化与语义检索
- 行业术语/业务规则/数据模型模板版本化管理
- 多源融合检索（代码向量 + 术语 + 规则 + 模板）
- Capability 集成（注册为 Skill 供 SDD Agent 调用）

## 模块结构

| 模块 | 职责 |
|------|------|
| `error` | RagError 统一错误类型（17 变体） |
| `warning` | RagWarningCode 警告码（6 变体） |
| `config` | rag.toml 异步加载 + notify 文件监听 + arc-swap 热替换 |
| `metrics` | 5 个 Prometheus 指标 |
| `corpus` | 项目语料扫描（packages/*/src/**/*.rs） |
| `chunking` | 语义分块（按 Rust item 边界） |
| `redact` | 源码脱敏（API Key/PEM/password） |
| `vectorize` | 向量化编排 + 断点续跑日志 |
| `store` | 版本化存储泛型（内存 + 文件原子写入） |
| `term` | 行业术语库（CRUD + 搜索 + 历史） |
| `rule` | 业务规则库（CRUD + 来源校验） |
| `template` | 数据模型模板库（CRUD + 搜索） |
| `search` | IndustryRagSearcher 多源融合检索 |
| `capability` | Capability 适配（注册为 Skill） |
| `audit` | 审计日志（查询脱敏 + 追加写） |
| `facade` | IndustryRag 全局单例 + 静态 API |

## 依赖关系

- `sz-rust-ai-facade`：EmbeddingProvider / VectorStore / AiError
- `sz-rust-capability`：Capability trait / CapabilityRegistry

## 配置

见 `config/rag.toml`（13 个字段）。

## 使用示例

```rust
use sz_rust_rag::{IndustryRag, RagSearchRequest};

// 初始化
IndustryRag::init(embedding, vector_store, config, term_store, rule_store, template_store, metrics)?;

// 注册为 Capability
IndustryRag::register_capability(&registry)?;

// 检索
let req = RagSearchRequest::new("生鲜称重商品如何计价", "tenant-001");
let result = IndustryRag::search(req).await?;
```

## 性能指标

- 单次检索 ≤ 800ms（含 Embedding + VectorStore + 融合 + 组装）
- 术语/规则/模板查询 ≤ 50ms（内存索引）
- 冷启动全量向量化 ≤ 30min
- 单 crate 增量向量化 ≤ 90s

## 安全约束

- `#![forbid(unsafe_code)]`
- 统一 `tokio::fs`（禁止 `std::fs`）
- 源码脱敏后向量化（API Key / PEM / password）
- 租户隔离（所有知识按 `tenant_id` 隔离）
- 审计日志查询文本脱敏
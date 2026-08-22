# sz-rust-sz300

鲜视达 SZ-300 业务应用 — 设备/商户/商品/订单管理示例。

## 分布式追踪

sz-rust-tracing 为独立库，未接入默认链路。如需分布式追踪，手动添加 sz-rust-tracing 依赖：

```toml
[dependencies]
sz-rust-tracing.workspace = true
```

并在 `main.rs` 中使用 `SzTracer::new("sz300")` 替代原生 `tracing_subscriber::fmt`。

当前默认使用 `tracing_subscriber` + EnvFilter + JSON 格式，已满足基本日志需求。
OTLP 导出可通过启用 `otlp` / `otlp-http` feature 激活。

## Qdrant 向量数据库

启用 `qdrant` feature 后，RAG 知识库可使用 Qdrant 作为专业向量数据库（替代默认的 FileVectorStore）：

```bash
# 编译时启用
cargo build --features qdrant

# 运行时配置（环境变量）
export SZ300_QDRANT_URL=http://localhost:6333
export SZ300_QDRANT_COLLECTION=sz300_vectors  # 可选，默认 sz300_vectors
export SZ300_QDRANT_API_KEY=your-key          # 可选，Qdrant Cloud / 启用 auth 时
```

未设置 `SZ300_QDRANT_URL` 时自动降级为 FileVectorStore，不影响现有功能。
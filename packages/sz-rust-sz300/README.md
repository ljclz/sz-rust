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
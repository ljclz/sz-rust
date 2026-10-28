---
name: sz-rust-performance-check
description: 性能基线检查 — 确保代码变更不引入性能回退。修改 hot path 时触发。
tools: [criterion, cargo-bench]
agentMode: auto
---

# 性能基线检查（sz-rust）

## 触发条件

- 修改路由处理、中间件、数据库查询等 hot path
- 新增循环内数据库操作

## 检查步骤

1. 运行基准测试：`cargo bench -p sz-rust-core --no-run` 然后 `cargo bench -p sz-rust-core`
2. 对比基线（`target/criterion/` 中的历史数据）
3. 检查 p99 延迟是否回退超过 10%

## 通过标准

- p99 延迟回退 <= 10%
- 吞吐量下降 <= 5%
- 无新增 N+1 查询

## 失败处理

性能回退时，分析火焰图（`cargo flamegraph`），优化热点代码。

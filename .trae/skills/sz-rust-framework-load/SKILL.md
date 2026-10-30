---
name: sz-rust-framework-load
description: 使用 Criterion 和 heaptrack 压测，确保空载内存 < 20MB，P99 < 5ms。
tools: [criterion, heaptrack]
agentMode: auto
---

# 负载与内存基线（sz-rust framework）

## 执行

```bash
cargo bench -p framework
heaptrack cargo run -p framework --release
```

## 通过标准

- 空载 RSS ≤ 20MB。
- 简单路由 P99 ≤ 5ms。
- 10 万次请求内存增长 < 1MB。

# SZ-Rust P2 长期路线图（v0.5+）

> ⚠️ 已归档，原因：P0-P4 已完成，仅 P2-1 待上游配合，归档日期：2026-08-09

> **生成日期**：2026-08-06
> **最后更新**：2026-08-10
> **当前版本**：v0.7.0（P0-P4 全部完成 + crates.io 全量发布 + 多并发压测）
> **基线测试**：4,610 passed, 0 failed
> **状态**：P2-6/P2-2/P2-3/P2-4/P2-5/P2-7 已完成，P2-1 待上游配合；P0-P4 评估建议全部完成

---

## 一、P2 方向总览

| ID | 方向 | 风险 | 依赖 | 状态 |
|----|------|------|------|------|
| P2-6 | 横向对比 benchmark（TechEmpower） | 低 | 无 | ✅ 完成 |
| P2-2 | 分布式追踪 OTLP | 中 | opentelemetry crate | ✅ 完成 |
| P2-3 | WebSocket 长连接 | 中 | axum WebSocket + Redis | ✅ 完成 |
| P2-4 | GraphQL 支持 | 中 | async-graphql crate | ✅ 完成 |
| P2-1 | 多后端 ORM 支持 | 高 | sz-orm 上游配合 | ⏸️ 待上游配合 |
| P2-5 | WASM 边缘计算 | 高 | WASI 兼容性 | ✅ 完成 |
| P2-7 | K8s Operator | 高 | k8s API + kube crate | ✅ 完成 |

**启动顺序**：P2-6（低风险）→ P2-2/3/4（中风险）→ P2-1/5/7（高风险）

---

## 二、详细方向

### P2-6 横向对比 benchmark（低风险）

**目标**：与 axum/actix/Rocket 同环境 TechEmpower benchmark，量化 sz-rust 在 Web 框架生态中的性能定位。

**依赖**：无外部依赖，仅需 TechEmpower FrameworkBenchmarks 仓库集成。

**风险分析**：
- 低风险：纯测量任务，不修改框架核心
- 风险点：benchmark 结果可能暴露性能短板，需客观面对

**预估工时**：4h

**展开条件**：P0 + P1 全部完成（已满足）

---

### P2-2 分布式追踪 OTLP（中风险）

**目标**：sz-rust-tracing 集成 OpenTelemetry OTLP 导出，支持 Jaeger/Tempo/Zipkin 后端。

**依赖**：
- `opentelemetry` crate（0.2x 系列）
- `opentelemetry-otlp` 导出器
- `tracing-opentelemetry` 桥接层

**风险分析**：
- 中风险：opentelemetry crate API 变动频繁，需锁定版本
- 风险点：与现有 `tracing` 生态集成可能有 trait 冲突

**预估工时**：8h

**展开条件**：P2-6 完成后启动

---

### P2-3 WebSocket 长连接（中风险）

**目标**：axum WebSocket + 心跳机制 + 集群广播（Redis pub/sub），支持实时通信场景。

**依赖**：
- `axum::extract::ws`（已内置，axum 0.8 ws feature）
- `redis` crate（workspace 已有）
- 心跳协议设计

**风险分析**：
- 中风险：WebSocket 连接管理 + 集群广播需处理分布式状态
- 风险点：连接泄漏、心跳超时、广播顺序性

**预估工时**：6h

**展开条件**：P2-6 完成后启动

---

### P2-4 GraphQL 支持（中风险）

**目标**：集成 `async-graphql` crate，提供 GraphQL 查询/变更/订阅端点，与现有 ORM 层对接。

**依赖**：
- `async-graphql` crate（7.x 系列）
- `async-graphql-axum` 集成层
- sz-orm 模型 → GraphQL 类型映射

**风险分析**：
- 中风险：GraphQL N+1 查询问题需 DataLoader 解决
- 风险点：与 REST 路由共存时的路由冲突、权限穿透

**预估工时**：8h

**展开条件**：P2-6 完成后启动

---

### P2-1 多后端 ORM 支持（高风险）

**目标**：sz-orm 扩展 PostgreSQL/MySQL/SQLite/Oracle/MSSQL 驱动支持，sz-rust 透明适配。

**依赖**：
- sz-orm 上游配合（**严禁修改上游仓库**，需协调 sz-orm 团队）
- `sqlx` 多驱动 feature gate

**风险分析**：
- 高风险：依赖 sz-orm 上游配合，跨团队协调成本高
- 风险点：Oracle/MSSQL 驱动成熟度、连接池差异、SQL 方言兼容

**预估工时**：16h（含 sz-orm 协调）

**展开条件**：P2-2/3/4 完成后启动，且 sz-orm 上游就绪

---

### P2-5 WASM 边缘计算（高风险）

**目标**：sz-rust-wasm 编译到 WASM，支持 Cloudflare Workers/Deno Deploy/边缘节点部署。

**依赖**：
- WASI 兼容性（WASI 0.2 preview2）
- `wasm-bindgen` + `wasmtime` 运行时
- axum → WASM 适配（axum 不直接支持 WASM，需适配层）

**风险分析**：
- 高风险：axum/hyper/tokio 生态对 WASM 支持有限
- 风险点：异步运行时差异（tokio vs WASM 异步）、文件系统 API 缺失

**预估工时**：12h

**展开条件**：P2-2/3/4 完成后启动，且 WASI 生态成熟

---

### P2-7 K8s Operator（高风险）

**目标**：sz-rust K8s Operator，自动化部署/扩缩容/配置管理/滚动更新。

**依赖**：
- `kube` crate（Rust K8s 客户端）
- `controller-runtime` 模式
- CRD（Custom Resource Definition）设计

**风险分析**：
- 高风险：K8s API 复杂度高，Operator 模式需深入理解 K8s 调谐循环
- 风险点：CRD 版本兼容、Leader Election、状态一致性

**预估工时**：12h

**展开条件**：P2-2/3/4 完成后启动

---

## 三、启动顺序与依赖关系

```
P2-6（低风险，无依赖）
  │
  ├──→ P2-2（中风险，依赖 opentelemetry）
  ├──→ P2-3（中风险，依赖 axum ws + redis）
  └──→ P2-4（中风险，依赖 async-graphql）
         │
         ├──→ P2-1（高风险，依赖 sz-orm 上游）
         ├──→ P2-5（高风险，依赖 WASI 生态）
         └──→ P2-7（高风险，依赖 k8s API）
```

**总预估工时**：~66h（P2-6: 4h + P2-2/3/4: 22h + P2-1/5/7: 40h）

---

## 四、本期状态

- **P0 阶段**：✅ 全部完成（v0.3.4，8 个任务）
- **P1 阶段**：✅ 全部完成（v0.4.0，9 个任务）
- **P2 阶段**：📋 路线图登记完成，待启动

**下一步**：P2-6（横向对比 benchmark）可作为 P2 首个启动项，无外部依赖，风险最低。
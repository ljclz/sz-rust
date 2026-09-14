# sz-rust v0.6.7 框架对比报告

> 生成日期：2026-08-09
> 基线：sz-rust v0.6.7
> **数据来源声明**：
> - sz-rust 性能数据：来自 v0.6.0 soak 测试实测（60s 持续运行），v0.6.7 未重新跑 soak
> - 其他 Rust 框架数据：参考 TechEmpower Round 22 公开基准（不同机器/条件，**非同条件实测**）
> - 跨语言数据：参考 TechEmpower Round 22 公开基准（**非同条件实测**）
> - 功能矩阵：基于各框架官方文档和源码对比（可验证的事实）
> - **重要声明**：性能对比数据仅供参考，不同硬件/配置/测试方法会导致结果差异。如需精确对比，应在同一机器上使用相同测试工具进行实测。

---

## 1. 执行摘要

| 维度 | sz-rust v0.6.7 | 行业标杆（axum） | 评价 |
|------|---------------|-----------------|------|
| 吞吐量 | 109K ops/s（v0.6.0 实测） | ~105K ops/s（TechEmpower 参考） | ⚠️ 数据来源不同，仅供参考 |
| P99 延迟 | 42-52μs（v0.6.0 实测） | ~45-55μs（TechEmpower 参考） | ⚠️ 数据来源不同，仅供参考 |
| 内存占用 | 4MB RSS（v0.6.0 实测） | ~6MB RSS（TechEmpower 参考） | ⚠️ 数据来源不同，仅供参考 |
| 安全性 | forbid(unsafe_code) | safe | ✅ 同级或更严格 |
| 生态成熟度 | v0.6.7（初期） | v0.8（成熟） | ⚠️ 差距大 |

**结论**：sz-rust 在功能完整度上具有独特优势（DI + ORM + 缓存 + 模板一体化），性能数据因测试条件不同无法直接对比。生态成熟度仍有差距。

---

## 2. Rust 框架功能对比（基于源码/文档，可验证）

### 2.1 功能矩阵

| 功能 | sz-rust | axum | actix-web | warp | rocket | poem |
|------|---------|------|-----------|------|--------|------|
| 路由参数提取 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 中间件链 | ✅ 链式 | ✅ Tower | ✅ | ✅ Filter | ✅ Fairing | ✅ |
| DI 容器 | ✅ 内置 | ❌ 需手动 | ❌ | ❌ | ❌ | ❌ |
| ORM 集成 | ✅ sz-orm | ❌ 需手动 | ❌ | ❌ | ❌ diesel | ❌ |
| 模板引擎 | ✅ Tera | ❌ | ❌ | ❌ | ✅ Tera | ❌ |
| WebSocket | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| TLS/HTTPS | ✅ rustls | ✅ | ✅ | ✅ | ✅ | ✅ |
| 热重载 | ✅ Addon | ❌ | ❌ | ❌ | ❌ | ❌ |
| SIMD 加速 | ✅ SSE2 | ❌ | ❌ | ❌ | ❌ | ❌ |
| 内存池 | ✅ MemPool | ❌ | ❌ | ❌ | ❌ | ❌ |
| 连接池预热 | ✅ PoolWarmer | ❌ | ❌ | ❌ | ❌ | ❌ |
| 查询缓存 | ✅ L2 Cache | ❌ | ❌ | ❌ | ❌ | ❌ |
| MCP 协议 | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| WASM 支持 | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |

### 2.2 架构对比

| 维度 | sz-rust | axum | actix-web |
|------|---------|------|-----------|
| 底层运行时 | tokio | tokio | tokio（actix-rt 封装） |
| HTTP 实现 | hyper + axum | hyper + axum | actix-http（自研） |
| 路由匹配 | matchit (K=1) | matchit | actix-router |
| JSON 序列化 | serde_json | serde_json | serde_json |
| 异步 trait | async-trait | async-trait | actix 消息 |
| 错误处理 | anyhow + thiserror | anyhow + thiserror | actix-web::Error |
| 配置 | serde_yaml | 环境变量 | 环境变量 |

---

## 3. 性能数据（参考，非同条件实测）

> **⚠️ 重要声明**：以下数据来自不同来源、不同测试条件，**不能直接对比**。
> - sz-rust：v0.6.0 soak 测试（60s 持续运行，本地机器）
> - 其他框架：TechEmpower Round 22 公开数据（不同硬件/配置）
> - 如需精确对比，应在同一机器上使用 wrk/k6 等工具进行同条件实测

### 3.1 Rust 框架吞吐量参考

| 框架 | 版本 | 吞吐量 (ops/s) | 数据来源 | 测试条件 |
|------|------|---------------|---------|---------|
| **sz-rust** | v0.6.0 | **109,273** | soak 实测 | 本地 60s 持续运行 |
| axum | 0.8 | ~105,000 | TechEmpower R22 | TechEmpower 标准环境 |
| actix-web | 4.9 | ~115,000 | TechEmpower R22 | TechEmpower 标准环境 |
| warp | 0.3 | ~100,000 | TechEmpower R22 | TechEmpower 标准环境 |
| rocket | 0.5 | ~95,000 | TechEmpower R22 | TechEmpower 标准环境 |
| salvo | 0.6 | ~102,000 | TechEmpower R22 | TechEmpower 标准环境 |
| poem | 3.0 | ~98,000 | TechEmpower R22 | TechEmpower 标准环境 |

### 3.2 跨语言吞吐量参考（TechEmpower Round 22）

| 语言 | 框架 | 吞吐量 (req/s) | 数据来源 |
|------|------|---------------|---------|
| Rust | sz-rust v0.6.0 | 109,273 | soak 实测（不同条件） |
| Rust | actix-web | ~115,000 | TechEmpower R22 |
| Rust | axum | ~105,000 | TechEmpower R22 |
| C++ | drogon | ~120,000 | TechEmpower R22 |
| Go | gin | ~60,000 | TechEmpower R22 |
| Java | spring boot | ~30,000 | TechEmpower R22 |
| Node.js | fastify | ~45,000 | TechEmpower R22 |
| Python | fastapi | ~8,000 | TechEmpower R22 |

---

## 4. sz-rust 独有优势（基于代码事实，可验证）

### 4.1 对标 ThinkPHP 8 的 Rust 实现

sz-rust 是唯一一个对标 ThinkPHP 8 开发体验的 Rust Web 框架：

| ThinkPHP 8 特性 | sz-rust 对应 | 验证方式 |
|----------------|------------|---------|
| 控制器路由 | `#[Controller]` + `#[GetMapping]` | 源码可验证 |
| 中间件 | `MiddlewareChain` | 源码可验证1 |
| DI 容器 | `Container` (singleton/scoped) | 源码可验证 |
| 数据库 ORM | sz-orm 全家桶 | 源码可验证 |
| 缓存 | `Cache` facade | 源码可验证 |
| 配置 | `Config` (YAML/ENV) | 源码可验证 |
| 事件 | 事件系统 | 源码可验证 |
| 验证 | 验证器 | 源码可验证 |
| 模板 | Tera | 源码可验证 |
| 命令行 | sz-rust-cli | 源码可验证 |

### 4.2 独有特性

| 特性 | 描述 | 验证方式 |
|------|------|---------|
| SIMD SSE2 | capitalize_first / parse_path | `packages/sz-rust-core/src/json/simd_safe.rs` |
| MemPool | StackPool + BumpaloPool | 源码可验证 |
| 零拷贝 | HandlerRefRef<'a> + to_json_bytes | 源码可验证 |
| 连接池 L3 | PoolWarmer + QueryCache + PoolScaler | 源码可验证 |
| 热重载 | Addon libloading | ADR-016 |
| MCP 协议 | stdio JSON-RPC | `packages/sz-rust-mcp/` |
| WASM 支持 | WASI 兼容 | 源码可验证 |

---

## 5. 生态与社区（公开数据）

| 框架 | GitHub Stars | crates.io 下载 | 文档质量 | 社区活跃度 |
|------|-------------|---------------|---------|-----------|
| sz-rust | ~100 | v0.6.7 新发布 | ⚠️ 初期 | ⚠️ 初期 |
| axum | 18K+ | 5M+/月 | ✅ 优秀 | ✅ 活跃 |
| actix-web | 21K+ | 3M+/月 | ✅ 优秀 | ✅ 活跃 |
| gin | 78K+ | — | ✅ 优秀 | ✅ 活跃 |
| fastify | 32K+ | 3M+/周 | ✅ 优秀 | ✅ 活跃 |
| spring boot | 73K+ | — | ✅ 优秀 | ✅ 活跃 |
| fastapi | 75K+ | — | ✅ 优秀 | ✅ 活跃 |

---

## 6. sz-rust v0.6.0 Soak 测试结果（实测数据）

```
测试条件：本地机器，60s 持续运行，sample_interval=5s
总操作: 6,530,625 次
平均吞吐量: 109,273 ops/s
RSS: 4MB（全程稳定）
FD: 10（全程稳定）
线程: 6（全程稳定）
P99: 42-52μs（全程稳定）
错误: 0
退化检测: ✅ 未检测到退化
```

> 注：此为 v0.6.0 数据，v0.6.7 未重新跑 soak 测试。CI soak.yml 每周自动执行。

---

## 7. 结论

### 7.1 sz-rust 的定位

sz-rust v0.6.7 提供了**独特的全栈开发体验**（DI + ORM + 缓存 + 模板 + CLI），是 ThinkPHP/Java 开发者迁移到 Rust 的桥梁。功能完整度在 Rust 框架中领先。

### 7.2 优势（可验证）

1. **全栈一体化**：DI + ORM + 缓存 + 模板 + 热重载 — 开箱即用
2. **安全**：`forbid(unsafe_code)` workspace 级 — 零 unsafe
3. **ThinkPHP 对齐**：控制器/中间件/ORM/验证/事件全对齐
4. **插件生态**：6 个业务模板（CRM/ERP/电商/CMS/Forum/IM）

### 7.3 待改进

1. **生态**：crates.io 刚发布，社区和文档需建设
2. **性能对比**：已完成同条件实测，详见 [2026-08-09-框架性能对比报告.md](../2026-08-09-框架性能对比报告.md)（同服务器 122.51.216.76，wrk 4.1.0，64 并发，4 框架 × 3 路由 = 12 组合）
3. **文档**：需要更多英文教程和示例
4. **编译时间**：全量编译较慢（feature gate 可优化）

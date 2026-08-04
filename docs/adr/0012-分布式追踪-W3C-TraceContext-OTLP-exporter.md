# ADR-012：分布式追踪（W3C TraceContext + OTLP exporter）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **优先级**：P1
> **关联 ADR**：ADR-011（可观测性模块，L3 指标层）
> **相关代码**：`packages/sz-rust-tracing/`

## 背景

SZ-Rust 的生产 Bug 定位遵循"四层模型"（详见《ADR 与生产 Bug 定位规范》第 3 节）：

| 层级 | 工具/产物 | 回答的问题 |
|------|----------|-----------|
| L1 决策层 | ADR | 这项行为是否违反了既定决策？ |
| **L2 运行时层** | **tracing 日志** | **请求实际走了哪条路径？哪一步出错？** |
| L3 指标层 | metrics 指标 | 异常是偶发还是持续？什么时间开始？ |
| L4 代码层 | 源码 + 测试 | 哪一行代码导致？是否有回归测试？ |

L2 运行时层目前缺失：生产环境只能依赖 L1（ADR 查阅），无法回答"请求实际走了哪条路径？哪一步出错？"。更关键的是，鲜视达涉及多个服务（szoa / szoapc / szweb / szadmin 等），跨服务调用链目前无法关联，一个请求经过多个服务后出错的根因定位极其困难。

sz-rust 需要决定如何实现分布式追踪，提供 L2 运行时层能力，并兼容 W3C TraceContext 标准，融入 OpenTelemetry 生态。

## 决策

采用 **Span 结构 + Tracer trait + W3C TraceContext + OTLP exporter 条件编译** 策略：

### 1. 创建 sz-rust-tracing 包（workspace 第 11 个成员）

```toml
# Cargo.toml
[workspace]
members = [
    "packages/sz-rust-core",
    # ... 前 10 个成员 ...
    "packages/sz-rust-tracing",  # 第 11 个成员
]
```

`sz-rust-tracing` 作为独立 workspace 成员，对外提供 `Span` 结构、`Tracer` trait、`SzTracer` 默认实现、W3C TraceContext 上下文传播、OTLP exporter 能力。

### 2. Span 结构 + Tracer trait + SzTracer 默认实现

```rust
// Span：追踪单元，记录一次操作的开始/结束/属性/事件
// Tracer trait：追踪器接口，负责创建 Span 与上下文传播
// SzTracer：框架默认实现，支持 W3C TraceContext 与 OTLP 导出
```

| 组件 | 职责 | 设计要点 |
|------|------|---------|
| `Span` | 追踪单元 | 记录 trace_id / span_id / parent_span_id / name / attributes / events / status |
| `Tracer` trait | 追踪器接口 | `start_span()` / `end_span()` / `inject_context()` / `extract_context()` |
| `SzTracer` | 默认实现 | 实现 `Tracer` trait，支持 W3C TraceContext 传播与 OTLP 导出 |

`Tracer` trait 抽象的动机：
- 业务可自定义 `Tracer` 实现（如 mock tracer 用于测试）
- 默认 `SzTracer` 集成 W3C TraceContext，但业务可替换为其他追踪器
- 与 sz-orm-tracing 的 `Tracer` trait 保持 API 一致

### 3. W3C TraceContext 规范

对齐 W3C TraceContext 规范（https://www.w3.org/TR/trace-context/）：

```rust
// traceparent header 格式：
// traceparent: 00-<trace_id>-<span_id>-<flags>
//
// 00          : version（固定 00）
// <trace_id>  : 32 字符十六进制（16 字节 trace ID）
// <span_id>   : 16 字符十六进制（8 字节 span ID）
// 01          : trace flags（01 表示 sampled）
//
// 示例：traceparent: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01
```

| 字段 | 长度 | 说明 |
|------|------|------|
| version | 2 字符 | 版本号，当前固定 `00` |
| trace_id | 32 字符 | 16 字节 trace ID，整个调用链唯一 |
| span_id | 16 字符 | 8 字节 span ID，单次操作唯一 |
| flags | 2 字符 | trace flags，`01` 表示 sampled（需记录并导出） |

选择 W3C TraceContext 而非 B3 / Jaeger 专有格式的原因：
- W3C 是国际标准，OpenTelemetry / Jaeger / Zipkin / Datadog 均支持
- 与 sz-orm-tracing 的实现一致，跨项目复用
- 避免供应商锁定

### 4. 向后兼容 legacy header（trace-id / span-id）

```rust
// 优先解析 W3C traceparent header
// 若不存在，回退解析 legacy header：
//   trace-id: <trace_id>
//   span-id:  <span_id>
//
// 用于兼容旧版客户端 / 中间件（如 PHP 端历史调用方）
```

向后兼容策略：
- **入站**：优先解析 `traceparent`，若缺失则解析 `trace-id` / `span-id`，两者均缺失则生成新 trace
- **出站**：始终发送 W3C `traceparent`，同时发送 `trace-id` / `span-id`（兼容旧版下游）

### 5. OTLP exporter 条件编译（`#[cfg(feature = "otlp")]`）

```toml
# packages/sz-rust-tracing/Cargo.toml
[features]
default = []                 # 默认仅本地 span 记录
otlp = ["dep:opentelemetry"] # 启用 OTLP gRPC 导出
```

```rust
// 仅在启用 otlp feature 时编译 OTLP exporter
#[cfg(feature = "otlp")]
pub struct OtlpExporter {
    // OTLP gRPC exporter 实现
}

#[cfg(not(feature = "otlp"))]
pub struct OtlpExporter;  // 空实现，编译期消除
```

| feature | 用途 | 默认 |
|---------|------|------|
| `otlp` | OTLP gRPC 导出（对接 OpenTelemetry Collector / Jaeger / Tempo） | ❌ 禁用 |

条件编译的原因：
- 默认场景（单机 / 本地开发）无需 OTLP，避免引入 opentelemetry SDK
- 生产场景启用 `otlp`，对接统一可观测性平台
- `#[cfg(not(feature = "otlp"))]` 时 `OtlpExporter` 为空实现，零运行时开销

### 6. OtlpGuard Drop 优雅关闭

```rust
// OtlpGuard 实现 Drop trait，在应用关闭时优雅 flush 未导出的 span
// 防止进程退出时丢失最后一批 span 数据
pub struct OtlpGuard {
    exporter: Option<OtlpExporter>,
}

impl Drop for OtlpGuard {
    fn drop(&mut self) {
        // 同步 flush 所有未导出的 span
        // 阻塞等待 flush 完成或超时（默认 5 秒）
    }
}
```

`OtlpGuard` 的使用模式：
- 应用启动时创建 `OtlpGuard`，持有 exporter 句柄
- 应用正常退出（`SIGTERM` / `Ctrl+C`）时，`Drop` 触发 flush
- flush 超时（默认 5 秒）后强制退出，避免阻塞过久

## 后果

### 正面后果

- **提供 L2 运行时层**：与 ADR 四层 Bug 定位模型对齐，生产 Bug 可通过 tracing span 链定位请求实际路径与出错步骤
- **W3C 标准兼容 OpenTelemetry 生态**：`traceparent` header 可被 Jaeger / Tempo / Datadog / Zipkin 等直接采集，无供应商锁定
- **跨服务追踪**：一个请求经过 szoa → szoapc → szweb 的完整调用链可通过 trace_id 关联，根因定位无需人工拼接日志
- **向后兼容 legacy header**：旧版 PHP 客户端 / 中间件无需改造即可接入追踪
- **条件编译降低默认开销**：`otlp` feature 按需启用，默认零外部依赖
- **对齐 sz-orm-tracing**：与关联项目 SZ-ORM 的追踪模块实现一致，跨项目复用

### 负面后果

- **`otlp` feature 依赖 opentelemetry SDK**：启用后编译时间显著增加（opentelemetry 依赖链庞大，含 tonic / prost / h2 等）
- **增加 1 个 workspace 成员**：workspace 成员从 10 增至 11，编译时间略增
- **trace_id 透传的侵入性**：跨服务调用需显式注入 / 提取 `traceparent` header，业务代码需感知（或通过中间件自动处理）
- **Span 内存开销**：每个 span 记录属性 / 事件，高 QPS 场景下内存占用需关注（建议采样率配置）
- **legacy header 兼容的维护成本**：未来 W3C TraceContext 普及后，legacy header 支持可能成为技术债务

## 替代方案

### 方案 A：直接使用 opentelemetry crate（拒绝）

```rust
// 直接依赖 opentelemetry crate，使用其内置的 Tracer / Span 接口
```

**拒绝原因**：
- `opentelemetry` crate 过于底层，需要自定义 Span 结构以对齐 sz-orm-tracing 的 API
- 强制依赖 opentelemetry SDK，即使不需要 OTLP 导出也要承受编译开销
- 无法自定义 `Tracer` trait 的方法签名（如 `start_span` 的参数），灵活性不足
- 与 sz-orm-tracing 的实现不一致，跨项目复用成本高

### 方案 B：仅实现日志追踪（拒绝）

仅通过 `tracing` crate 的日志（`info!` / `error!`）记录请求路径，不实现 Span 结构与上下文传播。

**拒绝原因**：
- 日志无法跨服务关联：服务 A 的日志与服务 B 的日志无法通过 trace_id 串联，跨服务根因定位仍需人工拼接
- 无法对接可视化平台：Jaeger / Tempo 等需要 Span 结构化数据，纯日志无法生成调用链拓扑
- 缺少上下文传播：`traceparent` header 的注入 / 提取需要显式实现，纯日志无法自动透传
- 与 L2 运行时层的定位目标不符：日志只能回答"发生了什么"，无法回答"请求走了哪条路径"

## 关联

- **对齐 sz-orm-tracing 的实现**：与关联项目 SZ-ORM 的追踪模块保持 API 一致，`Span` 结构、`Tracer` trait、`SzTracer` 默认实现、W3C TraceContext 传播逻辑对齐，支持跨项目复用
- **对应 ADR-011（observability 模块，L3 指标层）**：本 ADR 提供 L2 运行时层，ADR-011 提供 L3 指标层，两者共同构成"运行时可观测性"能力。L2 追踪请求路径定位出错步骤，L3 量化异常范围与时间窗口，配合 L1 决策层（ADR）与 L4 代码层（源码 + 测试）形成完整四层 Bug 定位闭环

## 注意事项

- **traceparent header 大小写**：HTTP header 不区分大小写，但建议统一使用小写 `traceparent`，与 W3C 规范示例一致
- **trace_id 生成**：trace_id 必须是 16 字节随机数，禁止使用 0 或全 F（W3C 规范规定这些是无效值）
- **span_id 生成**：span_id 必须是 8 字节随机数，禁止使用 0（W3C 规范规定 0 是无效值）
- **采样率配置**：高 QPS 场景建议配置采样率（如 10%），避免 span 数据量爆炸；错误请求建议 100% 采样
- **legacy header 优先级**：入站时若同时存在 `traceparent` 和 `trace-id` / `span-id`，以 `traceparent` 为准，忽略 legacy header
- **`otlp` feature 的初始化**：启用 `otlp` 时，需在应用启动时初始化 OTLP exporter，并通过 `OtlpGuard` 确保关闭时 flush
- **`OtlpGuard` 的生命周期**：`OtlpGuard` 必须在应用主函数作用域持有，不能在子函数中创建（否则函数返回时 Drop 触发，导致 exporter 过早关闭）
- **Span 的并发安全**：`Span` 通过 `Arc<Mutex<...>>` 保护，高并发场景需注意锁竞争

## Bug 定位提示

如果生产 Bug 表现为"调用链断裂"、"trace_id 丢失"或"Span 数据缺失"：

1. **L1 决策层**：查阅本 ADR，确认是否使用了 `SzTracer`，`traceparent` header 是否在中间件中正确注入 / 提取
2. **L2 运行时层**：
   - 调用链断裂 Bug → 检查 span 的 `parent_span_id` 是否正确设置，`trace_id` 是否在同一调用链内一致
   - trace_id 丢失 Bug → 检查入站中间件是否提取了 `traceparent` header，出站 HTTP 客户端是否注入了 `traceparent` header
   - Span 数据缺失 Bug → 检查 `Tracer::start_span()` 是否被调用，`end_span()` 是否在请求结束时触发
3. **L3 指标层**：检查 `trace.span.duration` 指标按 `span_name` 标签的分布，定位耗时异常的 span
4. **L4 代码层**：
   - traceparent 格式错误 Bug → 检查 `00-<trace_id>-<span_id>-01` 格式是否符合 W3C 规范，trace_id / span_id 长度是否正确
   - legacy header 兼容 Bug → 检查入站解析是否优先 `traceparent`，回退 `trace-id` / `span-id`
   - OTLP 导出失败 Bug → 检查 `otlp` feature 是否启用，OTLP exporter 初始化是否成功，`OtlpGuard` 是否在正确作用域持有
   - Span 丢失 Bug → 检查 `OtlpGuard::drop` 是否在进程退出时触发 flush，flush 超时是否过短
   - 跨服务 trace_id 不一致 Bug → 检查出站 HTTP 客户端是否调用了 `inject_context()` 注入 `traceparent` header
   - **trace_id/span_id 全零** → W3C 规范规定 trace_id 和 span_id 禁止全零，生成时需用加密安全的随机数（`getrandom` crate），不可用 `rand::random()` 的种子为 0 的实例
   - **`OtlpGuard` 作用域错误** → `OtlpGuard` 在子函数中创建会在函数返回时 Drop，导致 exporter 过早关闭，最后一批 span 丢失；必须在 `main()` 函数作用域持有

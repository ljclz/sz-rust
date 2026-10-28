# ADR-011：可观测性模块（MetricsRegistry + SLO 多窗口燃烧率）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **优先级**：P1
> **关联 ADR**：ADR-012（分布式追踪，L2 运行时层）
> **相关代码**：`packages/sz-rust-observability/`

## 背景

SZ-Rust 的生产 Bug 定位遵循"四层模型"（详见《ADR 与生产 Bug 定位规范》第 3 节）：

| 层级 | 工具/产物 | 回答的问题 |
|------|----------|-----------|
| L1 决策层 | ADR | 这项行为是否违反了既定决策？ |
| L2 运行时层 | tracing 日志 | 请求实际走了哪条路径？哪一步出错？ |
| **L3 指标层** | **metrics 指标** | **异常是偶发还是持续？什么时间开始？** |
| L4 代码层 | 源码 + 测试 | 哪一行代码导致？是否有回归测试？ |

L3 指标层目前缺失：生产环境只能依赖 L1（ADR 查阅）和 L2（tracing 日志），但无法回答"异常是偶发还是持续？什么时间开始？"这类时间维度问题。更关键的是，生产告警目前只能基于阈值（如 QPS > 1000）触发，无法基于 SLO 预算消耗速率触发，导致告警频繁误报或漏报。

sz-rust 需要决定如何实现可观测性模块，提供 L3 指标层能力，并与 Google SRE Workbook 第 5 章的多窗口多燃烧率告警模型对齐。

## 决策

采用 **MetricsRegistry + 三类型指标 + Prometheus 输出 + SLO 多窗口燃烧率** 策略：

### 1. 创建 sz-rust-observability 包（workspace 第 10 个成员）

```toml
# Cargo.toml
[workspace]
members = [
    "packages/sz-rust-core",
    # ... 前 9 个成员 ...
    "packages/sz-rust-observability",  # 第 10 个成员
]
```

`sz-rust-observability` 作为独立 workspace 成员，对外提供 `MetricsRegistry`、`Counter` / `Gauge` / `Histogram` 三类型指标、Prometheus 文本格式导出、SLO 多窗口燃烧率告警能力。

### 2. MetricsRegistry + 三类型指标

```rust
// 对齐 Prometheus 指标模型，提供三种核心指标类型
// Counter：单调递增（如请求总数、错误总数）
// Gauge：可增可减（如当前连接数、队列长度）
// Histogram：分桶统计（如请求延迟分布）
```

| 指标类型 | 语义 | 典型场景 |
|---------|------|---------|
| `Counter` | 单调递增计数器 | `http.requests.total`、`http.errors.total` |
| `Gauge` | 可增可减瞬时值 | `db.connections.active`、`queue.length` |
| `Histogram` | 分桶累计分布 | `http.request.duration`（P50/P90/P99） |

`MetricsRegistry` 作为全局注册中心，负责：
- 指标注册（名称 + 标签集合唯一性校验，防止重复注册）
- 指标查询（按名称 + 标签过滤）
- 指标导出（统一序列化为 Prometheus 文本格式）

### 3. Prometheus 文本格式输出

```rust
// 输出格式对齐 Prometheus exposition format
// # HELP http_requests_total Total HTTP requests
// # TYPE http_requests_total counter
// http_requests_total{method="GET",status="200"} 12345
```

选择 Prometheus 文本格式而非 Protobuf 的原因：
- 文本格式是 Prometheus 生态的事实标准
- 易于调试（可直接 curl /metrics 查看）
- 无需 Protobuf 依赖，减少编译时间

### 4. SLO 4 窗口多燃烧率告警（Google SRE Workbook 第 5 章）

对齐 Google SRE Workbook 第 5 章 "Alerting on SLOs" 的多窗口多燃烧率（Multi-Window Multi-Burn-Rate）模型：

```rust
// 4 个窗口 = 2 对（长窗口 + 短窗口）
// 燃烧率 = 实际错误率 / 允许错误率（SLO 预算消耗速率）
//
// 第 1 对：长窗口 1h + 短窗口 5m，燃烧率阈值 14.4
//   → 1 小时内消耗 2% SLO 预算，触发 Page 级告警
// 第 2 对：长窗口 6h + 短窗口 30m，燃烧率阈值 6.0
//   → 6 小时内消耗 5% SLO 预算，触发 Ticket 级告警
```

| 窗口对 | 长窗口 | 短窗口 | 燃烧率阈值 | SLO 预算消耗 | 告警级别 |
|--------|--------|--------|-----------|-------------|---------|
| 第 1 对 | 1h | 5m | 14.4 | 2% | Page（立即处理） |
| 第 2 对 | 6h | 30m | 6.0 | 5% | Ticket（工作时间内处理） |

**为什么需要短窗口**：长窗口单独使用会导致告警延迟（如 1h 窗口最坏情况要 1h 才触发）。短窗口作为"快速确认"，只有长窗口和短窗口同时超过燃烧率阈值才告警，减少因短暂抖动导致的误报。

**为什么需要 2 对窗口**：单对窗口只能覆盖一种告警级别。2 对窗口分别覆盖"快速严重故障"和"慢速持续退化"两种场景。

### 5. 条件编译：prometheus / otlp 两个可选 feature

```toml
# packages/sz-rust-observability/Cargo.toml
[features]
default = ["prometheus"]
prometheus = []              # Prometheus 文本格式导出（默认启用）
otlp = ["dep:opentelemetry"] # OTLP gRPC 导出（可选，对接 OTel Collector）
```

| feature | 用途 | 默认 |
|---------|------|------|
| `prometheus` | Prometheus 文本格式导出（`/metrics` 端点） | ✅ 启用 |
| `otlp` | OTLP gRPC 导出（对接 OpenTelemetry Collector） | ❌ 禁用 |

条件编译的原因：
- 默认场景（单机 / Prometheus pull 模式）只需 `prometheus` feature，零外部依赖
- 云原生场景（OTel Collector push 模式）启用 `otlp` feature，对接统一可观测性平台
- 避免 `opentelemetry` SDK 成为强制依赖，减少默认编译时间

## 后果

### 正面后果

- **提供 L3 指标层**：与 ADR 四层 Bug 定位模型对齐，生产 Bug 可通过指标层缩小时间范围与影响面
- **SLO 多窗口告警减少误报**：长窗口 + 短窗口双确认机制，避免短暂抖动触发误报；2 对窗口覆盖 Page / Ticket 两级告警
- **Prometheus 生态兼容**：文本格式输出可被 Prometheus / Grafana / VictoriaMetrics 等直接采集
- **条件编译降低默认开销**：`prometheus` feature 零外部依赖，`otlp` feature 按需启用
- **对齐 sz-orm-observability**：与关联项目 SZ-ORM 的可观测性模块实现一致，跨项目复用

### 负面后果

- **增加 1 个 workspace 成员**：workspace 成员从 9 增至 10，编译时间略增
- **SLO 燃烧率计算有内存开销**：4 个窗口需要维护 4 个滑动窗口计数器，每个 Histogram 分桶需独立维护
- **`otlp` feature 依赖 opentelemetry SDK**：启用后编译时间显著增加（opentelemetry 依赖链庞大）
- **SLO 阈值需要调优**：14.4 / 6.0 是 Google SRE 推荐值，但实际业务可能需要调整，初次接入需要观测周期

## 替代方案

### 方案 A：直接使用 prometheus crate（拒绝）

```rust
// 直接依赖 prometheus crate，使用其内置的 Registry / Counter / Gauge / Histogram
```

**拒绝原因**：
- `prometheus` crate 的 SLO 支持需要自行实现，无法直接复用
- 无法自定义指标注册逻辑（如标签唯一性校验、命名空间隔离）
- 与 sz-orm-observability 的实现不一致，跨项目复用成本高
- 强制依赖 prometheus crate，无法支持 OTLP 导出

### 方案 B：不实现 SLO 监控（拒绝）

仅实现 Counter / Gauge / Histogram 三类型指标和 Prometheus 输出，不实现 SLO 多窗口燃烧率告警。

**拒绝原因**：
- 生产告警依赖 SLO 燃烧率，仅靠阈值告警（QPS > N / 错误率 > X%）会导致大量误报
- 无法回答"SLO 预算还剩多少？什么时候会耗尽？"这类容量规划问题
- 与 Google SRE 最佳实践脱节，运维团队需要自行实现 SLO 逻辑

## 关联

- **对齐 sz-orm-observability 的实现**：与关联项目 SZ-ORM 的可观测性模块保持 API 一致，指标类型、注册接口、Prometheus 输出格式对齐，支持跨项目复用
- **对应 ADR-012（tracing 模块，L2 运行时层）**：本 ADR 提供 L3 指标层，ADR-012 提供 L2 运行时层，两者共同构成"运行时可观测性"能力。L2 追踪请求路径，L3 量化异常范围，配合 L1 决策层（ADR）与 L4 代码层（源码 + 测试）形成完整四层 Bug 定位闭环

## 注意事项

- **MetricsRegistry 全局单例**：通过 `OnceLock<MetricsRegistry>` 提供全局实例，测试时需显式重置或使用独立实例
- **指标命名规范**：指标名使用 `snake_case`，带 `.` 分隔命名空间（如 `http.requests.total`），导出时转换为 Prometheus 的 `_` 分隔
- **标签基数控制**：标签值必须是低基数（如 `method`、`status`），禁止使用 `user_id`、`request_id` 等高基数值，否则会导致内存爆炸
- **Histogram 分桶**：默认分桶对齐 Prometheus 标准桶（`0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10`），业务可自定义
- **SLO 窗口实现**：4 个窗口使用滑动窗口算法，需注意窗口边界的并发安全（`Arc<RwLock<...>>`）
- **`otlp` feature 的初始化**：启用 `otlp` 时，需在应用启动时初始化 OTLP exporter，并在关闭时优雅 flush

## Bug 定位提示

如果生产 Bug 表现为"指标缺失"、"指标值异常"或"SLO 告警误报/漏报"：

1. **L1 决策层**：查阅本 ADR，确认是否使用了 `MetricsRegistry` 全局实例，指标类型是否正确（Counter 单调递增、Gauge 可增可减、Histogram 分桶）
2. **L2 运行时层**：检查 tracing span `metrics.record` 中的 `name`、`labels`、`value` 字段，确认指标是否被记录
3. **L3 指标层**：
   - 指标缺失 Bug → 检查 `/metrics` 端点输出，确认指标名与标签是否匹配
   - 指标值异常 Bug → 检查 Counter 是否被错误重置、Gauge 是否并发竞争、Histogram 分桶是否正确
   - SLO 告警误报 Bug → 检查短窗口（5m / 30m）是否因抖动触发，长窗口是否同步超过阈值
   - SLO 告警漏报 Bug → 检查燃烧率阈值（14.4 / 6.0）是否被错误调高，SLO 目标是否设置过松
4. **L4 代码层**：
   - 指标未注册 Bug → 检查 `MetricsRegistry::register()` 是否在启动时调用
   - 标签基数爆炸 Bug → 检查标签值是否包含高基数变量（user_id / request_id）
   - OTLP 导出失败 Bug → 检查 `otlp` feature 是否启用，OTLP exporter 初始化是否成功
   - SLO 窗口计算错误 Bug → 检查滑动窗口的边界条件，特别是窗口切换时的计数器重置逻辑
   - **测试污染全局指标** → `MetricsRegistry` 通过 `OnceLock` 提供全局实例，测试间未重置会导致指标累加；使用 `MetricsRegistry::reset_for_test()` 或在每个测试中创建独立实例
   - **标签基数爆炸** → 标签值包含 `user_id` / `request_id` 等高基数变量时，Prometheus 内存占用线性增长；检查 `metrics.record` 的 `labels` 是否包含动态字符串

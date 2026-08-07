# SZ-Rust ADR 索引

> **项目**：SZ-Rust（鲜视达 Rust Web 框架）
> **维护规则**：每完成一项重大架构决策必须新增 ADR，并更新本索引
> **文档版本**：v1.2（2026-08-05）

---

## 1. ADR 目录

> 当前 ADR 数量：**16**（P0×4 + P1×6 + P2×6，覆盖 16 项关键架构决策）
> ADR 密度：16 / 28 模块 = **0.571**（超过 ≥ 0.15 目标，参见《ADR 与生产 Bug 定位规范》第 4 节）

| 编号 | 标题 | 状态 | 日期 | 决策者 | 文件 |
|------|------|------|------|--------|------|
| ADR-001 | 三层路由机制（属性宏 / 配置式 / 约定式） | 已接受 | 2026-07-22 | SZ-Rust Team | [0001-三层路由机制.md](0001-三层路由机制.md) |
| ADR-002 | 中间件模型（Tower Service + 洋葱模型） | 已接受 | 2026-07-22 | SZ-Rust Team | [0002-中间件模型-Tower-Service-洋葱模型.md](0002-中间件模型-Tower-Service-洋葱模型.md) |
| ADR-003 | 控制器抽象（SzController trait + 默认方法 + 组合） | 已接受 | 2026-07-22 | SZ-Rust Team | [0003-控制器抽象-trait-默认方法-组合.md](0003-控制器抽象-trait-默认方法-组合.md) |
| ADR-004 | Model 钩子实现（re-export sz-orm-core + 16 事件） | 已接受 | 2026-07-22 | SZ-Rust Team | [0004-Model钩子实现-re-export-sz-orm-core.md](0004-Model钩子实现-re-export-sz-orm-core.md) |
| ADR-005 | 事务管理策略（委托 sz-orm-core + 显式 begin/commit/rollback） | 已接受 | 2026-07-22 | SZ-Rust Team | [0005-事务管理策略-委托sz-orm-core.md](0005-事务管理策略-委托sz-orm-core.md) |
| ADR-006 | 认证授权机制（JWT + Middleware + Guard 三层分离） | 已接受 | 2026-07-22 | SZ-Rust Team | [0006-认证授权机制-JWT-Middleware-Guard三层分离.md](0006-认证授权机制-JWT-Middleware-Guard三层分离.md) |
| ADR-007 | addon 插件化机制（编译期注册 + Cargo feature） | 已接受 | 2026-07-22 | SZ-Rust Team | [0007-addon插件化机制-编译期注册-Cargo-feature.md](0007-addon插件化机制-编译期注册-Cargo-feature.md) |
| ADR-008 | 错误处理策略（AppError 枚举 + ErrorCode 映射 + BaseException 对齐） | 已接受 | 2026-07-22 | SZ-Rust Team | [0008-错误处理策略-AppError枚举-ErrorCode映射.md](0008-错误处理策略-AppError枚举-ErrorCode映射.md) |
| ADR-009 | 缓存策略（Cache facade + 全局实例 + 多驱动 + PHP 源码 bug 复刻） | 已接受 | 2026-07-22 | SZ-Rust Team | [0009-缓存策略-Cache-facade-全局实例-多驱动.md](0009-缓存策略-Cache-facade-全局实例-多驱动.md) |
| ADR-010 | 配置加载方式（serde + YAML + 环境变量覆盖 + 默认值） | 已接受 | 2026-07-22 | SZ-Rust Team | [0010-配置加载方式-serde-YAML-环境变量覆盖.md](0010-配置加载方式-serde-YAML-环境变量覆盖.md) |
| ADR-011 | 可观测性模块（MetricsRegistry + SLO 多窗口燃烧率） | 已接受 | 2026-07-22 | SZ-Rust Team | [0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md](0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) |
| ADR-012 | 分布式追踪（W3C TraceContext + OTLP exporter） | 已接受 | 2026-07-22 | SZ-Rust Team | [0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md](0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) |
| ADR-013 | 多租户支持（thread_local TenantContext + TenantRepository 装饰器） | 已接受 | 2026-08-02 | SZ-Rust Team | [0013-multi-tenant-thread-local-repository-decorator.md](0013-multi-tenant-thread-local-repository-decorator.md) |
| ADR-014 | GraphQL 集成（sz-orm-graphql facade 透传） | 已接受 | 2026-08-02 | SZ-Rust Team | [0014-graphql-integration-facade.md](0014-graphql-integration-facade.md) |
| ADR-015 | gRPC 支持（sz-orm-grpc facade 透传） | 已接受 | 2026-08-02 | SZ-Rust Team | [0015-grpc-support-facade.md](0015-grpc-support-facade.md) |
| ADR-016 | Addon 热加载探索（libloading 运行时动态加载 + unsafe_code 策略变更） | 已接受 | 2026-08-02 | SZ-Rust Team | [0016-addon-hot-reload-libloading-unsafe.md](0016-addon-hot-reload-libloading-unsafe.md) |

**状态说明**：
- `提议`（Proposed）：已提交但尚未评审
- `已接受`（Accepted）：评审通过，作为当前标准
- `已废弃`（Deprecated）：被新 ADR 取代
- `已替代`（Superseded）：被新 ADR 替代，保留历史

---

## 2. 为什么需要 ADR

### 2.1 背景

SZ-Rust 是一个对标 ThinkPHP 8 的 Rust Web 框架，底层依赖 axum（Tower 兼容是硬约束），上层借鉴 ThinkPHP 8 / Salvo / Spring Boot 的设计哲学。框架涉及大量架构决策（路由策略、中间件模型、控制器抽象、Model 钩子、事务管理、缓存策略、认证授权、addon 插件化等），这些决策一旦做出即难以推翻。

历史教训（来自关联项目 SZ-ORM）：
- 6 个 Critical SQL 注入漏洞源于"使用字符串拼接而非参数化查询"的隐性决策，从未被记录
- 7 个虚假/伪实现源于"开发阶段允许 todo!() 占位"的隐性约定，从未被显式禁止
- 8 处名实不符源于"API 命名随意"的隐性习惯，从未被审查
- feature flag 隔离失败源于"real-* feature 不参与 CI"的隐性默认，从未被质疑

这些"隐性决策"在缺乏 ADR 记录的情况下积累成技术债务，最终导致大规模返工。

### 2.2 ADR 的价值

| 价值 | 说明 |
|------|------|
| **显式化隐性决策** | 把"为什么这么做"从开发者脑中搬到文档中，避免知识随人员流失 |
| **阻止重复犯错** | 后续修改者可通过 ADR 了解历史决策的背景与约束，不重蹈覆辙 |
| **加速 Bug 定位** | 生产 Bug 往往源于违反了某项决策约束，ADR 提供决策层定位线索 |
| **支持架构演进** | 废弃旧 ADR、新增新 ADR 的过程即是架构演进的显式记录 |
| **AI 协作基础** | AI Agent 修改代码前必须阅读相关 ADR，避免违反既定决策 |

### 2.3 何时需要写 ADR

以下场景必须新增 ADR：

- 选择某种路由策略（约定路由 / 配置路由 / 属性宏路由）
- 选择某种中间件模型（Tower Service / 自定义 Middleware trait / Handler=Middleware 统一）
- 选择某种控制器抽象（trait + 默认方法 / 宏生成 / 手写 impl）
- 选择某种 Model 钩子实现（编译期注册表 / 运行时分派 / 派生宏）
- 选择某种事务管理策略（`#[transactional]` 宏 / 手动 begin/commit / 连接池级事务）
- 选择某种缓存策略（Service 注入 / `sz::cache!()` 宏 / thread_local）
- 选择某种认证授权机制（JWT / Session / Token / OAuth2）
- 选择某种 addon 插件化机制（编译期注册 / 运行时动态加载 / Cargo feature）
- 选择某种错误处理策略（`AppError` 枚举 / `anyhow` / `thiserror`）
- 选择某种配置加载方式（serde + TOML / 环境变量 / 启动时合并）
- 任何影响公共 API 表面的决策
- 任何影响性能特性（路由匹配复杂度、中间件链开销、序列化开销）的决策

---

## 3. ADR 与 Bug 定位（四层组合表）

> 详细规范参见 [《ADR 与生产 Bug 定位规范》](../ADR与生产Bug定位规范.md)

生产 Bug 的定位遵循"四层模型"，从决策层逐层下钻到代码层：

| 层级 | 工具/产物 | 回答的问题 | SZ-Rust 对应 |
|------|----------|-----------|--------------|
| **L1 决策层** | ADR | 这项行为是否违反了既定决策？ | 路由策略 ADR / 中间件模型 ADR / Model 钩子 ADR |
| **L2 运行时层** | tracing 日志 | 请求实际走了哪条路径？哪一步出错？ | `sz-rust-tracing`（W3C TraceContext）+ span 事件 |
| **L3 指标层** | metrics 指标 | 异常是偶发还是持续？什么时间开始？ | `sz-rust-observability`（Counter/Gauge/Histogram）|
| **L4 代码层** | 源码 + 测试 | 哪一行代码导致？是否有回归测试？ | `cargo test` + `#[test]` + git blame |

### 3.1 四层定位流程

```
生产 Bug 报告
      │
      ▼
┌─────────────────────────┐
│ L1 决策层：查阅相关 ADR  │  ← 判断 Bug 是否源于违反决策
└─────────────────────────┘
      │ 违反决策？
      ├── 是 → 修复代码使其符合 ADR（或更新 ADR）
      └── 否 ↓
┌─────────────────────────┐
│ L2 运行时层：查 tracing  │  ← 定位请求实际路径与出错步骤
└─────────────────────────┘
      │ 找到出错 span？
      ├── 是 → 进入 L4
      └── 否 ↓
┌─────────────────────────┐
│ L3 指标层：查 metrics    │  ← 判断异常范围与时间窗口
└─────────────────────────┘
      │ 缩小范围？
      ├── 是 → 进入 L4
      └── 否 ↓
┌─────────────────────────┐
│ L4 代码层：源码 + 测试   │  ← 定位具体代码行与回归测试
└─────────────────────────┘
      │
      ▼
  修复 + 回归测试 + 新增 ADR（若涉及决策变更）
```

### 3.2 SZ-Rust 关键路径的 ADR 覆盖要求

| 关键路径 | 必须有 ADR 覆盖 | 当前状态 |
|---------|----------------|---------|
| HTTP 请求生命周期 | ✅ 必须记录 | ✅ ADR-001/002 |
| 路由匹配（前缀树 / 通配符 / 参数提取） | ✅ 必须记录 | ✅ ADR-001 |
| 中间件链（洋葱模型 / 执行顺序） | ✅ 必须记录 | ✅ ADR-002 |
| 控制器分发（trait + 默认方法） | ✅ 必须记录 | ✅ ADR-003 |
| Model 钩子（编译期注册表） | ✅ 必须记录 | ✅ ADR-004 |
| 请求体解析（serde + FromRequest） | ✅ 必须记录 | ✅ ADR-003 |
| 响应序列化（IntoResponse） | ✅ 必须记录 | ✅ ADR-008 |
| 认证授权（JWT / 中间件） | ✅ 必须记录 | ✅ ADR-006 |
| 事务操作（`#[transactional]`） | ✅ 必须记录 | ✅ ADR-005 |
| 缓存读写（Service 注入） | ✅ 必须记录 | ✅ ADR-009 |
| 配置加载（YAML + 环境变量） | ✅ 必须记录 | ✅ ADR-010 |
| 可观测性（Metrics + SLO） | ✅ 必须记录 | ✅ ADR-011 |
| 分布式追踪（W3C TraceContext） | ✅ 必须记录 | ✅ ADR-012 |

---

## 4. ADR 写作模板

新建 ADR 时复制以下模板到 `adr/ADR-NNN-<short-title>.md`：

```markdown
# ADR-NNN：<标题>

> **状态**：提议 / 已接受 / 已废弃 / 已替代
> **日期**：YYYY-MM-DD
> **决策者**：<姓名 / 角色>
> **关联 ADR**：<编号列表，无则留空>

## 背景

<为什么需要做这个决策？当前面临什么问题？>

## 决策

<选择了什么方案？具体决策内容。>

## 后果

### 正面后果
- <列出正面影响>

### 负面后果
- <列出负面影响与权衡>

## 注意事项

<实施时需要注意的陷阱、约束、依赖关系。>

## Bug 定位提示

<如果生产 Bug 源于违反本 ADR，应如何定位？给出关键代码路径与 tracing span 名称。>
```

详细规范参见 [《ADR 与生产 Bug 定位规范》](../ADR与生产Bug定位规范.md) 第 3 节。

---

## 5. ADR 完成状态

所有已识别 ADR 均已完成编写（20/20）：

| 优先级 | 编号 | 标题 | 状态 | 完成日期 |
|--------|------|------|------|---------|
| P0 | ADR-001 | 三层路由机制（属性宏 / 配置式 / 约定式） | ✅ 已接受 | 2026-07-22 |
| P0 | ADR-002 | 中间件模型（Tower Service + 洋葱模型） | ✅ 已接受 | 2026-07-22 |
| P0 | ADR-003 | 控制器抽象（SzController trait + 默认方法 + 组合） | ✅ 已接受 | 2026-07-22 |
| P0 | ADR-004 | Model 钩子实现（re-export sz-orm-core + 16 事件） | ✅ 已接受 | 2026-07-22 |
| P1 | ADR-005 | 事务管理策略（委托 sz-orm-core） | ✅ 已接受 | 2026-07-22 |
| P1 | ADR-006 | 认证授权机制（JWT + Middleware + Guard） | ✅ 已接受 | 2026-07-22 |
| P1 | ADR-007 | addon 插件化机制（编译期注册 + Cargo feature） | ✅ 已接受 | 2026-07-22 |
| P1 | ADR-008 | 错误处理策略（AppError 枚举 + ErrorCode 映射） | ✅ 已接受 | 2026-07-22 |
| P2 | ADR-009 | 缓存策略（Cache facade + 全局实例 + 多驱动） | ✅ 已接受 | 2026-07-22 |
| P2 | ADR-010 | 配置加载方式（serde + YAML + 环境变量覆盖） | ✅ 已接受 | 2026-07-22 |
| P1 | ADR-011 | 可观测性模块（MetricsRegistry + SLO 多窗口燃烧率） | ✅ 已接受 | 2026-07-22 |
| P1 | ADR-012 | 分布式追踪（W3C TraceContext + OTLP exporter） | ✅ 已接受 | 2026-07-22 |
| P2 | ADR-013 | 多租户支持（thread_local TenantContext + TenantRepository 装饰器） | ✅ 已接受 | 2026-08-02 |
| P2 | ADR-014 | GraphQL 集成（sz-orm-graphql facade 透传） | ✅ 已接受 | 2026-08-02 |
| P2 | ADR-015 | gRPC 支持（sz-orm-grpc facade 透传） | ✅ 已接受 | 2026-08-02 |
| P2 | ADR-016 | Addon 热加载探索（libloading 运行时动态加载 + unsafe_code 策略变更） | ✅ 已接受 | 2026-08-02 |
| P2 | ADR-017 | sz-rust-core 拆包策略（Facade 渐进提取，7 个 facade） | ✅ 已接受 | 2026-08-03 |
| P2 | ADR-018 | Facade Crate 独立发布策略（0.x 统一版本 / 1.0 后 semver 独立） | ✅ 已接受 | 2026-08-03 |
| P3 | ADR-019 | P3 剩余模块解耦（四簇提取：orm-ext / router / middleware / mvc） | ✅ 已接受 | 2026-08-03 |
| P3 | ADR-020 | 异步文件 I/O 迁移（std::fs → tokio::fs，铁律 4 合规） | ✅ 已接受 | 2026-08-05 |

---

## 6. 引用

- [《ADR 与生产 Bug 定位规范》](../ADR与生产Bug定位规范.md) — ADR 写作规范与 Bug 定位流程
- [《软件项目审计清单》](../软件项目审计清单.md) — P0/P1/P2/P3 审计项
- [《SZ-Rust 工程化实践规范》](../sz-rust-engineering-practices.md) — 10 道门禁与五维审查
- [《SZ-Rust 架构设计与经验教训》](../sz-rust架构设计与经验教训.md) — 预架构设计文档
- [SZ-ORM ADR 索引](../../sz-orm/docs/adr/README.md) — 关联项目 ADR 参考

# SZ-Rust ADR Index

> **中文** | [English](README.en.md) — This is the English version of the ADR index.

> **Project**: SZ-Rust (Rust Web Framework for 鲜视达)
> **Maintenance Rule**: Each major architectural decision must add a new ADR and update this index
> **Document Version**: v1.2 (2026-08-05)

---

## 1. ADR Directory

> Current ADR count: **20** (P0×4 + P1×6 + P2×6 + P3×4, covering 20 key architectural decisions)
> ADR density: 20 / 31 modules = **0.645** (exceeds ≥ 0.15 target, see "ADR and Production Bug Localization Spec" Section 4)

| ID | Title | Status | Date | Decision Maker | File |
|----|-------|--------|------|----------------|------|
| ADR-001 | Three-layer routing (attribute macro / config / convention) | Accepted | 2026-07-22 | SZ-Rust Team | [0001-三层路由机制.md](0001-三层路由机制.md) |
| ADR-002 | Middleware model (Tower Service + onion model) | Accepted | 2026-07-22 | SZ-Rust Team | [0002-中间件模型-Tower-Service-洋葱模型.md](0002-中间件模型-Tower-Service-洋葱模型.md) |
| ADR-003 | Controller abstraction (SzController trait + default methods + composition) | Accepted | 2026-07-22 | SZ-Rust Team | [0003-控制器抽象-trait-默认方法-组合.md](0003-控制器抽象-trait-默认方法-组合.md) |
| ADR-004 | Model hooks (re-export sz-orm-core + 16 events) | Accepted | 2026-07-22 | SZ-Rust Team | [0004-Model钩子实现-re-export-sz-orm-core.md](0004-Model钩子实现-re-export-sz-orm-core.md) |
| ADR-005 | Transaction management (delegate to sz-orm-core + explicit begin/commit/rollback) | Accepted | 2026-07-22 | SZ-Rust Team | [0005-事务管理策略-委托sz-orm-core.md](0005-事务管理策略-委托sz-orm-core.md) |
| ADR-006 | Auth & authorization (JWT + Middleware + Guard three-layer separation) | Accepted | 2026-07-22 | SZ-Rust Team | [0006-认证授权机制-JWT-Middleware-Guard三层分离.md](0006-认证授权机制-JWT-Middleware-Guard三层分离.md) |
| ADR-007 | Addon plugin mechanism (compile-time registration + Cargo feature) | Accepted | 2026-07-22 | SZ-Rust Team | [0007-addon插件化机制-编译期注册-Cargo-feature.md](0007-addon插件化机制-编译期注册-Cargo-feature.md) |
| ADR-008 | Error handling (AppError enum + ErrorCode mapping + BaseException alignment) | Accepted | 2026-07-22 | SZ-Rust Team | [0008-错误处理策略-AppError枚举-ErrorCode映射.md](0008-错误处理策略-AppError枚举-ErrorCode映射.md) |
| ADR-009 | Cache strategy (Cache facade + global instance + multi-driver + PHP bug replication) | Accepted | 2026-07-22 | SZ-Rust Team | [0009-缓存策略-Cache-facade-全局实例-多驱动.md](0009-缓存策略-Cache-facade-全局实例-多驱动.md) |
| ADR-010 | Config loading (serde + YAML + env override + defaults) | Accepted | 2026-07-22 | SZ-Rust Team | [0010-配置加载方式-serde-YAML-环境变量覆盖.md](0010-配置加载方式-serde-YAML-环境变量覆盖.md) |
| ADR-011 | Observability (MetricsRegistry + SLO multi-window burn rate) | Accepted | 2026-07-22 | SZ-Rust Team | [0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md](0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md) |
| ADR-012 | Distributed tracing (W3C TraceContext + OTLP exporter) | Accepted | 2026-07-22 | SZ-Rust Team | [0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md](0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md) |
| ADR-013 | Multi-tenant (thread_local TenantContext + TenantRepository decorator) | Accepted | 2026-08-02 | SZ-Rust Team | [0013-multi-tenant-thread-local-repository-decorator.md](0013-multi-tenant-thread-local-repository-decorator.md) |
| ADR-014 | GraphQL integration (sz-orm-graphql facade passthrough) | Accepted | 2026-08-02 | SZ-Rust Team | [0014-graphql-integration-facade.md](0014-graphql-integration-facade.md) |
| ADR-015 | gRPC support (sz-orm-grpc facade passthrough) | Accepted | 2026-08-02 | SZ-Rust Team | [0015-grpc-support-facade.md](0015-grpc-support-facade.md) |
| ADR-016 | Addon hot-reload (libloading runtime dynamic load + unsafe_code policy change) | Accepted | 2026-08-02 | SZ-Rust Team | [0016-addon-hot-reload-libloading-unsafe.md](0016-addon-hot-reload-libloading-unsafe.md) |
| ADR-017 | sz-rust-core splitting (Facade progressive extraction, 7 facades) | Accepted | 2026-08-03 | SZ-Rust Team | [0017-sz-rust-core拆包策略-Facade渐进提取.md](0017-sz-rust-core拆包策略-Facade渐进提取.md) |
| ADR-018 | Facade crate independent publishing (0.x unified / 1.0+ semver independent) | Accepted | 2026-08-03 | SZ-Rust Team | [0018-facade-独立发布策略.md](0018-facade-独立发布策略.md) |
| ADR-019 | P3 remaining module decoupling (four-cluster extraction: orm-ext / router / middleware / mvc) | Accepted | 2026-08-03 | SZ-Rust Team | [0019-P3剩余模块解耦-四簇提取.md](0019-P3剩余模块解耦-四簇提取.md) |
| ADR-020 | Async file I/O migration (std::fs → tokio::fs, iron rule 4 compliance) | Accepted | 2026-08-05 | SZ-Rust Team | [0020-async-file-io-migration.md](0020-async-file-io-migration.md) |

**Status definitions**:
- `Proposed`: Submitted but not yet reviewed
- `Accepted`: Reviewed and approved as current standard
- `Deprecated`: Superseded by a new ADR
- `Superseded`: Replaced by a new ADR, kept for history

---

## 2. Why ADRs Are Needed

### 2.1 Background

SZ-Rust is a Rust Web framework aligned with ThinkPHP 8. It depends on axum (Tower compatibility is a hard constraint) at the lower level, and borrows design philosophy from ThinkPHP 8 / Salvo / Spring Boot at the upper level. The framework involves many architectural decisions (routing strategy, middleware model, controller abstraction, model hooks, transaction management, cache strategy, authentication, addon plugin mechanism, etc.), which are difficult to reverse once made.

Historical lessons (from related project SZ-ORM):
- 6 Critical SQL injection vulnerabilities stemmed from the implicit decision of "using string concatenation instead of parameterized queries", never recorded
- 7 fake/pseudo implementations stemmed from the implicit convention of "allowing todo!() placeholders during development", never explicitly prohibited
- 8 name-reality mismatches stemmed from the implicit habit of "casual API naming", never reviewed
- feature flag isolation failure stemmed from the implicit default of "real-* features not participating in CI", never questioned

These "implicit decisions" accumulated as technical debt without ADR records, eventually leading to large-scale rework.

### 2.2 Value of ADRs

| Value | Description |
|-------|-------------|
| **Explicitize implicit decisions** | Move "why we did this" from developers' minds to documentation, preventing knowledge loss |
| **Prevent repeated mistakes** | Future modifiers can understand historical decision context and constraints via ADR |
| **Accelerate bug localization** | Production bugs often stem from violating a decision constraint; ADR provides decision-level clues |
| **Support architecture evolution** | Deprecating old ADRs and adding new ones is the explicit record of architecture evolution |
| **AI collaboration foundation** | AI Agents must read relevant ADRs before modifying code, avoiding violations |

### 2.3 When to Write an ADR

ADR must be added in the following scenarios:

- Choosing a routing strategy (convention / config / attribute macro)
- Choosing a middleware model (Tower Service / custom Middleware trait / Handler=Middleware unification)
- Choosing a controller abstraction (trait + default methods / macro generation / manual impl)
- Choosing a model hook implementation (compile-time registry / runtime dispatch / derive macro)
- Choosing a transaction management strategy (`#[transactional]` macro / manual begin/commit / pool-level transaction)
- Choosing a cache strategy (Service injection / `sz::cache!()` macro / thread_local)
- Choosing an authentication mechanism (JWT / Session / Token / OAuth2)
- Choosing an addon plugin mechanism (compile-time registration / runtime dynamic loading / Cargo feature)
- Choosing an error handling strategy (`AppError` enum / `anyhow` / `thiserror`)
- Choosing a config loading method (serde + TOML / env vars / startup merge)
- Any decision affecting the public API surface
- Any decision affecting performance characteristics (routing match complexity, middleware chain overhead, serialization overhead)

---

## 3. ADR and Bug Localization (Four-Layer Model)

> Detailed spec: ["ADR and Production Bug Localization Spec"](../ADR与生产Bug定位规范.md)

Production bug localization follows a "four-layer model", drilling from decision layer to code layer:

| Layer | Tool/Artifact | Question Answered | SZ-Rust Mapping |
|-------|---------------|-------------------|-----------------|
| **L1 Decision** | ADR | Did this behavior violate an established decision? | Routing ADR / Middleware ADR / Model Hook ADR |
| **L2 Runtime** | tracing logs | What path did the request actually take? Which step failed? | `sz-rust-tracing` (W3C TraceContext) + span events |
| **L3 Metrics** | metrics | Is the anomaly sporadic or persistent? When did it start? | `sz-rust-observability` (Counter/Gauge/Histogram) |
| **L4 Code** | source + tests | Which line of code caused it? Is there a regression test? | `cargo test` + `#[test]` + git blame |

### 3.1 Four-Layer Localization Flow

```
Production Bug Report
       │
       ▼
┌─────────────────────────────┐
│ L1 Decision: Read relevant ADR │  ← Check if bug violates a decision
└─────────────────────────────┘
       │ Violated?
       ├── Yes → Fix code to comply with ADR (or update ADR)
       └── No ↓
┌─────────────────────────────┐
│ L2 Runtime: Check tracing     │  ← Locate actual request path and error step
└─────────────────────────────┘
       │ Found error span?
       ├── Yes → Enter L4
       └── No ↓
┌─────────────────────────────┐
│ L3 Metrics: Check metrics     │  ← Determine anomaly scope and time window
└─────────────────────────────┘
       │ Narrowed scope?
       ├── Yes → Enter L4
       └── No ↓
┌─────────────────────────────┐
│ L4 Code: Source + tests       │  ← Locate specific code line and regression test
└─────────────────────────────┘
       │
       ▼
   Fix + regression test + new ADR (if decision changed)
```

---

## 4. ADR Writing Template

Copy the following template to `adr/ADR-NNN-<short-title>.md`:

```markdown
# ADR-NNN: <Title>

> **Status**: Proposed / Accepted / Deprecated / Superseded
> **Date**: YYYY-MM-DD
> **Decision Maker**: <Name / Role>
> **Related ADRs**: <ID list, empty if none>

## Context

<Why is this decision needed? What problem are we facing?>

## Decision

<What solution was chosen? Specific decision content.>

## Consequences

### Positive Consequences
- <List positive impacts>

### Negative Consequences
- <List negative impacts and trade-offs>

## Notes

<Pitfalls, constraints, dependencies to watch for during implementation.>

## Bug Localization Tips

<If a production bug stems from violating this ADR, how to localize it? Provide key code paths and tracing span names.>
```

---

## 5. ADR Completion Status

All identified ADRs have been completed (20/20):

| Priority | ID | Title | Status | Completion Date |
|----------|----|-------|--------|-----------------|
| P0 | ADR-001 | Three-layer routing | Accepted | 2026-07-22 |
| P0 | ADR-002 | Middleware model (Tower + onion) | Accepted | 2026-07-22 |
| P0 | ADR-003 | Controller abstraction (trait + default methods) | Accepted | 2026-07-22 |
| P0 | ADR-004 | Model hooks (re-export + 16 events) | Accepted | 2026-07-22 |
| P1 | ADR-005 | Transaction management (delegate to sz-orm-core) | Accepted | 2026-07-22 |
| P1 | ADR-006 | Auth & authorization (JWT + Middleware + Guard) | Accepted | 2026-07-22 |
| P1 | ADR-007 | Addon plugin mechanism (compile-time + Cargo feature) | Accepted | 2026-07-22 |
| P1 | ADR-008 | Error handling (AppError + ErrorCode) | Accepted | 2026-07-22 |
| P2 | ADR-009 | Cache strategy (facade + global + multi-driver) | Accepted | 2026-07-22 |
| P2 | ADR-010 | Config loading (serde + YAML + env override) | Accepted | 2026-07-22 |
| P1 | ADR-011 | Observability (MetricsRegistry + SLO burn rate) | Accepted | 2026-07-22 |
| P1 | ADR-012 | Distributed tracing (W3C + OTLP) | Accepted | 2026-07-22 |
| P2 | ADR-013 | Multi-tenant (thread_local + decorator) | Accepted | 2026-08-02 |
| P2 | ADR-014 | GraphQL integration (facade passthrough) | Accepted | 2026-08-02 |
| P2 | ADR-015 | gRPC support (facade passthrough) | Accepted | 2026-08-02 |
| P2 | ADR-016 | Addon hot-reload (libloading + unsafe) | Accepted | 2026-08-02 |
| P2 | ADR-017 | Core splitting (Facade progressive extraction) | Accepted | 2026-08-03 |
| P2 | ADR-018 | Facade independent publishing | Accepted | 2026-08-03 |
| P3 | ADR-019 | P3 module decoupling (four-cluster) | Accepted | 2026-08-03 |
| P3 | ADR-020 | Async file I/O migration (tokio::fs) | Accepted | 2026-08-05 |

---

## 6. References

- ["ADR and Production Bug Localization Spec"](../ADR与生产Bug定位规范.md) — ADR writing spec and bug localization flow
- ["Software Project Audit Checklist"](../软件项目审计清单.md) — P0/P1/P2/P3 audit items
- ["SZ-Rust Engineering Practices"](../sz-rust-engineering-practices.md) — 10 gates and five-dimensional review
- [SZ-ORM ADR Index](../../sz-orm/docs/adr/README.md) — Related project ADR reference
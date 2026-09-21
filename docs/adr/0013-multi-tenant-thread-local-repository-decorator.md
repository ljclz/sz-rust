# ADR-013: 多租户支持 — thread_local TenantContext + TenantRepository 装饰器模式

- **状态**: Accepted
- **日期**: 2026-08-02
- **相关代码**: `packages/sz-rust-core/src/multi_tenant.rs (L42-L240)`、`packages/sz-rust-core/src/lib.rs (L88, L113-L121)`
- **修复编号**: P2 能力评估遗留项

## 背景

SZ-Rust 作为对标 ThinkPHP 8 的 Web 框架，需要支持 SaaS 场景下的多租户隔离。每个租户的数据必须严格隔离，不能互相访问。不实现多租户的后果：框架无法支撑 SaaS 业务，只能做单租户应用，商业场景受限。

多租户实现面临两个核心挑战：

1. **线程安全 vs 性能**：租户上下文需要在请求处理链路中传递（中间件 → 控制器 → Repository），传统方案用 `Arc<RwLock<Option<i64>>>`，但在高并发测试中会出现读写竞争（race condition），导致测试不稳定。
2. **Repository 透明装饰**：不能要求每个 Repository 调用方手动传入 tenant_id，需要透明地在 Repository 层自动注入租户过滤条件。

## 决策

### 方案选择：thread_local! + TenantRepository 装饰器

选择 `thread_local!` 存储当前租户 ID，而非 `Arc<RwLock<>>`。原因：

- axum 的请求处理在单个 tokio task 内同步执行（middleware chain 不跨 `.await` 切换线程），thread_local 在请求生命周期内稳定。
- 避免了 `Arc<RwLock>` 的锁竞争开销，读操作零开销。
- 实测：`Arc<RwLock>` 方案在 `cargo test --workspace` 并行执行时出现数据竞争告警，`thread_local!` 方案无此问题。

### 核心设计

```rust
// packages/sz-rust-core/src/multi_tenant.rs (L69-L95)
thread_local! {
    static TENANT_ID: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

pub struct TenantContext;
impl TenantContext {
    pub fn set_current(tenant_id: i64) { TENANT_ID.with(|cell| cell.set(Some(tenant_id))); }
    pub fn clear() { TENANT_ID.with(|cell| cell.set(None)); }
    pub fn current() -> Option<i64> { TENANT_ID.with(|cell| cell.get()) }
    pub fn require_current() -> Result<i64, TenantError> {
        Self::current().ok_or(TenantError::TenantNotSet)
    }
}
```

中间件在请求开始时调用 `TenantContext::set_current(tenant_id)`，请求结束时调用 `TenantContext::clear()`。

### TenantRepository 装饰器

```rust
// packages/sz-rust-core/src/multi_tenant.rs (L182-L240)
pub struct TenantRepository<E, R> {
    inner: Arc<R>,
    _marker: PhantomData<E>,
}

impl<E: TenantAware, R> TenantRepository<E, R> {
    // 透明包装任意 Repository，自动注入 tenant_id 过滤条件
    pub fn find_by_id(&self, id: &E::Key) -> RepositoryResult<Option<E>> {
        // 自动追加 WHERE tenant_id = {current_tenant_id}
    }
}
```

`TenantAware` trait（L105-L133）要求实体提供 `tenant_id_field()`、`tenant_id()`、`set_tenant_id()` 三个方法，TenantRepository 据此自动注入租户过滤。

### 关键约束

- `impl<E: TenantAware, R> TenantRepository<E, R>` — 固有 impl 块必须带 `E: TenantAware` 约束（L187），否则 `tenant_id_field()` 无法解析。
- 本模块使用 `#![forbid(unsafe_code)]`（L42），无 unsafe 操作。

## 后果

### 正面后果
- 租户上下文读取零开销（thread_local 直接访问，无锁）。
- Repository 调用方无需感知租户 ID，自动注入过滤条件，防止开发者遗漏。
- 与 axum 中间件模型无缝集成（TenantMiddleware 设置上下文）。

### 负面后果
- **async 边界限制**：thread_local 在 `.await` 后不一定保持（tokio 可能切换线程）。若控制器方法中有 `.await`，必须在 await 后重新调用 `TenantContext::require_current()` 验证，不能依赖 await 前的值。
- **测试隔离**：每个测试必须确保 `TenantContext::clear()` 被调用，否则线程复用时可能污染其他测试。使用 `#[should_panic]` 的测试尤其需要注意。
- **非 Send 语义**：`std::cell::Cell` 不是 `Send`，不能跨线程传递。若未来需要跨线程传递租户上下文，需改用 `Arc<AtomicI64>` 方案。
- **仅支持线程级隔离**：不支持协程级/任务级的细粒度租户切换（同一线程内多个并发请求共享 thread_local）。

## 注意事项

- **中间件顺序**：TenantMiddleware 必须在 AuthMiddleware 之后执行（先认证获取 user，再从 user 提取 tenant_id）。
- **clear 必须调用**：请求结束时必须调用 `TenantContext::clear()`，建议在中间件的 `finally` 块或 Drop guard 中调用。
- **E: TenantAware 约束**：使用 `TenantRepository<E, R>` 时，`E` 必须实现 `TenantAware`，否则编译失败。
- **与 sz-orm Repository trait 的关系**：`TenantRepository<E, R>` 实现了 `Repository<E>` trait，可直接替换原有 Repository 使用。

### Bug 定位提示

如果生产出现"租户数据泄漏"（租户 A 看到了租户 B 的数据）：
1. 检查是否有 Repository 调用绕过了 `TenantRepository`，直接使用了底层 `R`（未经装饰的原始 Repository）。
2. 检查 `TenantContext::clear()` 是否在请求结束时被正确调用（中间件 panic 路径是否遗漏）。
3. 检查 `.await` 后是否重新验证了 `TenantContext::current()`（await 后 thread_local 可能已切换）。
4. 检查 WHERE 条件是否被手动覆盖（`where_op("tenant_id", "=", other_tenant_id)` 绕过装饰器）。

如果生产出现"TenantNotSet 错误"：
1. 检查 TenantMiddleware 是否在路由链中注册。
2. 检查 AuthMiddleware 是否成功提取了 tenant_id（认证失败时 tenant_id 为空）。
3. 检查请求路径是否命中了白名单（某些公开路由不需要租户上下文）。

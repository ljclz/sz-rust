# ADR-039: 租户上下文注入方式 — request extensions 而非 thread_local

**状态**：Accepted
**日期**：2026-09-10
**决策者**：SZ-Rust Team

## 背景

多租户 SaaS 支持需要在请求处理链中传递租户上下文（`TenantContext`：tenant_id、is_platform_admin、resolve_source）。候选方案：

1. **request extensions**（axum `Request::extensions`）：将 `TenantContext` 注入到 axum request 的 extensions 中，后续中间件和 handler 通过 `Extension<TenantContext>` 提取。
2. **thread_local**：使用 `thread_local!` 静态变量存储当前请求的 `TenantContext`，handler 直接读取。
3. **显式参数传递**：将 `TenantContext` 作为每个 handler 的显式参数。

## 决策

选择 **request extensions**（方案 1）。

## 理由

1. **异步安全**：`thread_local` 在 async 运行时（tokio）中不可靠——任务可能在多个线程间迁移（work-stealing），`thread_local` 的值不会跟随迁移，导致租户上下文丢失或串租户。request extensions 存储在 `Request` 对象中，随请求传递，与线程无关。

2. **零侵入**：handler 通过 `Extension<TenantContext>` 提取，不改变函数签名（与 axum 生态一致），对现有代码无破坏。`thread_local` 需要显式调用 `with`/`enter`，侵入性更高。

3. **中间件链友好**：axum 中间件链天然支持 request extensions 传递，`tenant_resolve_middleware` 注入后，后续 `tenant_status_middleware`、`DataScopeMiddleware` 均可直接读取，无需额外接线。

4. **可测试性**：测试中通过 `req.extensions_mut().insert(TenantContext::new(...))` 注入，与生产路径一致，不依赖线程状态。`thread_local` 在测试中需要 `with` 块包裹，且并行测试间可能串扰。

5. **与 DataScopeContext 一致**：现有 `DataScopeContext` 已采用 request extensions 注入（`DataScopeMiddleware`），`TenantContext` 采用相同机制保持架构一致性。

## 后果

- `TenantContext` 必须实现 `Clone + Send + Sync`（axum extensions 要求）。
- handler 必须通过 `Extension<TenantContext>` 提取，而非全局变量。
- 在非 axum 上下文（如 orm-facade 层）中，通过 `TenantRequest` trait 抽象请求类型，避免 orm-facade 依赖 axum。

## 参考

- [ADR-0006: 认证授权机制 JWT-Middleware-Guard 三层分离](0006-认证授权机制-JWT-Middleware-Guard三层分离.md)
- [axum extensions 文档](https://docs.rs/axum/latest/axum/extract/struct.Request.html)
- spec 5.3.1.1（租户上下文解析）
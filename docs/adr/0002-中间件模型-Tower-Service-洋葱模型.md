# ADR-002：中间件模型（Tower Service + 洋葱模型）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-006（认证授权）、ADR-008（错误处理）
> **相关代码**：`packages/sz-rust-core/src/middleware/`（10 个子模块）

## 背景

SZ-Rust 基于 axum 0.8，axum 的中间件体系基于 `tower::Layer` + `tower::Service`。PHP 端中间件则通过 `app/middleware.php` 数组顺序执行。

需要解决以下问题：

1. **PHP 中间件顺序对齐**：PHP `app/middleware.php` 全局中间件顺序为 `[SessionInit, AllowCrossDomain]`，业务层中间件（如 `app\oapc\middleware\Auth`）追加在全局之后
2. **Tower 语义差异**：Rust 端 `tower::ServiceBuilder` 的 layer 是「后注册先执行」（stack 反向），与 PHP 数组顺序相反
3. **横切关注点分离**：日志/CORS/追踪/限流/鉴权各有不同的关注点，需要清晰的执行顺序
4. **Handler=Middleware 统一**：对齐 Salvo 设计，允许 Handler 直接作为 Middleware 使用

## 决策

采用 **Tower Service + 洋葱模型**，定义 5 个内置中间件，执行顺序固定：

### 中间件类型与执行顺序

```rust
pub enum MiddlewareKind {
    Trace,      // 1. 追踪 span（生成 request_id，对齐 PHP SessionInit）
    Cors,       // 2. CORS 跨域预处理（对齐 PHP AllowCrossDomain）
    Log,        // 3. 请求/响应日志（对齐 think-logger）
    RateLimit,  // 4. 限流（复用 sz-orm-limit）
    Auth,       // 5. JWT 鉴权（对齐 app\<app>\middleware\Auth）
}
```

执行顺序（业务期望）：

```
Trace → Cors → Log → RateLimit → Auth → [Guard] → Handler
```

### Tower 语义适配

```rust
// MiddlewareChain::order() 返回业务期望顺序（首元素最先执行）
// MiddlewareChain::service_builder_order() 返回 ServiceBuilder 注册顺序（逆序）
```

`MiddlewareChain` 内部会按需反转以适配 `ServiceBuilder::layer` 语义。

### 模块结构（10 个子模块）

| 模块 | 内容 | 实现阶段 |
|------|------|---------|
| `order` | `MiddlewareKind` 枚举 + `DEFAULT_ORDER` / `PHP_GLOBAL_ORDER` 常量 | Phase 3.1 ✅ |
| `chain` | `MiddlewareChain` 构建器（顺序定义 + 验证） | Phase 3.1 ✅ |
| `handler_as_middleware` | Handler=Middleware 双向转换器（对齐 Salvo 设计） | Phase 3.2 ✅ |
| `cors` | CORS 中间件（基于 `tower-http::cors`，对齐 PHP `app\CrossDomain`） | Phase 1.6 ✅ |
| `auth` | Auth 中间件（JWT 校验，复用 sz-orm-auth） | Phase 3.3 ✅ |
| `log` | Log 中间件（请求/响应日志，对齐 PHP `think-logger`） | Phase 3.4 ✅ |
| `rate_limit` | RateLimit 中间件（复用 sz-orm-limit） | Phase 3.5 ✅ |
| `trace` | Trace 中间件（W3C TraceContext 传播，复用 sz-orm-tracing） | Phase 3.6 ✅ |
| `builder` | `MiddlewareBuilder` 链构建器 | Phase 3.12 ✅ |
| `tower_compat` | `TowerCompat` 包装器（兼容 tower-http Compression/Timeout/TraceLayer） | Phase 3.13 ✅ |

### Handler=Middleware 统一设计

对齐 Salvo 的 `Handler` trait 同时可以作为 Middleware 使用：

```rust
// handler_as_middleware.rs
// 任何实现 Handler 的闭包/函数都可以作为 Middleware 使用
// 反之，任何 Middleware 也可以作为 Handler 调用
```

这避免了 PHP 端"中间件和控制器是不同类型"的割裂感。

## 后果

### 正面后果

- **生态兼容**：基于 Tower 生态，可直接使用 `tower-http` 提供的 Compression/Timeout/TraceLayer 等
- **PHP 对齐**：5 个中间件与 PHP 端一一对应，迁移路径清晰
- **顺序明确**：`DEFAULT_ORDER` 常量定义了完整顺序，`MiddlewareChain` 提供构建器
- **性能无损**：Tower 的 Layer 是零成本抽象，编译期生成中间件链
- **可扩展**：业务中间件通过 `tower_compat` 接入，无需修改框架核心

### 负面后果

- **学习曲线**：Tower 的「后注册先执行」语义需要开发者适应
- **async trait 限制**：当前不支持 `dyn Middleware`（动态分发），所有中间件必须静态组合
- **错误处理复杂**：中间件链中的错误需要通过 `Response` 传递，而非 `Result`（Tower 限制）

## 注意事项

- **Trace 必须最先执行**：包裹所有后续中间件，确保 `request_id` 在所有日志/追踪中可用
- **Cors 在 Auth 之前**：OPTIONS 预检请求直接返回，不进入鉴权逻辑
- **Log 在 RateLimit 之前**：记录所有请求（包括被限流拒绝的），用于审计
- **RateLimit 在 Auth 之前**：避免无效请求消耗鉴权开销
- **`tower_compat` 包装器**：用于兼容 `tower-http` 的 `CompressionLayer`/`TimeoutLayer`/`TraceLayer`，这些 Layer 的类型约束较复杂，需要包装器简化使用

## Bug 定位提示

如果生产 Bug 表现为"中间件执行顺序错误"或"中间件未执行"：

1. **L1 决策层**：查阅本 ADR，确认中间件是否按 `DEFAULT_ORDER` 顺序注册
2. **L2 运行时层**：检查 tracing span `middleware.enter` / `middleware.exit` 的顺序
3. **L3 指标层**：检查 `middleware.duration` 指标按 `kind` 标签的分布
4. **L4 代码层**：
   - 顺序 Bug → 检查 `packages/sz-rust-core/src/middleware/order.rs` 的 `DEFAULT_ORDER`
   - 反转 Bug → 检查 `MiddlewareChain::service_builder_order()` 的反转逻辑
   - 缺失 Bug → 检查 `MiddlewareBuilder` 的 5 个 `Option<Config>` 是否为 `None`
   - 兼容 Bug → 检查 `tower_compat::TowerCompat` 的类型约束是否满足

# ADR-006：认证授权机制（JWT + Middleware + Guard 三层分离）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-002（中间件模型）、ADR-008（错误处理）
> **相关代码**：`packages/sz-rust-core/src/middleware/auth.rs`、`packages/sz-rust-core/src/guard.rs`

## 背景

PHP 端 9 个应用各自独立鉴权：

| 应用 | 鉴权方式 |
|------|---------|
| szoa / szoapc / szweb | JWT + RBAC |
| szadmin | Basic Auth |
| 其他应用 | Cache token / Session |

PHP szoa RBAC 模型：
- `is_super=1` 绕过所有 RBAC 检查
- 错误码：`-1` not_login / `0` 无权限 / `-3` 用户已禁用
- 权限格式：`controller/action`（如 `user/list`、`user/save`）
- 用户表关系链：`users → user_role → role → role_access → access`

sz-rust 需要决定如何统一鉴权机制，同时保持与 PHP 端的兼容性。

## 决策

采用 **JWT + Middleware + Guard 三层分离** 模型：

### 1. Auth Middleware（认证层）

```rust
// middleware/auth.rs
// 负责 JWT 校验，注入 AuthenticatedUser 到 request extensions
// 不做鉴权决策（allow/deny），只做身份验证（who are you?）
```

- 复用 `sz-orm-auth` 的 JWT 校验能力
- 校验失败返回 `BaseException`（错误码 `-1` not_login）
- 校验成功注入 `AuthenticatedUser` 到 `request.extensions_mut()`

### 2. Guard（鉴权层）

```rust
// guard.rs
// 借鉴 NestJS Guard + Spring Security
// 基于 AuthenticatedUser 和 UserContext 进行鉴权决策（are you allowed?）
// 返回 Result<(), GuardError>（二元决策）
```

#### Guard vs Middleware

| 维度 | Middleware | Guard |
|------|-----------|-------|
| 关注点 | 横切关注点（日志/CORS/追踪/限流） | 鉴权决策（allow/deny） |
| 返回值 | Response（可修改请求/响应） | `Result<(), GuardError>`（二元决策） |
| 执行时机 | 请求全生命周期 | Auth 中间件之后、handler 之前 |
| 组合语义 | 链式（顺序执行） | AND 语义（全部通过） |

### 3. 内置 Guard

#### AuthGuard

校验用户已登录（`AuthenticatedUser` 存在），未登录返回 `NotLogin(-1)`。

#### PermissionGuard

校验用户是否拥有指定权限：

```rust
let guard = PermissionGuard::new("user/list");
// 校验 UserContext 的 permissions 是否包含 "user/list"
// is_super=1 绕过检查（对齐 PHP 行为）
```

#### GuardChain（AND 语义）

```rust
let chain = GuardChain::new()
    .with_guard(Arc::new(AuthGuard))
    .with_guard(Arc::new(PermissionGuard::new("user/list")));
// 所有 Guard 都通过才放行，任一失败即拒绝
```

### 4. 执行顺序

```
Trace → Cors → Log → RateLimit → Auth → [Guard] → Handler
```

Auth 中间件负责 JWT 校验并注入 `AuthenticatedUser`，Guard 基于 `AuthenticatedUser` 进行鉴权决策。

### 5. 错误码对齐

| 错误码 | 含义 | PHP 使用场景 |
|--------|------|-------------|
| `-1` | 未登录 | `not_login` |
| `0` | 无权限 | `renderError` 默认 |
| `-3` | 用户已禁用 | `员工信息待审核` / `您已离职` |

## 后果

### 正面后果

- **关注点分离**：认证（Auth Middleware）与鉴权（Guard）分离，符合单一职责原则
- **PHP 对齐**：错误码、RBAC 模型、`is_super` 绕过逻辑完全对齐 PHP
- **可组合**：GuardChain 支持 AND 语义，可灵活组合多个 Guard
- **可扩展**：业务可实现 `Guard` trait 自定义鉴权逻辑
- **测试友好**：Guard 返回 `Result<(), GuardError>`，便于单元测试

### 负面后果

- **三层分离增加复杂度**：相比 PHP 端"Controller 基类 checkLogin"的单点鉴权，三层分离需要开发者理解 Auth Middleware → Guard → Handler 的流程
- **Guard 不支持 OR 语义**：当前 GuardChain 只支持 AND 语义（全部通过），如果需要 OR 语义（任一通过），需要业务自行实现
- **`dyn Guard` 的 trait object**：Guard 使用 `Arc<dyn Guard>` 存储在 GuardChain 中，需要 Guard trait 是 object-safe 的

## 注意事项

- **`is_super=1` 绕过**：`PermissionGuard` 检查 `UserContext.is_super`，如果为 `true` 则跳过权限检查。这与 PHP 行为一致，但可能成为安全风险（超级管理员账号泄露）
- **权限格式**：权限字符串格式为 `controller/action`（如 `user/list`），与 PHP 一致
- **`AuthenticatedUser` 注入**：Auth Middleware 校验成功后，必须将 `AuthenticatedUser` 注入 `request.extensions_mut()`，否则 Guard 无法获取用户信息
- **Basic Auth 未内置**：szadmin 使用的 Basic Auth 需要业务自行实现 `Guard`，框架未内置 BasicAuthGuard

## Bug 定位提示

如果生产 Bug 表现为"未登录用户可访问受保护资源"或"权限校验失效"：

1. **L1 决策层**：查阅本 ADR，确认是否同时配置了 Auth Middleware 和 Guard（两者缺一不可）
2. **L2 运行时层**：检查 tracing span `guard.check` 中的 `guard` 和 `result` 字段
3. **L3 指标层**：检查 `guard.deny.count` 指标按 `guard` 标签的分布
4. **L4 代码层**：
   - 未登录可访问 Bug → 检查路由是否添加了 Auth Middleware
   - 权限校验失效 Bug → 检查 `PermissionGuard` 的权限字符串是否匹配，`is_super` 是否被错误设置为 `true`
   - Guard 未执行 Bug → 检查 `guard_middleware` 是否通过 `from_fn_with_state` 正确注册
   - 用户信息丢失 Bug → 检查 Auth Middleware 是否正确注入 `AuthenticatedUser` 到 `request.extensions_mut()`

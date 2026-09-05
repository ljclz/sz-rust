# ADR-005：事务管理策略（委托 sz-orm-core + 显式 begin/commit/rollback）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-004（Model 钩子）、ADR-008（错误处理）
> **相关代码**：`packages/sz-rust-core/src/`（通过 sz-orm-core 间接提供）

## 背景

PHP ThinkPHP 提供两种事务管理方式：

1. **手动事务**：`Db::startTrans()` / `Db::commit()` / `Db::rollback()`
2. **闭包事务**：`Db::transaction(function() { ... })`，自动 commit/rollback

sz-orm-core 已实现事务管理，包括：
- 嵌套事务（SAVEPOINT + 深度限制，参见 sz-orm ADR-0003）
- 连接池 acquire 持锁不 await close（参见 sz-orm ADR-0008）

sz-rust 需要决定如何接入事务管理，是否需要自研 `#[transactional]` 宏。

## 决策

**事务管理完全委托 sz-orm-core**，sz-rust 不自研事务机制：

### 1. Re-export sz-orm-core 事务 API

```rust
// sz-rust 通过 sz-orm-sqlx 或 sz-orm-core 获取连接
let mut conn = pool.acquire().await?;
conn.begin().await?;
// ... 执行 SQL
conn.commit().await?;
// 或 conn.rollback().await?
```

### 2. 闭包事务（对齐 PHP `Db::transaction`）

```rust
// 通过 sz-orm-core 提供的闭包事务
let result = conn.transaction(|txn| {
    Box::pin(async move {
        // ... 在事务中执行多个操作
        txn.execute("INSERT INTO users ...").await?;
        txn.execute("INSERT INTO user_role ...").await?;
        Ok(())
    })
}).await;
// 自动 commit（Ok）或 rollback（Err）
```

### 3. 不自研 `#[transactional]` 宏

考虑过自研 `#[transactional]` 属性宏，但决定**不自研**，原因：
- **PHP 端无对应物**：PHP 没有注解式事务，所有事务都是显式 `startTrans/commit/rollback`
- **Rust async 限制**：`#[transactional]` 宏需要注入 `async fn` 的 body，但 Rust 的 async trait 语义让宏实现复杂
- **sz-orm-core 已提供闭包事务**：闭包事务已经足够简洁，且类型安全

### 4. 嵌套事务（SAVEPOINT）

sz-orm-core 已实现嵌套事务（SAVEPOINT + 深度限制），sz-rust 直接使用，无需额外处理。

## 决策替代方案

### 方案 A：自研 `#[transactional]` 属性宏（拒绝）

```rust
// 设想的注解式事务
#[transactional]
async fn create_user(&self, data: CreateUserDTO) -> Result<User> {
    // 方法内所有 DB 操作自动在事务中执行
    // 返回 Err 时自动 rollback
}
```

**拒绝原因**（已在决策中详述）：
- PHP 端无对应物：PHP 没有注解式事务，所有事务都是显式 `startTrans/commit/rollback`
- Rust async 限制：`#[transactional]` 宏需要注入 `async fn` 的 body，但 Rust 的 async trait 语义让宏实现复杂
- sz-orm-core 已提供闭包事务：闭包事务已经足够简洁，且类型安全

### 方案 B：sz-rust 自研事务管理器（拒绝）

在 sz-rust-core 中实现独立的事务管理器，管理连接池和事务生命周期。

**拒绝原因**：
- sz-orm-core 已经实现了完整的事务管理（包括 SAVEPOINT 嵌套事务）
- 自研事务管理器需要重新实现连接池 acquire 持锁不 await close 等复杂机制
- 两套事务机制会导致业务代码困惑

### 方案 C：手动事务（仅 begin/commit/rollback，无闭包）（拒绝）

只提供 `conn.begin()` / `conn.commit()` / `conn.rollback()`，不提供闭包事务。

**拒绝原因**：
- PHP 端 `Db::transaction(function() { ... })` 的闭包事务是常用模式，缺少它会导致迁移困难
- 手动事务容易忘记 `rollback`（异常路径），闭包事务通过 `Result` 自动处理
- 闭包事务的代码更简洁，`Ok` 自动 commit，`Err` 自动 rollback

**最终选择**：完全委托 sz-orm-core + 闭包事务。不自研任何事务机制，通过 re-export 提供 sz-orm-core 的事务 API。

## 后果

### 正面后果

- **零重复实现**：sz-rust 不自研事务机制，避免代码冗余
- **PHP 对齐**：闭包事务对齐 PHP `Db::transaction(function() { ... })`
- **嵌套事务支持**：sz-orm-core 的 SAVEPOINT 机制自动可用
- **类型安全**：闭包事务的 `Result` 返回值确保 commit/rollback 正确触发
- **连接池安全**：sz-orm-core 的 acquire 持锁不 await close 机制避免死锁

### 负面后果

- **强依赖 sz-orm-core**：事务管理完全依赖 sz-orm-core，无法独立使用
- **无 `#[transactional]` 宏**：业务代码需要显式调用闭包事务，比注解式繁琐
- **异步闭包复杂**：Rust 的 async closure 语法（`Box::pin(async move { ... })`）比 PHP 的 `function() { ... }` 复杂

## 注意事项

- **SAVEPOINT 深度限制**：sz-orm-core 对嵌套事务有深度限制（默认 8 层），超过会返回错误
- **事务中禁止 acquire 新连接**：事务期间必须在同一连接上执行所有操作，禁止从连接池 acquire 新连接
- **钩子与事务的交互**：`before_insert` 等钩子在事务内部触发，钩子返回 `Err` 会触发 rollback
- **`DROP` 语句的隐式提交**：MySQL 的 `DROP TABLE` 等 DDL 语句会隐式提交事务，sz-orm-core 不会拦截这种行为

## Bug 定位提示

如果生产 Bug 表现为"事务未回滚"或"嵌套事务死锁"：

1. **L1 决策层**：查阅本 ADR，确认事务是否通过闭包事务或显式 `begin/commit/rollback` 管理
2. **L2 运行时层**：检查 tracing span `transaction.begin` / `transaction.commit` / `transaction.rollback` 的嵌套层级
3. **L3 指标层**：检查 `transaction.duration` 和 `transaction.rollback.count` 指标
4. **L4 代码层**：
   - 未回滚 Bug → 检查闭包事务是否返回 `Err`，`Err` 是否被 `?` 吞掉
   - 嵌套死锁 Bug → 检查 sz-orm-core 的 SAVEPOINT 深度限制
   - 连接泄漏 Bug → 检查事务期间是否 acquire 了新连接
   - 钩子中止 Bug → 检查 `before_insert` 等钩子是否返回 `Err`（会触发 rollback）
   - **Err 被 `?` 吞掉** → 闭包事务内 `some_op()?` 返回 Err 会触发 rollback，但若外层用 `.unwrap_or(default)` 吞掉 Err，调用方误以为成功
   - **嵌套事务超深** → SAVEPOINT 深度超过 sz-orm-core 限制（默认 8 层），返回错误而非创建 SAVEPOINT，导致内层操作在非事务上下文中执行

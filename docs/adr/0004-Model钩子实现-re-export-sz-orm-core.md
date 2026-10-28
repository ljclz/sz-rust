# ADR-004：Model 钩子实现（re-export sz-orm-core + 16 事件）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：无
> **相关代码**：`packages/sz-rust-core/src/hooks.rs`

## 背景

PHP `topthink/think-orm: ^2.0.52` 通过 `trigger()` 方法在 `insert/update/delete/restore/find` 内部自动触发钩子回调。PHP 原生提供 12 个事件：

| 操作 | 触发顺序 |
|------|---------|
| INSERT | `before_write` → `before_insert` → (INSERT) → `after_insert` → `after_write` |
| UPDATE | `before_write` → `before_update` → (UPDATE) → `after_update` → `after_write` |
| DELETE | `before_delete` → (DELETE) → `after_delete` |
| RESTORE | `before_restore` → (UPDATE deleted_at=NULL) → `after_restore` |
| FIND | `before_find` → (SELECT) → `after_find` |

PHP 项目实际使用情况：
- 仅实现 `onBeforeInsert` / `onBeforeUpdate` 两个回调
- 用于自动填充 `create_time` / `update_time` 时间戳
- `BaseModel::onBeforeInsert` 还会通过 `method_exists($model, "before_insert")` 反向调用业务级 `before_insert()` 方法

sz-orm-core 已在 PHP 原生 12 事件基础上扩展 4 个事件，共 16 事件。sz-rust 需要决定如何接入这套钩子机制。

## 决策

**sz-rust 端不重复实现 HookDispatcher，而是 re-export sz-orm-core::hooks 的所有公开类型**，并提供以下增强：

### 1. Re-export sz-orm-core::hooks

```rust
pub use sz_orm_core::hooks::{
    GlobalScope, HookContext, HookDispatcher, HookEvent, HookFn, HookRegistry, HookResult,
    Hookable, ScopeRegistry, SoftDelete, SoftDeleteScope, TenantModel, TenantScope,
};
```

### 2. `ALL_EVENTS` 常量

```rust
pub const ALL_EVENTS: [HookEvent; 16] = [
    // PHP think-orm 2.0.x 原生 12 事件
    HookEvent::BeforeInsert, HookEvent::AfterInsert,
    HookEvent::BeforeUpdate, HookEvent::AfterUpdate,
    HookEvent::BeforeDelete, HookEvent::AfterDelete,
    HookEvent::BeforeFind, HookEvent::AfterFind,
    HookEvent::BeforeRestore, HookEvent::AfterRestore,
    HookEvent::BeforeWrite, HookEvent::AfterWrite,
    // sz-orm-core 扩展 4 事件
    HookEvent::BeforeSave, HookEvent::AfterSave,
    HookEvent::BeforeValidate, HookEvent::AfterValidate,
];
```

### 3. PHP 风格字符串 ↔ HookEvent 双向映射

- `event_name(HookEvent) -> &'static str`：枚举转 PHP 风格字符串（如 `BeforeInsert → "before_insert"`）
- `event_from_name(&str) -> Option<HookEvent>`：PHP 风格字符串转枚举

### 4. HookExecutionRecorder（测试辅助工具）

记录钩子执行顺序用于断言，验证 INSERT/UPDATE 等操作的事件序列是否符合 PHP 行为。

### 5. PHP 行为对齐验证函数

- `validate_insert_order()`：验证 INSERT 操作的钩子顺序
- `validate_update_order()`：验证 UPDATE 操作的钩子顺序
- 等等

### 扩展后的 INSERT/UPDATE 顺序

```text
before_write → before_save → before_validate → validate → after_validate
→ before_insert → (INSERT) → after_insert → after_save → after_write
```

保持 PHP 原生顺序兼容，新增的 4 个事件插入到合理位置。

## 决策替代方案

### 方案 A：sz-rust 自研 HookDispatcher（拒绝）

在 sz-rust-core 中重新实现一套 `HookDispatcher`、`HookEvent`、`HookRegistry`。

**拒绝原因**：
- sz-orm-core 已经实现了完整的 16 事件钩子机制，重复实现是浪费
- 两套钩子机制会导致业务代码困惑（用哪一套？）
- sz-orm-core 的钩子严格对齐 PHP 行为（包括 bug），自研难以保证一致性

### 方案 B：仅 re-export，不提供增强工具（拒绝）

只 re-export sz-orm-core 的类型，不提供 `ALL_EVENTS` 常量、字符串映射、`HookExecutionRecorder` 等增强工具。

**拒绝原因**：
- 业务代码需要手动维护事件列表，容易遗漏新增事件
- PHP 迁移时需要字符串 ↔ 枚举的双向映射，缺少工具会增加迁移成本
- `HookExecutionRecorder` 是测试钩子顺序的关键工具，缺少它难以编写 PHP 行为对比测试

### 方案 C：使用第三方事件库（如 tokio::sync::broadcast）（拒绝）

用 tokio 的广播通道实现钩子回调。

**拒绝原因**：
- 广播通道是异步消息传递，与 sz-orm-core 的同步钩子语义不兼容
- 无法保证钩子执行顺序（PHP 要求 `before_write` 在 `before_insert` 之前）
- 钩子需要能中止操作（返回 `Err`），广播通道不支持这种语义

**最终选择**：re-export sz-orm-core + 增强工具。既不重复实现，又提供 PHP 迁移所需的便利工具。

## 后果

### 正面后果

- **零重复实现**：sz-rust 不重复实现 HookDispatcher，避免代码冗余
- **PHP 行为对齐**：16 事件完整覆盖 PHP 原生 12 事件 + 扩展 4 事件
- **测试友好**：`HookExecutionRecorder` + `validate_insert_order()` 等工具便于编写 PHP 行为对比测试
- **字符串映射**：PHP 风格字符串 ↔ 枚举双向映射，便于从 PHP 配置迁移
- **扩展性**：sz-orm-core 未来新增事件，sz-rust 自动获得（re-export）

### 负面后果

- **强依赖 sz-orm-core**：sz-rust-core 的 hooks 模块完全依赖 sz-orm-core，无法独立使用
- **`ALL_EVENTS` 需手动维护**：sz-orm-core 新增事件时，sz-rust 的 `ALL_EVENTS` 常量需要同步更新
- **PHP 反向调用未实现**：PHP 的 `method_exists($model, "before_insert")` 反向调用业务方法在 Rust 端无直接等价物（Rust 没有反射），需要业务控制器显式注册钩子

## 注意事项

- **PHP 源码 bug 复刻**：sz-orm-core 的 hooks 实现严格对齐 PHP 行为，包括 PHP 源码中的 quirk（如 `before_write` 在 `before_insert`/`before_update` 之前触发）
- **`Hookable` trait**：业务模型需要 `impl Hookable for User {}` 才能使用 `HookDispatcher::insert::<User, _>()`
- **`HookContext`**：钩子回调通过 `HookContext` 传递上下文，包含模型数据、操作类型等
- **`HookResult`**：钩子回调返回 `HookResult`，`Err` 表示中止后续操作
- **线程安全**：`HookRegistry` 使用 `Arc<dyn HookFn>` 存储回调，线程安全

## Bug 定位提示

如果生产 Bug 表现为"钩子未触发"或"钩子顺序错误"：

1. **L1 决策层**：查阅本 ADR，确认钩子是否通过 `HookRegistry::register()` 注册，`HookDispatcher::insert/update/delete` 是否被调用
2. **L2 运行时层**：检查 tracing span `hook.dispatch` 中的 `event` 和 `result` 字段
3. **L3 指标层**：检查 `hook.dispatch.count` 指标按 `event` 标签的分布
4. **L4 代码层**：
   - 钩子未触发 Bug → 检查业务模型是否 `impl Hookable`，`HookDispatcher::insert::<User, _>()` 是否被调用
   - 顺序错误 Bug → 检查 `sz-orm-core/src/hooks/dispatcher.rs` 的事件触发顺序
   - 钩子中止 Bug → 检查 `HookFn` 返回的 `HookResult` 是否为 `Err`
   - 上下文丢失 Bug → 检查 `HookContext` 是否正确传入 `HookDispatcher::insert(ctx, callback)`
   - **模型未注册 Hookable** → 业务模型忘记 `impl Hookable for User {}`，`HookDispatcher::insert::<User, _>()` 编译通过但钩子不触发
   - **ALL_EVENTS 与 sz-orm-core 不同步** → sz-orm-core 新增事件后，`ALL_EVENTS` 常量未更新，新事件无法通过字符串名称注册

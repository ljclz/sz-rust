# ADR-040: TenantScopeExt 以 DataScopeExt 为 supertrait

**状态**：Accepted
**日期**：2026-09-10
**决策者**：SZ-Rust Team

## 背景

多租户数据隔离需要在查询构建器上追加 `WHERE tenant_id = ?` 条件。现有 `DataScopeExt` trait 已提供 `with_data_scope_conditions` 机制用于追加数据范围条件。候选方案：

1. **supertrait 复用**：定义 `TenantScopeExt: DataScopeExt`，在 `tenant_scope_async` 中构造 `WHERE tenant_id = ?` 条件并调用 `self.with_data_scope_conditions(&[condition])`。
2. **独立 trait**：定义独立的 `TenantScopeExt` trait，自带条件注入方法，不依赖 `DataScopeExt`。
3. **合并 trait**：将租户隔离逻辑直接合并到 `DataScopeExt` 中。

## 决策

选择 **supertrait 复用**（方案 1）。

## 理由

1. **条件取交集**：spec 5.3.1.10 要求租户隔离与部门权限取交集（`WHERE tenant_id = ? AND dept_id = ?`）。通过 supertrait 复用 `with_data_scope_conditions`，两种条件追加到同一个 `Vec<WhereCondition>`，自然形成 AND 交集。独立 trait 需要额外的合并逻辑。

2. **零重复代码**：`with_data_scope_conditions` 的实现（追加到查询构建器的条件列表）只需一份。独立 trait 需要复制此逻辑，违反 DRY。

3. **链式调用**：业务代码可链式调用 `.data_scope_async(ctx, rule, evaluator).tenant_scope_async(ctx, registry, table, metrics)`，两种条件按顺序追加，语义清晰。

4. **不破坏 DataScopeExt**：合并 trait（方案 3）会使 `DataScopeExt` 承担租户隔离职责，违反单一职责原则，且使非多租户场景被迫依赖租户模块。supertrait 方式保持 `DataScopeExt` 纯粹，`TenantScopeExt` 是可选扩展。

5. **自动实现**：`impl<T: DataScopeExt> TenantScopeExt for T {}` 为所有实现 `DataScopeExt` 的类型自动实现 `TenantScopeExt`，业务层无需手动实现。

## 后果

- `TenantScopeExt` 的 `tenant_scope_async` 默认实现不可覆盖（trait 提供 default method）。
- 平台管理员 bypass、tenant_id ≤ 0 拒绝、全局表不附加条件等逻辑在 `tenant_scope_async` 默认实现中处理。
- 业务查询构建器只需实现 `DataScopeExt`，自动获得 `TenantScopeExt` 能力。

## 参考

- [ADR-039: 租户上下文注入方式 — request extensions](ADR-039-tenant-context-request-extensions.md)
- spec 5.3.1.10（租户隔离 + 部门权限取交集）
- design 1.2.1（TenantScopeExt supertrait 决策）
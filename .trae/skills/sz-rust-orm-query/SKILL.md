---
name: sz-rust-orm-query
description: ORM 查询优化检查 — 检测 N+1、缺失索引、低效查询模式。修改 repository/model 时触发。
tools: [sqlx, cargo-clippy]
agentMode: auto
---

# ORM 查询优化检查（sz-rust）

## 触发条件

- 新增或修改 `Repository` 实现
- 修改 `EntityAttributes` 或关联关系
- 新增 service 层数据访问

## 检查步骤

1. 扫描循环体内的 `fetch_related` / `find_by_id` 调用
2. 检查 WHERE 条件字段是否有索引
3. 确认无 `SELECT *`（使用显式字段投影）

## 通过标准

- 无 N+1 查询（循环内单次 DB 调用）
- WHERE/ORDER BY 字段有索引覆盖
- 大表查询使用分页（`paginate_by`）
- 关联查询使用 `JOIN` 或批量预加载

## 常见反模式

```rust
// ❌ N+1
for item in items {
    let user = repo.find_by_id(&item.user_id).await?;
}

// ✅ 批量预加载
let user_ids: Vec<_> = items.iter().map(|i| i.user_id).collect();
let users = user_repo.find_by_ids(&user_ids).await?;
```

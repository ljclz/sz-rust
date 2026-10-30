---
name: sz-rust-n-plus-one
description: N+1 查询专项检测 — 深度扫描循环内数据库操作并给出修复方案。
tools: [cargo-clippy, sqlx]
agentMode: auto
---

# N+1 查询专项检测（sz-rust）

## 触发条件

- 任何包含循环 + repository 调用的代码变更

## 检测规则

### 规则 1：循环内 find_by_id
```rust
// ❌ 违规
for id in ids {
    let entity = repo.find_by_id(&id).await?;
}
```

### 规则 2：循环内 fetch_related
```rust
// ❌ 违规
for order in orders {
    let items = order_item_repo.find_by_order(order.id).await?;
}
```

### 规则 3：嵌套循环查询
```rust
// ❌ 违规
for customer in customers {
    for order in orders {
        if order.customer_id == customer.id { ... }
    }
}
```

## 修复方案

1. 使用 `find_by_ids(&[id])` 批量查询
2. 使用 `JOIN` 一次性获取关联数据
3. 在应用层用 HashMap 做内存关联

## 通过标准

零 N+1 违规。

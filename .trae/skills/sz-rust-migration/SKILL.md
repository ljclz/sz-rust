---
name: sz-rust-migration
description: 数据库迁移检查 — 确保 schema 变更可回滚、幂等。修改 SQL/ORM 模型时触发。
tools: [sqlx, sea-orm-cli]
agentMode: auto
---

# 数据库迁移检查（sz-rust）

## 触发条件

- 新增或修改 `migrations/` 中的 SQL 文件
- 修改 model 字段（影响 schema）

## 检查步骤

1. 检查迁移文件命名：`YYYYMMDDHHMMSS_description.sql`
2. 确认迁移包含 `-- rollback` 注释块
3. 验证迁移幂等性（重复执行不报错）

## 通过标准

- 每个迁移有对应的 rollback SQL
- ALTER TABLE 操作有索引重建计划
- 无 DROP COLUMN（使用软删除标记代替）
- 大数据量迁移使用分批处理

## 失败处理

补充 rollback SQL，拆分大迁移为小批次。

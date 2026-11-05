-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段2：哨兵列改 NULLABLE（category.parent_id）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B / §7.4
-- 目的：把哨兵值 0（表示「顶级类目」）改为 NULL，消除哨兵值与合法主键 0 的
-- 语义冲突，为后续 FK(parent_id) → category(cat_id) 自引用铺路。
-- 模型侧：models/category.rs:15 `pub parent_id: i64` → `Option<i64>`。
--
-- 锁风险：ALTER TABLE MODIFY COLUMN 在 MySQL 8.0 为 INSTANT/INPLACE 操作，
-- 仅元数据变更；存量 UPDATE 全表扫描，类目表行数通常很小。
--
-- 回滚（人工执行，勿自动跑）：
--   UPDATE category SET parent_id = 0 WHERE parent_id IS NULL;
--   ALTER TABLE `category` MODIFY COLUMN parent_id INT UNSIGNED NOT NULL DEFAULT 0;

-- 1) 改列为 NULLABLE
ALTER TABLE `category` MODIFY COLUMN parent_id INT UNSIGNED NULL DEFAULT 0;

-- 2) 存量哨兵 0 → NULL
UPDATE `category` SET parent_id = NULL WHERE parent_id = 0;
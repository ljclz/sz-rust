-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段2：哨兵列改 NULLABLE（ai_category.cat_id）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B / §7.4
-- 目的：把哨兵值 0（表示「未关联商品类目」）改为 NULL，消除哨兵值与合法主键 0
-- 的语义冲突，为后续 FK(cat_id) → category(cat_id) 铺路。
-- 模型侧：models/ai_category.rs:15 `pub cat_id: i64` → `Option<i64>`。
--
-- 锁风险：ALTER TABLE MODIFY COLUMN 在 MySQL 8.0 为 INSTANT/INPLACE 操作，
-- 仅元数据变更；存量 UPDATE 全表扫描，AI 分类表行数通常很小。
--
-- 回滚（人工执行，勿自动跑）：
--   UPDATE ai_category SET cat_id = 0 WHERE cat_id IS NULL;
--   ALTER TABLE `ai_category` MODIFY COLUMN cat_id INT UNSIGNED NOT NULL DEFAULT 0;

-- 1) 改列为 NULLABLE
ALTER TABLE `ai_category` MODIFY COLUMN cat_id INT UNSIGNED NULL DEFAULT 0;

-- 2) 存量哨兵 0 → NULL
UPDATE `ai_category` SET cat_id = NULL WHERE cat_id = 0;
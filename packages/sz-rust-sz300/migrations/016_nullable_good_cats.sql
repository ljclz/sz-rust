-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段2：哨兵列改 NULLABLE（good.cat_id + good.ai_class_id）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B / §7.4
-- 目的：把两个哨兵值 0（cat_id=0 表示「未分类」，ai_class_id=0 表示「未 AI 分类」）
-- 改为 NULL，消除哨兵值与合法主键 0 的语义冲突，为后续
-- FK(cat_id) → category(cat_id) 与 FK(ai_class_id) → ai_category(ai_class_id) 铺路。
-- 模型侧：models/product.rs:15 `pub cat_id: i64` → `Option<i64>`；
--         models/product.rs:25 `pub ai_class_id: i64` → `Option<i64>`。
--
-- 同一表两列合并到一个 migration，避免对 good 表做两次 ALTER。
--
-- 锁风险：ALTER TABLE MODIFY COLUMN 在 MySQL 8.0 为 INSTANT/INPLACE 操作，
-- 仅元数据变更；两条 UPDATE 各全表扫描一次，行数小时级。
--
-- 回滚（人工执行，勿自动跑）：
--   UPDATE good SET cat_id = 0 WHERE cat_id IS NULL;
--   UPDATE good SET ai_class_id = 0 WHERE ai_class_id IS NULL;
--   ALTER TABLE `good` MODIFY COLUMN cat_id INT UNSIGNED NOT NULL DEFAULT 0;
--   ALTER TABLE `good` MODIFY COLUMN ai_class_id INT UNSIGNED NOT NULL DEFAULT 0;

-- 1) 改两列为 NULLABLE
ALTER TABLE `good` MODIFY COLUMN cat_id INT UNSIGNED NULL DEFAULT 0;
ALTER TABLE `good` MODIFY COLUMN ai_class_id INT UNSIGNED NULL DEFAULT 0;

-- 2) 存量哨兵 0 → NULL
UPDATE `good` SET cat_id = NULL WHERE cat_id = 0;
UPDATE `good` SET ai_class_id = NULL WHERE ai_class_id = 0;
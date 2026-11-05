-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段2：哨兵列改 NULLABLE（operate_log.merchant_id）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B / §7.4
-- 目的：把哨兵值 0（表示「系统级操作/无商户归属」）改为 NULL，消除哨兵值与
-- 合法主键 0 的语义冲突，为后续 FK(merchant_id) → merchant(merchant_id) 铺路。
-- 模型侧：models/operate_log.rs:13 `pub merchant_id: i64` → `Option<i64>`。
--
-- 锁风险：ALTER TABLE MODIFY COLUMN 在 MySQL 8.0 为 INSTANT/INPLACE 操作，
-- 仅元数据变更；存量 UPDATE 全表扫描，操作日志表行数可能较大，建议低峰执行。
--
-- 回滚（人工执行，勿自动跑）：
--   UPDATE operate_log SET merchant_id = 0 WHERE merchant_id IS NULL;
--   ALTER TABLE `operate_log` MODIFY COLUMN merchant_id INT UNSIGNED NOT NULL DEFAULT 0;

-- 1) 改列为 NULLABLE
ALTER TABLE `operate_log` MODIFY COLUMN merchant_id INT UNSIGNED NULL DEFAULT 0;

-- 2) 存量哨兵 0 → NULL
UPDATE `operate_log` SET merchant_id = NULL WHERE merchant_id = 0;
-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段2：哨兵列改 NULLABLE（`order`.device_id）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B / §7.4
-- 目的：把哨兵值 0（表示「未关联设备」，如后台手工单）改为 NULL，消除哨兵值
-- 与合法主键 0 的语义冲突，为后续 FK(device_id) → device(device_id) 铺路。
-- 模型侧：models/order.rs:17 `pub device_id: i64` → `Option<i64>`。
--
-- 注意：`order` 是 MySQL 保留字，必须反引号 `order`。
--
-- 锁风险：ALTER TABLE MODIFY COLUMN 在 MySQL 8.0 为 INSTANT/INPLACE 操作，
-- 仅元数据变更；存量 UPDATE 全表扫描，行数小时级。
--
-- 回滚（人工执行，勿自动跑）：
--   UPDATE `order` SET device_id = 0 WHERE device_id IS NULL;
--   ALTER TABLE `order` MODIFY COLUMN device_id INT UNSIGNED NOT NULL DEFAULT 0;

-- 1) 改列为 NULLABLE
ALTER TABLE `order` MODIFY COLUMN device_id INT UNSIGNED NULL DEFAULT 0;

-- 2) 存量哨兵 0 → NULL
UPDATE `order` SET device_id = NULL WHERE device_id = 0;
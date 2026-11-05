-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段2：哨兵列改 NULLABLE（device.merchant_id）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B / §7.4
-- 目的：把哨兵值 0（表示「未绑定商户」）改为 NULL，消除哨兵值与合法主键 0 的
-- 语义冲突，为后续 FK(merchant_id) → merchant(merchant_id) 铺路。
-- 模型侧：models/device.rs:13 `pub merchant_id: i64` → `Option<i64>`。
--
-- 锁风险：ALTER TABLE MODIFY COLUMN 在 MySQL 8.0 为 INSTANT/INPLACE 操作，
-- 仅元数据变更（不重建表、不阻塞读写）；存量 UPDATE 全表扫描，行数小时级。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制，
--       以下语句以注释形式提供供回滚参考）：
--   UPDATE device SET merchant_id = 0 WHERE merchant_id IS NULL;
--   ALTER TABLE `device` MODIFY COLUMN merchant_id INT UNSIGNED NOT NULL DEFAULT 0;

-- 1) 改列为 NULLABLE（保留默认 0 兼容旧写入路径，存量 0 由下一步 UPDATE 清为 NULL）
ALTER TABLE `device` MODIFY COLUMN merchant_id INT UNSIGNED NULL DEFAULT 0;

-- 2) 存量哨兵 0 → NULL
UPDATE `device` SET merchant_id = NULL WHERE merchant_id = 0;
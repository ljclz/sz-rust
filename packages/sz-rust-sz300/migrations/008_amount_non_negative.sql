-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段1：金额/计数列非负 CHECK 约束
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §5.2-B（11 个金额/计数列）
-- 目的：为金额与计数列加 CHECK (col >= 0) 双保险。schema 多为 INT UNSIGNED
-- （DB 层已防负），CHECK 提供明确约束名 + 文档化语义，统一错误来源（MySQL
-- error code 3819）。
--
-- 列名核对（packages/sz-rust-sz300/migrations/001_init.sql + 004_good_stock.sql）：
--   good.price            INT UNSIGNED NOT NULL  (001_init.sql:50)
--   good.stock            INT NOT NULL DEFAULT 0 (004_good_stock.sql:15)
--   order.total_fen       INT UNSIGNED NOT NULL  (001_init.sql:93)
--   order.total_weight_g  INT UNSIGNED DEFAULT 0 (001_init.sql:94)
--   order.item_count      SMALLINT UNSIGNED DEFAULT 0 (001_init.sql:95)
--   order_item.price_fen  INT UNSIGNED NOT NULL  (001_init.sql:114)
--   order_item.weight_g   INT UNSIGNED DEFAULT 0 (001_init.sql:115)
--   order_item.total_fen  INT UNSIGNED NOT NULL  (001_init.sql:116)
--   settlement.total_fen  INT UNSIGNED NOT NULL  (001_init.sql:126)
--   settlement.fee_fen    INT UNSIGNED DEFAULT 0 (001_init.sql:128)
--   settlement.order_count INT UNSIGNED DEFAULT 0 (001_init.sql:127)
--
-- 锁风险：ADD CHECK 在 MySQL 8.0.16+ 为 INSTANT 操作（仅元数据），不阻塞读写。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制，
--       以下 DROP 语句以注释形式提供供回滚参考）：
--   ALTER TABLE `good`        DROP CONSTRAINT chk_good_price_nonneg;
--   ALTER TABLE `good`        DROP CONSTRAINT chk_good_stock_nonneg;
--   ALTER TABLE `order`       DROP CONSTRAINT chk_order_total_fen_nonneg;
--   ALTER TABLE `order`       DROP CONSTRAINT chk_order_total_weight_g_nonneg;
--   ALTER TABLE `order`       DROP CONSTRAINT chk_order_item_count_nonneg;
--   ALTER TABLE `order_item`  DROP CONSTRAINT chk_order_item_price_fen_nonneg;
--   ALTER TABLE `order_item`  DROP CONSTRAINT chk_order_item_weight_g_nonneg;
--   ALTER TABLE `order_item`  DROP CONSTRAINT chk_order_item_total_fen_nonneg;
--   ALTER TABLE `settlement`  DROP CONSTRAINT chk_settlement_total_fen_nonneg;
--   ALTER TABLE `settlement`  DROP CONSTRAINT chk_settlement_fee_fen_nonneg;
--   ALTER TABLE `settlement`  DROP CONSTRAINT chk_settlement_order_count_nonneg;

-- 1) good（单价、库存）
ALTER TABLE `good` ADD CONSTRAINT chk_good_price_nonneg CHECK (price >= 0);
ALTER TABLE `good` ADD CONSTRAINT chk_good_stock_nonneg CHECK (stock >= 0);

-- 2) order（总额、总重量、商品种类数）
ALTER TABLE `order` ADD CONSTRAINT chk_order_total_fen_nonneg       CHECK (total_fen >= 0);
ALTER TABLE `order` ADD CONSTRAINT chk_order_total_weight_g_nonneg  CHECK (total_weight_g >= 0);
ALTER TABLE `order` ADD CONSTRAINT chk_order_item_count_nonneg      CHECK (item_count >= 0);

-- 3) order_item（单价快照、重量、小计）
ALTER TABLE `order_item` ADD CONSTRAINT chk_order_item_price_fen_nonneg CHECK (price_fen >= 0);
ALTER TABLE `order_item` ADD CONSTRAINT chk_order_item_weight_g_nonneg  CHECK (weight_g >= 0);
ALTER TABLE `order_item` ADD CONSTRAINT chk_order_item_total_fen_nonneg CHECK (total_fen >= 0);

-- 4) settlement（结算金额、手续费、订单数）
ALTER TABLE `settlement` ADD CONSTRAINT chk_settlement_total_fen_nonneg   CHECK (total_fen >= 0);
ALTER TABLE `settlement` ADD CONSTRAINT chk_settlement_fee_fen_nonneg     CHECK (fee_fen >= 0);
ALTER TABLE `settlement` ADD CONSTRAINT chk_settlement_order_count_nonneg CHECK (order_count >= 0);
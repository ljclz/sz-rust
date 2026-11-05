-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段1：订单项数量正数 CHECK 约束
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §5.2-C（1 个数量列）
-- 目的：order_item.quantity 必须 > 0；quantity=0 的订单项无业务意义。
--
-- 列名核对：order_item.quantity SMALLINT UNSIGNED DEFAULT 1 (001_init.sql:117)
--
-- 前置存量校验（生产加 CHECK 前先跑，若 >0 需人工清理）：
--   SELECT COUNT(*) AS zero_qty FROM `order_item` WHERE quantity = 0 OR quantity IS NULL;
-- schema DEFAULT 1，正常写入不会为 0；现有测试夹具均用 quantity>=1。
--
-- 锁风险：ADD CHECK 在 MySQL 8.0.16+ 为 INSTANT 操作，不阻塞读写。
--
-- 回滚（人工执行）：ALTER TABLE `order_item` DROP CONSTRAINT chk_order_item_quantity_pos;

ALTER TABLE `order_item` ADD CONSTRAINT chk_order_item_quantity_pos CHECK (quantity > 0);
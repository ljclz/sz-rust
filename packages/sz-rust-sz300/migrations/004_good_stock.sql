-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-10-28 数据层修复：商品库存列
--
-- 背景：v19-saga 的 DbStockService 执行
--   UPDATE product SET stock = stock - ? WHERE product_id = ? AND stock >= ?
-- 但生产 Schema 既没有 product 表，good 表也没有 stock 列（该路径只在测试夹具的
-- 幻影 Schema 上通过）。本迁移为 good 增加 stock，Saga 代码同步改为 good/good_id。
--
-- 锁风险：MySQL 8.0.12+ 新增列默认走 INSTANT；带 AFTER 定位时降级为 INPLACE。
-- 这里不指定 AFTER，尽量走 INSTANT（仅元数据变更）。

ALTER TABLE `good`
    ADD COLUMN `stock` INT NOT NULL DEFAULT 0 COMMENT '库存数量（件/份）',
    ALGORITHM=INSTANT;

-- 库存扣减的守卫查询：good_id 是主键，无需额外索引。

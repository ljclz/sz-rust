-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 性能门禁数据集（scripts/perf-regression.js 配套，供 .github/workflows/perf-gate.yml 使用）
--
-- 目的：让 /api/v1/product/list 与 /api/v1/order/list 的 SQL 在真实规模上执行，
--       而不是在空表上测出"零成本查询"。行数固定、内容确定，保证基线与后续对比同构。
--
-- 前置：已按顺序应用 migrations/001..007（本文件依赖 001 的列 + 002 的 status CHECK）。
-- 用法：mysql -h 127.0.0.1 -u root -p sz300 < deploy/sql/perf-seed.sql
--
-- 数据集：1 个市场 / 1 个商户 / 1 个管理员用户（user_id=1，对应 JWT user_id）
--         20000 个商品（20 个分类均摊）+ 20000 个订单（70% 已支付 / 20% 待支付 / 10% 已取消）
--
-- 列约束依据（本机 migrations/001_init.sql 实测 information_schema）：
--   merchant.market_id 与 good.price / order.total_fen 均 NOT NULL 且无默认值，必须显式给出；
--   market 表同理（name NOT NULL）。
-- created_at 按 n 分钟递增铺开（而非全部 CURRENT_TIMESTAMP），
-- 使 idx_merchant_created(merchant_id, created_at) 具备真实区分度，避免测出"索引无选择性"的假象。

SET SESSION cte_max_recursion_depth = 30000;

-- 清空既有数据，保证幂等重跑（顺序：子表在前，避免将来加 FK 时失败）
DELETE FROM `order`;
DELETE FROM good;
DELETE FROM merchant_user;
DELETE FROM merchant;
DELETE FROM market;

INSERT INTO `market` (`market_id`, `name`, `address`, `status`)
VALUES (1, 'perf-gate-market', 'perf-gate', 1);

INSERT INTO `merchant` (`merchant_id`, `market_id`, `name`, `stall_no`, `status`)
VALUES (1, 1, 'perf-gate-merchant', 'PG-001', 1);

-- role=1 即管理员（controllers/common.rs::effective_merchant_scope_from_user 依赖此字段）
INSERT INTO `merchant_user` (`user_id`, `merchant_id`, `username`, `password_hash`, `phone`, `role`, `status`)
VALUES (1, 1, 'perf-gate-admin', '', '', 1, 1);

INSERT INTO `good` (`merchant_id`, `cat_id`, `name`, `barcode`, `price`, `unit`, `stock`, `status`, `created_at`)
WITH RECURSIVE seq(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM seq WHERE n < 20000
)
SELECT 1, n % 20, CONCAT('perf-good-', n), CONCAT('BC', LPAD(n, 8, '0')),
       100 + (n % 500), '斤', (n % 100) * 10, 1,
       DATE_ADD('2026-01-01 00:00:00', INTERVAL n MINUTE)
FROM seq;

-- 状态取值依 002_order_status_check.sql 的列注释：0=已取消 1=待支付 2=已支付 3=已退款。
-- 分布按真实业务形态铺开：70% 已支付 / 20% 待支付 / 10% 已取消（确定性，可重复）。
INSERT INTO `order` (`order_no`, `merchant_id`, `device_id`, `total_fen`, `total_weight_g`,
                     `item_count`, `status`, `pay_method`, `pay_at`, `created_at`)
WITH RECURSIVE seq(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM seq WHERE n < 20000
)
SELECT CONCAT('PERF', LPAD(n, 10, '0')), 1, 0,
       100 + (n % 900), 500 + (n % 2000), 1,
       CASE WHEN MOD(n, 10) = 0 THEN 0
            WHEN MOD(n, 10) IN (1, 2) THEN 1
            ELSE 2 END,
       0,
       CASE WHEN MOD(n, 10) = 0 OR MOD(n, 10) IN (1, 2) THEN NULL
            ELSE DATE_ADD('2026-01-01 00:00:00', INTERVAL n MINUTE) END,
       DATE_ADD('2026-01-01 00:00:00', INTERVAL n MINUTE)
FROM seq;

ANALYZE TABLE good;
ANALYZE TABLE `order`;

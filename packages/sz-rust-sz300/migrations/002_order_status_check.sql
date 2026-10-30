-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-10-28 数据层修复：订单状态语义单一化 + 约束
--
-- 背景：001_init.sql 的列注释写的是「0待支付 1已支付 2已退款 3已取消」，
-- 而代码（models/order.rs、order_service、mqtt_service）使用
-- 「0=已取消 1=待支付 2=已支付 3=已退款」，且新建订单一律写 status=1。
-- 本迁移以代码语义为准：修正注释、把 DEFAULT 从 0 改为 1、补 CHECK 约束。
--
-- 锁风险：MODIFY COLUMN 与 ADD CHECK 均可能重建表（ALGORITHM=COPY），
-- 请在低峰执行；执行前先用下面两条只读 SQL 核对存量分布：
--   SELECT status, COUNT(*) FROM `order` GROUP BY status;
--   SELECT COUNT(*) FROM `order` WHERE status IS NULL OR status NOT IN (0,1,2,3);

-- 1) 非法/NULL 状态归一到「待支付」
UPDATE `order` SET status = 1 WHERE status IS NULL OR status NOT IN (0, 1, 2, 3);

-- 2) 对齐注释与默认值
ALTER TABLE `order`
    MODIFY COLUMN `status` TINYINT NOT NULL DEFAULT 1
        COMMENT '0=已取消 1=待支付 2=已支付 3=已退款',
    ALGORITHM=COPY;

-- 3) 枚举约束
ALTER TABLE `order`
    ADD CONSTRAINT chk_order_status CHECK (status IN (0, 1, 2, 3));

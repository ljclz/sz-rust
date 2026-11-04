-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-10-28 数据层修复：补齐高频查询的复合索引
--
-- 现状：001_init.sql 只有单列索引（idx_merchant / idx_device / idx_status / idx_created），
-- 而订单列表的典型过滤是「商户 + 时间区间」或「设备 + 状态」，单列索引只能命中一列。
--
-- 锁风险：MySQL 8 加二级索引支持 INPLACE + LOCK=NONE（不阻塞读写），
-- 但大表仍会产生 IO 抖动，建议低峰执行并观察 innodb_online_alter_log_max_size。

-- 订单列表：商户维度 + 时间区间（ORDER BY order_id DESC 可复用主键）
ALTER TABLE `order`
    ADD KEY idx_merchant_created (merchant_id, created_at),
    ADD KEY idx_merchant_status_created (merchant_id, status, created_at),
    ADD KEY idx_device_status (device_id, status),
    ALGORITHM=INPLACE, LOCK=NONE;

-- 商品列表：商户维度 + 上下架状态
ALTER TABLE `good`
    ADD KEY idx_merchant_status (merchant_id, status),
    ALGORITHM=INPLACE, LOCK=NONE;

-- 操作日志：商户维度 + 时间倒序
ALTER TABLE operate_log
    ADD KEY idx_merchant_created (merchant_id, created_at),
    ALGORITHM=INPLACE, LOCK=NONE;

-- 结算：商户 + 结算日（001 已有 idx_merchant_date 则跳过本句）
-- ALTER TABLE settlement ADD KEY idx_merchant_date (merchant_id, settle_date), ALGORITHM=INPLACE, LOCK=NONE;

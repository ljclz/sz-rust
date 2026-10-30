-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-10-28 数据层修复：事务性 Outbox（解决「订单已写、消息未发」的不一致）
--
-- 写入方在**同一数据库事务**内插入 outbox_event；独立投递器按
-- (status, next_retry_at) 抢占并投递到消息中间件，成功后置 status=1。
-- 这样订单与事件要么同时存在、要么同时不存在，不再依赖应用层双写。

CREATE TABLE IF NOT EXISTS `outbox_event` (
    `id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `aggregate_type` VARCHAR(64) NOT NULL COMMENT '聚合类型，如 order',
    `aggregate_id` VARCHAR(64) NOT NULL COMMENT '聚合 ID，如 order_id',
    `event_type` VARCHAR(64) NOT NULL COMMENT '事件类型，如 order.created',
    `payload` JSON NOT NULL COMMENT '事件负载',
    `status` TINYINT NOT NULL DEFAULT 0 COMMENT '0=待投递 1=已投递 2=投递失败',
    `attempts` INT NOT NULL DEFAULT 0 COMMENT '投递尝试次数',
    `next_retry_at` DATETIME(3) NULL COMMENT '下次重试时间（退避）',
    `locked_until` DATETIME(3) NULL COMMENT '投递租约到期时间（多实例抢占用）',
    `last_error` VARCHAR(512) NULL COMMENT '最近一次错误（截断）',
    `created_at` DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    `updated_at` DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
    KEY `idx_outbox_pending` (`status`, `next_retry_at`),
    KEY `idx_outbox_aggregate` (`aggregate_type`, `aggregate_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='事务性 Outbox 事件表';

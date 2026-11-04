-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-10-28 数据层修复：MySQL 版 sz_dtx_log 建表
--
-- 背景：分布式事务日志表此前只有 PostgreSQL 版
-- （packages/sz-rust-distributed-tx/migrations/0001_create_sz_dtx_log.sql），
-- sz300 默认 MySQL 部署下 DbTxLogStore 的写入必然失败。本迁移补齐 MySQL 结构。
--
-- state 取值与 chk_dtx_state_v19 约束一致（见 add_sz_dtx_log_manual_intervention.sql）：
-- running / committed / aborted / compensated / compensating / manual_intervention / timeout

CREATE TABLE IF NOT EXISTS `sz_dtx_log` (
    `tx_id` VARCHAR(128) NOT NULL COMMENT '事务 ID（全局唯一）',
    `tx_type` VARCHAR(16) NOT NULL DEFAULT 'saga' COMMENT 'saga / tcc',
    `state` VARCHAR(32) NOT NULL DEFAULT 'running' COMMENT '事务状态',
    `steps` JSON NOT NULL COMMENT '步骤列表（JSON 数组）',
    `payload` JSON NOT NULL COMMENT '事务负载',
    `created_at` BIGINT NOT NULL COMMENT '创建时间（Unix 毫秒）',
    `updated_at` BIGINT NOT NULL COMMENT '更新时间（Unix 毫秒）',
    `retry_count` INT NOT NULL DEFAULT 0 COMMENT '恢复重试次数',
    PRIMARY KEY (`tx_id`),
    KEY `idx_sz_dtx_log_state` (`state`),
    KEY `idx_sz_dtx_log_updated_at` (`updated_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='分布式事务日志表';

-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-10-28 数据层修复：设备离线订单幂等唯一键
--
-- 背景：设备断网重传会重复落单；原 offline_seq 仅为普通 VARCHAR 且 NOT NULL DEFAULT ''，
-- 无法用唯一索引去重（'' 会互相冲突）。本迁移改为可空列 + (device_id, offline_seq) 唯一键：
-- MySQL 唯一索引允许多个 NULL，因此「没有离线序列号的在线订单」不会互相阻塞。
--
-- 执行前核对（只读）：
--   SELECT device_id, offline_seq, COUNT(*) c FROM `order`
--   WHERE offline_seq <> '' GROUP BY device_id, offline_seq HAVING c > 1;
-- 若上面有结果，必须先人工归并/清理重复单，否则唯一索引创建失败。
--
-- 锁风险：MODIFY COLUMN 走 COPY（重建表）→ 停机窗口；唯一索引本身 INPLACE。

ALTER TABLE `order`
    MODIFY COLUMN `offline_seq` VARCHAR(50) NULL DEFAULT NULL
        COMMENT '离线序列号（设备本地，用于幂等去重）',
    ALGORITHM=COPY;

UPDATE `order` SET offline_seq = NULL WHERE offline_seq = '';

ALTER TABLE `order`
    ADD UNIQUE KEY uk_device_offline_seq (device_id, offline_seq),
    ALGORITHM=INPLACE, LOCK=NONE;

-- v1.9.0: 为 sz_dtx_log.state 枚举扩展 manual_intervention 状态值
-- 2026-10-28 修正：状态词表与 saga_order.rs 的 state_to_str / str_to_state 对齐，
--                 并补齐遗漏的 timeout，默认值改为 running（与 007 建表一致）。
-- 执行顺序：先把历史行归一到新词表，再加 CHECK —— 否则加约束会因旧值失败。

ALTER TABLE sz_dtx_log
  MODIFY COLUMN state VARCHAR(32) NOT NULL DEFAULT 'running';

-- 历史值归一（旧代码写入的 completed / cancelled / failed）
UPDATE sz_dtx_log SET state = 'committed'           WHERE state = 'completed';
UPDATE sz_dtx_log SET state = 'aborted'             WHERE state = 'cancelled';
UPDATE sz_dtx_log SET state = 'manual_intervention' WHERE state = 'failed';

-- MySQL CHECK 约束（MySQL 8.0.16+）
ALTER TABLE sz_dtx_log
  ADD CONSTRAINT chk_dtx_state_v19
  CHECK (state IN (
    'init',
    'running',
    'committed',
    'aborted',
    'compensated',
    'compensating',
    'manual_intervention',
    'timeout'
  ));

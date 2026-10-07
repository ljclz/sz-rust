-- v1.9.0: 为 sz_dtx_log.state 枚举扩展 manual_intervention 状态值
-- 向前兼容：仅添加 CHECK 约束允许新值，不删除已有约束
-- 对齐 spec 5.3.1 规则 3：补偿失败时记录 manual_intervention

ALTER TABLE sz_dtx_log
  MODIFY COLUMN state VARCHAR(32) NOT NULL DEFAULT 'init';

-- MySQL CHECK 约束（MySQL 8.0.16+）
ALTER TABLE sz_dtx_log
  ADD CONSTRAINT chk_dtx_state_v19
  CHECK (state IN ('init', 'running', 'committed', 'aborted', 'compensated', 'compensating', 'manual_intervention'));
-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2024-2026 SZ-Rust Team
--
-- 分布式事务日志表（T025）
-- 对应 sz-rust-distributed-tx/src/persistence.rs

CREATE TABLE IF NOT EXISTS sz_dtx_log (
    tx_id          VARCHAR(128)  PRIMARY KEY,
    tx_type        VARCHAR(16)   NOT NULL DEFAULT 'saga',
    state          VARCHAR(32)   NOT NULL DEFAULT 'running',
    steps          JSONB         NOT NULL DEFAULT '[]'::jsonb,
    payload        JSONB         NOT NULL DEFAULT '{}'::jsonb,
    created_at     BIGINT        NOT NULL,
    updated_at     BIGINT        NOT NULL,
    retry_count    INT           NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_sz_dtx_log_state       ON sz_dtx_log (state);
CREATE INDEX IF NOT EXISTS idx_sz_dtx_log_tx_type     ON sz_dtx_log (tx_type);
CREATE INDEX IF NOT EXISTS idx_sz_dtx_log_updated_at  ON sz_dtx_log (updated_at);
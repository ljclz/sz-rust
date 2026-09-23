-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2024-2026 SZ-Rust Team
--
-- Agent 执行记录表（T037）
-- 对应 sz-rust-ai-facade/src/agent/persistence.rs

CREATE TABLE IF NOT EXISTS sz_agent_executions (
    execution_id   VARCHAR(128)  PRIMARY KEY,
    agent_name     VARCHAR(128)  NOT NULL,
    state          VARCHAR(32)   NOT NULL DEFAULT 'pending',
    current_step   INT           NOT NULL DEFAULT 0,
    total_steps    INT           NOT NULL DEFAULT 0,
    input          JSONB         NOT NULL DEFAULT '{}'::jsonb,
    output         JSONB,
    step_traces    JSONB         NOT NULL DEFAULT '[]'::jsonb,
    total_tokens   BIGINT        NOT NULL DEFAULT 0,
    created_at     BIGINT        NOT NULL,
    updated_at     BIGINT        NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sz_agent_exec_state      ON sz_agent_executions (state);
CREATE INDEX IF NOT EXISTS idx_sz_agent_exec_agent      ON sz_agent_executions (agent_name);
CREATE INDEX IF NOT EXISTS idx_sz_agent_exec_updated    ON sz_agent_executions (updated_at);
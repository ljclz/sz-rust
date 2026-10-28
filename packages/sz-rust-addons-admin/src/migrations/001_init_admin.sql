-- SPDX-License-Identifier: Apache-2.0
-- sz-rust-addons-admin 初始化迁移
-- 8 张表：users, roles, permissions, menus, configs, operation_logs, user_roles, role_permissions

-- 用户表
CREATE TABLE IF NOT EXISTS users (
    id BIGSERIAL PRIMARY KEY,
    username VARCHAR(50) NOT NULL,
    password VARCHAR(120) NOT NULL,
    email VARCHAR(100),
    phone VARCHAR(20),
    status VARCHAR(10) NOT NULL DEFAULT 'active',
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ,
    CONSTRAINT uk_users_username_tenant UNIQUE (username, tenant_id)
);

-- 角色表
CREATE TABLE IF NOT EXISTS roles (
    id BIGSERIAL PRIMARY KEY,
    name VARCHAR(50) NOT NULL,
    code VARCHAR(50) NOT NULL,
    description VARCHAR(200),
    is_builtin BOOLEAN NOT NULL DEFAULT FALSE,
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uk_roles_code_tenant UNIQUE (code, tenant_id)
);

-- 权限项表
CREATE TABLE IF NOT EXISTS permissions (
    id BIGSERIAL PRIMARY KEY,
    code VARCHAR(100) NOT NULL,
    name VARCHAR(100) NOT NULL,
    module VARCHAR(20) NOT NULL,
    description VARCHAR(200),
    tenant_id BIGINT NOT NULL DEFAULT 0,
    CONSTRAINT uk_permissions_code_tenant UNIQUE (code, tenant_id)
);

-- 菜单表
CREATE TABLE IF NOT EXISTS menus (
    id BIGSERIAL PRIMARY KEY,
    name VARCHAR(50) NOT NULL,
    code VARCHAR(50) NOT NULL,
    path VARCHAR(200) NOT NULL,
    icon VARCHAR(100),
    parent_id BIGINT NOT NULL DEFAULT 0,
    sort INT NOT NULL DEFAULT 0,
    is_visible BOOLEAN NOT NULL DEFAULT TRUE,
    permission_code VARCHAR(100),
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uk_menus_code_tenant UNIQUE (code, tenant_id)
);

-- 系统配置表
CREATE TABLE IF NOT EXISTS configs (
    id BIGSERIAL PRIMARY KEY,
    key VARCHAR(100) NOT NULL,
    value JSONB NOT NULL,
    value_type VARCHAR(10) NOT NULL,
    "group" VARCHAR(50),
    description VARCHAR(200),
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uk_configs_key_tenant UNIQUE (key, tenant_id)
);

-- 操作日志表
CREATE TABLE IF NOT EXISTS operation_logs (
    id BIGSERIAL PRIMARY KEY,
    operator_id BIGINT NOT NULL,
    operator_name VARCHAR(50) NOT NULL,
    operation_type VARCHAR(20) NOT NULL,
    target_type VARCHAR(20) NOT NULL,
    target_id BIGINT,
    detail JSONB,
    ip VARCHAR(50),
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_op_logs_operator ON operation_logs (operator_id);
CREATE INDEX IF NOT EXISTS idx_op_logs_tenant ON operation_logs (tenant_id);
CREATE INDEX IF NOT EXISTS idx_op_logs_created ON operation_logs (created_at);

-- 用户-角色关联表
CREATE TABLE IF NOT EXISTS user_roles (
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL,
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, role_id, tenant_id)
);

-- 角色-权限关联表
CREATE TABLE IF NOT EXISTS role_permissions (
    role_id BIGINT NOT NULL,
    permission_code VARCHAR(100) NOT NULL,
    tenant_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (role_id, permission_code, tenant_id)
);
-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段1：merchant_user.role 语义对齐 + CHECK 约束
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §5.3（语义冲突列）
-- 背景：001_init.sql:170 注释「1管理员 2操作员」与
--       models/merchant_user.rs:21 注释「0=普通用户，1=管理员」冲突。
--       010_status_enum.sql 文件头已声明跳过此列，留给本 migration 处理。
--
-- 用户确认的映射规则（以 models 为准 0/1）：
--   schema 1(管理员) → models 1(管理员)   不变
--   schema 2(操作员) → models 0(普通)     2 → 0
--
-- 锁风险：
--   - UPDATE 全表扫描，行数小时级（merchant_user 表通常 < 100K 行）；
--   - ADD CHECK 在 MySQL 8.0.16+ 为 INSTANT 操作（仅元数据），不阻塞读写。
--   顺序：先 UPDATE 存量、后 ADD CHECK，避免 CHECK 拒绝合法的中间态。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制）：
--   ALTER TABLE `merchant_user` DROP CONSTRAINT chk_merchant_user_role;
--   -- 数据回滚不可逆（schema 1 与 schema 2→0 后，models 0 无法区分原 schema 1 还是 2）；
--   -- 如需回滚数据，请从备份恢复。

-- 1) 存量数据迁移（schema 语义 → models 语义）
UPDATE `merchant_user` SET role = CASE
  WHEN role = 2 THEN 0  -- 操作员：schema 2 → models 0(普通)
  ELSE role             -- 1(管理员) 不变；0(已迁移) 保持
END;

-- 2) 加 CHECK 约束（与 models/merchant_user.rs:21 枚举对齐）
ALTER TABLE `merchant_user` ADD CONSTRAINT chk_merchant_user_role CHECK (role IN (0, 1));
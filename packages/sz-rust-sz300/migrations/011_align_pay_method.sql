-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段1：order.pay_method 语义对齐 + CHECK 约束
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §5.3（语义冲突列）
-- 背景：001_init.sql:97 注释「0扫码 1现金 2其他」与
--       models/order.rs:26 注释「0=未支付，1=微信，2=支付宝，3=现金」冲突。
--       010_status_enum.sql 文件头已声明跳过此列，留给本 migration 处理。
--
-- 用户确认的映射规则（以 models 为准 0/1/2/3）：
--   schema 0(扫码) → models 0(未支付)   不变
--   schema 1(现金) → models 3(现金)     1 → 3
--   schema 2(其他) → models 0(未支付)   2 → 0
--   说明：schema 语义中「扫码」无对应 models 值，归并到 0(未支付)；
--         「其他」同样归并到 0(未支付)。这是用户决策，非本 migration 推断。
--
-- 锁风险：
--   - UPDATE 全表扫描，行数小时级（order 表通常 < 10M 行）；
--   - ADD CHECK 在 MySQL 8.0.16+ 为 INSTANT 操作（仅元数据），不阻塞读写。
--   顺序：先 UPDATE 存量、后 ADD CHECK，避免 CHECK 拒绝合法的中间态。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制）：
--   ALTER TABLE `order` DROP CONSTRAINT chk_order_pay_method;
--   -- 数据回滚不可逆（schema 0 与 schema 2 均映射到 models 0，无法区分）；
--   -- 如需回滚数据，请从备份恢复。

-- 1) 存量数据迁移（schema 语义 → models 语义）
UPDATE `order` SET pay_method = CASE
  WHEN pay_method = 1 THEN 3  -- 现金：schema 1 → models 3
  WHEN pay_method = 2 THEN 0  -- 其他：schema 2 → models 0(未支付)
  ELSE pay_method             -- 0(扫码) → 0(未支付) 不变；3(已迁移) 保持
END;

-- 2) 加 CHECK 约束（与 models/order.rs:26 枚举对齐）
ALTER TABLE `order` ADD CONSTRAINT chk_order_pay_method CHECK (pay_method IN (0, 1, 2, 3));
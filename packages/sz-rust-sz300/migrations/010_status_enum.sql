-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段1：状态枚举列 CHECK 约束
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §5.2-D（13 个状态枚举列）
--
-- 本 migration 实施 11 个；跳过 2 个语义冲突列（留给 011/012，需业务确认）：
--   - merchant_user.role：001_init.sql:170「1管理员 2操作员」
--                        vs models/merchant_user.rs:22「0=普通用户 1=管理员」
--   - order.pay_method： 001_init.sql:97「0扫码 1现金 2其他」
--                        vs models/order.rs:27「0=未支付 1=微信 2=支付宝 3=现金」
--   上述两列需先与业务方对齐语义 + 存量数据 UPDATE 迁移，再加 CHECK（ADR §5.3）。
--   注：order.status 已由 002_order_status_check.sql 加 chk_order_status CHECK (status IN (0,1,2,3))，
--       本 migration 不重复。
--
-- 枚举值来源：以 models/*.rs 注释为准（ADR §5.2-D 已逐列附 file:line 证据）。
--   - settlement.status：001_init.sql:129 注释仅「0待结算 1已结算」，但
--     models/settlement.rs:22 明确含「2=已失败」，故用 IN (0,1,2)
--     （宁缺毋滥，避免拒绝合法的 2=已失败）。
--   - outbox_event.status：006_outbox.sql:16 注释「0=待投递 1=已投递 2=投递失败」。
--   - ota_version.status：001_init.sql:145 注释「1发布 0草稿」，枚举值 0/1。
--
-- 锁风险：ADD CHECK 在 MySQL 8.0.16+ 为 INSTANT 操作，不阻塞读写。
--
-- 回滚（人工执行，勿自动跑）：
--   ALTER TABLE `market`        DROP CONSTRAINT chk_market_status_enum;
--   ALTER TABLE `merchant`      DROP CONSTRAINT chk_merchant_status_enum;
--   ALTER TABLE `good`          DROP CONSTRAINT chk_good_status_enum;
--   ALTER TABLE `device`        DROP CONSTRAINT chk_device_status_enum;
--   ALTER TABLE `category`      DROP CONSTRAINT chk_category_status_enum;
--   ALTER TABLE `ai_category`   DROP CONSTRAINT chk_ai_category_status_enum;
--   ALTER TABLE `settlement`    DROP CONSTRAINT chk_settlement_status_enum;
--   ALTER TABLE `ota_version`   DROP CONSTRAINT chk_ota_version_forced_enum;
--   ALTER TABLE `ota_version`   DROP CONSTRAINT chk_ota_version_status_enum;
--   ALTER TABLE `merchant_user` DROP CONSTRAINT chk_merchant_user_status_enum;
--   ALTER TABLE `outbox_event`  DROP CONSTRAINT chk_outbox_event_status_enum;

-- 1) 0/1 二值状态（启用/禁用、上架/下架、在线/离线）
ALTER TABLE `market`      ADD CONSTRAINT chk_market_status_enum       CHECK (status IN (0, 1));
ALTER TABLE `merchant`    ADD CONSTRAINT chk_merchant_status_enum     CHECK (status IN (0, 1));
ALTER TABLE `good`        ADD CONSTRAINT chk_good_status_enum         CHECK (status IN (0, 1));
-- device.status：0=离线 1=在线 2/3=设备上报扩展状态（故障/告警等）。
-- 2026-11-05 修复：原 CHECK (status IN (0,1)) 与 MQTT 设备状态上报业务冲突
-- （handle_device_status / DeviceService::update_status 接受设备上报的 2/3，
-- 见 tests/mqtt_dispatch_test.rs:199、tests/service_coverage_test.rs:331/408、
-- tests/e2e_device_test.rs:355、tests/e2e_mqtt_test.rs:235），UPDATE 被约束拒绝
-- 导致 dispatch 后 DB 行未更新。放宽至 IN (0,1,2,3) 与 order.status 枚举对齐。
ALTER TABLE `device`      ADD CONSTRAINT chk_device_status_enum       CHECK (status IN (0, 1, 2, 3));
ALTER TABLE `category`    ADD CONSTRAINT chk_category_status_enum     CHECK (status IN (0, 1));
ALTER TABLE `ai_category` ADD CONSTRAINT chk_ai_category_status_enum  CHECK (status IN (0, 1));

-- 2) settlement（0=待结算 1=已结算 2=已失败，见 models/settlement.rs:22）
ALTER TABLE `settlement`  ADD CONSTRAINT chk_settlement_status_enum   CHECK (status IN (0, 1, 2));

-- 3) ota_version（forced 0=否 1=是；status 0=草稿 1=发布）
ALTER TABLE `ota_version` ADD CONSTRAINT chk_ota_version_forced_enum  CHECK (forced IN (0, 1));
ALTER TABLE `ota_version` ADD CONSTRAINT chk_ota_version_status_enum  CHECK (status IN (0, 1));

-- 4) merchant_user.status（0/1；role 跳过 —— 语义冲突见文件头，留给 012）
ALTER TABLE `merchant_user` ADD CONSTRAINT chk_merchant_user_status_enum CHECK (status IN (0, 1));

-- 5) outbox_event（0=待投递 1=已投递 2=投递失败，006_outbox.sql:16）
ALTER TABLE `outbox_event` ADD CONSTRAINT chk_outbox_event_status_enum CHECK (status IN (0, 1, 2));
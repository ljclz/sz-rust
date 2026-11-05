-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段3：NOT NULL 直接 FK 约束（6 个）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-A
-- 目的：为 6 个 NOT NULL 列加外键约束，建立核心实体关系：
--   merchant → market、good/order/settlement/merchant_user → merchant、order_item → order
-- 级联策略：
--   ON DELETE RESTRICT：核心父实体（market/merchant），拒绝物理删除，配合软删除
--     （status=0）不触发，仅拦截物理 DELETE，强制人工处理
--   ON DELETE CASCADE：强从属子表（order_item→order、merchant_user→merchant），
--     父删子必删（现有 saga 补偿已手动先删 item，CASCADE 为双保险）
--
-- 前置条件（生产）：加 FK 前 MySQL 全表校验引用完整性，若存在孤儿行则整个 ALTER
-- 失败（error 1452）。生产执行前必须先跑孤儿检查：
--   SELECT COUNT(*) FROM merchant m LEFT JOIN market t ON m.market_id=t.market_id WHERE t.market_id IS NULL;
--   SELECT COUNT(*) FROM good g LEFT JOIN merchant m ON g.merchant_id=m.merchant_id WHERE m.merchant_id IS NULL;
--   SELECT COUNT(*) FROM `order` o LEFT JOIN merchant m ON o.merchant_id=m.merchant_id WHERE m.merchant_id IS NULL;
--   SELECT COUNT(*) FROM order_item oi LEFT JOIN `order` o ON oi.order_id=o.order_id WHERE o.order_id IS NULL;
--   SELECT COUNT(*) FROM settlement s LEFT JOIN merchant m ON s.merchant_id=m.merchant_id WHERE m.merchant_id IS NULL;
--   SELECT COUNT(*) FROM merchant_user u LEFT JOIN merchant m ON u.merchant_id=m.merchant_id WHERE m.merchant_id IS NULL;
-- 若某查询 >0，先人工处理（DELETE 孤儿行或 UPDATE 引用至合法父键）再执行本 migration。
-- 测试环境：setup 时表被 truncate/DELETE，无存量数据，不会触发孤儿检查失败。
--
-- 锁风险：ADD FOREIGN KEY 在 MySQL 8.0 走 ALGORITHM=INPLACE, LOCK=NONE（不阻塞
-- 读写），但会全表扫描子表验证引用完整性，大表有 IO 抖动。测试环境表小，无影响。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制，
--       以下 DROP 语句以注释形式提供供回滚参考）：
--   ALTER TABLE `merchant`      DROP FOREIGN KEY fk_merchant_market;
--   ALTER TABLE `good`          DROP FOREIGN KEY fk_good_merchant;
--   ALTER TABLE `order`         DROP FOREIGN KEY fk_order_merchant;
--   ALTER TABLE `order_item`    DROP FOREIGN KEY fk_order_item_order;
--   ALTER TABLE `settlement`    DROP FOREIGN KEY fk_settlement_merchant;
--   ALTER TABLE `merchant_user` DROP FOREIGN KEY fk_merchant_user_merchant;

-- 1) merchant.market_id → market.market_id（商户必须属于市场）
ALTER TABLE `merchant` ADD CONSTRAINT fk_merchant_market
  FOREIGN KEY (market_id) REFERENCES `market`(market_id) ON DELETE RESTRICT;

-- 2) good.merchant_id → merchant.merchant_id（商品归属商户）
ALTER TABLE `good` ADD CONSTRAINT fk_good_merchant
  FOREIGN KEY (merchant_id) REFERENCES `merchant`(merchant_id) ON DELETE RESTRICT;

-- 3) order.merchant_id → merchant.merchant_id（订单归属商户；order 为保留字须反引号）
ALTER TABLE `order` ADD CONSTRAINT fk_order_merchant
  FOREIGN KEY (merchant_id) REFERENCES `merchant`(merchant_id) ON DELETE RESTRICT;

-- 4) order_item.order_id → order.order_id（订单项强从属于订单，CASCADE 双保险）
ALTER TABLE `order_item` ADD CONSTRAINT fk_order_item_order
  FOREIGN KEY (order_id) REFERENCES `order`(order_id) ON DELETE CASCADE;

-- 5) settlement.merchant_id → merchant.merchant_id（结算记录是财务凭证，禁止级联删）
ALTER TABLE `settlement` ADD CONSTRAINT fk_settlement_merchant
  FOREIGN KEY (merchant_id) REFERENCES `merchant`(merchant_id) ON DELETE RESTRICT;

-- 6) merchant_user.merchant_id → merchant.merchant_id（用户强从属于商户）
ALTER TABLE `merchant_user` ADD CONSTRAINT fk_merchant_user_merchant
  FOREIGN KEY (merchant_id) REFERENCES `merchant`(merchant_id) ON DELETE CASCADE;
-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层 Phase 2 阶段3：可空 FK 约束（8 个）
--
-- 依据：docs/adr/0051-数据层FK-CHECK补齐.md §4.2-B
-- 前置：阶段2（migration 013~019）已把 8 个哨兵列改 NULLABLE + 存量 0→NULL，
--   对应模型字段已改 Option<i64>，服务层已适配。本 migration 在此基础上加 FK。
-- 目的：为 8 个可空列加外键约束，建立弱引用关系：
--   device→merchant、order→device、order_item→good、good→category/ai_category、
--   category→category(自引用)、ai_category→category、operate_log→merchant
-- 级联策略：
--   ON DELETE SET NULL：父删则子表引用置 NULL，保留子行（哨兵语义已迁移到 NULL）
--   ON DELETE RESTRICT：category.parent_id 自引用，防止删有子类的类目（删前需先迁子类）
--
-- 自引用环风险：fk_category_parent 为 category.parent_id → category.cat_id 自引用，
-- 需确保类目树无环。阶段2 已把 parent_id=0 改为 NULL（顶级类目），存量数据中
-- parent_id 非 NULL 的行必须指向已存在的 cat_id 且不形成环。生产执行前用递归 CTE
-- 校验（见 ADR §8.5）。测试夹具中 category 表 parent_id 均为 NULL（顶级），无环。
--
-- 前置条件（生产）：加 FK 前 MySQL 全表校验引用完整性，若存在孤儿行则整个 ALTER
-- 失败（error 1452）。生产执行前必须先跑孤儿检查（仅检查非 NULL 引用）：
--   SELECT COUNT(*) FROM device d LEFT JOIN merchant m ON d.merchant_id=m.merchant_id
--     WHERE d.merchant_id IS NOT NULL AND m.merchant_id IS NULL;
--   SELECT COUNT(*) FROM `order` o LEFT JOIN device d ON o.device_id=d.device_id
--     WHERE o.device_id IS NOT NULL AND d.device_id IS NULL;
--   SELECT COUNT(*) FROM order_item oi LEFT JOIN good g ON oi.good_id=g.good_id
--     WHERE oi.good_id IS NOT NULL AND g.good_id IS NULL;
--   SELECT COUNT(*) FROM good g LEFT JOIN category c ON g.cat_id=c.cat_id
--     WHERE g.cat_id IS NOT NULL AND c.cat_id IS NULL;
--   SELECT COUNT(*) FROM good g LEFT JOIN ai_category a ON g.ai_class_id=a.ai_class_id
--     WHERE g.ai_class_id IS NOT NULL AND a.ai_class_id IS NULL;
--   SELECT COUNT(*) FROM category c LEFT JOIN category p ON c.parent_id=p.cat_id
--     WHERE c.parent_id IS NOT NULL AND p.cat_id IS NULL;
--   SELECT COUNT(*) FROM ai_category a LEFT JOIN category c ON a.cat_id=c.cat_id
--     WHERE a.cat_id IS NOT NULL AND c.cat_id IS NULL;
--   SELECT COUNT(*) FROM operate_log l LEFT JOIN merchant m ON l.merchant_id=m.merchant_id
--     WHERE l.merchant_id IS NOT NULL AND m.merchant_id IS NULL;
-- 若某查询 >0，先人工处理（UPDATE 引用置 NULL 或迁至合法父键）再执行本 migration。
-- 测试环境：setup 时表被 truncate/DELETE，无存量数据，不会触发孤儿检查失败。
--
-- 锁风险：ADD FOREIGN KEY 在 MySQL 8.0 走 ALGORITHM=INPLACE, LOCK=NONE（不阻塞
-- 读写），但会全表扫描子表验证引用完整性，大表有 IO 抖动。测试环境表小，无影响。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制，
--       以下 DROP 语句以注释形式提供供回滚参考）：
--   ALTER TABLE `device`       DROP FOREIGN KEY fk_device_merchant;
--   ALTER TABLE `order`        DROP FOREIGN KEY fk_order_device;
--   ALTER TABLE `order_item`   DROP FOREIGN KEY fk_order_item_good;
--   ALTER TABLE `good`         DROP FOREIGN KEY fk_good_cat;
--   ALTER TABLE `good`         DROP FOREIGN KEY fk_good_ai_class;
--   ALTER TABLE `category`     DROP FOREIGN KEY fk_category_parent;
--   ALTER TABLE `ai_category`  DROP FOREIGN KEY fk_ai_category_cat;
--   ALTER TABLE `operate_log`  DROP FOREIGN KEY fk_operate_log_merchant;

-- 7) device.merchant_id → merchant.merchant_id（0=未绑定→NULL，父删置 NULL 保留设备）
ALTER TABLE `device` ADD CONSTRAINT fk_device_merchant
  FOREIGN KEY (merchant_id) REFERENCES `merchant`(merchant_id) ON DELETE SET NULL;

-- 8) order.device_id → device.device_id（0=非设备下单→NULL；order 为保留字须反引号）
ALTER TABLE `order` ADD CONSTRAINT fk_order_device
  FOREIGN KEY (device_id) REFERENCES `device`(device_id) ON DELETE SET NULL;

-- 9) order_item.good_id → good.good_id（0=手工录入→NULL，保留 good_name 快照）
ALTER TABLE `order_item` ADD CONSTRAINT fk_order_item_good
  FOREIGN KEY (good_id) REFERENCES `good`(good_id) ON DELETE SET NULL;

-- 10) good.cat_id → category.cat_id（0=未分类→NULL）
ALTER TABLE `good` ADD CONSTRAINT fk_good_cat
  FOREIGN KEY (cat_id) REFERENCES `category`(cat_id) ON DELETE SET NULL;

-- 11) good.ai_class_id → ai_category.ai_class_id（0=无 AI 分类→NULL）
ALTER TABLE `good` ADD CONSTRAINT fk_good_ai_class
  FOREIGN KEY (ai_class_id) REFERENCES `ai_category`(ai_class_id) ON DELETE SET NULL;

-- 12) category.parent_id → category.cat_id（自引用；0=顶级→NULL；RESTRICT 防删有子类的类目）
ALTER TABLE `category` ADD CONSTRAINT fk_category_parent
  FOREIGN KEY (parent_id) REFERENCES `category`(cat_id) ON DELETE RESTRICT;

-- 13) ai_category.cat_id → category.cat_id（0=无关联→NULL）
ALTER TABLE `ai_category` ADD CONSTRAINT fk_ai_category_cat
  FOREIGN KEY (cat_id) REFERENCES `category`(cat_id) ON DELETE SET NULL;

-- 14) operate_log.merchant_id → merchant.merchant_id（0=系统级→NULL，系统级日志保留）
ALTER TABLE `operate_log` ADD CONSTRAINT fk_operate_log_merchant
  FOREIGN KEY (merchant_id) REFERENCES `merchant`(merchant_id) ON DELETE SET NULL;
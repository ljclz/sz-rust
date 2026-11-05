-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- 2026-11-05 数据层深化：商品表复合索引（商户 + 类目）
--
-- 背景：ProductService::list 的高频查询路径为
--   WHERE merchant_id = ? AND cat_id = ? [AND name LIKE ?] ORDER BY good_id DESC
-- （controllers/product.rs:50/152 从请求解析 cat_id 传入 ProductFilters）。
-- 现有 idx_merchant_status(merchant_id, status)（003_indexes.sql）不覆盖 cat_id，
-- 按类目浏览商品时只能命中 merchant_id 前导，cat_id 过滤需回表扫描。
--
-- 本索引：idx_merchant_cat(merchant_id, cat_id)
--   - 等值条件 merchant_id 在前、cat_id 在后，符合最左前缀原则
--   - ORDER BY good_id DESC：good_id 是主键，InnoDB 二级索引尾部隐式带主键，
--     索引叶子节点按 (merchant_id, cat_id, good_id) 排序，反向扫描可直接产出有序行，
--     无需 filesort
--   - name LIKE '%kw%' 前导通配符无法走索引，是固有局限，本索引不解决
--
-- 冗余说明：001_init.sql 的 idx_merchant(merchant_id) 单列索引现已被
-- idx_merchant_status / idx_merchant_cat 两个复合索引的前导列覆盖，理论上冗余。
-- 本 migration 不删除该单列索引（删除有 FORCE INDEX 引用风险，且不在本次范围），
-- 仅作记录，后续如需清理应单独评估。
--
-- 锁风险：MySQL 8.0 ADD KEY 走 ALGORITHM=INPLACE, LOCK=NONE（不阻塞读写），
-- 大表会有 IO 抖动，建议低峰执行并观察 innodb_online_alter_log_max_size。
--
-- 回滚（人工执行，勿自动跑 —— 本仓库 migration runner 无 sql_down 机制）：
--   ALTER TABLE `good` DROP KEY idx_merchant_cat;

ALTER TABLE `good`
    ADD KEY idx_merchant_cat (merchant_id, cat_id),
    ALGORITHM=INPLACE, LOCK=NONE;
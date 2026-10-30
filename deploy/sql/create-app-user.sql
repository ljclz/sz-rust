-- SPDX-License-Identifier: Apache-2.0
-- Copyright (c) 2026 SZ-Rust Team
--
-- sz300 应用账号（最小权限）
--
-- 用法（以 DBA/root 执行，替换占位符后运行）：
--   mysql -uroot -p < deploy/sql/create-app-user.sql
--
-- 之后应用侧只配置：
--   SZ300_DB_USER=sz300_app
--   SZ300_DB_PASSWORD=<强口令>
--
-- 权限说明：
--   SELECT/INSERT/UPDATE/DELETE —— 业务读写
--   CREATE/ALTER/INDEX/DROP    —— 应用启动时执行 migrations
--   不授予 GRANT OPTION / FILE / SUPER / PROCESS / REPLICATION CLIENT
--
-- 若迁移改由发布流水线用独立账号执行，可去掉 CREATE/ALTER/INDEX/DROP，
-- 只保留 DML（见文末注释的严格版）。

CREATE DATABASE IF NOT EXISTS `sz300`
    DEFAULT CHARACTER SET utf8mb4
    DEFAULT COLLATE utf8mb4_0900_ai_ci;

CREATE USER IF NOT EXISTS 'sz300_app'@'%' IDENTIFIED BY 'CHANGE_ME_STRONG_PASSWORD';

GRANT SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, INDEX, DROP
    ON `sz300`.* TO 'sz300_app'@'%';

FLUSH PRIVILEGES;

-- 严格版（迁移与运行时账号分离时使用）：
--   REVOKE CREATE, ALTER, INDEX, DROP ON `sz300`.* FROM 'sz300_app'@'%';
--   GRANT SELECT, INSERT, UPDATE, DELETE ON `sz300`.* TO 'sz300_app'@'%';

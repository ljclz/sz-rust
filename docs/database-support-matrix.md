# 数据库支持矩阵（2026-10-28）

> 目的：把"代码里能跑"和"生产上被验证过"分开声明，避免多方言路径看起来支持、
> 实际从未执行。每格状态都必须有可复现证据，未验证就写未验证。

## 1. 结论

| 组件 | MySQL 8.0.16+ | PostgreSQL | SQLite | Oracle | SQL Server |
|---|---|---|---|---|---|
| `sz-rust-sz300`（业务应用） | **生产支持** | 未验证 | 不支持 | 不支持 | 不支持 |
| `sz-rust-distributed-tx`（Saga/TCC） | 生产支持（MySQL 方言） | 仅建表脚本 | 不支持 | 不支持 | 不支持 |
| `sz-rust-orm-facade` / 任务队列 | 生产支持 | 未验证 | 不支持 | 不支持 | 不支持 |
| `sz-rust-cli migrate` | 生产支持 | 生产支持 | 生产支持 | 代码路径存在，未验证 | 代码路径存在，未验证 |
| `sz-orm`（外部依赖 `../sz-orm`） | 支持 | 支持 | 支持 | 独立 crate | 独立 crate |

**sz300 的部署承诺只有一个：MySQL 单主（+ 可选只读副本）。**
PostgreSQL 的 `migrations/001_init_pg.sql` 是历史草稿，与 MySQL 版本已分叉
（缺少 002–007 的对等 DDL），不作为支持承诺。

## 2. 证据

| 结论 | 证据 |
|---|---|
| sz300 迁移全部为 MySQL DDL | `packages/sz-rust-sz300/migrations/001_init.sql` ~ `007_sz_dtx_log.sql`，使用 `ENGINE=InnoDB`、反引号标识符、`ON UPDATE CURRENT_TIMESTAMP(3)` |
| sz300 代码使用 MySQL 专有语法 | `order_service.rs` 的 `LAST_INSERT_ID()`；`saga_order.rs` 的 `ON DUPLICATE KEY UPDATE`；`scale_task_store` 类语句的 `FOR UPDATE SKIP LOCKED`；迁移锁 `GET_LOCK`（`packages/sz-rust-cli/src/cmd/migrate.rs`） |
| PG 版 DDL 已分叉 | `001_init_pg.sql` 仅 13 张表，无对应的 002–007 |
| sz-orm PG 占位符已修 | `../sz-orm/packages/sz-orm-sqlx/src/any.rs` 中 `pg_rewrite_placeholders` + `bind_pg_value` |
| Oracle / MSSQL 仅代码路径 | `packages/sz-rust-cli/src/cmd/migrate.rs` 的 `create_connection` 分支；无 CI 实例验证 |

## 3. 若要新增方言，必须同时满足

1. 该方言的完整迁移目录（`001`–最新，含 `ALTER`），不是只有建表；
2. 迁移器事务语义：`supports_transactional_ddl` 与迁移锁实现；
3. 一条真实数据库的 CI job（不是 `cargo check`）；
4. 本文件对应行状态从"未验证"改为"生产支持"，并附 CI 运行链接。

## 4. 运维基线（MySQL）

| 项 | 要求 |
|---|---|
| 版本 | 8.0.16+（`CHECK` 约束生效的最低版本，见 `002_order_status_check.sql`） |
| 字符集 | `utf8mb4` / `utf8mb4_0900_ai_ci` |
| 隔离级别 | `REPEATABLE READ`（默认）；下单事务依赖 `LAST_INSERT_ID()` 同连接语义 |
| binlog | `log_bin=ON`，`binlog_format=ROW`，保留 ≥ 7 天（增量恢复依赖） |
| 传输加密 | 生产 `SZ300_DB_TLS=required`（默认）；有 CA 时用 `verify-identity` + `SZ300_DB_TLS_CA`；生产禁止 `disabled`（`db::resolve_tls_mode` 启动即报错） |
| 慢查询 | `slow_query_log=ON`，`long_query_time=0.2`，`log_queries_not_using_indexes=OFF`；用 `pt-query-digest` 按天聚合 |
| 应用账号 | 禁止 root，见 `deploy/sql/create-app-user.sql` |
| 备份 | 全量 + binlog 归档 + 恢复演练：`scripts/db/backup-restore-drill.sh` |

慢查询在数据库侧采集而非应用侧打点：应用层打点需要包裹每一次 `conn.query`，
要么漏采样、要么给热路径加计时开销；MySQL 自带日志是权威且无侵入的来源。
应用侧只暴露连接池指标（`/metrics` 的 `sz300_db_pool_*`），用于判断池耗尽与连接重建。

# ADR-036: 可靠任务队列（数据库持久化 Job 表）

- **状态**: Accepted
- **日期**: 2026-08-15
- **相关代码**: `packages/sz-rust-orm-facade/src/jobs.rs`, `packages/sz-rust-sz300/src/main.rs`

## 背景

`tokio::spawn` 只解决并发执行，不解决任务可靠性：进程重启丢任务、失败无人重试、
重试无上限打爆下游、重复执行产生副作用。sz-orm-scheduler 提供 cron 定时触发与
内存态 DAG 编排，但无持久化任务队列。本次新增数据库持久化的可靠任务队列，
对齐行业实践（任务数据化 / 状态机 / 原子领取 / 退避重试 / 幂等 / 死信 / 队列健康观测）。

## 决策

1. **落点**：`sz-rust-orm-facade/src/jobs.rs`（新模块，与 pool_config/pool_scaler 同为 facade 增强层），sz300 接线演示
2. **任务数据化**：`sz_jobs` 表（kind/payload/status/attempts/run_after/locked_until/last_error/dedupe_key/created_at/updated_at），时间统一 BIGINT 毫秒时间戳（UTC），避免 DATETIME 时区歧义
3. **状态机**：pending（含延迟与退避，靠 run_after 表达）/ running（locked_until 租约）/ succeeded / dead（永久失败，可重放）；失败与完成分离，临时失败与永久失败分离（JobErrorKind::Temporary/Permanent）
4. **原子领取**：事务内 `SELECT id ... FOR UPDATE SKIP LOCKED` 锁定候选（MySQL 8.0.1+），再 UPDATE 抢占（attempts+1, locked_until 租约），commit 后按 `locked_until` 精确读回；多实例 worker 安全
   - **2026-08-15 修正（验证发现）**：v1 曾用"单条 `UPDATE ... WHERE id IN (SELECT ...)` 抢占"，并发集成测试实测重复执行 2×——MySQL 默认 REPEATABLE READ 下子查询为快照读，且 InnoDB UPDATE 锁等待后不重新评估 WHERE（semi-consistent read 仅 READ COMMITTED 启用），乐观锁条件失效。`FOR UPDATE SKIP LOCKED` 在锁定阶段即跳过他人已锁行，为 MySQL/PostgreSQL 通用标准做法。
5. **退避重试**：指数退避 `base * 2^min(attempts,6)` 封顶 + 随机抖动（防批量失败同时冲击下游），`max_attempts` 上限，超限进死信
6. **幂等**：`UNIQUE(kind, dedupe_key)` 约束 + `ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id)`，重复入队返回已有任务 ID
7. **崩溃自愈**：worker 每轮回收 `locked_until` 超时的 running 任务回 pending
8. **死信**：dead 状态保留 last_error/attempts，`retry_dead()` 人工重放
9. **观测**：`queue_snapshot()` 输出 pending/running/dead/最老 pending 等待秒数
10. **安全**：全部 SQL 参数化绑定（execute_with_params），显式列投影，无 `SELECT *`

## 替代方案

- **外部 MQ（Kafka/NATS/RabbitMQ）**：sz-orm-queue 已封装客户端，但引入运维组件；DB 任务队列复用现有连接池，事务性天然一致（任务与业务同库同事务）
- **内存队列（tokio::sync::mpsc）**：进程重启丢任务，无法多实例
- **改造 sz-orm-scheduler**：其职责是 cron 定时触发，任务队列是另一层能力，保持单一职责

## Bug 定位提示

- `jobs.rs:claim_batch` — 原子领取（单条 UPDATE 抢占 + locked_until 过滤读回）
- `jobs.rs:handle_failure` — 退避重试/死信分派（attempts 在领取时 +1，此处判断是否超限）
- `jobs.rs:backoff_delay_ms` — 指数退避 + 抖动
- `jobs.rs:enqueue_at` — 幂等入队（ON DUPLICATE KEY）
- `jobs.rs:reclaim_stale` — 租约超时回收（worker 崩溃自愈）
- `sz300/src/main.rs` — JobQueue 初始化 + OrderExpireCheckHandler 示例 handler + worker 启动接线

## 影响

- 新增 facade pub API：JobQueue / JobQueueConfig / Job / JobStatus / JobError / TaskHandler / QueueSnapshot（`sz-rust-orm-facade/src/lib.rs` 导出）
- 测试：facade 单测 6 例（状态机转换/退避上限/抖动范围/单调时间/错误分类）+ sz300 集成测试 5 例（幂等/延迟/成功与重试/死信重放/状态转换，`--ignored` 需真实 MySQL）
- 验证：`cargo test -p sz-rust-sz300 --test jobs_integration_test -- --ignored` → 5 passed（真实 MySQL 9.6，本机 127.0.0.1:3306）
- 表结构由 `init_schema()` 幂等建表（CREATE TABLE IF NOT EXISTS），不引入迁移流程；生产环境如需版本化迁移可后续接 sz-orm migration
- 已知取舍：领取 SQL 依赖 `FOR UPDATE SKIP LOCKED`（MySQL 8.0.1+ / PostgreSQL 9.5+ 均支持），建表 SQL 为 MySQL 方言（sz300 主数据源）；其他数据库需调整建表 SQL 与领取语句

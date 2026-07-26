# SzRSQL Agent 指南

> **项目**：SzRSQL — Rust 实现的分布式 SQL 数据库
> **规模**：16 个 crate，约 258K 行代码，约 3874 个测试
> **技术栈**：Rust、pgwire 协议、Raft 共识、Percolator 分布式事务模型
> **参考数据库**：本机 PostgreSQL 18（`postgres://postgres:test123@127.0.0.1:5432/sz_orm_test`）

## 门禁清单（AI 每次修改前必须自检）

- [ ] 是否读取了 `.trae/rules/project_rules.md`？
- [ ] 本次修改涉及的技能是否已加载（`.trae/skills/`）？
- [ ] 是否运行了 `cargo check --workspace`？
- [ ] 是否运行了受影响 crate 的全量测试？
- [ ] 修改 SQL 逻辑时是否运行差分比对？
- [ ] 修改事务/存储时是否运行 loom 并发测试？
- [ ] 是否检查了代码中是否残留 `unwrap`/`expect`？

## 关键路径文件

| 模块 | 路径 |
|------|------|
| SQL 解析器 | `crates/szrsql-sql/src/parser.rs` |
| SQL 执行器 | `crates/szrsql-sql/src/executor.rs` |
| 事务管理器 | `crates/szrsql-tx/src/mvcc.rs` |
| 锁管理器 | `crates/szrsql-tx/src/lock.rs` |
| WAL 写入器 | `crates/szrsql-tx/src/wal.rs` |
| Buffer Pool | `crates/szrsql-storage/src/buffer.rs` |
| B+Tree 存储 | `crates/szrsql-storage/src/btree.rs` |
| pgwire 协议 | `crates/szrsql-protocol/src/` |
| Raft 共识 | `crates/szrsql-consensus/src/` |

## 可用技能列表

| 技能 | 路径 | 触发条件 |
|------|------|----------|
| 变异测试 | `.trae/skills/szrsql-mutation-testing/SKILL.md` | 修改核心逻辑时 |
| 差分模糊测试 | `.trae/skills/szrsql-differential-fuzzing/SKILL.md` | 修改 SQL 逻辑时 |
| 混沌工程 | `.trae/skills/szrsql-chaos-engineering/SKILL.md` | 修改事务/存储时 |
| 反向黑盒审查 | `.trae/skills/szrsql-spec-review/SKILL.md` | SQL/协议/事务变更时 |
| 影子流量回放 | `.trae/skills/szrsql-shadow-traffic/SKILL.md` | 上线前 |

## 本机数据库连接信息

| 数据库 | 连接串 |
|--------|--------|
| PostgreSQL 18 | `postgres://postgres:test123@127.0.0.1:5432/sz_orm_test` |
| MySQL 9.6 | `mysql://root:test123@127.0.0.1:3306/sz_orm_test` |
| Oracle 23ai | `127.0.0.1:1521/freepdb1` (sys/test123) |

# SzRSQL Agent 指南

> **项目**：SzRSQL — Rust 实现的分布式 SQL 数据库
> **规模**：22 个 crate，约 406K 行 .rs 代码（含测试，其中约 339K 行生产代码），约 10.2K 个测试标注（2026-08-01 实测：9,943 `#[test]` + 264 `#[tokio::test]`）
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
| Raft 共识 | `crates/szrsql-dist/src/raft.rs` |

## 可用技能列表

| 技能 | 路径 | 触发条件 |
|------|------|----------|
| 变异测试 | `.trae/skills/szrsql-mutation-testing/SKILL.md` | 修改核心逻辑时 |
| 差分模糊测试 | `.trae/skills/szrsql-differential-fuzzing/SKILL.md` | 修改 SQL 逻辑时 |
| 混沌工程 | `.trae/skills/szrsql-chaos-engineering/SKILL.md` | 修改事务/存储时 |
| 反向黑盒审查 | `.trae/skills/szrsql-spec-review/SKILL.md` | SQL/协议/事务变更时 |
| 影子流量回放 | `.trae/skills/szrsql-shadow-traffic/SKILL.md` | 上线前 |
| 审计质量 | `.trae/skills/szrsql-framework-audit-quality/SKILL.md` | 修改任何审计相关文档时 |
| 审计证据链 | `.trae/skills/szrsql-audit-evidence/SKILL.md` | 生成/更新审计报告、状态报告、评估报告时 |
| Navicat 门禁 | `.trae/skills/szrsql-navicat-validation/SKILL.md` | Navicat 连接测试相关改动合入前 |
| Navicat 方言切换 | `.trae/skills/szrsql-navicat-dialect-switch/SKILL.md` | 切换 Navicat 数据库类型下拉框后连接异常、保留字冲突、标识符引号错误时 |
| Navicat 错误码映射 | `.trae/skills/szrsql-navicat-errorcode-mapper/SKILL.md` | Navicat 报错框显示乱码、错误码不识别、连接被强制关闭时 |
| Navicat 黄金回放 | `.trae/skills/szrsql-navicat-golden-recorder/SKILL.md` | 调试 Navicat/DBeaver/DataGrip 客户端兼容性问题时（必须先录制真实基线） |
| Navicat 元数据模拟 | `.trae/skills/szrsql-navicat-metadata-mock/SKILL.md` | Navicat 左侧树无法展开、表列表为空、列信息错乱时 |
| Navicat 预处理语句 | `.trae/skills/szrsql-navicat-prepared-statement/SKILL.md` | Navicat 查询编辑器执行带参数的筛选查询失败、字段类型码不匹配时 |
| Navicat 变量快照 | `.trae/skills/szrsql-navicat-variables-snapshot/SKILL.md` | Navicat 工具栏按钮失效、事务状态错乱、字符集乱码时 |

## 本机数据库连接信息

| 数据库 | 连接串 |
|--------|--------|
| PostgreSQL 18 | `postgres://postgres:test123@127.0.0.1:5432/sz_orm_test` |
| MySQL 9.6 | `mysql://root:test123@127.0.0.1:3306/sz_orm_test` |
| Oracle 23ai | `127.0.0.1:1521/freepdb1` (sys/test123) |

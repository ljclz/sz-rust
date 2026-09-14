# 服务器真实数据全链路验证 — 编码任务分解

> **任务编号**：P0-1
> **基线版本**：sz-rust v0.6.7
> **生成日期**：2026-08-08
> **依据文档**：`spec.md` + `design.md`
> **目标服务器**：122.51.216.76（root，SSH 22，宝塔面板）
> **输出目录**：`docs/spec/production-validation/`

---

## 一、任务总览

| 维度 | 数值 |
|------|------|
| 里程碑数 | 4（M1 基础设施 / M2 验证模块 / M3 编排清理 / M4 集成交付） |
| 主任务数 | 16 |
| 子任务数 | 52 |
| 预计总耗时 | ≤ 10 分钟（编译 ~5min + 部署 ~1min + 验证 ~3min + 清理 ~1min） |
| 涉及需求覆盖 | spec 5.1-5.7 全部核心能力 + 4.1-4.5 全部 DFX 约束 |

---

## 二、任务依赖图

```
                  ┌─────────────────────────────────────────────┐
                  │              M1 基础设施层                  │
                  └─────────────────────────────────────────────┘
                                  │
        ┌─────────────┬──────────┼──────────┬─────────────┐
        ▼             ▼          ▼          ▼             ▼
       T1            T2         T3         T4            (并行)
   目录+配置      SSHOperator  LocalBuilder  EvidenceCollector
        │             │          │          │
        └─────────────┴──────────┴──────────┘
                                  │
                  ┌─────────────────────────────────────────────┐
                  │              M2 验证模块层                  │
                  └─────────────────────────────────────────────┘
                                  │
        ┌──────────┬──────────┬──────────┬──────────┬──────────┐
        ▼          ▼          ▼          ▼          ▼          ▼
       T5         T6         T7         T8         T9         T10
     MySQL       PgSQL      Redis      MQTT       Deploy      E2E
   (依赖T2)    (依赖T2)   (依赖T2)  (依赖T2)  (依赖T2,T3) (依赖T9)
                                  │
                  ┌─────────────────────────────────────────────┐
                  │            M3 编排与清理层                  │
                  └─────────────────────────────────────────────┘
                                  │
                          ┌───────┼───────┐
                          ▼       ▼       ▼
                         T11     T12     T13
                       Cleaner  Report   Orchestrator
                      (依赖T2) (依赖T4) (依赖全部)
                                  │
                  ┌─────────────────────────────────────────────┐
                  │            M4 集成验证与交付                │
                  └─────────────────────────────────────────────┘
                                  │
                          ┌───────┼───────┐
                          ▼       ▼       ▼
                         T14     T15     T16
                       端到端   报告审查  清理确认+
                       集成测试  证据校验  文档归档
                     (依赖T13) (依赖T14) (依赖T14)
```

**关键依赖关系**：
- T2（SSHOperator）是所有验证模块的基础设施依赖
- T9（DeployValidator）依赖 T3（LocalBuilder）产物
- T10（E2EValidator）依赖 T9 部署完成的进程
- T13（Orchestrator）依赖所有模块，是最终编排入口
- T14-T16 为串行交付链

---

## 三、里程碑与任务清单

# 里程碑 M1：基础设施层

> **目标**：搭建验证系统的 SSH 操作、本地编译、证据收集、配置加载四大基础设施。
> **验收**：四个模块可独立单元测试通过，SSH 可连接 122.51.216.76，cargo build 可产出 musl 静态产物。

## T1 创建验证脚本目录结构与配置文件

**描述**：按 design.md 2.6.1 节定义的目录结构创建验证脚本骨架，并生成 `validation-config.json` 配置文件，注入服务器地址、SSH 密钥路径、数据库凭证、应用端口、编译产物路径等配置项。

**输入**：
- design.md 2.6.1 节目录结构定义
- spec.md 6.2 节验证环境配置
- 服务器信息.md（MySQL 8802 / PG lewuli / Redis / MQTT iot.鲜视达.cn:8883）

**输出**：
- `docs/spec/production-validation/scripts/` 目录骨架（lib/ + validators/ + sql/）
- `docs/spec/production-validation/validation-config.json`
- `docs/spec/production-validation/scripts/package.json`（声明 ssh2 依赖）

**依赖**：无

**验收标准**：
- [ ] 1.1 创建 `scripts/`、`scripts/lib/`、`scripts/validators/`、`scripts/sql/` 四级目录
- [ ] 1.2 生成 `validation-config.json`，包含 server/mysql/postgresql/redis/mqtt/applications 六大配置段，所有取值对齐 design.md 2.1.2 节配置项表
- [ ] 1.3 MySQL 段包含 test/shop/njszjt 三个数据库凭证（用户名/密码/端口 8802），密码字段标注 `<<skip_serializing>>`
- [ ] 1.4 PostgreSQL 段配置 lewuli 库（用户 lewuli / 密码 JkbC2jsaWAYDe2Gz）
- [ ] 1.5 applications 段配置双应用：sz-pay-server（端口 8788，本地路径 `E:\vue\test\sz-pay\server\sz-rust`）+ sz-rust-sz300（端口 8300，本地路径 `packages/sz-rust-sz300`）
- [ ] 1.6 remoteDir 配置为 `/www/wwwroot/default`（spec 6.2.8）
- [ ] 1.7 szRustVersion=0.6.7，szOrmVersion=2.3.0
- [ ] 1.8 `scripts/package.json` 声明 ssh2 ^1.17.0 依赖，并配置 `"type": "module"` 支持 ES Module
- [ ] 1.9 配置文件中不出现明文 SSH 私钥内容，仅引用 `deploy_key` 路径

**预估复杂度**：低（纯配置，无逻辑）

---

## T2 实现 SSHOperator（SSH 操作封装模块）

**描述**：基于 Node.js `ssh2` 包封装 SSH 操作原语，提供 `execCommand` / `uploadFile` / `downloadFile` / `close` 四个方法，加载 `deploy_key` ED25519 密钥认证服务器 122.51.216.76。所有方法返回 Promise，错误统一映射为 `SSH_AUTH_FAILED` / `SSH_CONNECTION_TIMEOUT` / `EXEC_NONZERO_EXIT` 三类。

**输入**：
- design.md 2.2.2 节 SSHOperator 接口签名
- `E:\vue\test\鲜视达\rust\sz-rust\deploy_key` 密钥文件
- sz-pay `deploy.js:32-39` 存量 ssh2 用法参考

**输出**：
- `scripts/lib/ssh-operator.js`

**依赖**：T1（需要配置文件路径）

**验收标准**：
- [ ] 2.1 实现 `class SSHOperator`，构造函数接收 `{host, port, username, privateKeyPath}`，使用 `fs.readFileSync` 加载密钥并通过 `client.connect({privateKey})` 认证
- [ ] 2.2 实现 `async execCommand(command, options)`，返回 `{stdout, stderr, exitCode}`，命令执行超时默认 30s（可配置）
- [ ] 2.3 实现 `async uploadFile(localPath, remotePath)`，使用 `sftp.fastPut` 上传文件，上传前通过 `sftp.stat` 检查目标目录存在
- [ ] 2.4 实现 `async downloadFile(remotePath, localPath)`，使用 `sftp.fastGet` 下载文件
- [ ] 2.5 实现 `async close()`，依次关闭 sftp 和 client 连接，释放资源
- [ ] 2.6 错误映射：认证失败抛出 `SSH_AUTH_FAILED`（含 deploy_key 路径），连接超时抛出 `SSH_CONNECTION_TIMEOUT`，exitCode!==0 抛出 `EXEC_NONZERO_EXIT`（含命令与 stderr）
- [ ] 2.7 单元测试：连接 122.51.216.76 执行 `whoami` 返回 `root`，执行 `fuser 8788/tcp` 验证命令执行能力
- [ ] 2.8 禁止出现 `sshpass` 字符串，禁止使用 PowerShell 重定向（代码中不得出现 `powershell` / `cmd.exe` 调用）

**预估复杂度**：中（封装 ssh2，需处理异步连接与错误映射）

---

## T3 实现 LocalBuilder（本地编译模块）

**描述**：使用 `cargo build --release --target x86_64-unknown-linux-musl` 编译 sz-pay-server 与 sz-rust-sz300 两个 release 产物，设置 `CARGO_INCREMENTAL=0`（Windows 环境约束），编译失败捕获 stderr 输出并附 `Cargo.toml:23` 证据。

**输入**：
- design.md 2.7.2 节编译方案
- sz-pay 本地路径：`E:\vue\test\sz-pay\server\sz-rust`
- sz300 本地路径：`E:\vue\test\鲜视达\rust\sz-rust\packages\sz-rust-sz300`

**输出**：
- `scripts/lib/local-builder.js`
- 编译产物路径（sz-pay-server 与 sz-rust-sz300 二进制）

**依赖**：T1

**验收标准**：
- [ ] 3.1 实现 `async buildAll(applications)`，遍历应用配置依次编译，返回每个应用的产物路径
- [ ] 3.2 编译命令：`cargo build --release --target x86_64-unknown-linux-musl`，工作目录设置为应用 `localBinaryPath`
- [ ] 3.3 设置环境变量 `CARGO_INCREMENTAL=0`，通过 `child_process.spawn` 透传
- [ ] 3.4 编译超时设置为 600s（10 分钟，编译耗时较长）
- [ ] 3.5 编译失败时捕获完整 stderr，附 `Cargo.toml:23`（sz-rust-core 0.6.7 依赖行）作为 file:line 证据
- [ ] 3.6 编译成功后验证产物存在：sz-pay-server 产物在 `target/x86_64-unknown-linux-musl/release/sz-pay-server`，sz300 同理
- [ ] 3.7 单元测试：执行 `buildAll` 编译 sz-pay-server，验证产物文件存在且 size > 0
- [ ] 3.8 编译过程不修改任何 `../sz-orm/` 仓库文件（通过 git status 验证上游仓库无变更）

**预估复杂度**：中（cargo build 耗时较长，需正确处理 spawn 异步流）

---

## T4 实现 EvidenceCollector（file:line 证据收集模块）

**描述**：实现 file:line 证据收集与校验，每条证据包含 `{conclusion, file, line, verified}`，`verified` 字段在生成报告时通过读取源码文件验证对应行号真实存在。依据 project_rules.md 第 13 条，禁止仅声称"已通过"而无代码定位。

**输入**：
- design.md 2.3.2 节 Evidence 数据模型
- project_rules.md 第 13 条审计合规要求

**输出**：
- `scripts/lib/evidence-collector.js`

**依赖**：T1

**验收标准**：
- [ ] 4.1 实现 `createEvidence(conclusion, file, line)`，返回 `{conclusion, file, line, verified: false}` 对象
- [ ] 4.2 实现 `async verifyEvidence(evidence)`，读取 `evidence.file` 文件，验证 `evidence.line` 行号范围内有真实内容（非空行），更新 `verified` 字段
- [ ] 4.3 行号支持单行（"127"）和范围（"127-135"）两种格式
- [ ] 4.4 文件路径支持绝对路径与相对路径（相对于项目根 `E:\vue\test\鲜视达\rust\sz-rust`）
- [ ] 4.5 实现 `async verifyAll(evidences)`，批量校验所有证据，返回未通过校验的证据列表
- [ ] 4.6 单元测试：创建证据引用 `packages/sz-rust-sz300/src/db.rs:8-32`，验证 `verifyEvidence` 返回 `verified: true`；引用不存在行号返回 `verified: false`
- [ ] 4.7 校验失败的证据在报告中标注"证据行不存在"，不阻断验证流程但记录告警

**预估复杂度**：低（文件读取 + 行号校验）

---

# 里程碑 M2：验证模块层

> **目标**：实现 6 大验证模块（MySQL / PostgreSQL / Redis / MQTT / 部署 / 全链路端到端），每个模块通过 SSH 在服务器执行验证脚本，收集 file:line 证据。
> **验收**：每个模块可独立运行，输出 `ValidationResult`（passed + evidence + errors + duration）。

## T5 实现 MySQLValidator（MySQL 验证模块）

**描述**：通过 SSH 在服务器执行参数化 SQL 验证脚本，验证 MySQL 连接池初始化、CRUD 全操作、事务 commit/rollback、SQL 注入防护、连接池 20 并发无超时。验证数据使用独立临时表 `sz_validation_tmp`，验证后 DROP 清理。

**输入**：
- design.md 2.1.3.3 节 MySQL 验证流程
- spec.md 5.1 节 MySQL 集成验证业务规则
- MySQL 连接信息：122.51.216.76:8802，test/shop/njszjt 三个库

**输出**：
- `scripts/validators/mysql-validator.js`
- `scripts/sql/verify-mysql.sql`（SQL 模板）

**依赖**：T2（SSHOperator）

**验收标准**：
- [ ] 5.1 实现 `async validateMySQL(ssh, config)`，返回 `ValidationResult`（passed + evidence + errors + duration）
- [ ] 5.2 生成参数化 SQL 脚本上传到 `/tmp/verify_mysql_$$.sql`（`$$` 为 SSH 会话 PID，避免并发冲突）
- [ ] 5.3 连接池验证：通过 SSH 执行 `mysql -h 122.51.216.76 -P 8802 -u test -p*** test < verify.sql`，连接成功附 `db.rs:8-32` 证据
- [ ] 5.4 CRUD 验证：CREATE TABLE `sz_validation_tmp` → INSERT (1,'test_value') → SELECT val WHERE id=? → UPDATE → DELETE，每步校验数据一致性
- [ ] 5.5 事务验证：BEGIN→INSERT→COMMIT→SELECT 验证可见；BEGIN→INSERT→ROLLBACK→SELECT 验证不可见，回滚后无脏数据
- [ ] 5.6 SQL 注入防护验证：参数化查询 `WHERE username = ?` 输入 `"admin' OR '1'='1"`，验证返回 0 行，附 `db_integration_test.rs:256-270` 证据
- [ ] 5.7 连接池并发验证：并发 20 个 `SELECT 1` 请求，全部 30s 内完成，附 `db.rs:22`（acquire_timeout=30s）证据
- [ ] 5.8 禁止 `SELECT *`，所有查询使用显式列投影（如 `SELECT val FROM sz_validation_tmp`）
- [ ] 5.9 所有 WHERE 条件使用参数化绑定（`?` 占位符），禁止字符串拼接
- [ ] 5.10 验证完成后 DROP TABLE `sz_validation_tmp`，删除 `/tmp/verify_mysql_$$.sql`
- [ ] 5.11 对 test/shop/njszjt 三个库分别执行验证（每个库独立临时表）
- [ ] 5.12 异常映射：连接失败 `MYSQL_CONNECTION_FAILED`，数据不一致 `MYSQL_DATA_INCONSISTENT`，回滚失败 `MYSQL_TX_ROLLBACK_FAILED`，注入防护失效 `MYSQL_INJECTION_BYPASS`（严重，阻断验证）
- [ ] 5.13 密码通过 mysql `-p` 参数传递，但验证报告中密码以 `***` 脱敏呈现

**预估复杂度**：高（CRUD + 事务 + 注入防护 + 并发，SQL 脚本复杂）

---

## T6 实现 PostgreSQLValidator（PostgreSQL 验证模块）

**描述**：通过 SSH 在服务器执行 psql 验证脚本，验证 PostgreSQL 连接池初始化（max_size=10, min_idle=5）与 CRUD 全操作，验证 lewuli 库数据一致性。严禁修改上游 sz-orm 仓库任何 PostgreSQL 相关文件。

**输入**：
- design.md 2.1.2 节模块划分
- spec.md 5.2 节 PostgreSQL 集成验证业务规则
- PG 连接信息：lewuli 库，用户 lewuli，密码 JkbC2jsaWAYDe2Gz

**输出**：
- `scripts/validators/postgresql-validator.js`
- `scripts/sql/verify-postgres.sql`

**依赖**：T2（SSHOperator）

**验收标准**：
- [ ] 6.1 实现 `async validatePostgreSQL(ssh, config)`，返回 `ValidationResult`
- [ ] 6.2 生成 psql 验证脚本上传到 `/tmp/verify_pg_$$.sql`
- [ ] 6.3 连接池验证：`psql -h 127.0.0.1 -U lewuli -d lewuli -f verify.sql`，连接成功附 `db.rs:35-49` 证据
- [ ] 6.4 CRUD 验证：CREATE TABLE `sz_pg_validation_tmp` → INSERT → SELECT → UPDATE → DELETE，每步校验数据一致性
- [ ] 6.5 验证连接池配置 max_size=10, min_idle=5（50% 预热策略），附 `db.rs:44` 注释 P3-7 证据
- [ ] 6.6 禁止 `SELECT *`，使用显式列投影
- [ ] 6.7 所有 WHERE 条件参数化绑定（`$1` 占位符，PostgreSQL 风格）
- [ ] 6.8 验证完成后 DROP TABLE `sz_pg_validation_tmp`，删除 `/tmp/verify_pg_$$.sql`
- [ ] 6.9 验证完成后检查 `../sz-orm/` 仓库 `git status`，确认无任何文件变更（附 git status 输出作为证据）
- [ ] 6.10 异常映射：密码错误 `PG_AUTH_FAILED`（"password authentication failed"），连接失败 `PG_CONNECTION_FAILED`

**预估复杂度**：中（CRUD 验证，无事务/注入复杂度）

---

## T7 实现 RedisValidator（Redis 验证模块）

**描述**：通过 SSH 执行 redis-cli 脚本，验证 Redis 连接（PING/PONG）、SET/GET/DEL/EXPIRE/TTL 全套缓存操作、TTL 过期自动删除、分布式锁互斥性（SET NX EX 原子操作）。验证 key 统一使用 `sz_*_test` 前缀避免与生产 key 冲突。

**输入**：
- design.md 2.1.3.4 节 Redis 验证流程
- spec.md 5.3 节 Redis 集成验证业务规则
- sz-pay `Cargo.toml:83` redis 0.27 依赖证据

**输出**：
- `scripts/validators/redis-validator.js`

**依赖**：T2（SSHOperator）

**验收标准**：
- [ ] 7.1 实现 `async validateRedis(ssh, config)`，返回 `ValidationResult`
- [ ] 7.2 生成 Redis 验证脚本上传到 `/tmp/verify_redis_$$.sh`
- [ ] 7.3 连接验证：`redis-cli PING` 返回 `PONG`，连接失败附 `Cargo.toml:83` 证据
- [ ] 7.4 CRUD 验证：SET `sz_val_test "hello"` → GET 返回 "hello" → DEL → GET 返回 nil
- [ ] 7.5 TTL 过期验证：`SET sz_ttl_test "temp" EX 1` → TTL 返回 >0 → sleep 2 → GET 返回 nil
- [ ] 7.6 分布式锁互斥性验证：`SET sz_lock_test "owner_A" NX EX 10` 成功 → `SET sz_lock_test "owner_B" NX EX 10` 失败（返回 nil），互斥性失效标记严重违规 `REDIS_LOCK_MUTEX_FAILED`
- [ ] 7.7 验证 key 统一使用 `sz_*_test` 前缀，验证完成后 `DEL sz_val_test sz_ttl_test sz_lock_test` 清理
- [ ] 7.8 密码通过 `-a` 参数传递，验证报告中密码以 `***` 脱敏呈现
- [ ] 7.9 删除 `/tmp/verify_redis_$$.sh`
- [ ] 7.10 异常映射：Redis 不可达 `REDIS_CONNECTION_REFUSED`，OOM `REDIS_OOM`，互斥失效 `REDIS_LOCK_MUTEX_FAILED`（阻断验证）

**预估复杂度**：中（SET NX EX 原子操作 + TTL 时序）

---

## T8 实现 MQTTValidator（MQTT 验证模块）

**描述**：通过 SSH 使用 mosquitto_pub/sub 验证 MQTT Broker 连通性、发布/订阅消息一致性、QoS 1 至少送达一次、通配符 topic 路由。验证 topic 使用 `/sz/validation/*` 前缀与生产 topic `/sz/device/*` 隔离，不影响生产消息流。

**输入**：
- design.md 2.1.3.5 节 MQTT 验证流程
- spec.md 5.4 节 MQTT 集成验证业务规则
- MQTT Broker：`iot.鲜视达.cn:8883`（TLS）

**输出**：
- `scripts/validators/mqtt-validator.js`

**依赖**：T2（SSHOperator）

**验收标准**：
- [ ] 8.1 实现 `async validateMQTT(ssh, config)`，返回 `ValidationResult`
- [ ] 8.2 检查服务器 `mosquitto_clients` 是否安装，未安装则 `apt-get install -y mosquitto-clients`（临时安装）
- [ ] 8.3 连接 + 发布/订阅验证：`mosquitto_sub -h iot.鲜视达.cn -p 8883 --cafile xxx -t /sz/validation/test -C 1 -W 5 &` 后台订阅 → sleep 1 → `mosquitto_pub -m "hello_mqtt"` → 验证订阅方收到 "hello_mqtt"，附 `mqtt_service.rs:10-30` 证据
- [ ] 8.4 QoS 1 验证：`mosquitto_sub -q 1 -t /sz/validation/qos1 -C 1 -W 5` → `mosquitto_pub -q 1 -m "qos1_message"` → 验证至少收到一次
- [ ] 8.5 通配符 topic 路由验证：订阅 `/sz/device/+/status` → 发布到 `/sz/device/TEST001/status` → 验证通配符订阅收到消息，附 `mqtt_listener.rs:27-33` 证据
- [ ] 8.6 使用 `mosquitto_sub -C 1` 限制只接收 1 条消息后退出，避免进程残留
- [ ] 8.7 使用 `-W 5` 设置 5 秒超时，避免无限等待
- [ ] 8.8 验证 topic 使用 `/sz/validation/*` 前缀，与生产 topic `/sz/device/*` 隔离
- [ ] 8.9 验证完成后 `pkill -f mosquitto_sub`（仅终止验证专用进程，严禁 `killall webman`）
- [ ] 8.10 异常映射：Broker 不可达 `MQTT_CONNECTION_FAILED`（附 `mqtt_listener.rs:67` 证据），消息丢失 `MQTT_MESSAGE_LOST`

**预估复杂度**：中（mosquitto_sub 后台进程管理 + TLS cafile 配置）

---

## T9 实现 DeployValidator（双应用部署验证模块）

**描述**：复用并扩展 sz-pay `deploy.js` 模式，实现双应用部署（sz-pay-server + sz-rust-sz300），含隔离验证→上传→备份→fuser -k 精准终止→启动→健康检查→版本更新确认→内存验证（RSS ≤ 30MB）。部署目标路径 `/www/wwwroot/default`。

**输入**：
- design.md 2.1.3.2 节部署与进程管理流程
- spec.md 5.5 节部署与进程管理验证业务规则
- sz-pay `deploy.js:1-236` 存量部署脚本参考
- T3 编译产物路径

**输出**：
- `scripts/validators/deploy-validator.js`

**依赖**：T2（SSHOperator）、T3（LocalBuilder 产物）

**验收标准**：
- [ ] 9.1 实现 `async validateDeploy(ssh, config)`，返回 `DeployResult`（passed + processes + evidence + errors）
- [ ] 9.2 双应用循环部署：依次部署 sz-pay-server（端口 8788）与 sz-rust-sz300（端口 8300）
- [ ] 9.3 隔离验证：`fuser ${PORT}/tcp` 检查端口占用，被占用则 `fuser -k ${PORT}/tcp` 精准终止 → sleep 2 → 再次 `fuser` 验证已释放
- [ ] 9.4 端口释放失败时记录 `PORT_OCCUPIED` 错误并终止该应用部署
- [ ] 9.5 上传二进制：`sftp.fastPut` 上传到 `/www/wwwroot/default/${remoteBinaryName}`
- [ ] 9.6 备份策略：上传前备份旧版本到 `backup/${name}.bak.${timestamp}`，保留最近 5 个备份
- [ ] 9.7 `chmod +x` 赋予执行权限，`nohup ./${name} > ${name}.log 2>&1 &` 后台启动
- [ ] 9.8 健康检查：sleep 3 后 `curl -s http://127.0.0.1:${PORT}/health`，非 200 自动回滚到备份版本（复用 deploy.js:200-207 回滚逻辑）
- [ ] 9.9 版本更新确认：`ps -p ${PID} -o lstart` 查询启动时间，对比启动时间 > 部署开始时间
- [ ] 9.10 内存验证：`ps -p ${PID} -o rss` 查询空载 RSS，超过 30MB 记录 `RSS_EXCEED_30MB`（依据 project_rules.md 第 9 条）
- [ ] 9.11 记录每个应用的 `ProcessInfo`（name/pid/port/startedAt/rssBytes）
- [ ] 9.12 严禁使用 `killall` 或 `pkill webman`，仅允许 `fuser -k ${PORT}/tcp` 按端口精准终止
- [ ] 9.13 严禁使用 PowerShell 进行文件替换操作
- [ ] 9.14 异常映射：端口冲突 `PORT_OCCUPIED`，健康检查失败 `HEALTH_CHECK_FAILED`（自动回滚），内存超限 `RSS_EXCEED_30MB`

**预估复杂度**：高（双应用 + 备份 + 回滚 + 健康检查 + 版本确认 + 内存验证）

---

## T10 实现 E2EValidator（全链路端到端验证模块）

**描述**：通过 HTTP 请求触发 sz-pay 与 sz300 的完整调用路径，验证 HTTP→DB、HTTP→缓存、HTTP→MQ 全链路，以及错误传播链（无 JWT 返回 401）。每跳证据引用对应源码文件行号。

**输入**：
- design.md 2.1.3.6 节全链路端到端验证流程
- spec.md 5.6 节全链路端到端验证业务规则
- T9 部署完成的进程端口（8788 / 8300）

**输出**：
- `scripts/validators/e2e-validator.js`

**依赖**：T9（DeployValidator，进程须先启动）

**验收标准**：
- [ ] 10.1 实现 `async validateE2E(config)`，返回 `ValidationResult`
- [ ] 10.2 sz-pay HTTP→DB 全链路：`GET http://122.51.216.76:8788/health` 验证 200+status=ok（附 `health.rs:10-21` 证据）→ `GET /health/ready` 验证 200+db=ok（附 `health.rs:32-60` 证据）
- [ ] 10.3 sz300 HTTP→DB 全链路：`GET http://122.51.216.76:8300/health` + `GET /health/ready` 验证 DB 探活通过（附 `health_service.rs:24-38` 证据，覆盖路由→中间件→控制器→service→ORM→DB 完整路径）
- [ ] 10.4 sz-pay HTTP→缓存全链路：`POST /api/v1/auth/login` 触发 Redis 缓存读写，验证响应含 token
- [ ] 10.5 错误传播链验证：`POST http://122.51.216.76:8300/api/v1/merchant/list` 无 JWT token，验证响应 401（附 `router.rs:92` 中间件拦截证据）
- [ ] 10.6 HTTP 请求超时设置为 5s（依据 project_rules.md 第 5 条外部 IO 超时兜底）
- [ ] 10.7 每跳验证结论附 file:line 证据，证据引用真实存在的源码行
- [ ] 10.8 file:line 证据完整性校验：审查验证报告每条结论均有 file:line 引用且对应文件行存在
- [ ] 10.9 异常映射：响应非 200 `HTTP_NON_200`，超时 `HTTP_TIMEOUT`，证据缺失 `EVIDENCE_MISSING`

**预估复杂度**：中（HTTP 请求 + 响应校验 + 证据收集）

---

# 里程碑 M3：编排与清理层

> **目标**：实现统一清理、报告生成、主入口编排三大模块，串联所有验证模块按状态机顺序执行。
> **验收**：`node orchestrator.js --config validation-config.json` 可完整执行验证流程，输出 `validation-report.md`。

## T11 实现 Cleaner（统一清理模块）

**描述**：实现统一清理模块，在验证流程末尾强制执行（无论成功/失败），清理服务器测试脚本、生产测试数据、测试进程、本地临时文件。清理确认记录包含每项产物的删除状态。

**输入**：
- design.md 2.1.3.7 节清理流程
- spec.md 5.7 节验证产物清理业务规则
- 验证过程中持续追加的 `ArtifactInventory`

**输出**：
- `scripts/lib/cleaner.js`

**依赖**：T2（SSHOperator）

**验收标准**：
- [ ] 11.1 实现 `async cleanAll(ssh, artifacts)`，返回 `CleanResult`（cleaned + failed）
- [ ] 11.2 服务器测试脚本清理：`rm -f /tmp/verify_mysql_*.sql /tmp/verify_pg_*.sql /tmp/verify_redis_*.sh /tmp/verify_mqtt_*.sh` → `ls /tmp/verify_*` 确认无残留
- [ ] 11.3 生产测试数据清理：`mysql DROP TABLE sz_validation_tmp`（test/shop/njszjt 三库）+ `redis-cli DEL sz_val_test sz_ttl_test sz_lock_test` + `psql DROP TABLE sz_pg_validation_tmp`
- [ ] 11.4 测试进程释放：`pkill -f mosquitto_sub`（仅验证专用进程）+ `fuser 8788/tcp 8300/tcp` 确认仅目标进程
- [ ] 11.5 本地临时文件清理：删除本地编译临时文件、本地验证日志
- [ ] 11.6 严禁 `killall webman` 或 `pkill webman`，仅 `pkill -f mosquitto_sub`（验证专用）
- [ ] 11.7 清理失败项记录 `{artifact, reason, needsManualIntervention: true}`，不阻断清理流程
- [ ] 11.8 生成清理确认记录，含每项产物的删除状态（已删除/未删除 + 原因）
- [ ] 11.9 清理模块在验证流程末尾**强制执行**，即使验证失败也必须执行清理
- [ ] 11.10 异常映射：权限不足 `CLEAN_PERMISSION_DENIED`（记录需人工介入）

**预估复杂度**：中（多类产物清理 + 状态记录）

---

## T12 实现 ReportGenerator（报告生成模块）

**描述**：按 spec.md 6.1 节报告结构生成 Markdown 格式验证报告，包含报告版本、验证时间、服务器地址、7 大类验证项结论、每项 file:line 证据、错误详情。生成时校验每条 Evidence 的 file:line 真实存在。

**输入**：
- design.md 2.3.1 节报告格式方案
- spec.md 6.1 节验证报告数据约束
- T4 EvidenceCollector（校验证据）
- 所有验证模块的 `ValidationResult`

**输出**：
- `scripts/lib/report-generator.js`
- `docs/spec/production-validation/validation-report.md`（运行时生成）

**依赖**：T4（EvidenceCollector 校验证据）

**验收标准**：
- [ ] 12.1 实现 `async generateReport(moduleResults, cleanResult)`，返回报告文件路径
- [ ] 12.2 报告头部：标注基于 sz-rust v0.6.7、验证起止时间戳、目标服务器 IP 122.51.216.76
- [ ] 12.3 报告主体：7 大类验证项（DB-MySQL / DB-PostgreSQL / 缓存-Redis / MQ-MQTT / 部署 / 全链路 / 清理），每项结论为"通过/失败"二值（禁止"可能/大概"）
- [ ] 12.4 每条结论附 file:line 证据，调用 `EvidenceCollector.verifyAll` 校验证据行真实存在，未通过校验的证据标注"证据行不存在"
- [ ] 12.5 失败项包含错误消息、堆栈、复现步骤
- [ ] 12.6 密码字段以 `***` 脱敏呈现（DatabaseCredential.password / Redis 密码 / MQTT 凭证）
- [ ] 12.7 报告尾部：清理确认记录（每项产物的删除状态）+ 整体结论（可上生产/不可上生产 + 阻断项清单）
- [ ] 12.8 报告文件大小 ≤ 100KB
- [ ] 12.9 报告输出到 `docs/spec/production-validation/validation-report.md`
- [ ] 12.10 报告格式为 Markdown，支持表格/代码块/链接，便于人工阅读

**预估复杂度**：中（模板组装 + 证据校验）

---

## T13 实现 Orchestrator（主入口编排模块）

**描述**：实现验证流程主入口，按 design.md 2.1.3.1 节状态机顺序编排所有模块：编译→部署→MySQL→PG→Redis→MQTT→E2E→清理→报告。任何阶段失败均不立即终止，转入 Cleaning 阶段确保产物清理，最终报告标注失败项。

**输入**：
- design.md 2.1.3.1 节验证流程状态机
- design.md 2.6.2 节执行流程
- 所有验证模块（T5-T10）+ 清理模块（T11）+ 报告模块（T12）

**输出**：
- `scripts/orchestrator.js`

**依赖**：T2、T3、T4、T5、T6、T7、T8、T9、T10、T11、T12（全部模块）

**验收标准**：
- [ ] 13.1 实现 `async runValidation(configPath)`，返回 `FinalReport`（overallPassed + moduleResults + reportPath + cleanResult + startedAt + finishedAt）
- [ ] 13.2 命令行接口：`node orchestrator.js --config validation-config.json`，解析 `--config` 参数加载配置
- [ ] 13.3 状态机顺序执行：Building → Deploying → ValidatingMySQL → ValidatingPg → ValidatingRedis → ValidatingMQTT → ValidatingE2E → Cleaning → Completed
- [ ] 13.4 每个阶段执行前记录状态转换日志（含时间戳）
- [ ] 13.5 单个模块失败不终止整体验证（除 SQL 注入防护失效、分布式锁互斥失效等严重违规），继续执行后续模块以收集完整结论
- [ ] 13.6 任何失败路径均转入 Cleaning 阶段，确保产物不残留
- [ ] 13.7 严重违规（SQL 注入防护失效 / 分布式锁互斥失效）立即终止验证，转入 Cleaning
- [ ] 13.8 验证全程耗时 ≤ 10 分钟
- [ ] 13.9 编排进程内存占用 ≤ 100MB
- [ ] 13.10 最终调用 `ReportGenerator.generateReport` 生成报告，输出报告路径
- [ ] 13.11 验证完成后关闭 SSH 连接（调用 `ssh.close()`），释放资源
- [ ] 13.12 整体结论：所有模块通过 → "可上生产"；任一失败 → "不可上生产" + 阻断项清单

**预估复杂度**：中（状态机编排 + 错误处理）

---

# 里程碑 M4：集成验证与交付

> **目标**：在真实服务器执行完整验证流程，审查报告完整性与证据准确性，确认产物清理完成并归档文档。
> **验收**：`validation-report.md` 生成且所有结论附真实 file:line 证据，服务器与本地无残留产物。

## T14 端到端集成测试（真实服务器执行完整验证流程）

**描述**：在真实服务器 122.51.216.76 上执行 `node orchestrator.js --config validation-config.json`，验证完整流程可跑通，产出 `validation-report.md`。

**输入**：
- T13 Orchestrator 主入口
- `validation-config.json` 配置文件
- 真实服务器 122.51.216.76 + deploy_key 密钥

**输出**：
- 验证执行日志
- `validation-report.md` 报告文件

**依赖**：T13（Orchestrator）

**验收标准**：
- [ ] 14.1 执行 `cd scripts && node orchestrator.js --config ../validation-config.json`，验证命令成功执行（exitCode=0）
- [ ] 14.2 验证 `validation-report.md` 文件已生成且大小 > 0
- [ ] 14.3 验证报告包含 7 大类验证项结论（DB-MySQL / DB-PostgreSQL / 缓存-Redis / MQ-MQTT / 部署 / 全链路 / 清理）
- [ ] 14.4 验证报告每条结论均有 file:line 证据，且证据行真实存在（人工抽查 5 条证据）
- [ ] 14.5 验证报告无"可能/大概"等模糊结论，均为"通过/失败"二值
- [ ] 14.6 验证报告密码字段均以 `***` 脱敏呈现，无明文密码
- [ ] 14.7 验证全程耗时 ≤ 10 分钟
- [ ] 14.8 验证期间未误杀其他 webman 项目进程（部署前后对比 `ps aux | grep webman` 输出一致）
- [ ] 14.9 验证期间未修改 `../sz-orm/` 仓库任何文件（`git -C ../sz-orm status` 输出无变更）

**预估复杂度**：高（真实环境执行，需排查运行时问题）

---

## T15 验证报告审查与 file:line 证据完整性校验

**描述**：人工审查 `validation-report.md` 报告，校验每条结论的 file:line 证据真实存在且引用准确，确认失败项的错误详情与复现步骤完整。

**输入**：
- T14 生成的 `validation-report.md`
- T4 EvidenceCollector（批量校验证据）

**输出**：
- 报告审查记录（含证据校验结果）
- 失败项跟进清单（如有）

**依赖**：T14

**验收标准**：
- [ ] 15.1 调用 `EvidenceCollector.verifyAll` 批量校验报告中所有 file:line 证据，输出未通过校验的证据列表
- [ ] 15.2 人工抽查 10 条证据，确认引用的源码文件行号真实存在且与结论语义匹配
- [ ] 15.3 失败项审查：每条失败项包含错误消息、堆栈、复现步骤，缺一项标注不完整
- [ ] 15.4 整体结论审查：所有模块通过 → "可上生产"；任一失败 → "不可上生产" + 阻断项清单
- [ ] 15.5 阻断项清单审查：每个阻断项有具体错误描述 + file:line 证据 + 修复建议
- [ ] 15.6 报告格式审查：Markdown 格式正确，表格/代码块/链接渲染正常
- [ ] 15.7 报告大小 ≤ 100KB
- [ ] 15.8 审查结果记录到 `docs/spec/production-validation/report-review.md`

**预估复杂度**：低（人工审查 + 证据校验）

---

## T16 产物清理确认与文档归档

**描述**：确认验证产物已全部清理（服务器测试脚本、本地临时文件、测试进程、生产测试数据），将验证报告与相关文档归档到 `docs/spec/production-validation/` 目录。

**输入**：
- T14 验证执行日志
- T11 Cleaner 清理确认记录
- `validation-report.md` 报告文件

**输出**：
- 清理确认记录
- 归档文档清单

**依赖**：T14

**验收标准**：
- [ ] 16.1 服务器测试脚本清理确认：SSH 执行 `ls /tmp/verify_*` 返回空，无残留脚本
- [ ] 16.2 本地临时文件清理确认：检查本地临时目录无验证产生的临时文件
- [ ] 16.3 测试进程释放确认：SSH 执行 `pgrep -f mosquitto_sub` 返回空，`fuser 8788/tcp 8300/tcp` 确认目标进程状态符合预期
- [ ] 16.4 生产测试数据清理确认：SSH 执行 `mysql -e "SHOW TABLES LIKE 'sz_validation_tmp'"` 返回空，`redis-cli EXISTS sz_val_test sz_ttl_test sz_lock_test` 全部返回 0，`psql -c "\dt sz_pg_validation_tmp"` 返回空
- [ ] 16.5 未清理项标注原因（如权限不足），提示人工介入
- [ ] 16.6 文档归档：`validation-report.md` + `validation-config.json` + `report-review.md`（如有）归档到 `docs/spec/production-validation/`
- [ ] 16.7 归档文档清单记录到 `docs/spec/production-validation/archive-inventory.md`
- [ ] 16.8 验证完成后服务器上其他 webman 项目进程正常运行（`ps aux | grep webman` 数量与验证前一致）
- [ ] 16.9 验证完成后 `../sz-orm/` 仓库无任何文件变更（`git -C ../sz-orm status` 输出 "nothing to commit"）

**预估复杂度**：低（清理确认 + 文档归档）

---

## 四、需求覆盖矩阵

| spec.md 需求章节 | 对应任务 | 覆盖状态 |
|-----------------|---------|---------|
| 5.1 MySQL 集成验证 | T5 | ✅ 完整覆盖 |
| 5.2 PostgreSQL 集成验证 | T6 | ✅ 完整覆盖 |
| 5.3 Redis 集成验证 | T7 | ✅ 完整覆盖 |
| 5.4 MQTT 集成验证 | T8 | ✅ 完整覆盖 |
| 5.5 部署与进程管理验证 | T9 | ✅ 完整覆盖 |
| 5.6 全链路端到端验证 | T10 | ✅ 完整覆盖 |
| 5.7 验证产物清理 | T11、T16 | ✅ 完整覆盖 |
| 4.1 性能（连接耗时/CRUD/并发/内存） | T5、T9 | ✅ 完整覆盖 |
| 4.2 可靠性（自愈/回滚/存活/池对齐） | T5、T9、T10 | ✅ 完整覆盖 |
| 4.3 安全性（脱敏/SSH/参数化/禁 SELECT */上游只读） | T2、T5、T6、T9、T12 | ✅ 完整覆盖 |
| 4.4 可维护性（结构化报告/产物清理/精准管理） | T11、T12、T16 | ✅ 完整覆盖 |
| 4.5 兼容性（sz-rust/sz-orm/SQLx 版本对齐） | T1、T3 | ✅ 完整覆盖 |
| 6.1 验证报告数据约束 | T12、T15 | ✅ 完整覆盖 |
| 6.2 验证环境配置 | T1 | ✅ 完整覆盖 |
| 6.3 进程状态 | T9 | ✅ 完整覆盖 |
| 6.4 清理确认记录 | T11、T16 | ✅ 完整覆盖 |

---

## 五、关键约束清单（贯穿所有任务）

| 约束项 | 要求 | 依据 | 责任任务 |
|--------|------|------|---------|
| SSH 连接方式 | 必须使用 ssh2 包加载 deploy_key 密钥 | spec 5.5.1.1 / session-rules | T2 |
| 禁止 sshpass | 不得出现 sshpass 命令 | session-rules 部署方式 | T2 |
| 禁止 PowerShell 替换 | 不得使用 PowerShell 进行文件替换（破坏 UTF-8） | spec 5.5.1.6 | T2、T9 |
| 进程精准终止 | 仅使用 `fuser -k ${PORT}/tcp`，禁止 killall | spec 5.5.1.4 | T9、T11 |
| 参数化查询 | 所有 SQL 使用 `?` 占位符，禁止字符串拼接 | AGENTS.md 关键约束 | T5、T6 |
| 禁止 SELECT * | 使用显式列投影 | AGENTS.md 关键约束 | T5、T6 |
| 密码脱敏 | 报告中密码字段以 `***` 呈现 | spec 4.3.1 / project_rules.md 第 7 条 | T5、T6、T7、T12 |
| 上游仓库只读 | 严禁修改 `../sz-orm/` 任何文件 | spec 4.3.5 / AGENTS.md | T3、T6、T14、T16 |
| 产物清理 | 验证完成后删除所有上传脚本/临时文件/测试进程 | spec 5.7.1.5 | T11、T16 |
| CARGO_INCREMENTAL=0 | Windows 环境设为 0 | 关键约束 | T3 |
| file:line 证据 | 每条结论附源码文件路径与行号，且该行真实存在 | project_rules.md 第 13 条 | T4、T12、T15 |
| 不误杀 webman | fuser 按端口精准定位，不影响其他项目 | spec 5.5.1.4 / session-rules | T9、T11、T16 |
| 测试数据独立 | 验证数据使用独立临时表/key/topic，不污染生产 | spec 5.7 / design 2.3.1 | T5、T6、T7、T8 |
| async fn Send + 'static | 所有 async 函数满足 Send + 'static | AGENTS.md 统一约束 | T3（Rust 编译侧） |
| 禁止 std::fs | 统一使用 tokio::fs | AGENTS.md 统一约束 | T3（Rust 编译侧） |

---

## 六、风险与缓解

| 风险 | 影响 | 缓解措施 | 责任任务 |
|------|------|---------|---------|
| 编译耗时超 10 分钟 | M1 阻塞 | T3 设置 600s 超时，编译失败附完整 stderr | T3 |
| SSH 连接不稳定 | 全流程阻塞 | T2 实现连接重试（最多 3 次，间隔 5s） | T2 |
| 服务器 mosquitto-clients 未安装 | T8 阻塞 | T8 自动 `apt-get install -y mosquitto-clients` | T8 |
| 端口 8788/8300 被其他进程占用 | T9 部署失败 | T9 隔离验证 + fuser -k 精准终止 + 端口释放验证 | T9 |
| 误杀其他 webman 进程 | 生产事故 | T9/T11 严禁 killall，仅 fuser -k 按端口；T14/T16 部署前后对比 webman 进程 | T9、T11、T14、T16 |
| 验证数据污染生产 | 数据污染 | T5/T6/T7/T8 使用独立临时表/key/topic，验证后清理 | T5、T6、T7、T8、T11 |
| file:line 证据行不存在 | 审计无效 | T4 EvidenceCollector 校验证据行真实存在；T15 人工抽查 | T4、T12、T15 |
| MQTT Broker 不可达 | T8 失败 | T8 使用 `-W 5` 超时，失败标记 `MQTT_CONNECTION_FAILED` 不阻断其他模块 | T8 |
| Redis 内存不足 | T7 失败 | T7 捕获 OOM 错误，标记 `REDIS_OOM` 不阻断其他模块 | T7 |

---

> 本任务分解文档基于 `spec.md` 需求规格与 `design.md` 技术设计生成，所有任务均映射到具体需求章节，所有约束均附 spec/project_rules 依据。
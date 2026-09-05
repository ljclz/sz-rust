# 服务器真实数据全链路验证 — 需求规格说明书

> **任务编号**：P0-1
> **优先级**：P0（最高）
> **来源**：项目评估报告 `docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md` 第十章"后续发展方向建议"
> **基线版本**：sz-rust v0.6.7（crates.io 26/26 发布成功）
> **验证对象**：sz-pay（支付中台）+ sz-rust-sz300（设备/商户/商品/订单管理）
> **目标服务器**：122.51.216.76（宝塔面板环境）
> **编写日期**：2026-08-08

---

# **1. 组件定位**

## **1.1 核心职责**

本组件负责在真实生产服务器环境下，端到端验证 sz-rust 框架 v0.6.7 通过 sz-pay 与 sz-rust-sz300 两个业务应用连接生产数据库、缓存、消息队列的全链路集成能力，产出可验证的通过/失败结论。

## **1.2 核心输入**

1. **sz-pay 编译产物**：基于 sz-rust-core 0.6.7 编译的支付中台二进制（来源：本地 `E:\vue\test\sz-pay\server\sz-rust` 编译）
2. **sz-rust-sz300 编译产物**：基于 sz-rust-core 0.6.7 编译的设备管理业务二进制（来源：workspace `packages/sz-rust-sz300` 编译）
3. **生产服务器连接凭证**：SSH 密钥（来源：`deploy_key` 文件）、服务器地址 122.51.216.76
4. **生产数据库连接信息**：MySQL（端口 8802，test/shop/njszjt 库）、PostgreSQL（lewuli 库）、Redis（来源：`服务器信息.md`）
5. **环境变量配置**：`SZ300_DB_PASSWORD`、`SZ300_PG_PASSWORD` 等数据库密码（来源：环境变量注入）
6. **验证触发指令**：人工发起的全链路验证执行命令

## **1.3 核心输出**

1. **全链路验证报告**：包含 DB/缓存/MQ 各项集成的通过/失败状态、错误详情、file:line 证据（目标：`docs/spec/production-validation/validation-report.md`）
2. **进程状态确认结果**：sz-pay-server 与 sz-rust-sz300 进程在服务器上的运行状态、监听端口、PID
3. **数据一致性验证结果**：写入生产数据库的数据与读出数据的比对结论
4. **清理确认记录**：测试脚本、临时文件、测试进程的清理完成确认
5. **生产就绪结论**：基于验证结果给出的"可上生产/不可上生产"判定及阻断项清单

## **1.4 职责边界**

本组件**不负责**以下事项：

1. **不负责** sz-rust 框架代码的修改或功能补全 — 仅验证现有 v0.6.7 能力
2. **不负责** 上游 `../sz-orm/` 仓库任何文件的修改 — 严禁触碰
3. **不负责** 渗透测试 — 那是 P0-3 的职责
4. **不负责** 性能压测与基准对比 — 那是 P2-9 的职责
5. **不负责** 长时间 Soak 测试 — 那是 P4-18 的职责
6. **不负责** 生产数据的持久化维护 — 验证产生的测试数据须在验证完成后清理
7. **不负责** 误杀其他 webman 项目进程 — 须严格按端口精准定位目标进程

---

# **2. 领域术语**

**全链路验证**
: 从 HTTP 请求入口经路由/中间件/控制器/ORM 到达数据库/缓存/消息队列，再原路返回响应的完整调用路径的端到端验证，要求每一跳均有真实数据交互证据。

**DB 集成**
: sz-rust 框架通过 sz-orm-sqlx 连接池与生产 MySQL/PostgreSQL 建立连接，执行建表、插入、查询、更新、删除、事务提交/回滚等操作的能力验证。

**缓存集成**
: sz-rust 框架通过 sz-rust-cache-facade 的 Redis 驱动与生产 Redis 实例建立连接，执行 SET/GET/DEL/EXPIRE/TTL 等操作的能力验证。

**MQ 集成**
: sz-rust-sz300 通过 sz-orm-mqtt 与 MQTT Broker 建立连接，执行发布/订阅/消息接收的能力验证。

**连接池双层架构**
: sz-rust-sz300 采用 SQLx 原生池（max_connections=20）+ sz-orm Pool（max_size=20, min_idle=10）的双层连接池设计，两层容量须对齐以避免并发 acquire 超时。

**进程精准定位**
: 通过 `fuser -k <端口>/tcp` 按端口杀进程的方式，确保仅终止目标应用进程，不误杀同服务器上其他 webman 项目进程。

**file:line 证据**
: 每条验证结论必须附带源码文件路径与行号引用，且该文件行必须真实存在，禁止仅声称"已通过"而无代码定位。

**验证产物清理**
: 验证过程中上传到服务器的测试脚本、写入本地硬盘的临时文件、启动的测试进程，在验证完成后必须全部删除/释放。

---

# **3. 角色与边界**

## **3.1 核心角色**

- **验证执行者**：发起全链路验证命令、收集验证结果、判定通过/失败的人工操作角色
- **运维管理员**：拥有生产服务器 root 权限，负责确认进程部署、端口监听、资源占用的角色

## **3.2 外部系统**

- **生产服务器（122.51.216.76）**：宝塔面板环境，承载 sz-pay-server 与 sz-rust-sz300 进程运行
- **生产 MySQL（端口 8802）**：承载 test/shop/njszjt 数据库，验证 DB 读写能力
- **生产 PostgreSQL**：承载 lewuli 数据库，验证 PostgreSQL 连接池能力
- **生产 Redis**：承载分布式锁与缓存数据，验证缓存读写与 TTL 能力
- **MQTT Broker**：承载消息发布/订阅，验证 MQ 集成能力
- **sz-orm 仓库（上游）**：提供 ORM 能力，本验证严禁修改其任何文件

## **3.3 交互上下文**

```plantuml
@startuml
skinparam componentStyle rectangle

actor "验证执行者" as Validator
actor "运维管理员" as Operator

rectangle "全链路验证系统" as ValidationSystem {
}

cloud "生产服务器\n122.51.216.76" as Server
database "MySQL\n(端口8802)" as MySQL
database "PostgreSQL\n(lewuli)" as PgSQL
database "Redis" as RedisDB
queue "MQTT Broker" as MQTT
rectangle "sz-orm 上游仓库\n(只读)" as SzOrm

Validator --> ValidationSystem : 发起验证指令
ValidationSystem --> Server : SSH 部署/启动进程 (ssh2)
ValidationSystem --> MySQL : 连接池/CRUD/事务验证
ValidationSystem --> PgSQL : 连接池/CRUD 验证
ValidationSystem --> RedisDB : SET/GET/DEL/TTL 验证
ValidationSystem --> MQTT : 发布/订阅验证
ValidationSystem --> SzOrm : 只读引用 (严禁修改)
ValidationSystem --> Validator : 验证报告 + file:line 证据
Operator --> Server : 确认进程/端口/资源

@enduml
```

---

# **4. DFX约束**

## **4.1 性能**

1. **连接建立耗时**：WHEN 验证系统发起数据库连接池初始化 THEN THE SYSTEM SHALL 在 10 秒内完成连接池建立（依据 `db.rs:28` connection_timeout = 10s）
2. **单次 CRUD 响应**：WHEN 执行单条 SQL 的 INSERT/SELECT/UPDATE/DELETE THEN THE SYSTEM SHALL 在 5 秒内返回结果（依据 project_rules.md 第 5 条外部 IO 超时兜底 5s）
3. **连接池容量**：WHEN 并发请求数 ≤ 20 THEN THE SYSTEM SHALL 不出现连接池 acquire 超时（依据 `db.rs:20-27` max_connections=20, max_size=20）
4. **进程启动内存**：WHEN sz-pay-server 或 sz-rust-sz300 进程启动完成 THEN THE SYSTEM SHALL 空载 RSS 不超过 30MB（依据 project_rules.md 第 9 条）

## **4.2 可靠性**

1. **连接池自愈**：WHEN 数据库连接因网络抖动断开 THEN THE SYSTEM SHALL 自动重建连接且后续请求正常返回
2. **事务回滚**：WHEN 事务中任一 SQL 执行失败 THEN THE SYSTEM SHALL 回滚全部已执行操作且数据库无脏数据残留
3. **进程存活**：WHEN 验证期间持续请求健康检查端点 THEN THE SYSTEM SHALL 全程返回 200 状态码
4. **双层池容量对齐**：WHEN sz-orm Pool max_size=20 THEN THE SYSTEM SHALL SQLx max_connections 同样为 20（依据 `db.rs:20` 注释，避免第 11 个并发 acquire 超时）

## **4.3 安全性**

1. **密码不明文暴露**：WHEN 日志输出数据库连接信息 THEN THE SYSTEM SHALL 密码字段以 `***` 脱敏呈现（依据 project_rules.md 第 7 条）
2. **SSH 密钥安全**：WHEN 执行 SSH 操作 THEN THE SYSTEM SHALL 使用 Node.js ssh2 包加载密钥，禁止 sshpass 或 PowerShell 重定向（依据 session-rules 部署方式约束）
3. **参数化查询**：WHEN 执行任何带 WHERE 条件的 SQL THEN THE SYSTEM SHALL 使用参数化绑定，禁止字符串拼接（依据 AGENTS.md 关键约束）
4. **禁止 SELECT \***：WHEN 执行查询操作 THEN THE SYSTEM SHALL 使用显式列投影，禁止 `SELECT *`（依据 AGENTS.md 关键约束）
5. **上游仓库只读**：WHEN 验证过程涉及 sz-orm 代码 THEN THE SYSTEM SHALL 不修改 `../sz-orm/` 下任何文件

## **4.4 可维护性**

1. **验证报告结构化**：WHEN 验证完成 THEN THE SYSTEM SHALL 输出包含每项验证的通过/失败状态、错误详情、file:line 证据的结构化报告
2. **产物可清理**：WHEN 验证结束 THEN THE SYSTEM SHALL 所有上传到服务器的测试脚本、本地临时文件、测试进程均已删除/释放
3. **进程精准管理**：WHEN 需要终止目标进程 THEN THE SYSTEM SHALL 使用 `fuser -k <端口>/tcp` 按端口杀进程，不误杀其他 webman 项目进程

## **4.5 兼容性**

1. **sz-rust 版本对齐**：WHEN sz-pay 与 sz-rust-sz300 编译 THEN THE SYSTEM SHALL 两者均依赖 sz-rust-core 0.6.7（依据 `sz-pay/server/sz-rust/Cargo.toml:23`）
2. **sz-orm 版本对齐**：WHEN sz-pay 引用 sz-orm THEN THE SYSTEM SHALL 统一使用 2.3.0 版本（依据 `sz-pay/server/sz-rust/Cargo.toml:27-33`）
3. **SQLx 版本对齐**：WHEN sz-pay 使用 sqlx THEN THE SYSTEM SHALL 版本与 sz-orm-sqlx 依赖的 sqlx 版本一致（依据 `sz-pay/server/sz-rust/Cargo.toml:64` 注释）

---

# **5. 核心能力**

## **5.1 数据库集成验证（MySQL）**

### **5.1.1 业务规则**

1. **连接池初始化**：验证系统必须通过环境变量 `SZ300_DB_PASSWORD` 获取数据库密码，构建 `mysql://user:password@host:8802/database` 连接串，初始化 SQLx 池（max_connections=20）与 sz-orm Pool（max_size=20, min_idle=10）双层连接池。
   a. 验收条件：WHEN 设置正确的 MySQL 环境变量并启动应用 THEN THE SYSTEM SHALL 连接池初始化成功且日志显示 "pool initialized" 无错误

2. **CRUD 全操作**：验证系统必须对生产 MySQL 执行完整的 INSERT/SELECT/UPDATE/DELETE 操作，且每条操作的写入数据与读出数据一致。
   a. 验收条件：WHEN 向 test/shop/njszjt 库执行 CRUD 操作 THEN THE SYSTEM SHALL 每条操作返回成功且数据一致性校验通过

3. **事务验证**：验证系统必须执行事务提交与回滚两种路径，回滚路径下数据库无脏数据残留。
   a. 验收条件：WHEN 执行事务回滚路径 THEN THE SYSTEM SHALL 数据库中无事务中已写入的临时数据

4. **连接池并发**：验证系统必须在 20 并发请求下不出现连接池 acquire 超时。
   a. 验收条件：WHEN 20 个并发请求同时访问数据库 THEN THE SYSTEM SHALL 全部请求在 30 秒内完成（依据 `db.rs:22` acquire_timeout=30s）

5. **禁止项**：禁止使用 `SELECT *` 查询，禁止字符串拼接 WHERE 条件。
   a. 验收条件：WHEN 审查验证期间所有 SQL THEN THE SYSTEM SHALL 不出现 `SELECT *` 且所有 WHERE 条件均经参数化绑定

### **5.1.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
database "生产 MySQL\n(端口8802)" as DB

V -> VS : 发起 MySQL 集成验证
VS -> VS : 读取环境变量 SZ300_DB_*
VS -> DB : 建立双层连接池 (SQLx + sz-orm Pool)
DB --> VS : 连接池就绪
VS -> DB : INSERT 测试数据
DB --> VS : 写入成功
VS -> DB : SELECT 验证数据
DB --> VS : 返回数据
VS -> VS : 数据一致性校验
VS -> DB : UPDATE 修改数据
DB --> VS : 更新成功
VS -> DB : DELETE 清理数据
DB --> VS : 删除成功
VS -> DB : 事务提交/回滚验证
DB --> VS : 事务结果
VS --> V : MySQL 验证报告 + file:line 证据
@enduml
```

### **5.1.3 异常场景**

1. **数据库密码错误**
   a. 触发条件：环境变量 `SZ300_DB_PASSWORD` 与生产数据库实际密码不匹配
   b. 系统行为：连接池初始化返回认证失败错误，进程不启动
   c. 用户感知：错误日志显示 "Access denied for user" 且附带 file:line 定位

2. **数据库不可达**
   a. 触发条件：生产 MySQL 端口 8802 未监听或网络不通
   b. 系统行为：连接池在 10 秒超时后返回连接失败错误
   c. 用户感知：错误日志显示 "Connection refused" 或 "timeout" 且附带 file:line 定位

3. **连接池耗尽**
   a. 触发条件：并发请求数超过 20 且连接未及时释放
   b. 系统行为：第 21 个请求在 30 秒后返回 acquire 超时错误
   c. 用户感知：错误日志显示 "pool acquire timeout" 且附带 file:line 定位

## **5.2 数据库集成验证（PostgreSQL）**

### **5.2.1 业务规则**

1. **Pg 连接池初始化**：验证系统必须通过环境变量 `SZ300_PG_PASSWORD` 获取 PostgreSQL 密码，构建连接串初始化 PgPoolHandle（max_size=10, min_idle=5）。
   a. 验收条件：WHEN 设置正确的 PostgreSQL 环境变量并启动应用 THEN THE SYSTEM SHALL Pg 连接池初始化成功且日志无错误

2. **Pg CRUD 验证**：验证系统必须对生产 PostgreSQL（lewuli 库）执行完整的 CRUD 操作并校验数据一致性。
   a. 验收条件：WHEN 向 lewuli 库执行 CRUD 操作 THEN THE SYSTEM SHALL 每条操作返回成功且数据一致性校验通过

3. **禁止项**：禁止修改上游 sz-orm 仓库中任何 PostgreSQL 相关文件。
   a. 验收条件：WHEN 验证完成后检查 `../sz-orm/` 仓库 THEN THE SYSTEM SHALL 无任何文件变更

### **5.2.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
database "生产 PostgreSQL\n(lewuli)" as PG

V -> VS : 发起 PostgreSQL 集成验证
VS -> VS : 读取环境变量 SZ300_PG_*
VS -> PG : 建立 Pg 连接池 (max_size=10, min_idle=5)
PG --> VS : 连接池就绪
VS -> PG : CRUD 全操作
PG --> VS : 操作结果
VS -> VS : 数据一致性校验
VS --> V : PostgreSQL 验证报告 + file:line 证据
@enduml
```

### **5.2.3 异常场景**

1. **PostgreSQL 密码错误**
   a. 触发条件：环境变量 `SZ300_PG_PASSWORD` 与生产 PostgreSQL 实际密码不匹配
   b. 系统行为：PgPoolHandle 连接返回认证失败错误
   c. 用户感知：错误日志显示 "password authentication failed" 且附带 file:line 定位

## **5.3 缓存集成验证（Redis）**

### **5.3.1 业务规则**

1. **Redis 连接建立**：验证系统必须通过 sz-rust-cache-facade 的 Redis 驱动连接生产 Redis 实例。
   a. 验收条件：WHEN 发起 Redis 连接 THEN THE SYSTEM SHALL 连接成功且 PING 返回 PONG

2. **缓存 CRUD 验证**：验证系统必须执行 SET/GET/DEL/EXPIRE/TTL 全套缓存操作并校验结果。
   a. 验收条件：WHEN 执行 SET key value 后 GET key THEN THE SYSTEM SHALL 返回值与写入值一致

3. **TTL 过期验证**：验证系统必须验证设置了 EXPIRE 的 key 在 TTL 到期后自动删除。
   a. 验收条件：WHEN 对 key 设置 EXPIRE 1 秒并等待 2 秒后 GET key THEN THE SYSTEM SHALL 返回 nil

4. **分布式锁验证**：sz-pay 使用 Redis 分布式锁，验证系统必须验证锁的获取/释放/互斥性。
   a. 验收条件：WHEN 进程 A 持有锁时进程 B 尝试获取同一锁 THEN THE SYSTEM SHALL 进程 B 获取失败

5. **禁止项**：禁止在验证报告中明文暴露 Redis 连接密码。
   a. 验收条件：WHEN 审查验证报告 THEN THE SYSTEM SHALL Redis 密码字段以 `***` 脱敏呈现

### **5.3.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
database "生产 Redis" as R

V -> VS : 发起 Redis 缓存验证
VS -> R : 建立连接 (sz-rust-cache-facade Redis 驱动)
R --> VS : 连接就绪
VS -> R : PING
R --> VS : PONG
VS -> R : SET / GET / DEL / EXPIRE / TTL
R --> VS : 操作结果
VS -> VS : 结果校验
VS -> R : 分布式锁获取/释放
R --> VS : 锁操作结果
VS --> V : Redis 验证报告 + file:line 证据
@enduml
```

### **5.3.3 异常场景**

1. **Redis 不可达**
   a. 触发条件：生产 Redis 实例未启动或网络不通
   b. 系统行为：连接建立超时返回错误
   c. 用户感知：错误日志显示 "Redis connection refused" 且附带 file:line 定位

2. **Redis 内存不足**
   a. 触发条件：Redis 实例内存已满，SET 操作触发 OOM
   b. 系统行为：SET 返回 OOM 错误
   c. 用户感知：错误日志显示 "OOM command not allowed" 且附带 file:line 定位

## **5.4 消息队列集成验证（MQTT）**

### **5.4.1 业务规则**

1. **MQTT 连接建立**：验证系统必须通过 sz-orm-mqtt 连接 MQTT Broker。
   a. 验收条件：WHEN 发起 MQTT 连接 THEN THE SYSTEM SHALL 连接成功且收到 CONNACK

2. **发布/订阅验证**：验证系统必须验证消息发布后订阅方能收到完整消息体。
   a. 验收条件：WHEN 向 topic 执行 PUBLISH message THEN THE SYSTEM SHALL 订阅该 topic 的客户端收到 message 且内容一致

3. **消息不丢失**：验证系统必须验证 QoS 1 级别下消息至少送达一次。
   a. 验收条件：WHEN 以 QoS 1 发布消息 THEN THE SYSTEM SHALL 订阅方至少收到一次该消息

4. **禁止项**：禁止在验证期间影响生产环境已有的 MQTT 消息流。
   a. 验收条件：WHEN 验证完成后 THEN THE SYSTEM SHALL 生产环境原有 MQTT 订阅无中断

### **5.4.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
queue "MQTT Broker" as MQ

V -> VS : 发起 MQTT 集成验证
VS -> MQ : 建立连接 (sz-orm-mqtt)
MQ --> VS : CONNACK
VS -> MQ : SUBSCRIBE test/topic
MQ --> VS : SUBACK
VS -> MQ : PUBLISH test/topic "hello"
MQ --> VS : 消息投递
VS -> VS : 消息一致性校验
VS --> V : MQTT 验证报告 + file:line 证据
@enduml
```

### **5.4.3 异常场景**

1. **MQTT Broker 不可达**
   a. 触发条件：MQTT Broker 未启动或网络不通
   b. 系统行为：连接超时返回错误
   c. 用户感知：错误日志显示 "MQTT connection failed" 且附带 file:line 定位

## **5.5 部署与进程管理验证**

### **5.5.1 业务规则**

1. **SSH 安全连接**：验证系统必须使用 Node.js ssh2 包加载 `deploy_key` 密钥连接服务器 122.51.216.76，禁止 sshpass 或 PowerShell 重定向。
   a. 验收条件：WHEN 发起 SSH 连接 THEN THE SYSTEM SHALL 使用 ssh2 包认证成功且禁止出现 sshpass 命令

2. **二进制部署**：验证系统必须将 sz-pay-server 与 sz-rust-sz300 的 release 编译产物上传至服务器 `/www/wwwroot/default` 路径。
   a. 验收条件：WHEN 上传编译产物 THEN THE SYSTEM SHALL 服务器目标路径下存在可执行二进制文件

3. **进程启动与端口监听**：验证系统必须启动 sz-pay-server 与 sz-rust-sz300 进程并确认各自监听端口。
   a. 验收条件：WHEN 启动进程后检查端口 THEN THE SYSTEM SHALL 目标端口处于 LISTEN 状态且进程 PID 可查

4. **进程精准终止**：验证系统必须使用 `fuser -k <端口>/tcp` 按端口杀进程，确保不误杀其他 webman 项目进程。
   a. 验收条件：WHEN 终止目标进程 THEN THE SYSTEM SHALL 仅目标端口进程被终止且其他 webman 进程不受影响

5. **进程更新验证**：部署后必须验证服务器上运行的进程已更新为新版本。
   a. 验收条件：WHEN 部署新版本后查询进程 THEN THE SYSTEM SHALL 进程启动时间/版本号确认已更新

6. **禁止项**：禁止使用 PowerShell 进行文件替换操作（会破坏 UTF-8 编码）。
   a. 验收条件：WHEN 执行文件操作 THEN THE SYSTEM SHALL 不调用 PowerShell 替换命令

### **5.5.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
cloud "生产服务器\n122.51.216.76" as S

V -> VS : 发起部署验证
VS -> S : SSH 连接 (ssh2 + deploy_key)
S --> VS : 认证成功
VS -> S : 上传 sz-pay-server 二进制
VS -> S : 上传 sz-rust-sz300 二进制
VS -> S : fuser -k 旧端口/tcp (精准终止旧进程)
VS -> S : 启动新进程
S --> VS : 进程启动 + PID
VS -> S : 验证端口监听 + 进程版本
S --> VS : 确认已更新
VS --> V : 部署验证报告 + file:line 证据
@enduml
```

### **5.5.3 异常场景**

1. **SSH 认证失败**
   a. 触发条件：deploy_key 密钥与服务器 authorized_keys 不匹配
   b. 系统行为：ssh2 连接返回认证失败错误
   c. 用户感知：错误日志显示 "SSH authentication failed" 且附带 file:line 定位

2. **端口被占用**
   a. 触发条件：目标端口已被其他进程占用
   b. 系统行为：进程启动失败，返回 "Address already in use"
   c. 用户感知：错误日志显示端口冲突且附带 file:line 定位

3. **误杀其他 webman 进程**
   a. 触发条件：未按端口精准定位而使用 `killall` 等粗粒度命令
   b. 系统行为：其他 webman 项目进程被意外终止
   c. 用户感知：生产环境其他服务不可用 — 此为严重违规，必须立即恢复

## **5.6 全链路端到端验证**

### **5.6.1 业务规则**

1. **HTTP → DB 全链路**：验证系统必须通过 HTTP 请求触发控制器 → 服务层 → ORM → MySQL 的完整调用路径，且每一跳均有日志证据。
   a. 验收条件：WHEN 发起 HTTP 请求 THEN THE SYSTEM SHALL 请求经路由→中间件→控制器→ORM→DB 完成后返回 200 且每跳有日志

2. **HTTP → 缓存全链路**：验证系统必须通过 HTTP 请求触发缓存读写路径，验证缓存命中/未命中两种场景。
   a. 验收条件：WHEN 首次请求（缓存未命中）THEN THE SYSTEM SHALL 回源 DB 并写入缓存；WHEN 二次请求（缓存命中）THEN THE SYSTEM SHALL 直接返回缓存数据

3. **HTTP → MQ 全链路**：验证系统必须通过 HTTP 请求触发消息发布路径，且订阅方能收到消息。
   a. 验收条件：WHEN HTTP 请求触发消息发布 THEN THE SYSTEM SHALL MQTT 订阅方收到消息且内容一致

4. **错误传播链**：验证系统必须验证当 DB/缓存/MQ 任一环节失败时，错误能正确传播至 HTTP 响应。
   a. 验收条件：WHEN DB 查询失败 THEN THE SYSTEM SHALL HTTP 响应返回 500 且错误信息含 file:line 定位

5. **file:line 证据完整性**：验证报告的每条结论必须附带源码文件路径与行号，且该行真实存在。
   a. 验收条件：WHEN 审查验证报告每条结论 THEN THE SYSTEM SHALL 均有 file:line 引用且对应文件行存在

### **5.6.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
participant "sz-pay-server" as Pay
participant "sz-rust-sz300" as Sz300
database "MySQL" as DB
database "Redis" as R
queue "MQTT" as MQ

V -> VS : 发起全链路端到端验证
VS -> Pay : HTTP 请求 (支付接口)
Pay -> DB : ORM 查询/写入
DB --> Pay : 数据
Pay -> R : 缓存读写
R --> Pay : 缓存结果
Pay --> VS : HTTP 200 + 业务数据

VS -> Sz300 : HTTP 请求 (设备管理接口)
Sz300 -> DB : ORM 查询/写入
DB --> Sz300 : 数据
Sz300 -> MQ : 消息发布
MQ --> Sz300 : 发布确认
Sz300 --> VS : HTTP 200 + 业务数据

VS -> VS : 汇总全链路证据
VS --> V : 全链路验证报告 + 每跳 file:line 证据
@enduml
```

### **5.6.3 异常场景**

1. **中间件拦截**
   a. 触发条件：请求未通过认证/限流/CSRF 等中间件
   b. 系统行为：请求在中间件层被拦截，不进入控制器
   c. 用户感知：HTTP 响应返回 401/429/403 且错误信息含 file:line 定位

2. **ORM 映射失败**
   a. 触发条件：数据库字段与模型结构体映射不匹配
   b. 系统行为：ORM 返回反序列化错误
   c. 用户感知：HTTP 响应返回 500 且错误信息含 file:line 定位

3. **跨服务调用失败**
   a. 触发条件：sz-pay 调用 sz-rust-sz300 或反向调用时目标服务不可达
   b. 系统行为：调用超时返回错误
   c. 用户感知：HTTP 响应返回 502/504 且错误信息含 file:line 定位

## **5.7 验证产物清理**

### **5.7.1 业务规则**

1. **服务器测试脚本清理**：验证完成后，上传到服务器上的所有测试脚本必须删除。
   a. 验收条件：WHEN 验证完成后检查服务器 THEN THE SYSTEM SHALL 验证期间上传的测试脚本均已删除

2. **本地临时文件清理**：验证完成后，写入本地硬盘的临时文件必须删除。
   a. 验收条件：WHEN 验证完成后检查本地临时目录 THEN THE SYSTEM SHALL 验证产生的临时文件均已删除

3. **测试进程释放**：验证完成后，验证期间启动的测试进程必须全部释放。
   a. 验收条件：WHEN 验证完成后检查进程列表 THEN THE SYSTEM SHALL 验证期间启动的测试进程均已退出

4. **生产数据清理**：验证期间写入生产数据库的测试数据必须清理，不影响生产环境原有数据。
   a. 验收条件：WHEN 验证完成后检查生产数据库 THEN THE SYSTEM SHALL 验证产生的测试数据均已删除且原有数据不受影响

5. **禁止项**：禁止遗留任何验证产物在服务器或本地。
   a. 验收条件：WHEN 验证完成后 THEN THE SYSTEM SHALL 清理确认记录中所有产物状态为"已清理"

### **5.7.2 交互流程**

```plantuml
@startuml
actor "验证执行者" as V
participant "全链路验证系统" as VS
cloud "生产服务器" as S
database "生产数据库" as DB

V -> VS : 发起清理
VS -> S : 删除上传的测试脚本
S --> VS : 删除确认
VS -> DB : 删除验证产生的测试数据
DB --> VS : 删除确认
VS -> VS : 删除本地临时文件
VS -> VS : 终止测试进程
VS --> V : 清理完成确认记录
@enduml
```

### **5.7.3 异常场景**

1. **清理失败**
   a. 触发条件：测试脚本或临时文件因权限不足无法删除
   b. 系统行为：清理操作返回权限错误，记录未清理项
   c. 用户感知：清理报告中标注未清理项及原因，需人工介入

---

# **6. 数据约束**

## **6.1 验证报告**

1. **报告版本**：必须标注基于 sz-rust v0.6.7
2. **验证时间**：必须记录验证执行的起止时间戳
3. **服务器地址**：必须记录验证目标服务器 IP（122.51.216.76）
4. **验证项清单**：必须包含 DB-MySQL/DB-PostgreSQL/缓存-Redis/MQ-MQTT/部署/全链路/清理 7 大类
5. **每项结论**：必须为"通过/失败"二值，禁止"可能/大概"
6. **file:line 证据**：每条结论必须附带源码文件路径与行号，且该行真实存在
7. **错误详情**：失败项必须包含错误消息、堆栈、复现步骤

## **6.2 验证环境配置**

1. **服务器 IP**：122.51.216.76
2. **SSH 端口**：22
3. **SSH 用户**：root
4. **SSH 密钥**：deploy_key 文件（禁止明文写入文档）
5. **MySQL 端口**：8802
6. **MySQL 数据库**：test / shop / njszjt（验证用）
7. **PostgreSQL 数据库**：lewuli
8. **网站路径**：/www/wwwroot/default
9. **sz-rust 版本**：0.6.7
10. **sz-orm 版本**：2.3.0

## **6.3 进程状态**

1. **进程名**：sz-pay-server / sz-rust-sz300 二进制名
2. **监听端口**：必须记录各进程实际监听端口
3. **PID**：必须记录各进程 PID 用于精准管理
4. **启动时间**：必须记录进程启动时间用于版本更新确认
5. **内存占用**：必须记录空载 RSS 用于验证 ≤ 30MB 约束

## **6.4 清理确认记录**

1. **服务器测试脚本**：文件路径 + 删除状态（已删除/未删除）
2. **本地临时文件**：文件路径 + 删除状态
3. **测试进程**：进程名 + PID + 退出状态（已退出/未退出）
4. **生产测试数据**：数据库 + 表 + 删除状态
5. **清理时间**：清理操作执行的时间戳
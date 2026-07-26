# SzRSQL

> 基于 Rust 的分布式 SQL 数据库，兼容 PostgreSQL 协议（pgwire v3.0），支持 MVCC 事务、WAL 持久化、B+Tree 存储、行级锁、多方言解析。

SzRSQL 是一个用 Rust 实现的分布式 SQL 数据库，目标是提供与 PostgreSQL 协议级兼容（pgwire v3.0）的嵌入式/独立数据库服务。项目采用 Workspace 多 crate 架构，涵盖 SQL 解析、查询优化、存储引擎、事务管理、WAL 持久化、复制、安全、运维等完整数据库栈。

- **版本**：v0.1.0
- **许可证**：MIT
- **Rust Edition**：2021
- **兼容协议**：pgwire v3.0（PostgreSQL 14 兼容）

---

## 目录

- [特性概览](#特性概览)
- [快速开始](#快速开始)
- [命令行参数](#命令行参数)
- [连接示例](#连接示例)
- [HTTP 管理端点](#http-管理端点)
- [项目结构](#项目结构)
- [编译与测试](#编译与测试)
- [数据持久化](#数据持久化)
- [并发与锁](#并发与锁)
- [方言兼容性](#方言兼容性)
- [安全特性](#安全特性)
- [性能基准](#性能基准)
- [文档索引](#文档索引)
- [限制与已知问题](#限制与已知问题)
- [许可证](#许可证)

---

## 特性概览

| 模块 | 能力 |
|------|------|
| **SQL 引擎** | 多方言解析（PG/MySQL/Oracle/SQL Server/SQLite）、AST、执行器、PL/pgSQL 解释器、CTE/窗口函数/MERGE/UPSERT/RETURNING/SAVEPOINT |
| **查询优化** | 逻辑/物理计划、基于代价的优化、物化视图查询重写、增量维护 |
| **存储引擎** | B+Tree 索引、Buffer Pool、Freelist、Page 管理、TOAST、列存、外部格式（Arrow/Parquet/CSV/JSONLines） |
| **事务** | MVCC 多版本并发控制、Strict 2PL 行级锁、死锁检测（DFS 环检测）、Savepoint、事务 ID 全局唯一 |
| **持久化** | WAL 预写日志、log-then-commit 事务模型、WAL 压缩（zstd）、全页镜像（FPI）、WAL 摘要、Group Commit |
| **协议** | pgwire v3.0、SCRAM-SHA-256 认证、TLS 1.3（rustls）、扩展协议（Prepared/Portal）、ParameterStatus |
| **分布式** | 分布式事务、CDC 变更数据捕获、Schema Registry |
| **索引** | B+Tree、Bitmap、BRIN、GiST、全文检索（FTS5 风格）、空间索引、部分覆盖索引 |
| **安全** | TDE 透明加密（AES-256-CTR）、列级加密（AES-256-GCM）、SCRAM-SHA-256、SQL 注入检测、防火墙、审计日志、脱敏 |
| **复制** | 物理复制、逻辑复制、消费者偏移管理 |
| **运维** | Autovacuum、在线 DDL、升级迁移、影子比对（Shadow） |
| **AI** | AI 辅助查询优化、向量化执行 |
| **HTTP 管理** | healthz/readyz/metrics、会话管理、备份、配置热加载 |
| **信号** | SIGTERM 优雅关闭、SIGINT 立即关闭、Crash Handler（panic 日志 + backtrace） |
| **守护进程** | Unix 双 fork + setsid 守护化、PID 文件 RAII 管理、stale 清理 |

---

## 快速开始

### 环境要求

- **Rust**：1.81+（使用 Edition 2021）
- **操作系统**：Linux / macOS / Windows（守护进程模式仅 Unix 支持）
- **依赖**：Cargo 自动管理（tokio、sqlparser、rustls、aes-gcm、zstd、arrow/parquet 等）

### 编译

```bash
# 开发构建（debug，启用 overflow-checks）
cargo build

# 发布构建（启用 LTO）
cargo build --release

# 仅检查（不生成二进制）
cargo check --workspace
```

### 启动服务

```bash
# 1. 最简启动（默认 127.0.0.1:5432，无 WAL，仅用于测试）
cargo run -p szrsql-bin

# 2. 生产环境启动（启用 WAL log-then-commit + HTTP 管理 + 远程绑定）
cargo run -p szrsql-bin -- \
  --host 0.0.0.0 \
  --port 5432 \
  --wal-path /var/lib/szrsql/wal.log \
  --http-port 8080 \
  --http-host 127.0.0.1 \
  --http-auth-token "$SZRSQL_HTTP_TOKEN"

# 3. 守护进程模式（Unix，后台运行 + PID 文件）
cargo run -p szrsql-bin -- \
  --daemon \
  --pid-file /var/run/szrsql.pid \
  --wal-path /var/lib/szrsql/wal.log \
  --http-port 8080

# 4. 查看版本
cargo run -p szrsql-bin -- --version
```

### 连接方式

```bash
# 使用 psql 连接
psql -h 127.0.0.1 -p 5432 -U postgres

# 使用 Python asyncpg 连接
python -c "import asyncio, asyncpg; \
  asyncio.run(asyncpg.connect('postgresql://postgres@127.0.0.1:5432'))"
```

> **注意**：未指定 `--wal-path` 时仅运行于 commit-then-log 模式（ACK 成功但数据可能未持久化），**生产环境务必设置 `--wal-path`**。

---

## 命令行参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `--host` | String | `127.0.0.1` | pgwire 监听地址 |
| `--port` | u16 | `5432` | pgwire 监听端口 |
| `--server-version` | String | `14.0-szrsql` | server_version ParameterStatus（发送给客户端） |
| `--shutdown-timeout` | u64 | `30` | 优雅关闭超时（秒）；SIGTERM 后等待活跃连接最长时间，超时强制中止 |
| `--crash-log-dir` | PathBuf | `.` | 崩溃日志输出目录（panic 时写入含 backtrace + WAL LSN 的日志） |
| `--no-backtrace` | bool | `false` | 禁用 backtrace 捕获（减少 panic hook 开销） |
| `--daemon` | bool | `false` | 守护进程模式（Unix 双 fork + setsid；Windows 不支持） |
| `--pid-file` | PathBuf | `None` | PID 文件路径（防止重复启动、stale 清理、退出自动删除） |
| `--http-port` | u16 | `0` | HTTP 管理端口（0 = 不监听） |
| `--http-host` | String | `127.0.0.1` | HTTP 监听地址（建议仅绑定本地） |
| `--http-auth-token` | String | `None` | `/api/v1/*` 端点 Bearer token 鉴权（healthz/readyz/metrics 无需鉴权） |
| `--wal-path` | PathBuf | `None` | WAL 文件路径；启用 log-then-commit 事务模型（**生产环境强烈建议设置**） |

**信号行为**：
- `SIGTERM`（Unix）/ `Ctrl+C`（Windows）→ 优雅关闭（排空活跃连接，最多 `shutdown-timeout`）
- `SIGINT` / `Ctrl+C`（Unix）→ 立即关闭（不等待活跃事务，直接 abort_all）
- `panic` → 写入崩溃日志到 `--crash-log-dir`

---

## 连接示例

### Rust（tokio-postgres）

```rust
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (client, connection) =
        tokio_postgres::connect("host=127.0.0.1 port=5432 user=postgres", NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });

    // 建表
    client.batch_execute(
        "CREATE TABLE IF NOT EXISTS accounts (
            id BIGINT PRIMARY KEY,
            name TEXT,
            balance BIGINT
        )"
    ).await?;

    // 插入
    client.execute(
        "INSERT INTO accounts (id, name, balance) VALUES ($1, $2, $3)",
        &[&1i64, &"alice", &1000i64],
    ).await?;

    // 查询
    let rows = client.query("SELECT id, name, balance FROM accounts WHERE id = $1", &[&1i64]).await?;
    for row in &rows {
        let id: i64 = row.get(0);
        let name: &str = row.get(1);
        let balance: i64 = row.get(2);
        println!("id={id}, name={name}, balance={balance}");
    }
    Ok(())
}
```

### 事务示例（Strict 2PL + 死锁检测）

```rust
// 转账事务：from -> to，金额 amount
// SzRSQL 在 COMMIT/ABORT 时通过 unlock_all() 释放所有行锁（Strict 2PL）
// 若两个事务循环等待，DFS 环检测会中止其中一个（返回 deadlock 错误）
async fn transfer(
    client: &tokio_postgres::Client,
    from: i64,
    to: i64,
    amount: i64,
) -> Result<(), tokio_postgres::Error> {
    let tx = client.transaction().await?;

    // 对行加排他锁（FOR UPDATE），锁持有至 COMMIT
    tx.execute("UPDATE accounts SET balance = balance - $1 WHERE id = $2", &[&amount, &from]).await?;
    tx.execute("UPDATE accounts SET balance = balance + $1 WHERE id = $2", &[&amount, &to]).await?;

    // 校验余额非负
    let row = tx.query_one("SELECT balance FROM accounts WHERE id = $1", &[&from]).await?;
    let balance: i64 = row.get(0);
    if balance < 0 {
        tx.rollback().await?; // ABORT：自动释放所有行锁
        return Err(tokio_postgres::Error::封锁("insufficient balance".into()));
    }

    tx.commit().await?; // COMMIT：WAL fsync 后释放所有行锁
    Ok(())
}
```

> **死锁处理**：当 `transfer(A, B)` 与 `transfer(B, A)` 并发执行形成等待环时，`LockManager::detect_deadlock` 通过 DFS 检测到环，中止其中一个事务并返回 `LockError::Deadlock(txn_id)`，应用层应捕获并重试。

---

## HTTP 管理端点

启用 `--http-port` 后可用。`/api/v1/*` 端点需要 `Authorization: Bearer <token>` 头（`--http-auth-token` 设置时）。

| 端点 | 方法 | 鉴权 | 说明 |
|------|------|------|------|
| `/healthz` | GET | 无 | 存活探针（K8s liveness） |
| `/readyz` | GET | 无 | 就绪探针（K8s readiness） |
| `/metrics` | GET | 无 | Prometheus 指标抓取 |
| `/api/v1/sessions` | GET | Bearer | 列出活跃会话 |
| `/api/v1/cancel/{pid}` | POST | Bearer | 取消指定 PID 的查询 |
| `/api/v1/backup` | POST | Bearer | 触发备份 |
| `/api/v1/config/reload` | POST | Bearer | 配置热加载 |

```bash
# 示例：查询会话（带鉴权）
curl -H "Authorization: Bearer $SZRSQL_HTTP_TOKEN" \
     http://127.0.0.1:8080/api/v1/sessions

# Prometheus 抓取（无需鉴权）
curl http://127.0.0.1:8080/metrics
```

---

## 项目结构

```
szrsql/
├── crates/                          # Workspace 子 crate（16 库 + 1 二进制）
│   ├── szrsql-types/                # 共享类型定义（数据类型、错误类型）
│   ├── szrsql-storage/              # 存储引擎（B+Tree、Buffer Pool、Page、Freelist、TOAST、列存、远程 FS、溢写）
│   ├── szrsql-tx/                   # 事务（MVCC、行锁、WAL、死锁检测、Vacuum、Undo、Schema Registry、CDC）
│   ├── szrsql-cdc/                  # 变更数据捕获
│   ├── szrsql-sql/                  # SQL 引擎（Parser、AST、Executor、PL/pgSQL、索引、触发器、分区）
│   ├── szrsql-catalog/              # 系统目录（表/列/索引元数据）
│   ├── szrsql-protocol/             # pgwire v3.0 协议、HTTP 管理服务、信号处理、守护进程
│   ├── szrsql-optimizer/            # 查询优化器（逻辑/物理计划、代价模型）
│   ├── szrsql-ai/                   # AI 辅助优化、向量化执行
│   ├── szrsql-security/             # 安全（TDE、列级加密、SCRAM、防火墙、审计、脱敏、SQL 注入检测）
│   ├── szrsql-scheduler/            # 任务调度
│   ├── szrsql-replication/          # 物理复制、逻辑复制
│   ├── szrsql-dist/                 # 分布式事务、节点管理
│   ├── szrsql-pgcompat/             # PostgreSQL 兼容性测试套件
│   ├── szrsql-ops/                  # 运维（Autovacuum、在线 DDL、升级）
│   ├── szrsql-shadow/               # 影子比对（与 PostgreSQL 18 对拍测试）
│   └── szrsql-bin/                  # 二进制入口（main.rs / health_main.rs）
├── docs/                            # 设计文档与测试报告
├── benchmarks/                      # 性能基准
├── fuzz/                            # 模糊测试
├── ci/                              # CI 配置
├── deploy/                          # 部署配置
├── scripts/                         # 辅助脚本
├── tools/                           # 工具
├── Cargo.toml                       # Workspace 清单
├── deny.toml                        # cargo-deny 配置
├── rustfmt.toml                     # 格式化配置
└── Dockerfile / docker-compose.yml  # 容器化
```

---

## 编译与测试

```bash
# 1. 静态检查
cargo check --workspace

# 2. Lint（Clippy，零警告门禁）
cargo clippy --workspace --all-targets -- -D warnings

# 3. 格式检查
cargo fmt --all -- --check

# 4. 单元 + 集成测试
cargo test --workspace

# 5. 并发集成测试（多 session 跨连接共享状态）
cargo test -p szrsql-protocol --test concurrency -- --test-threads=4

# 6. PostgreSQL 兼容性测试
cargo run -p szrsql-pgcompat --example print_report

# 7. 性能基准（与 PostgreSQL 18 对拍）
cargo test -p szrsql-shadow --test bench_pgbench -- --nocapture

# 8. 变异测试（cargo-mutants）
cargo mutants --workspace

# 9. 模糊测试（cargo-fuzz，需 nightly）
cargo +nightly fuzz run btree_fuzz
cargo +nightly fuzz run wal_fuzz
cargo +nightly fuzz run mvcc_fuzz

# 10. 依赖审计
cargo deny check
```

---

## 数据持久化

> **SzRSQL 不是内存数据库**。启用 `--wal-path` 后，所有 COMMIT 操作先写 WAL 并 `fsync` 再 ACK 客户端，保证数据落盘。

| 机制 | 实现文件 | 说明 |
|------|---------|------|
| **WAL 预写日志** | `szrsql-tx/src/wal.rs` | 顺序追加写入，Commit 记录 fsync 后才返回成功 |
| **B+Tree 持久化** | `szrsql-storage/src/btree.rs` | 索引/数据 B+Tree，支持 Page 级持久化 |
| **远程存储** | `szrsql-storage/src/remote_fs.rs` | S3 / HTTPFS 远程文件系统 |
| **溢写盘** | `szrsql-storage/src/spill.rs` | 大结果集溢写到磁盘 |
| **备份恢复** | `szrsql-ops` + HTTP `/api/v1/backup` | 在线备份 |
| **WAL 压缩** | `szrsql-tx/src/wal_compression.rs` | zstd 压缩 WAL 记录 |
| **全页镜像（FPI）** | `szrsql-tx/src/wal_fpi.rs` | 页面首次修改时写全页镜像，防止部分写撕裂 |
| **WAL 摘要** | `szrsql-tx/src/wal_summarizer.rs` | WAL 摘要加速增量备份/复制 |
| **Buffer Pool** | `szrsql-storage/src/buffer.rs` | 页面缓存与淘汰 |
| **外部格式** | `szrsql-storage/src/external_format.rs` | Arrow IPC / Parquet / CSV / JSONLines 导入导出 |

### log-then-commit 事务模型

启用 `--wal-path` 后的 COMMIT 流程：

```
客户端发送 COMMIT
      │
      ▼
┌─────────────────────────────┐
│ 1. 写入 WAL Commit 记录       │  szrsql-tx/src/wal.rs
│    (WalOpType::Commit = 3)   │
└─────────────────────────────┘
      │
      ▼
┌─────────────────────────────┐
│ 2. fsync 强制刷盘             │  WalWriter::flush()
│    （数据持久化到磁盘）        │
└─────────────────────────────┘
      │
      ▼
┌─────────────────────────────┐
│ 3. 向客户端返回 CommandComplete│  ACK 成功
└─────────────────────────────┘
      │
      ▼
┌─────────────────────────────┐
│ 4. 释放所有行锁（unlock_all）  │  Strict 2PL
└─────────────────────────────┘
```

**未设置 `--wal-path` 时**：退化为 commit-then-log（ACK 成功但数据可能未持久化），仅用于测试/兼容，**生产环境禁用**。

---

## 并发与锁

### 多线程并发执行

- pgwire 服务器支持多 session 并发，共享 `Arc<RwLock<HashMap<Table>>>`、`Arc<LockManager>`、`Arc<AtomicU32>`（事务 ID 计数器）
- 事务 ID 全局唯一（跨 session 共享 `AtomicU32` 递增）

### 行级锁（Strict 2PL）

| 锁模式 | 兼容性 | 说明 |
|--------|--------|------|
| **Shared（S）** | S 兼容；X 不兼容 | 读锁，`SELECT ... FOR SHARE` |
| **Exclusive（X）** | 与所有锁不兼容 | 写锁，`UPDATE/DELETE/SELECT ... FOR UPDATE` |
| **升级** | S → X | 写操作时自动升级 |

**Strict 2PL 协议**：
- 增长阶段：事务执行过程中按需获取行锁
- 收缩阶段：**只在 COMMIT 或 ABORT 时**统一释放所有锁（`unlock_all(txn_id)`）

### 死锁检测（DFS 环检测）

- **触发时机**：事务进入等待队列前立即检查；等待循环中周期性检查
- **算法**：等待图 DFS 环检测
- **处理**：检测到环 → 中止其中一个事务（返回 `LockError::Deadlock(txn_id)`），应用层重试
- 实现文件：`szrsql-tx/src/lock.rs`

```rust
// 死锁错误示例
Err(LockError::Deadlock(txn_id)) => {
    // 应用层：重试事务
}
```

---

## 方言兼容性

| 数据库 | 兼容等级 | 通过率 | 说明 |
|--------|---------|--------|------|
| **PostgreSQL** | L2（协议级兼容） | 89.6% (95/106) | pgwire v3.0 + SCRAM-SHA-256，客户端驱动可直接连接；5 项语法不支持、5 项数据类型未实现 |
| **MySQL** | L1（方言解析） | ~70%（估算） | 文本级预处理，无存储引擎兼容 |
| **SQLite** | L0（语法借鉴） | N/A | 无方言支持，仅 FTS5 风格全文检索借鉴 |
| **Oracle** | L1（方言解析） | ~60%（估算） | 文本级预处理（ROWNUM/DECODE/NVL） |
| **SQL Server** | L1（方言解析） | ~65%（估算） | 文本级预处理（TOP/ISNULL/GETDATE） |

**兼容等级定义**：
- **L0**：语法借鉴（无实际兼容性）
- **L1**：方言解析（能解析 SQL，但语义/行为可能不一致）
- **L2**：协议级兼容（客户端驱动可直接连接）
- **L3**：行为级兼容（SQL 语义、错误码、数据类型完全一致）

> 数据来源：`docs/兼容性评估报告.md`（基于 `szrsql-pgcompat` 测试套件实测）

---

## 安全特性

| 特性 | 实现文件 | 说明 |
|------|---------|------|
| **TDE 透明加密** | `szrsql-security/src/tde.rs` | AES-256-CTR 表空间级加密 |
| **列级加密** | `szrsql-security/src/column_enc.rs` | AES-256-GCM 认证加密 |
| **SCRAM-SHA-256** | `szrsql-protocol` | pgwire 认证协议 |
| **TLS 1.3** | `szrsql-protocol`（rustls + ring） | 传输层加密 |
| **SQL 注入防护** | `szrsql-security/src/sqli_detector.rs` | 注入检测引擎 |
| **DoS 防护** | `szrsql-security` | 连接数/查询复杂度限制 |
| **审计日志** | `szrsql-security/src/audit.rs` + `audit_hash.rs` | 防篡改审计（哈希链） |
| **防火墙** | `szrsql-security/src/firewall.rs` | IP/SQL 规则过滤 |
| **数据脱敏** | `szrsql-security/src/masking.rs` | 列级脱敏策略 |
| **密码策略** | `szrsql-security/src/password_profile.rs` | 密码强度/过期策略 |
| **合规** | `szrsql-security/src/compliance.rs` | 合规检查 |
| **unsafe 审计** | Workspace 禁用 `unsafe`（除 libc/windows-sys 系统调用） | 编译期门禁 |

---

## 性能基准

基于 `crates/szrsql-shadow/tests/bench_pgbench.rs` 与 PostgreSQL 18（127.0.0.1:5432）对拍测试。以下为 1K 行规模下的实测 P50 延迟（100K 行吞吐测试通过，未单独测 P50）：

| 操作 | 行数 | PG 18 P50 (ms) | SzRSQL P50 (ms) | sz/PG 比 | 备注 |
|------|------|----------------|-----------------|----------|------|
| **INSERT** | 1,000 | 0.327 | 0.072 | 0.22x | SzRSQL 快 4.5x |
| **SELECT** | 1,102 | 0.283 | 0.068 | 0.24x | SzRSQL 快 4.2x |
| **UPDATE** | 2,000 | 0.333 | 0.283 | 0.85x | SzRSQL 快 1.18x |
| **DELETE** | 2,000 | — | — | — | 吞吐测试通过（匹配率 100%） |
| **综合工作负载** | 302 | 0.293 | 0.105 | 0.36x | SzRSQL 快 2.8x |

**测试条件**：
- SzRSQL 运行于 `InMemoryTable` 模式（无 WAL，单线程）
- 数据规模：1K-100K 行，单线程工作负载
- 匹配率：100%（结果与 PG 18 完全一致）
- INSERT 100K 顺序插入吞吐测试通过

> 数据来源：`docs/PERF_BENCH_REPORT.md`（2026-07-25 实测）。P95/P99 详见报告。

---

## 文档索引

| 文档 | 说明 |
|------|------|
| `docs/ADR与生产Bug定位规范.md` | ADR 规范与生产 Bug 定位流程 |
| `docs/CHAOS_REPORT.md` | 混沌测试报告 |
| `docs/downgrade_plan.md` | 降级方案 |
| `docs/evaluation.md` | 评估报告 |
| `docs/FUZZ_REPORT.md` | 模糊测试报告 |
| `docs/MUTATION_REPORT.md` | 变异测试报告 |
| `docs/PERF_BENCH_REPORT.md` | 性能基准报告（与 PG 18 对拍） |
| `docs/PolarSSL对szrsql的参考价值分析.md` | PolarSSL 参考价值分析 |
| `docs/release_stages.md` | 发布阶段定义 |
| `docs/SHADOW_REPORT.md` | 影子比对测试报告 |
| `docs/SPEC_REVIEW_REPORT.md` | 规范评审报告 |
| `docs/szrsql-engineering-practices.md` | 工程实践规范 |
| `docs/全面排查汇总报告.md` | 全面排查汇总 |
| `docs/兼容性评估报告.md` | 数据库兼容性评估 |
| `docs/大数据量对比测试报告.md` | 大数据量对比测试 |
| `docs/对抗性边界审计清单.md` | 对抗性边界审计清单 |
| `docs/技能触发命令速查.md` | 技能触发命令速查 |
| `docs/软件项目审计清单.md` | 软件项目审计清单 |
| `docs/项目成熟度评估报告.md` | 项目成熟度评估 |

---

## 限制与已知问题

### 未实现的 PostgreSQL 功能

- **SQL 语法**（6 项）：`SUBSTRING` 函数、`ILIKE` 运算符、`SIMILAR TO` 运算符、`IS DISTINCT FROM`、`~` 正则匹配运算符、`UUID` 类型（语法层）
- **数据类型**（5 项）：`interval`、`bit`/`varbit`、`cidr`/`inet`、`point`/几何类型、`xml`
- **分布式**：分布式事务协调器为初步实现，未达到生产级一致性保证
- **复制**：物理/逻辑复制为基础实现，未支持级联复制与同步复制
- **守护进程**：Windows 不支持 `--daemon`（仅 Unix 双 fork + setsid）
- **大数据测试**：当前最大 100K 行，未做 1M+ 大数据量测试
- **多线程基准**：性能测试为单线程，未做多线程并发基准
- **方言兼容**：MySQL/Oracle/SQL Server 仅文本级预处理，无存储引擎兼容（L1）
- **PostGIS**：几何类型未实现，PostGIS 不可用

### 已知问题

- 未设置 `--wal-path` 时为 commit-then-log 模式，ACK 成功但数据可能未持久化（仅测试用）
- 守护进程化后 tracing stderr 输出被重定向到 `/dev/null`，需配置 file appender

---

## 许可证

MIT License（见 `Cargo.toml` 中 `license = "MIT"`）

Copyright (c) SzRSQL Team
## 连接示例

### Rust（tokio-postgres）

```rust
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (client, connection) =
        tokio_postgres::connect("host=127.0.0.1 port=5432 user=postgres", NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });

    // 建表
    client.batch_execute(
        "CREATE TABLE IF NOT EXISTS accounts (
            id BIGINT PRIMARY KEY,
            name TEXT,
            balance BIGINT
        )"
    ).await?;

    // 插入
    client.execute(
        "INSERT INTO accounts (id, name, balance) VALUES ($1, $2, $3)",
        &[&1i64, &"alice", &1000i64],
    ).await?;

    // 查询
    let rows = client.query("SELECT id, name, balance FROM accounts WHERE id = $1", &[&1i64]).await?;
    for row in &rows {
        let id: i64 = row.get(0);
        let name: &str = row.get(1);
        let balance: i64 = row.get(2);
        println!("id={id}, name={name}, balance={balance}");
    }
    Ok(())
}
```

### 事务示例（Strict 2PL + 死锁检测）

```rust
// 转账事务：from -> to，金额 amount
// SzRSQL 在 COMMIT/ABORT 时通过 unlock_all() 释放所有行锁（Strict 2PL）
// 若两个事务循环等待，DFS 环检测会中止其中一个（返回 deadlock 错误）
async fn transfer(
    client: &tokio_postgres::Client,
    from: i64,
    to: i64,
    amount: i64,
) -> Result<(), tokio_postgres::Error> {
    let tx = client.transaction().await?;

    // 对行加排他锁（FOR UPDATE），锁持有至 COMMIT
    tx.execute("UPDATE accounts SET balance = balance - $1 WHERE id = $2", &[&amount, &from]).await?;
    tx.execute("UPDATE accounts SET balance = balance + $1 WHERE id = $2", &[&amount, &to]).await?;

    // 校验余额非负
    let row = tx.query_one("SELECT balance FROM accounts WHERE id = $1", &[&from]).await?;
    let balance: i64 = row.get(0);
    if balance < 0 {
        tx.rollback().await?; // ABORT：自动释放所有行锁
        return Err(tokio_postgres::Error::other("insufficient balance"));
    }

    tx.commit().await?; // COMMIT：WAL fsync 后释放所有行锁
    Ok(())
}
```

> **死锁处理**：当 `transfer(A, B)` 与 `transfer(B, A)` 并发执行形成等待环时，`LockManager::detect_deadlock` 通过 DFS 检测到环，中止其中一个事务并返回 `LockError::Deadlock(txn_id)`，应用层应捕获并重试。

---

## HTTP 管理端点

启用 `--http-port` 后可用。`/api/v1/*` 端点需要 `Authorization: Bearer <token>` 头（`--http-auth-token` 设置时）。

| 端点 | 方法 | 鉴权 | 说明 |
|------|------|------|------|
| `/healthz` | GET | 无 | 存活探针（K8s liveness） |
| `/readyz` | GET | 无 | 就绪探针（K8s readiness） |
| `/metrics` | GET | 无 | Prometheus 指标抓取 |
| `/api/v1/sessions` | GET | Bearer | 列出活跃会话 |
| `/api/v1/cancel/{pid}` | POST | Bearer | 取消指定 PID 的查询 |
| `/api/v1/backup` | POST | Bearer | 触发备份 |
| `/api/v1/config/reload` | POST | Bearer | 配置热加载 |

```bash
# 示例：查询会话（带鉴权）
curl -H "Authorization: Bearer $SZRSQL_HTTP_TOKEN" \
     http://127.0.0.1:8080/api/v1/sessions

# Prometheus 抓取（无需鉴权）
curl http://127.0.0.1:8080/metrics
```

---
## 项目结构

```
szrsql/
├── crates/                          # Workspace 子 crate（16 库 + 1 二进制）
│   ├── szrsql-types/                # 共享类型定义（数据类型、错误类型）
│   ├── szrsql-storage/              # 存储引擎（B+Tree、Buffer Pool、Page、Freelist、TOAST、列存、远程 FS、溢写）
│   ├── szrsql-tx/                   # 事务（MVCC、行锁、WAL、死锁检测、Vacuum、Undo、Schema Registry、CDC）
│   ├── szrsql-cdc/                  # 变更数据捕获
│   ├── szrsql-sql/                  # SQL 引擎（Parser、AST、Executor、PL/pgSQL、索引、触发器、分区）
│   ├── szrsql-catalog/              # 系统目录（表/列/索引元数据）
│   ├── szrsql-protocol/             # pgwire v3.0 协议、HTTP 管理服务、信号处理、守护进程
│   ├── szrsql-optimizer/            # 查询优化器（逻辑/物理计划、代价模型）
│   ├── szrsql-ai/                   # AI 辅助优化、向量化执行
│   ├── szrsql-security/             # 安全（TDE、列级加密、SCRAM、防火墙、审计、脱敏、SQL 注入检测）
│   ├── szrsql-scheduler/            # 任务调度
│   ├── szrsql-replication/          # 物理复制、逻辑复制
│   ├── szrsql-dist/                 # 分布式事务、节点管理
│   ├── szrsql-pgcompat/             # PostgreSQL 兼容性测试套件
│   ├── szrsql-ops/                  # 运维（Autovacuum、在线 DDL、升级）
│   ├── szrsql-shadow/               # 影子比对（与 PostgreSQL 18 对拍测试）
│   └── szrsql-bin/                  # 二进制入口（main.rs / health_main.rs）
├── docs/                            # 设计文档与测试报告
├── benchmarks/                      # 性能基准
├── fuzz/                            # 模糊测试
├── ci/                              # CI 配置
├── deploy/                          # 部署配置
├── scripts/                         # 辅助脚本
├── tools/                           # 工具
├── Cargo.toml                       # Workspace 清单
├── deny.toml                        # cargo-deny 配置
├── rustfmt.toml                     # 格式化配置
└── Dockerfile / docker-compose.yml  # 容器化
```

---

## 编译与测试

```bash
# 1. 静态检查
cargo check --workspace

# 2. Lint（Clippy，零警告门禁）
cargo clippy --workspace --all-targets -- -D warnings

# 3. 格式检查
cargo fmt --all -- --check

# 4. 单元 + 集成测试
cargo test --workspace

# 5. 并发集成测试（多 session 跨连接共享状态）
cargo test -p szrsql-protocol --test concurrency -- --test-threads=4

# 6. PostgreSQL 兼容性测试
cargo run -p szrsql-pgcompat --example print_report

# 7. 性能基准（与 PostgreSQL 18 对拍）
cargo test -p szrsql-shadow --test bench_pgbench -- --nocapture

# 8. 变异测试（cargo-mutants）
cargo mutants --workspace

# 9. 模糊测试（cargo-fuzz，需 nightly）
cargo +nightly fuzz run btree_fuzz
cargo +nightly fuzz run wal_fuzz
cargo +nightly fuzz run mvcc_fuzz

# 10. 依赖审计
cargo deny check
```

---

## 数据持久化

> **SzRSQL 不是内存数据库**。启用 `--wal-path` 后，所有 COMMIT 操作先写 WAL 并 `fsync` 再 ACK 客户端，保证数据落盘。

| 机制 | 实现文件 | 说明 |
|------|---------|------|
| **WAL 预写日志** | `szrsql-tx/src/wal.rs` | 顺序追加写入，Commit 记录 fsync 后才返回成功 |
| **B+Tree 持久化** | `szrsql-storage/src/btree.rs` | 索引/数据 B+Tree，支持 Page 级持久化 |
| **远程存储** | `szrsql-storage/src/remote_fs.rs` | S3 / HTTPFS 远程文件系统 |
| **溢写盘** | `szrsql-storage/src/spill.rs` | 大结果集溢写到磁盘 |
| **备份恢复** | `szrsql-ops` + HTTP `/api/v1/backup` | 在线备份 |
| **WAL 压缩** | `szrsql-tx/src/wal_compression.rs` | zstd 压缩 WAL 记录 |
| **全页镜像（FPI）** | `szrsql-tx/src/wal_fpi.rs` | 页面首次修改时写全页镜像，防止部分写撕裂 |
| **WAL 摘要** | `szrsql-tx/src/wal_summarizer.rs` | WAL 摘要加速增量备份/复制 |
| **Buffer Pool** | `szrsql-storage/src/buffer.rs` | 页面缓存与淘汰 |
| **外部格式** | `szrsql-storage/src/external_format.rs` | Arrow IPC / Parquet / CSV / JSONLines 导入导出 |

### log-then-commit 事务模型

启用 `--wal-path` 后的 COMMIT 流程：

1. **写入 WAL Commit 记录**（`WalOpType::Commit = 3`）— `szrsql-tx/src/wal.rs`
2. **fsync 强制刷盘**（数据持久化到磁盘）— `WalWriter::flush()`
3. **向客户端返回 CommandComplete**（ACK 成功）
4. **释放所有行锁**（`unlock_all`）— Strict 2PL

**未设置 `--wal-path` 时**：退化为 commit-then-log（ACK 成功但数据可能未持久化），仅用于测试/兼容，**生产环境禁用**。

---
## 并发与锁

### 多线程并发执行

- pgwire 服务器支持多 session 并发，共享 `Arc<RwLock<HashMap<Table>>>`、`Arc<LockManager>`、`Arc<AtomicU32>`（事务 ID 计数器）
- 事务 ID 全局唯一（跨 session 共享 `AtomicU32` 递增）

### 行级锁（Strict 2PL）

| 锁模式 | 兼容性 | 说明 |
|--------|--------|------|
| **Shared（S）** | S 兼容；X 不兼容 | 读锁，`SELECT ... FOR SHARE` |
| **Exclusive（X）** | 与所有锁不兼容 | 写锁，`UPDATE/DELETE/SELECT ... FOR UPDATE` |
| **升级** | S → X | 写操作时自动升级 |

**Strict 2PL 协议**：
- 增长阶段：事务执行过程中按需获取行锁
- 收缩阶段：**只在 COMMIT 或 ABORT 时**统一释放所有锁（`unlock_all(txn_id)`）

### 死锁检测（DFS 环检测）

- **触发时机**：事务进入等待队列前立即检查；等待循环中周期性检查
- **算法**：等待图 DFS 环检测
- **处理**：检测到环 → 中止其中一个事务（返回 `LockError::Deadlock(txn_id)`），应用层重试
- 实现文件：`szrsql-tx/src/lock.rs`

```rust
// 死锁错误示例
Err(LockError::Deadlock(txn_id)) => {
    // 应用层：重试事务
}
```

---

## 方言兼容性

| 数据库 | 兼容等级 | 通过率 | 说明 |
|--------|---------|--------|------|
| **PostgreSQL** | L2（协议级兼容） | 89.6% (95/106) | pgwire v3.0 + SCRAM-SHA-256，客户端驱动可直接连接；5 项语法不支持、5 项数据类型未实现 |
| **MySQL** | L1（方言解析） | ~70%（估算） | 文本级预处理，无存储引擎兼容 |
| **SQLite** | L0（语法借鉴） | N/A | 无方言支持，仅 FTS5 风格全文检索借鉴 |
| **Oracle** | L1（方言解析） | ~60%（估算） | 文本级预处理（ROWNUM/DECODE/NVL） |
| **SQL Server** | L1（方言解析） | ~65%（估算） | 文本级预处理（TOP/ISNULL/GETDATE） |

**兼容等级定义**：
- **L0**：语法借鉴（无实际兼容性）
- **L1**：方言解析（能解析 SQL，但语义/行为可能不一致）
- **L2**：协议级兼容（客户端驱动可直接连接）
- **L3**：行为级兼容（SQL 语义、错误码、数据类型完全一致）

> 数据来源：`docs/兼容性评估报告.md`（基于 `szrsql-pgcompat` 测试套件实测）

---

## 安全特性

| 特性 | 实现文件 | 说明 |
|------|---------|------|
| **TDE 透明加密** | `szrsql-security/src/tde.rs` | AES-256-CTR 表空间级加密 |
| **列级加密** | `szrsql-security/src/column_enc.rs` | AES-256-GCM 认证加密 |
| **SCRAM-SHA-256** | `szrsql-protocol` | pgwire 认证协议 |
| **TLS 1.3** | `szrsql-protocol`（rustls + ring） | 传输层加密 |
| **SQL 注入防护** | `szrsql-security/src/sqli_detector.rs` | 注入检测引擎 |
| **DoS 防护** | `szrsql-security` | 连接数/查询复杂度限制 |
| **审计日志** | `szrsql-security/src/audit.rs` + `audit_hash.rs` | 防篡改审计（哈希链） |
| **防火墙** | `szrsql-security/src/firewall.rs` | IP/SQL 规则过滤 |
| **数据脱敏** | `szrsql-security/src/masking.rs` | 列级脱敏策略 |
| **密码策略** | `szrsql-security/src/password_profile.rs` | 密码强度/过期策略 |
| **合规** | `szrsql-security/src/compliance.rs` | 合规检查 |
| **unsafe 审计** | Workspace 禁用 `unsafe`（除 libc/windows-sys 系统调用） | 编译期门禁 |

---
## 性能基准

基于 `crates/szrsql-shadow/tests/bench_pgbench.rs` 与 PostgreSQL 18（127.0.0.1:5432）对拍测试。以下为 1K 行规模下的实测 P50 延迟（100K 行吞吐测试通过，未单独测 P50）：

| 操作 | 行数 | PG 18 P50 (ms) | SzRSQL P50 (ms) | sz/PG 比 | 备注 |
|------|------|----------------|-----------------|----------|------|
| **INSERT** | 1,000 | 0.327 | 0.072 | 0.22x | SzRSQL 快 4.5x |
| **SELECT** | 1,102 | 0.283 | 0.068 | 0.24x | SzRSQL 快 4.2x |
| **UPDATE** | 2,000 | 0.333 | 0.283 | 0.85x | SzRSQL 快 1.18x |
| **DELETE** | 2,000 | — | — | — | 吞吐测试通过（匹配率 100%） |
| **综合工作负载** | 302 | 0.293 | 0.105 | 0.36x | SzRSQL 快 2.8x |

**测试条件**：
- SzRSQL 运行于 `InMemoryTable` 模式（无 WAL，单线程）
- 数据规模：1K-100K 行，单线程工作负载
- 匹配率：100%（结果与 PG 18 完全一致）
- INSERT 100K 顺序插入吞吐测试通过

> 数据来源：`docs/PERF_BENCH_REPORT.md`（2026-07-25 实测）。P95/P99 详见报告。

---

## 文档索引

| 文档 | 说明 |
|------|------|
| `docs/ADR与生产Bug定位规范.md` | ADR 规范与生产 Bug 定位流程 |
| `docs/CHAOS_REPORT.md` | 混沌测试报告 |
| `docs/downgrade_plan.md` | 降级方案 |
| `docs/evaluation.md` | 评估报告 |
| `docs/FUZZ_REPORT.md` | 模糊测试报告 |
| `docs/MUTATION_REPORT.md` | 变异测试报告 |
| `docs/PERF_BENCH_REPORT.md` | 性能基准报告（与 PG 18 对拍） |
| `docs/PolarSSL对szrsql的参考价值分析.md` | PolarSSL 参考价值分析 |
| `docs/release_stages.md` | 发布阶段定义 |
| `docs/SHADOW_REPORT.md` | 影子比对测试报告 |
| `docs/SPEC_REVIEW_REPORT.md` | 规范评审报告 |
| `docs/szrsql-engineering-practices.md` | 工程实践规范 |
| `docs/全面排查汇总报告.md` | 全面排查汇总 |
| `docs/兼容性评估报告.md` | 数据库兼容性评估 |
| `docs/大数据量对比测试报告.md` | 大数据量对比测试 |
| `docs/对抗性边界审计清单.md` | 对抗性边界审计清单 |
| `docs/技能触发命令速查.md` | 技能触发命令速查 |
| `docs/软件项目审计清单.md` | 软件项目审计清单 |
| `docs/项目成熟度评估报告.md` | 项目成熟度评估 |

---

## 限制与已知问题

### 未实现的 PostgreSQL 功能

- **SQL 语法**（6 项）：`SUBSTRING` 函数、`ILIKE` 运算符、`SIMILAR TO` 运算符、`IS DISTINCT FROM`、`~` 正则匹配运算符、`UUID` 类型（语法层）
- **数据类型**（5 项）：`interval`、`bit`/`varbit`、`cidr`/`inet`、`point`/几何类型、`xml`
- **分布式**：分布式事务协调器为初步实现，未达到生产级一致性保证
- **复制**：物理/逻辑复制为基础实现，未支持级联复制与同步复制
- **守护进程**：Windows 不支持 `--daemon`（仅 Unix 双 fork + setsid）
- **大数据测试**：当前最大 100K 行，未做 1M+ 大数据量测试
- **多线程基准**：性能测试为单线程，未做多线程并发基准
- **方言兼容**：MySQL/Oracle/SQL Server 仅文本级预处理，无存储引擎兼容（L1）
- **PostGIS**：几何类型未实现，PostGIS 不可用

### 已知问题

- 未设置 `--wal-path` 时为 commit-then-log 模式，ACK 成功但数据可能未持久化（仅测试用）
- 守护进程化后 tracing stderr 输出被重定向到 `/dev/null`，需配置 file appender

---

## 许可证

MIT License（见 `Cargo.toml` 中 `license = "MIT"`）

Copyright (c) SzRSQL Team
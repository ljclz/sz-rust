> [中文版](README.md) | English

# SzRSQL

> A Rust-based distributed SQL database, compatible with the PostgreSQL protocol (pgwire v3.0), supporting MVCC transactions, WAL durability, B+Tree storage, row-level locks, and multi-dialect parsing.

SzRSQL is a distributed SQL database implemented in Rust, aiming to provide an embedded/standalone database service with PostgreSQL protocol-level compatibility (pgwire v3.0). The project adopts a Workspace multi-crate architecture, covering the complete database stack including SQL parsing, query optimization, storage engine, transaction management, WAL durability, replication, security, and operations.

- **Version**: v1.0.0-rc.1
- **License**: MIT
- **Rust Edition**: 2021
- **Compatible Protocols**: pgwire v3.0 (PostgreSQL 14 compatible), MySQL Wire Protocol v10, SQL Server TDS, SQLite file format, Oracle SQL dialect

---

## Table of Contents

- [Features Overview](#features-overview)
- [Quick Start](#quick-start)
- [Command-Line Arguments](#command-line-arguments)
- [Connection Examples](#connection-examples)
- [HTTP Management Endpoints](#http-management-endpoints)
- [Project Structure](#project-structure)
- [Build and Test](#build-and-test)
- [Data Durability](#data-durability)
- [Concurrency and Locking](#concurrency-and-locking)
- [Dialect Compatibility](#dialect-compatibility)
- [Security Features](#security-features)
- [Performance Benchmarks](#performance-benchmarks)
- [Distributed Transactions (Raft + Percolator)](#distributed-transactions-raft--percolator)
- [CDC Change Data Capture](#cdc-change-data-capture)
- [AI Capabilities](#ai-capabilities)
- [Replication and Disaster Recovery](#replication-and-disaster-recovery)
- [Backup and Recovery](#backup-and-recovery)
- [Documentation Index](#documentation-index)
- [Limitations and Known Issues](#limitations-and-known-issues)
- [License](#license)

---

## Features Overview

| Module | Capabilities |
|--------|--------------|
| **SQL Engine** | Multi-dialect parsing (PG/MySQL/Oracle/SQL Server/SQLite), AST, executor, PL/pgSQL interpreter, CTE/window functions/MERGE/UPSERT/RETURNING/SAVEPOINT |
| **Query Optimization** | Logical/physical plans, cost-based optimization, materialized view query rewrite, incremental maintenance |
| **Storage Engine** | B+Tree index, Buffer Pool, Freelist, Page management, TOAST, columnar storage, external formats (Arrow/Parquet/CSV/JSONLines) |
| **Transactions** | MVCC multi-version concurrency control, Strict 2PL row-level locks, deadlock detection (DFS cycle detection), Savepoint, globally unique transaction IDs |
| **Durability** | WAL write-ahead log, log-then-commit transaction model, WAL compression (zstd), Full Page Image (FPI), WAL summarizer, Group Commit |
| **Protocols** | pgwire v3.0, SCRAM-SHA-256 authentication, TLS 1.3 (rustls), extended protocol (Prepared/Portal), ParameterStatus, MySQL Wire Protocol (HandshakeV10 + mysql_native_password), SQL Server TDS protocol, SQLite file format read/write, Oracle SQL dialect bridge |
| **Distributed** | Distributed transactions, CDC change data capture, Schema Registry |
| **Indexes** | B+Tree, Bitmap, BRIN, GiST, full-text search (FTS5 style), spatial index, partial covering index |
| **Security** | TDE transparent encryption (AES-256-CTR), column-level encryption (AES-256-GCM), SCRAM-SHA-256, SQL injection detection, firewall, audit log, masking |
| **Replication** | Physical replication, logical replication, consumer offset management |
| **Operations** | Autovacuum, online DDL, upgrade migration, Shadow comparison |
| **AI** | AI-assisted query optimization, vectorized execution |
| **HTTP Management** | healthz/readyz/metrics, session management, backup, hot configuration reload |
| **Signals** | SIGTERM graceful shutdown, SIGINT immediate shutdown, Crash Handler (panic log + backtrace) |
| **Daemon** | Unix double fork + setsid daemonization, PID file RAII management, stale cleanup |

---

## Quick Start

### Requirements

- **Rust**: 1.81+ (Edition 2021)
- **OS**: Linux / macOS / Windows (daemon mode is Unix-only)
- **Dependencies**: Managed automatically by Cargo (tokio, sqlparser, rustls, aes-gcm, zstd, arrow/parquet, etc.)

### Build

```bash
# Development build (debug, overflow-checks enabled)
cargo build

# Release build (LTO enabled)
cargo build --release

# Check only (no binary output)
cargo check --workspace
```

### Start the Service

```bash
# 1. Minimal startup (default 127.0.0.1:5432, no WAL, for testing only)
cargo run -p szrsql-bin

# 2. Production startup (WAL log-then-commit + HTTP management + remote binding)
cargo run -p szrsql-bin -- \
  --host 0.0.0.0 \
  --port 5432 \
  --wal-path /var/lib/szrsql/wal.log \
  --http-port 8080 \
  --http-host 127.0.0.1 \
  --http-auth-token "$SZRSQL_HTTP_TOKEN"

# 3. Daemon mode (Unix, background + PID file)
cargo run -p szrsql-bin -- \
  --daemon \
  --pid-file /var/run/szrsql.pid \
  --wal-path /var/lib/szrsql/wal.log \
  --http-port 8080

# 4. Show version
cargo run -p szrsql-bin -- --version
```

### Connection Methods

```bash
# Connect with psql
psql -h 127.0.0.1 -p 5432 -U postgres

# Connect with Python asyncpg
python -c "import asyncio, asyncpg; \
  asyncio.run(asyncpg.connect('postgresql://postgres@127.0.0.1:5432'))"
```

> **Note**: When `--wal-path` is not specified, the server runs in commit-then-log mode (ACK succeeds but data may not be persisted). **In production, always set `--wal-path`**.

---

## Command-Line Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--host` | String | `127.0.0.1` | pgwire listen address |
| `--port` | u16 | `5432` | pgwire listen port |
| `--server-version` | String | `14.0-szrsql` | server_version ParameterStatus (sent to client) |
| `--shutdown-timeout` | u64 | `30` | Graceful shutdown timeout (seconds); max wait for active connections after SIGTERM, force-abort on expiry |
| `--crash-log-dir` | PathBuf | `.` | Crash log output directory (writes log with backtrace + WAL LSN on panic) |
| `--no-backtrace` | bool | `false` | Disable backtrace capture (reduces panic hook overhead) |
| `--daemon` | bool | `false` | Daemon mode (Unix double fork + setsid; not supported on Windows) |
| `--pid-file` | PathBuf | `None` | PID file path (prevents duplicate startup, stale cleanup, auto-removed on exit) |
| `--http-port` | u16 | `0` | HTTP management port (0 = do not listen) |
| `--http-host` | String | `127.0.0.1` | HTTP listen address (recommend binding locally only) |
| `--http-auth-token` | String | `None` | Bearer token auth for `/api/v1/*` endpoints (healthz/readyz/metrics do not require auth) |
| `--wal-path` | PathBuf | `None` | WAL file path; enables log-then-commit transaction model (**strongly recommended in production**) |

**Signal Behavior**:
- `SIGTERM` (Unix) / `Ctrl+C` (Windows) → Graceful shutdown (drain active connections, up to `shutdown-timeout`)
- `SIGINT` / `Ctrl+C` (Unix) → Immediate shutdown (does not wait for active transactions, abort_all directly)
- `panic` → Writes crash log to `--crash-log-dir`

---

## Connection Examples

### Rust (tokio-postgres)

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

    // Create table
    client.batch_execute(
        "CREATE TABLE IF NOT EXISTS accounts (
            id BIGINT PRIMARY KEY,
            name TEXT,
            balance BIGINT
        )"
    ).await?;

    // Insert
    client.execute(
        "INSERT INTO accounts (id, name, balance) VALUES ($1, $2, $3)",
        &[&1i64, &"alice", &1000i64],
    ).await?;

    // Query
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

### Transaction Example (Strict 2PL + Deadlock Detection)

```rust
// Transfer transaction: from -> to, amount
// SzRSQL releases all row locks at COMMIT/ABORT via unlock_all() (Strict 2PL)
// If two transactions wait on each other cyclically, DFS cycle detection aborts one (returns deadlock error)
async fn transfer(
    client: &tokio_postgres::Client,
    from: i64,
    to: i64,
    amount: i64,
) -> Result<(), tokio_postgres::Error> {
    let tx = client.transaction().await?;

    // Acquire exclusive locks on rows (FOR UPDATE), held until COMMIT
    tx.execute("UPDATE accounts SET balance = balance - $1 WHERE id = $2", &[&amount, &from]).await?;
    tx.execute("UPDATE accounts SET balance = balance + $1 WHERE id = $2", &[&amount, &to]).await?;

    // Verify non-negative balance
    let row = tx.query_one("SELECT balance FROM accounts WHERE id = $1", &[&from]).await?;
    let balance: i64 = row.get(0);
    if balance < 0 {
        tx.rollback().await?; // ABORT: auto-release all row locks
        return Err(tokio_postgres::Error::封锁("insufficient balance".into()));
    }

    tx.commit().await?; // COMMIT: release all row locks after WAL fsync
    Ok(())
}
```

> **Deadlock Handling**: When `transfer(A, B)` and `transfer(B, A)` execute concurrently forming a wait cycle, `LockManager::detect_deadlock` detects the cycle via DFS, aborts one transaction and returns `LockError::Deadlock(txn_id)`. The application layer should catch and retry.

---

## HTTP Management Endpoints

Available when `--http-port` is enabled. `/api/v1/*` endpoints require the `Authorization: Bearer <token>` header (when `--http-auth-token` is set).

| Endpoint | Method | Auth | Description |
|----------|--------|------|-------------|
| `/healthz` | GET | None | Liveness probe (K8s liveness) |
| `/readyz` | GET | None | Readiness probe (K8s readiness) |
| `/metrics` | GET | None | Prometheus metrics scrape |
| `/api/v1/sessions` | GET | Bearer | List active sessions |
| `/api/v1/cancel/{pid}` | POST | Bearer | Cancel query for a given PID |
| `/api/v1/backup` | POST | Bearer | Trigger backup |
| `/api/v1/config/reload` | POST | Bearer | Hot configuration reload |

```bash
# Example: query sessions (with auth)
curl -H "Authorization: Bearer $SZRSQL_HTTP_TOKEN" \
     http://127.0.0.1:8080/api/v1/sessions

# Prometheus scrape (no auth required)
curl http://127.0.0.1:8080/metrics
```

---

## Project Structure

```
szrsql/
├── crates/                          # Workspace sub-crates (16 libraries + 1 binary)
│   ├── szrsql-types/                # Shared type definitions (data types, error types)
│   ├── szrsql-storage/              # Storage engine (B+Tree, Buffer Pool, Page, Freelist, TOAST, columnar, remote FS, spill)
│   ├── szrsql-tx/                   # Transactions (MVCC, row locks, WAL, deadlock detection, Vacuum, Undo, Schema Registry, CDC)
│   ├── szrsql-cdc/                  # Change data capture
│   ├── szrsql-sql/                  # SQL engine (Parser, AST, Executor, PL/pgSQL, indexes, triggers, partitioning)
│   ├── szrsql-catalog/              # System catalog (table/column/index metadata)
│   ├── szrsql-protocol/             # pgwire v3.0 protocol, HTTP management service, signal handling, daemon
│   ├── szrsql-optimizer/            # Query optimizer (logical/physical plan, cost model)
│   ├── szrsql-ai/                   # AI-assisted optimization, vectorized execution
│   ├── szrsql-security/             # Security (TDE, column-level encryption, SCRAM, firewall, audit, masking, SQL injection detection)
│   ├── szrsql-scheduler/            # Task scheduling
│   ├── szrsql-replication/          # Physical replication, logical replication
│   ├── szrsql-dist/                 # Distributed transactions, node management
│   ├── szrsql-pgcompat/             # PostgreSQL compatibility test suite
│   ├── szrsql-ops/                  # Operations (Autovacuum, online DDL, upgrade)
│   ├── szrsql-shadow/               # Shadow comparison (differential testing against PostgreSQL 18)
│   └── szrsql-bin/                  # Binary entry (main.rs / health_main.rs)
├── docs/                            # Design documents and test reports
├── benchmarks/                      # Performance benchmarks
├── fuzz/                            # Fuzz testing
├── ci/                              # CI configuration
├── deploy/                          # Deployment configuration
├── scripts/                         # Helper scripts
├── tools/                           # Tools
├── Cargo.toml                       # Workspace manifest
├── deny.toml                        # cargo-deny configuration
├── rustfmt.toml                     # Formatting configuration
└── Dockerfile / docker-compose.yml  # Containerization
```

---

## Build and Test

```bash
# 1. Static check
cargo check --workspace

# 2. Lint (Clippy, zero-warning gate)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Format check
cargo fmt --all -- --check

# 4. Unit + integration tests
cargo test --workspace

# 5. Concurrency integration tests (multi-session shared state across connections)
cargo test -p szrsql-protocol --test concurrency -- --test-threads=4

# 6. PostgreSQL compatibility tests
cargo run -p szrsql-pgcompat --example print_report

# 7. Performance benchmark (differential testing against PostgreSQL 18)
cargo test -p szrsql-shadow --test bench_pgbench -- --nocapture

# 8. Mutation testing (cargo-mutants)
cargo mutants --workspace

# 9. Fuzz testing (cargo-fuzz, requires nightly)
cargo +nightly fuzz run btree_fuzz
cargo +nightly fuzz run wal_fuzz
cargo +nightly fuzz run mvcc_fuzz

# 10. Dependency audit
cargo deny check
```

---

## Data Durability

> **SzRSQL is not an in-memory database**. When `--wal-path` is enabled, all COMMIT operations first write the WAL and `fsync` before ACKing the client, ensuring data is persisted to disk.

| Mechanism | Implementation File | Description |
|-----------|---------------------|-------------|
| **WAL Write-Ahead Log** | `szrsql-tx/src/wal.rs` | Sequential append-only writes; Commit record fsync before returning success |
| **B+Tree Persistence** | `szrsql-storage/src/btree.rs` | Index/data B+Tree, supports Page-level persistence |
| **Remote Storage** | `szrsql-storage/src/remote_fs.rs` | S3 / HTTPFS remote filesystem |
| **Spill to Disk** | `szrsql-storage/src/spill.rs` | Large result sets spill to disk |
| **Backup and Recovery** | `szrsql-ops` + HTTP `/api/v1/backup` | Online backup |
| **WAL Compression** | `szrsql-tx/src/wal_compression.rs` | zstd-compressed WAL records |
| **Full Page Image (FPI)** | `szrsql-tx/src/wal_fpi.rs` | Writes full page image on first modification to prevent partial-write tearing |
| **WAL Summarizer** | `szrsql-tx/src/wal_summarizer.rs` | WAL summary to speed up incremental backup/replication |
| **Buffer Pool** | `szrsql-storage/src/buffer.rs` | Page cache and eviction |
| **External Formats** | `szrsql-storage/src/external_format.rs` | Arrow IPC / Parquet / CSV / JSONLines import and export |

### log-then-commit Transaction Model

COMMIT flow when `--wal-path` is enabled:

```
Client sends COMMIT
      │
      ▼
┌─────────────────────────────┐
│ 1. Write WAL Commit record   │  szrsql-tx/src/wal.rs
│    (WalOpType::Commit = 3)   │
└─────────────────────────────┘
      │
      ▼
┌─────────────────────────────┐
│ 2. fsync force flush         │  WalWriter::flush()
│    (persist data to disk)    │
└─────────────────────────────┘
      │
      ▼
┌─────────────────────────────┐
│ 3. Return CommandComplete   │  ACK success
│    to client                 │
└─────────────────────────────┘
      │
      ▼
┌─────────────────────────────┐
│ 4. Release all row locks     │  Strict 2PL
│    (unlock_all)              │
└─────────────────────────────┘
```

**When `--wal-path` is not set**: Degrades to commit-then-log (ACK succeeds but data may not be persisted), for testing/compatibility only, **not for production**.

---

## Concurrency and Locking

### Multi-threaded Concurrent Execution

- The pgwire server supports multi-session concurrency, sharing `Arc<RwLock<HashMap<Table>>>`, `Arc<LockManager>`, `Arc<AtomicU32>` (transaction ID counter)
- Transaction IDs are globally unique (cross-session shared `AtomicU32` increment)

### Row-Level Locks (Strict 2PL)

| Lock Mode | Compatibility | Description |
|-----------|---------------|-------------|
| **Shared (S)** | Compatible with S; incompatible with X | Read lock, `SELECT ... FOR SHARE` |
| **Exclusive (X)** | Incompatible with all locks | Write lock, `UPDATE/DELETE/SELECT ... FOR UPDATE` |
| **Upgrade** | S → X | Auto-upgrade on write operations |

**Strict 2PL Protocol**:
- Growing phase: Acquire row locks on demand during transaction execution
- Shrinking phase: **Only at COMMIT or ABORT** release all locks at once (`unlock_all(txn_id)`)

### Deadlock Detection (DFS Cycle Detection)

- **Trigger timing**: Immediate check before a transaction enters the wait queue; periodic check in the wait loop
- **Algorithm**: Wait-for graph DFS cycle detection
- **Handling**: On cycle detected → abort one transaction (return `LockError::Deadlock(txn_id)`), application layer retries
- Implementation file: `szrsql-tx/src/lock.rs`

```rust
// Deadlock error example
Err(LockError::Deadlock(txn_id)) => {
    // Application layer: retry the transaction
}
```

---

## Dialect Compatibility

| Database | Compatibility Level | Pass Rate | Description |
|----------|---------------------|-----------|-------------|
| **PostgreSQL** | L2 (protocol-level compatibility) | 89.6% (95/106) | pgwire v3.0 + SCRAM-SHA-256, client drivers can connect directly; 5 syntax features unsupported, 5 data types not implemented |
| **MySQL** | L1 (dialect parsing) | ~70% (estimated) | Text-level preprocessing, no storage engine compatibility |
| **SQLite** | L0 (syntax borrowing) | N/A | No dialect support, only FTS5-style full-text search borrowed |
| **Oracle** | L1 (dialect parsing) | ~60% (estimated) | Text-level preprocessing (ROWNUM/DECODE/NVL) |
| **SQL Server** | L1 (dialect parsing) | ~65% (estimated) | Text-level preprocessing (TOP/ISNULL/GETDATE) |

**Compatibility Level Definitions**:
- **L0**: Syntax borrowing (no actual compatibility)
- **L1**: Dialect parsing (can parse SQL, but semantics/behavior may differ)
- **L2**: Protocol-level compatibility (client drivers can connect directly)
- **L3**: Behavior-level compatibility (SQL semantics, error codes, data types fully consistent)

> Data source: `docs/兼容性评估报告.md` (based on `szrsql-pgcompat` test suite measurements)

---

## Security Features

| Feature | Implementation File | Description |
|---------|---------------------|-------------|
| **TDE Transparent Encryption** | `szrsql-security/src/tde.rs` | AES-256-CTR tablespace-level encryption |
| **Column-Level Encryption** | `szrsql-security/src/column_enc.rs` | AES-256-GCM authenticated encryption |
| **SCRAM-SHA-256** | `szrsql-protocol` | pgwire authentication protocol |
| **TLS 1.3** | `szrsql-protocol` (rustls + ring) | Transport layer encryption |
| **SQL Injection Protection** | `szrsql-security/src/sqli_detector.rs` | Injection detection engine |
| **DoS Protection** | `szrsql-security` | Connection count/query complexity limits |
| **Audit Log** | `szrsql-security/src/audit.rs` + `audit_hash.rs` | Tamper-proof audit (hash chain) |
| **Firewall** | `szrsql-security/src/firewall.rs` | IP/SQL rule filtering |
| **Data Masking** | `szrsql-security/src/masking.rs` | Column-level masking policy |
| **Password Policy** | `szrsql-security/src/password_profile.rs` | Password strength/expiration policy |
| **Compliance** | `szrsql-security/src/compliance.rs` | Compliance checks |
| **unsafe Audit** | Workspace disables `unsafe` (except libc/windows-sys syscalls) | Compile-time gate |

---

## Performance Benchmarks

Based on `crates/szrsql-shadow/tests/bench_pgbench.rs` differential testing against PostgreSQL 18 (127.0.0.1:5432). Below are measured P50 latencies at 1K row scale (100K row throughput tests passed, P50 not measured separately):

| Operation | Rows | PG 18 P50 (ms) | SzRSQL P50 (ms) | sz/PG Ratio | Notes |
|-----------|------|----------------|-----------------|-------------|-------|
| **INSERT** | 1,000 | 0.327 | 0.072 | 0.22x | SzRSQL 4.5x faster |
| **SELECT** | 1,102 | 0.283 | 0.068 | 0.24x | SzRSQL 4.2x faster |
| **UPDATE** | 2,000 | 0.333 | 0.283 | 0.85x | SzRSQL 1.18x faster |
| **DELETE** | 2,000 | — | — | — | Throughput test passed (100% match rate) |
| **Mixed Workload** | 302 | 0.293 | 0.105 | 0.36x | SzRSQL 2.8x faster |

**Test Conditions**:
- SzRSQL running in `InMemoryTable` mode (no WAL, single-threaded)
- Data scale: 1K-100K rows, single-threaded workload
- Match rate: 100% (results fully consistent with PG 18)
- INSERT 100K sequential insert throughput test passed

> Data source: `docs/archive/phase-7-排查报告/PERF_BENCH_REPORT.md` (measured 2026-07-25) + `docs/大数据量对比测试报告.md` (1K-10M rows). See report for P95/P99 details.

---

## Distributed Transactions (Raft + Percolator)

SzRSQL provides a complete distributed stack, built from scratch (no dependency on etcd/TiKV):

| Capability | Implementation File | Description |
|------------|---------------------|-------------|
| **Raft Consensus** | `szrsql-dist/src/raft.rs` | Leader election (<10s), Log Replication, Multi-Raft; follows paper §5.2-5.4 |
| **Percolator Distributed Tx** | `szrsql-dist/src/txn.rs` | Timestamp Oracle / Prewrite / Commit; cross-shard ACID; auto lock cleanup on coordinator failure |
| **HLC Hybrid Logical Clock** | `szrsql-dist/src/conflict.rs` | Causal consistency; O(1) metadata; no dedicated hardware required (see ADR 0005) |
| **Range Sharding** | `szrsql-dist/src/shard.rs` | Dynamic split/merge; friendly to range queries (see ADR 0006) |
| **Conflict Detection and Resolution** | `szrsql-dist/src/conflict.rs` | Distributed transaction conflict detection |

**Consistency Verification**:
- Jepsen Bank test (total conservation)
- Jepsen Register test (read-write consistency)
- Jepsen Set test
- Crash recovery fuzz testing (kill-9 scenarios, 28 tests passed)

See ADR 0003 (Raft) / 0004 (Percolator) / 0005 (HLC) / 0006 (Range Sharding) for details.

---

## CDC Change Data Capture

Listens to WAL changes via WalObserver and emits standardized change events:

| Capability | Implementation File | Description |
|------------|---------------------|-------------|
| **CdcEngine** | `szrsql-cdc/src/lib.rs` | WalObserver + ChangeEvent (Insert/Update/Delete/Commit/Abort) |
| **Backpressure Control** | `szrsql-cdc/src/backpressure.rs` | Auto rate-limiting when consumers are slow, preventing OOM |
| **Schema Evolution** | `szrsql-cdc/src/schema.rs` | CDC continues uninterrupted on schema changes |
| **Debezium JSON** | `szrsql-cdc/src/debezium.rs` | Debezium protocol compatible, can integrate with Kafka Connect |
| **Debezium + Avro** | `szrsql-cdc/src/debezium_avro.rs` | Avro serialization (Schema Registry integration) |
| **Failover** | `szrsql-cdc/src/failover.rs` | Automatic consumer failover |

**Consumer Offset Management**: `szrsql-tx/src/consumer_offset.rs` persists consumption offsets, resuming from breakpoint after restart.

---

## AI Capabilities

SzRSQL has built-in AI assistance capabilities, requiring no external services:

| Capability | Implementation File | Description |
|------------|---------------------|-------------|
| **NL2SQL** | `szrsql-ai/src/nl2sql.rs` | Natural language to SQL (`Nl2SqlEngine::translate`) |
| **RAG Retrieval Augmentation** | `szrsql-ai/src/rag.rs` | Retrieval-augmented generation (`RagDocument` / `RagAnswer`) |
| **Vector Embedding** | `szrsql-ai/src/embedding.rs` | Text/image vectorization |
| **HNSW Vector Index** | `szrsql-ai/src/index/hnsw_accel.rs` | High-dimensional vector nearest neighbor search (Scalar + SIMD dual backends) |
| **MCP Server** | `szrsql-ai/src/mcp.rs` + `mcp_server.rs` | Model Context Protocol, integrates with AI assistants |
| **LLM Cache** | `szrsql-ai/src/llm_cache.rs` | Caches LLM responses, reducing call cost |
| **Auto Index** | `szrsql-ai/src/auto_index.rs` | Automatic index recommendation and creation |
| **Auto Ops** | `szrsql-ai/src/auto_ops.rs` | Autonomous operations (Vacuum/Analyze scheduling) |

---

## Replication and Disaster Recovery

| Capability | Implementation File | Description |
|------------|---------------------|-------------|
| **Physical Streaming Replication** | `szrsql-replication/src/stream.rs` | Primary-replica replication; `ReplicationPrimary` manages WAL stream |
| **Logical Replication** | `szrsql-tx/src/consumer_offset.rs` | Consumer offset management; supports breakpoint resumption |
| **Rolling Upgrade** | `szrsql-replication/src/rolling.rs` | Zero-downtime upgrade (upgrade replicas first, then promote) |
| **Disaster Recovery (DR)** | `szrsql-replication/src/dr.rs` | Remote disaster recovery |
| **Physical Backup** | `szrsql-replication/src/backup.rs` | Baseline backup + incremental backup |

**Backup Trigger**: Via HTTP API `POST /api/v1/backup` (requires Bearer token auth).

---

## Backup and Recovery

### Online Physical Backup

```bash
# Trigger baseline + incremental backup
curl -X POST -H "Authorization: Bearer $SZRSQL_HTTP_TOKEN" \
     http://127.0.0.1:8080/api/v1/backup
```

### Backup Strategy

- **Baseline backup**: Full data snapshot (`backup.rs`)
- **Incremental backup**: Incremental push based on WAL summary (`wal_summarizer.rs`)
- **Remote storage**: Backups can be pushed to S3 / HTTPFS (`remote_fs.rs`)

### Crash Recovery Flow

When `--wal-path` is enabled, crash recovery runs automatically on startup:

```
1. Read WAL file
      │
      ▼
2. Replay WAL records (WalReader)
      │  - Skip Abort records
      │  - Apply Insert/Update/Delete records
      │  - Apply Commit records (mark transaction visible)
      ▼
3. Repair partial-write tearing using Full Page Image (FPI)
      │
      ▼
4. zstd decompress WAL records
      │
      ▼
5. Recovery complete, start accepting new connections
```

**kill-9 crash recovery test**: 28 tests passed, covering combined WAL + MVCC crash scenarios.

---

## Documentation Index

### Top-Level Documents (docs/)

| Document | Description |
|---------|-------------|
| `docs/项目成熟度评估报告.md` | Project maturity assessment (based on code verification, periodically updated) |
| `docs/全面排查汇总报告.md` | Comprehensive investigation summary (5 major skill results) |
| `docs/兼容性评估报告.md` | Database compatibility assessment (PG 89.6%) |
| `docs/大数据量对比测试报告.md` | Large-scale data comparison tests (1K-10M rows × 5 databases) |
| `docs/对抗性边界审计清单.md` | Adversarial boundary audit checklist (60 audit cases) |
| `docs/ADR与生产Bug定位规范.md` | ADR specification and production bug localization process (v1.1.0) |
| `docs/release_stages.md` | Release stage definitions (Alpha→Beta→RC→GA) |
| `docs/downgrade_plan.md` | Downgrade plan (Patch/Minor/Major three categories) |
| `docs/szrsql-engineering-practices.md` | Engineering practice specification (7 gates + database hardening) |

### ADR Decision Records (docs/adr/)

| ADR | Title |
|-----|-------|
| 0001 | Durability Model: log-then-commit migration path |
| 0002 | MVCC over 2PL |
| 0003 | Raft Consensus |
| 0004 | Percolator Distributed Tx |
| 0005 | HLC Clock |
| 0006 | Range Sharding |
| 0007 | Identifier Escaping |
| 0008 | Page Size 16KB |
| 0009 | WAL Group Commit |
| 0010 | Buffer Pool Sharded LRU |

### Archived Documents (docs/archive/)

| Document | Description |
|---------|-------------|
| `archive/phase-7-排查报告/` | 6 phase test reports (CHAOS/FUZZ/MUTATION/PERF_BENCH/SHADOW/SPEC_REVIEW) |
| `archive/phase-1/` | SZ-ORM phase history (jwt-test / sz-orm-deps / toolchain) |
| `archive/PolarSSL对szrsql的参考价值分析.md` | Security layer architecture reference |
| `archive/技能触发命令速查.md` | 5 major skill trigger command quick reference |
| `archive/软件项目审计清单.md` | Long-term audit checklist (P0-P3 seven-level classification) |

---

## Limitations and Known Issues

### Unimplemented PostgreSQL Features

- **SQL Syntax** (6 items): `SUBSTRING` function, `ILIKE` operator, `SIMILAR TO` operator, `IS DISTINCT FROM`, `~` regex match operator, `UUID` type (at syntax level)
- **Data Types** (5 items): `interval`, `bit`/`varbit`, `cidr`/`inet`, `point`/geometric types, `xml`
- **Isolation Levels**: Only 3 (ReadCommitted / RepeatableRead / Serializable), **missing ReadUncommitted**
- **Daemon**: Windows does not support `--daemon` (Unix double fork + setsid only)
- **Multi-threaded Benchmarks**: Performance tests are single-threaded; no multi-threaded concurrent benchmarks
- **Dialect Compatibility**: Only PostgreSQL reaches L2 (protocol-level); MySQL/Oracle/SQL Server/SQLite are L1 (dialect parsing, dialect-compat has only 25 tests)
- **PostGIS**: Geometric types not implemented; PostGIS unavailable
- **Not Published to crates.io**: Version 0.1.0, no API stability commitment
- **No Production Cases**: No real online business validation
- **No Third-Party Security Audit**: Only self-tests, lacking external audit reports

### Known Issues

- Without `--wal-path` set, runs in commit-then-log mode where ACK succeeds but data may not be persisted (for testing only)
- After daemonization, tracing stderr output is redirected to `/dev/null`; needs file appender configuration

---

## License

MIT License (see `license = "MIT"` in `Cargo.toml`)

Copyright (c) SzRSQL Team

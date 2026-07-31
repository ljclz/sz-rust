//! P7-2 真实 PostgreSQL 端到端往返集成测试 — 验证源端 CDC → 目标端写入全链路
//!
//! # 测试目标
//!
//! 1. 验证 `PgRealSourceConnector` 能从真实 PG 18 源端发现 schema / 抽取快照 / 捕获 CDC 增量
//! 2. 验证 `PgRealWriter` 能向真实 PG 18 目标端建表 / 写入 DML / 健康检查
//! 3. 验证端到端一致性：源端 INSERT/UPDATE/DELETE → 目标端数据状态完全一致
//! 4. 验证幂等性：重复写入不产生副作用
//! 5. 验证位点管理：ack_offset / confirmed_offset 支持断点续传
//!
//! # 测试架构
//!
//! ```
//! ┌────────────────┐     CDC 事件流     ┌──────────────────┐
//! │  PG 18 源端    │ ──────────────────> │  PG 18 目标端    │
//! │  (sz_orm_test) │   触发器 + 日志表   │  (sz_orm_test)   │
//! └────────────────┘                     └──────────────────┘
//!        │                                        │
//!        └───────── 数据一致性比对 ──────────────┘
//! ```
//!
//! 源端和目标端位于同一数据库的不同表（`cdc_src_*` vs `cdc_dst_*`），
//! 避免跨数据库连接的开销，同时验证 CDC 全链路。
//!
//! # 运行前置条件
//!
//! - PostgreSQL 18 运行在 127.0.0.1:5432
//! - 连接串：`postgresql://postgres:test123@127.0.0.1:5432/sz_orm_test`
//! - 数据库 `sz_orm_test` 已存在
//!
//! # 运行方式
//!
//! ```bash
//! cargo test -p szrsql-cdc --test integration_real_pg -- --nocapture --test-threads=1
//! ```

use postgres::NoTls;
use std::sync::{Arc, Mutex};
use szrsql_cdc::decoder::DecodedRow;
use szrsql_cdc::schema::TableSchema;
use szrsql_cdc::source::pg_real::PgRealSourceConnector;
use szrsql_cdc::source::{SourceConfig, SourceConnector, SourceEventOp, SourceOffset};
use szrsql_cdc::target::pg_real::PgRealWriter;
use szrsql_cdc::target::TargetWriter;
use szrsql_types::value::Value as SzValue;

/// 全局串行锁 — 所有集成测试操作相同的源/目标表，并发执行会导致表冲突。
static PG_RT_LOCK: Mutex<()> = Mutex::new(());

/// 在测试函数开头调用，获取全局锁
fn lock_pg_rt() -> std::sync::MutexGuard<'static, ()> {
    PG_RT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// PG 连接串（对应 AGENTS.md 中的本机 PostgreSQL 18）
const PG_URL: &str = "postgresql://postgres:test123@127.0.0.1:5432/sz_orm_test";

/// 尝试连接 PG，返回连接客户端；失败则返回 None（测试将跳过）
fn try_pg() -> Option<postgres::Client> {
    match postgres::Client::connect(PG_URL, NoTls) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("[integration_real_pg] 跳过：无法连接 PG ({e})");
            None
        }
    }
}

/// 源端测试表名
const SRC_TABLE: &str = "cdc_rt_src_users";
/// 目标端测试表名
const DST_TABLE: &str = "cdc_rt_dst_users";
/// 源端测试表 2（用于多表场景）
const SRC_TABLE2: &str = "cdc_rt_src_orders";
const DST_TABLE2: &str = "cdc_rt_dst_orders";

/// 清理所有测试表 + 触发器 + CDC 日志表
fn cleanup_all(client: &mut postgres::Client) {
    let tables = [SRC_TABLE, DST_TABLE, SRC_TABLE2, DST_TABLE2];
    for t in tables {
        let _ = client.batch_execute(&format!("DROP TRIGGER IF EXISTS _szrsql_cdc_trg_{t} ON {t};"));
        let _ = client.batch_execute(&format!("DROP TABLE IF EXISTS {t} CASCADE;"));
    }
    // 清空 CDC 日志表（保留表结构，便于下次使用）
    let _ = client.batch_execute("TRUNCATE TABLE _szrsql_cdc_log;");
}

/// 创建源端测试表（带主键，便于 UPDATE/DELETE）
fn create_src_table(client: &mut postgres::Client, table: &str) {
    let sql = format!(
        "CREATE TABLE {table} (
            id BIGINT PRIMARY KEY,
            name TEXT NOT NULL,
            age INTEGER,
            score DOUBLE PRECISION,
            active BOOLEAN DEFAULT TRUE,
            created_at TIMESTAMP DEFAULT NOW()
        );",
        table = table
    );
    client.batch_execute(&sql).expect("create_src_table failed");
}

/// 验证目标端表数据与预期 SQL 查询结果一致
fn query_count(client: &mut postgres::Client, table: &str) -> i64 {
    let rows = client
        .query(&format!("SELECT COUNT(*) FROM {table}"), &[])
        .expect("query_count failed");
    rows[0].get::<_, i64>(0)
}

/// 查询单列单行的字符串值
fn query_one_str(client: &mut postgres::Client, sql: &str) -> String {
    let rows = client.query(sql, &[]).expect("query_one_str failed");
    if rows.is_empty() {
        return String::new();
    }
    rows[0].get::<_, String>(0)
}

// =====================================================================
// 集成测试用例
// =====================================================================

/// 集成测试 1：Schema 发现 — discover_schemas 从真实 PG 18 提取表结构
///
/// **流程**：
/// 1. 在源端建表（包含多种类型：BIGINT/TEXT/INTEGER/DOUBLE PRECISION/BOOLEAN/TIMESTAMP）
/// 2. 调用 `PgRealSourceConnector::discover_schemas` 提取 schema
/// 3. 验证列名、类型、可空性正确
#[test]
fn integration_real_pg_schema_discovery() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);

    // 创建源端连接器
    let source = PgRealSourceConnector::connect(
        PG_URL,
        SourceConfig::postgres(PG_URL),
        NoTls,
    )
    .expect("PgRealSourceConnector::connect failed");
    source.connect().expect("source.connect failed");

    // 发现 schema
    let schemas = source
        .discover_schemas(&[SRC_TABLE.to_string()])
        .expect("discover_schemas failed");
    assert_eq!(schemas.len(), 1, "应发现 1 张表");
    let schema = &schemas[0];
    assert_eq!(schema.table_name, SRC_TABLE);
    assert!(schema.columns.len() >= 5, "应至少有 5 列");

    // 验证关键列存在
    let col_names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(col_names.contains(&"id"), "缺少 id 列");
    assert!(col_names.contains(&"name"), "缺少 name 列");
    assert!(col_names.contains(&"age"), "缺少 age 列");

    // 验证 id 列不可空
    let id_col = schema.columns.iter().find(|c| c.name == "id").unwrap();
    assert!(!id_col.nullable, "id 列应 NOT NULL");

    // 清理
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 2：全量快照抽取 — extract_snapshot 从源端拉取全表数据
///
/// **流程**：
/// 1. 在源端建表并插入 5 行数据
/// 2. 调用 `extract_snapshot` 抽取
/// 3. 验证返回行数 = 5，且每行数据正确
#[test]
fn integration_real_pg_snapshot_extraction() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);

    // 插入 5 行数据
    for i in 1..=5i64 {
        let age: i32 = 20 + i as i32;
        let score: f64 = i as f64 * 1.5;
        let sql = format!(
            "INSERT INTO {SRC_TABLE} (id, name, age, score, active) VALUES ({i}, 'user{i}', {age}, {score}, TRUE);"
        );
        client.batch_execute(&sql).expect("insert failed");
    }

    let source = PgRealSourceConnector::connect(
        PG_URL,
        SourceConfig::postgres(PG_URL),
        NoTls,
    )
    .expect("source connect failed");
    source.connect().expect("source.connect failed");
    source
        .discover_schemas(&[SRC_TABLE.to_string()])
        .expect("discover_schemas failed");

    // 抽取快照
    let collected = Arc::new(Mutex::new(Vec::<DecodedRow>::new()));
    let collected_clone = collected.clone();
    let total = source
        .extract_snapshot(SRC_TABLE, 100, &move |rows| {
            let mut buf = collected_clone.lock().unwrap();
            for r in rows {
                buf.push(r.clone());
            }
            Ok(())
        })
        .expect("extract_snapshot failed");

    assert_eq!(total, 5, "应抽取 5 行");
    let rows = collected.lock().unwrap();
    assert_eq!(rows.len(), 5, "回调应收集 5 行");

    // 验证第一行的 id 列
    let first_row = &rows[0];
    assert!(
        first_row
            .columns
            .iter()
            .any(|(n, _)| n == "id"),
        "第一行应包含 id 列"
    );

    // 清理
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 3：CDC 触发器安装 — install_cdc_triggers 在源表上创建触发器
///
/// **流程**：
/// 1. 建源表
/// 2. 调用 `install_cdc_triggers` 安装触发器
/// 3. 在源表上 INSERT 一行
/// 4. 查询 `_szrsql_cdc_log` 验证触发器捕获了变更
#[test]
fn integration_real_pg_cdc_trigger_install() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);

    let source = PgRealSourceConnector::connect(
        PG_URL,
        SourceConfig::postgres(PG_URL),
        NoTls,
    )
    .expect("source connect failed");
    source.connect().expect("source.connect failed");

    // 安装触发器
    source
        .install_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("install_cdc_triggers failed");

    // 在源表上 INSERT 一行
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE} (id, name, age) VALUES (1, 'alice', 30);"
        ))
        .expect("insert failed");

    // 验证 _szrsql_cdc_log 表捕获了 INSERT 事件
    let rows = client
        .query(
            "SELECT table_name, op FROM _szrsql_cdc_log WHERE table_name = $1 ORDER BY id DESC LIMIT 1",
            &[&SRC_TABLE],
        )
        .expect("query cdc_log failed");
    assert!(!rows.is_empty(), "_szrsql_cdc_log 应捕获到事件");
    let captured_table: String = rows[0].get(0);
    let captured_op: String = rows[0].get(1);
    assert_eq!(captured_table, SRC_TABLE);
    assert_eq!(captured_op, "INSERT");

    // 清理
    source
        .uninstall_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("uninstall failed");
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 4：CDC 增量流捕获 + 目标端写入（INSERT）—— 端到端往返
///
/// **流程**：
/// 1. 源端建表 + 目标端建表（结构相同）
/// 2. 源端安装 CDC 触发器
/// 3. 子线程启动 `start_cdc_stream`（阻塞），主线程插入 3 行后调用 `stop_cdc_stream`
/// 4. CDC 流捕获事件 → 通过 `PgRealWriter` 写入目标端
/// 5. 验证目标端有 3 行数据，且内容与源端一致
#[test]
fn integration_real_pg_cdc_insert_roundtrip() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    // 1. 源端建表 + 目标端建表（结构相同）
    create_src_table(&mut client, SRC_TABLE);
    create_src_table(&mut client, DST_TABLE);

    // 2. 创建源端连接器 + 安装触发器
    let source = Arc::new(
        PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
            .expect("source connect failed"),
    );
    source.connect().expect("source.connect failed");
    let schemas = source
        .discover_schemas(&[SRC_TABLE.to_string()])
        .expect("discover_schemas failed");
    let src_schema = schemas.into_iter().next().expect("should have schema");
    source
        .install_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("install triggers failed");

    // 3. 创建目标端写入器（独立连接）
    let writer_client =
        postgres::Client::connect(PG_URL, NoTls).expect("writer client connect failed");
    let writer = Arc::new(PgRealWriter::new(writer_client).expect("PgRealWriter::new failed"));

    // 4. 子线程启动 CDC 流，主线程插入数据后 stop
    let source_for_stream = source.clone();
    let writer_for_stream = writer.clone();
    let schema_for_stream = Arc::new(src_schema);
    let schema_inner = schema_for_stream.clone();

    // 收集已处理的事件数
    let processed = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let processed_clone = processed.clone();

    let handle = std::thread::spawn(move || {
        let writer_inner = writer_for_stream.clone();
        let schema_inner2 = schema_inner.clone();
        let processed_inner = processed_clone.clone();

        let result = source_for_stream.start_cdc_stream(0, &move |events| {
            for ev in events {
                if ev.op == SourceEventOp::Insert {
                    if let Some(after) = &ev.after {
                        // 构造 ChangeEvent 写入目标端（目标端表名为 DST_TABLE，需调整 schema）
                        let mut dst_schema = (*schema_inner2).clone();
                        dst_schema.table_name = DST_TABLE.to_string();
                        use szrsql_cdc::{CdcEventOp, ChangeEvent};
                        let change = ChangeEvent {
                            tx_id: ev.tx_id.unwrap_or(0) as u32,
                            lsn: ev.lsn,
                            op: CdcEventOp::Insert,
                            table_id: Some(dst_schema.table_id),
                            old_row: None,
                            new_row: None,
                            timestamp: ev.timestamp,
                            schema_version: None,
                        };
                        if let Err(e) =
                            writer_inner.write_event(&change, &dst_schema, Some(after))
                        {
                            eprintln!("[rt] write_event failed: {e}");
                        } else {
                            processed_inner.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                }
            }
            Ok(())
        });
        let _ = result;
    });

    // 5. 主线程：在源端插入 3 行
    std::thread::sleep(std::time::Duration::from_millis(200)); // 等待子线程启动
    for i in 1..=3i64 {
        let age: i32 = 20 + i as i32;
        let score: f64 = i as f64 * 2.0;
        let sql = format!(
            "INSERT INTO {SRC_TABLE} (id, name, age, score, active) VALUES ({i}, 'rt_user{i}', {age}, {score}, TRUE);"
        );
        client.batch_execute(&sql).expect("insert failed");
    }

    // 6. 等待 CDC 流处理事件
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let count = processed.load(std::sync::atomic::Ordering::SeqCst);
        if count >= 3 {
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // 7. 停止 CDC 流
    source.stop_cdc_stream().expect("stop failed");
    let _ = handle.join();

    // 8. 验证目标端有 3 行数据
    let dst_count = query_count(&mut client, DST_TABLE);
    assert_eq!(dst_count, 3, "目标端应有 3 行数据，实际: {dst_count}");

    // 9. 验证目标端内容与源端一致
    let src_count = query_count(&mut client, SRC_TABLE);
    assert_eq!(src_count, 3, "源端应有 3 行数据");
    assert_eq!(src_count, dst_count, "源端和目标端行数应一致");

    // 验证第一行内容
    let src_name = query_one_str(
        &mut client,
        &format!("SELECT name FROM {SRC_TABLE} WHERE id = 1"),
    );
    let dst_name = query_one_str(
        &mut client,
        &format!("SELECT name FROM {DST_TABLE} WHERE id = 1"),
    );
    assert_eq!(src_name, dst_name, "id=1 的 name 应一致");
    assert_eq!(src_name, "rt_user1");

    // 清理
    source
        .uninstall_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("uninstall failed");
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 5：CDC 增量流捕获 UPDATE/DELETE 事件
///
/// **流程**：
/// 1. 源端建表 + 安装触发器
/// 2. 插入 1 行
/// 3. UPDATE 该行
/// 4. DELETE 该行
/// 5. 验证 _szrsql_cdc_log 捕获 INSERT/UPDATE/DELETE 各 1 条
#[test]
fn integration_real_pg_cdc_update_delete_capture() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);

    let source = PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
        .expect("source connect failed");
    source.connect().expect("source.connect failed");
    source
        .install_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("install triggers failed");

    // INSERT
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE} (id, name, age) VALUES (1, 'initial', 30);"
        ))
        .expect("insert failed");
    // UPDATE
    client
        .batch_execute(&format!(
            "UPDATE {SRC_TABLE} SET name = 'updated', age = 31 WHERE id = 1;"
        ))
        .expect("update failed");
    // DELETE
    client
        .batch_execute(&format!("DELETE FROM {SRC_TABLE} WHERE id = 1;"))
        .expect("delete failed");

    // 验证 _szrsql_cdc_log 捕获 3 种事件
    let ops: Vec<String> = client
        .query(
            "SELECT op FROM _szrsql_cdc_log WHERE table_name = $1 ORDER BY id ASC",
            &[&SRC_TABLE],
        )
        .expect("query log failed")
        .into_iter()
        .map(|r| r.get::<_, String>(0))
        .collect();

    assert_eq!(ops.len(), 3, "应捕获 3 个事件");
    assert_eq!(ops[0], "INSERT", "第一个事件应为 INSERT");
    assert_eq!(ops[1], "UPDATE", "第二个事件应为 UPDATE");
    assert_eq!(ops[2], "DELETE", "第三个事件应为 DELETE");

    // 清理
    source
        .uninstall_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("uninstall failed");
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 6：PgRealWriter 建表 + 写入 DML
///
/// **流程**：
/// 1. 创建 PgRealWriter
/// 2. 调用 `ensure_table` 在目标端建表
/// 3. 验证目标端表存在
/// 4. 构造 ChangeEvent（Insert）写入
/// 5. 验证目标端有 1 行数据
#[test]
fn integration_real_pg_writer_ensure_table_and_write() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    // 创建 writer（用独立连接）
    let writer_client = postgres::Client::connect(PG_URL, NoTls).expect("writer client failed");
    let writer = PgRealWriter::new(writer_client).expect("PgRealWriter::new failed");

    // 构造 schema
    let schema = TableSchema {
        table_id: 1,
        table_name: DST_TABLE.to_string(),
        columns: vec![
            szrsql_cdc::schema::ColumnDef::not_null("id", szrsql_cdc::schema::DataType::Int64),
            szrsql_cdc::schema::ColumnDef::nullable("name", szrsql_cdc::schema::DataType::Text),
        ],
        version: 1,
    };

    // ensure_table
    writer
        .ensure_table(&schema)
        .expect("ensure_table failed");

    // 验证表存在
    let count: i64 = client
        .query(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = $1",
            &[&DST_TABLE],
        )
        .expect("query failed")[0]
        .get(0);
    assert_eq!(count, 1, "目标端表应已创建");

    // 构造 Insert 事件
    use szrsql_cdc::{CdcEventOp, ChangeEvent};
    let row = DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(42)),
            ("name".to_string(), SzValue::Text("hello".to_string())),
        ],
    };
    let event = ChangeEvent {
        tx_id: 1,
        lsn: 1,
        op: CdcEventOp::Insert,
        table_id: Some(1),
        old_row: None,
        new_row: None,
        timestamp: 0,
        schema_version: None,
    };

    writer
        .write_event(&event, &schema, Some(&row))
        .expect("write_event failed");

    // 验证目标端有 1 行
    let row_count = query_count(&mut client, DST_TABLE);
    assert_eq!(row_count, 1, "目标端应有 1 行");

    // 验证内容
    let name = query_one_str(
        &mut client,
        &format!("SELECT name FROM {DST_TABLE} WHERE id = 42"),
    );
    assert_eq!(name, "hello", "目标端 name 列应为 'hello'");

    // 清理
    cleanup_all(&mut client);
}

/// 集成测试 7：幂等性 — 重复 INSERT 不产生重复数据
///
/// **流程**：
/// 1. ensure_table 建表
/// 2. 写入 Insert 事件 2 次（相同主键）
/// 3. 验证目标端只有 1 行
#[test]
fn integration_real_pg_writer_idempotent_insert() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    let writer_client = postgres::Client::connect(PG_URL, NoTls).expect("writer client failed");
    let writer = PgRealWriter::new(writer_client).expect("PgRealWriter::new failed");

    let schema = TableSchema {
        table_id: 1,
        table_name: DST_TABLE.to_string(),
        columns: vec![
            szrsql_cdc::schema::ColumnDef::not_null("id", szrsql_cdc::schema::DataType::Int64),
            szrsql_cdc::schema::ColumnDef::nullable("name", szrsql_cdc::schema::DataType::Text),
        ],
        version: 1,
    };
    writer.ensure_table(&schema).expect("ensure_table failed");

    use szrsql_cdc::{CdcEventOp, ChangeEvent};
    let row = DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(100)),
            ("name".to_string(), SzValue::Text("dup".to_string())),
        ],
    };
    let event = ChangeEvent {
        tx_id: 1,
        lsn: 1,
        op: CdcEventOp::Insert,
        table_id: Some(1),
        old_row: None,
        new_row: None,
        timestamp: 0,
        schema_version: None,
    };

    // 第一次写入
    writer
        .write_event(&event, &schema, Some(&row))
        .expect("first write failed");
    // 第二次写入（相同主键，应 upsert 不重复）
    writer
        .write_event(&event, &schema, Some(&row))
        .expect("second write failed");

    let count = query_count(&mut client, DST_TABLE);
    assert_eq!(count, 1, "幂等性：重复 INSERT 应只有 1 行");

    // 清理
    cleanup_all(&mut client);
}

/// 集成测试 8：健康检查 — health_check 验证连接活性
#[test]
fn integration_real_pg_health_check() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    let writer = PgRealWriter::new(client).expect("writer new failed");
    writer.health_check().expect("health_check should pass");

    // 源端 health_check
    let source = PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
        .expect("source connect failed");
    source.connect().expect("source.connect failed");
    source.health_check().expect("source health_check should pass");
}

/// 集成测试 9：位点管理 — ack_offset / confirmed_offset 支持断点续传
///
/// **流程**：
/// 1. 创建源端连接器
/// 2. 调用 ack_offset 设置位点
/// 3. 调用 confirmed_offset 验证位点已持久化
#[test]
fn integration_real_pg_offset_management() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    let source = PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
        .expect("source connect failed");
    source.connect().expect("source.connect failed");

    // 初始位点应为 0
    let initial = source
        .confirmed_offset()
        .expect("confirmed_offset failed");
    assert_eq!(initial.lsn, 0, "初始位点应为 0");

    // 设置位点
    let new_offset = SourceOffset::new(12345);
    source
        .ack_offset(&new_offset)
        .expect("ack_offset failed");

    // 验证位点已更新
    let confirmed = source
        .confirmed_offset()
        .expect("confirmed_offset 2 failed");
    assert_eq!(confirmed.lsn, 12345, "ack 后位点应为 12345");

    // 验证位点是单调的（更小的位点不应覆盖）
    let smaller = SourceOffset::new(100);
    source.ack_offset(&smaller).expect("ack smaller failed");
    let confirmed2 = source
        .confirmed_offset()
        .expect("confirmed_offset 3 failed");
    assert_eq!(confirmed2.lsn, 12345, "更小的位点不应覆盖");

    // 清理
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 10：current_lsn — 返回 CDC 日志表当前最大 id
///
/// **流程**：
/// 1. 建表 + 安装触发器
/// 2. 插入数据触发 CDC 日志
/// 3. 验证 current_lsn 返回 > 0
#[test]
fn integration_real_pg_current_lsn() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);

    let source = PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
        .expect("source connect failed");
    source.connect().expect("source.connect failed");
    source
        .install_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("install triggers failed");

    // 初始 current_lsn 应为 0（无 CDC 日志）
    let initial_lsn = source.current_lsn().expect("current_lsn failed");
    assert_eq!(initial_lsn, 0, "初始 current_lsn 应为 0");

    // 插入数据
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE} (id, name, age) VALUES (1, 'lsn_test', 25);"
        ))
        .expect("insert failed");

    // current_lsn 应 > 0
    let after_lsn = source.current_lsn().expect("current_lsn 2 failed");
    assert!(after_lsn > 0, "插入后 current_lsn 应 > 0，实际: {after_lsn}");

    // 清理
    source
        .uninstall_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("uninstall failed");
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 11：UPDATE 事件写入目标端
///
/// **流程**：
/// 1. 目标端建表 + 插入 1 行
/// 2. 构造 Update ChangeEvent 写入目标端
/// 3. 验证目标端数据已更新
#[test]
fn integration_real_pg_writer_update_event() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    let writer_client = postgres::Client::connect(PG_URL, NoTls).expect("writer client failed");
    let writer = PgRealWriter::new(writer_client).expect("PgRealWriter::new failed");

    let schema = TableSchema {
        table_id: 1,
        table_name: DST_TABLE.to_string(),
        columns: vec![
            szrsql_cdc::schema::ColumnDef::not_null("id", szrsql_cdc::schema::DataType::Int64),
            szrsql_cdc::schema::ColumnDef::nullable("name", szrsql_cdc::schema::DataType::Text),
        ],
        version: 1,
    };
    writer.ensure_table(&schema).expect("ensure_table failed");

    // 先 INSERT 一行
    use szrsql_cdc::{CdcEventOp, ChangeEvent};
    let insert_row = DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("name".to_string(), SzValue::Text("before".to_string())),
        ],
    };
    let insert_event = ChangeEvent {
        tx_id: 1,
        lsn: 1,
        op: CdcEventOp::Insert,
        table_id: Some(1),
        old_row: None,
        new_row: None,
        timestamp: 0,
        schema_version: None,
    };
    writer
        .write_event(&insert_event, &schema, Some(&insert_row))
        .expect("insert write failed");

    // UPDATE
    let update_row = DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("name".to_string(), SzValue::Text("after".to_string())),
        ],
    };
    let update_event = ChangeEvent {
        tx_id: 2,
        lsn: 2,
        op: CdcEventOp::Update,
        table_id: Some(1),
        old_row: None,
        new_row: None,
        timestamp: 0,
        schema_version: None,
    };
    writer
        .write_event(&update_event, &schema, Some(&update_row))
        .expect("update write failed");

    // 验证 name 已更新
    let name = query_one_str(
        &mut client,
        &format!("SELECT name FROM {DST_TABLE} WHERE id = 1"),
    );
    assert_eq!(name, "after", "UPDATE 后 name 应为 'after'");

    // 清理
    cleanup_all(&mut client);
}

/// 集成测试 12：DELETE 事件写入目标端
#[test]
fn integration_real_pg_writer_delete_event() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    let writer_client = postgres::Client::connect(PG_URL, NoTls).expect("writer client failed");
    let writer = PgRealWriter::new(writer_client).expect("PgRealWriter::new failed");

    let schema = TableSchema {
        table_id: 1,
        table_name: DST_TABLE.to_string(),
        columns: vec![
            szrsql_cdc::schema::ColumnDef::not_null("id", szrsql_cdc::schema::DataType::Int64),
            szrsql_cdc::schema::ColumnDef::nullable("name", szrsql_cdc::schema::DataType::Text),
        ],
        version: 1,
    };
    writer.ensure_table(&schema).expect("ensure_table failed");

    use szrsql_cdc::{CdcEventOp, ChangeEvent};

    // 先 INSERT
    let insert_row = DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("name".to_string(), SzValue::Text("temp".to_string())),
        ],
    };
    let insert_event = ChangeEvent {
        tx_id: 1,
        lsn: 1,
        op: CdcEventOp::Insert,
        table_id: Some(1),
        old_row: None,
        new_row: None,
        timestamp: 0,
        schema_version: None,
    };
    writer
        .write_event(&insert_event, &schema, Some(&insert_row))
        .expect("insert failed");

    assert_eq!(query_count(&mut client, DST_TABLE), 1);

    // DELETE
    let delete_row = DecodedRow {
        columns: vec![("id".to_string(), SzValue::Int64(1))],
    };
    let delete_event = ChangeEvent {
        tx_id: 2,
        lsn: 2,
        op: CdcEventOp::Delete,
        table_id: Some(1),
        old_row: None,
        new_row: None,
        timestamp: 0,
        schema_version: None,
    };
    writer
        .write_event(&delete_event, &schema, Some(&delete_row))
        .expect("delete failed");

    assert_eq!(query_count(&mut client, DST_TABLE), 0, "DELETE 后应有 0 行");

    // 清理
    cleanup_all(&mut client);
}

/// 集成测试 13：execute_ddl 执行 DDL 语句
#[test]
fn integration_real_pg_writer_execute_ddl() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);

    let writer_client = postgres::Client::connect(PG_URL, NoTls).expect("writer client failed");
    let writer = PgRealWriter::new(writer_client).expect("PgRealWriter::new failed");

    // 执行 CREATE TABLE DDL
    let ddl = format!(
        "CREATE TABLE IF NOT EXISTS {DST_TABLE} (id BIGINT PRIMARY KEY, val TEXT);"
    );
    writer.execute_ddl(&ddl).expect("execute_ddl failed");

    // 验证表已创建
    let count: i64 = client
        .query(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = $1",
            &[&DST_TABLE],
        )
        .expect("query failed")[0]
        .get(0);
    assert_eq!(count, 1, "DDL 应创建表");

    // 清理
    cleanup_all(&mut client);
}

/// 集成测试 14：多表 CDC 触发器
///
/// **流程**：
/// 1. 建两张源表
/// 2. 安装触发器（一次调用传多表）
/// 3. 在两张表上分别 INSERT
/// 4. 验证 _szrsql_cdc_log 捕获了两张表的事件
#[test]
fn integration_real_pg_multi_table_triggers() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);
    create_src_table(&mut client, SRC_TABLE2);

    let source = PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
        .expect("source connect failed");
    source.connect().expect("source.connect failed");
    source
        .install_cdc_triggers(&[SRC_TABLE.to_string(), SRC_TABLE2.to_string()])
        .expect("install triggers failed");

    // 在两张表上 INSERT
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE} (id, name, age) VALUES (1, 'a', 1);"
        ))
        .expect("insert 1 failed");
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE2} (id, name, age) VALUES (1, 'b', 2);"
        ))
        .expect("insert 2 failed");

    // 验证 _szrsql_cdc_log 捕获了两张表的事件
    let table_names: Vec<&str> = vec![SRC_TABLE, SRC_TABLE2];
    let count: i64 = client
        .query(
            "SELECT COUNT(DISTINCT table_name) FROM _szrsql_cdc_log WHERE table_name = ANY($1)",
            &[&table_names],
        )
        .expect("query failed")[0]
        .get(0);
    assert_eq!(count, 2, "应捕获 2 张表的事件");

    // 清理
    source
        .uninstall_cdc_triggers(&[SRC_TABLE.to_string(), SRC_TABLE2.to_string()])
        .expect("uninstall failed");
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

/// 集成测试 15：CDC 日志表幂等清理 — clear_cdc_log 不影响触发器
#[test]
fn integration_real_pg_clear_cdc_log() {
    let _lock = lock_pg_rt();
    let mut client = match try_pg() {
        Some(c) => c,
        None => return,
    };
    cleanup_all(&mut client);
    create_src_table(&mut client, SRC_TABLE);

    let source = PgRealSourceConnector::connect(PG_URL, SourceConfig::postgres(PG_URL), NoTls)
        .expect("source connect failed");
    source.connect().expect("source.connect failed");
    source
        .install_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("install triggers failed");

    // 插入数据生成日志
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE} (id, name, age) VALUES (1, 'a', 1);"
        ))
        .expect("insert failed");
    let before: i64 = source.current_lsn().expect("lsn failed") as i64;
    assert!(before > 0, "应有日志");

    // 清空日志
    source.clear_cdc_log().expect("clear failed");

    // current_lsn 应为 0
    let after = source.current_lsn().expect("lsn 2 failed");
    assert_eq!(after, 0, "清空后 current_lsn 应为 0");

    // 再次插入，触发器仍应工作
    client
        .batch_execute(&format!(
            "INSERT INTO {SRC_TABLE} (id, name, age) VALUES (2, 'b', 2);"
        ))
        .expect("insert 2 failed");
    let after2 = source.current_lsn().expect("lsn 3 failed");
    assert!(after2 > 0, "清空后再次插入应有新日志");

    // 清理
    source
        .uninstall_cdc_triggers(&[SRC_TABLE.to_string()])
        .expect("uninstall failed");
    cleanup_all(&mut client);
    let _ = source.drop_cdc_log();
}

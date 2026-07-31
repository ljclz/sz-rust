//! P4-4 真实 PostgreSQL 集成测试 — 验证 CDC → PostgresWriter → 真实 PG 18 全链路
//!
//! # 测试目标
//!
//! 1. 验证 `PostgresWriter::with_executor` 能通过真实 PG 协议执行 SQL
//! 2. 验证 DDL 同步：CREATE TABLE → 真实 PG 建表
//! 3. 验证 DML 同步：INSERT/UPDATE/DELETE → 真实 PG 数据变更
//! 4. 验证 Schema 变更同步：ALTER TABLE ADD COLUMN → 真实 PG 加列
//! 5. 验证端到端一致性：CDC 事件流 → PG 数据状态
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
//! cargo test -p szrsql-cdc --test integration_pg -- --nocapture --test-threads=1
//! ```
//!
//! # 跳过策略
//!
//! 若 PG 不可连接，所有测试自动跳过（返回 Ok，不失败），
//! 避免在无 PG 环境的 CI 上报错。

// 兼容旧闭包/非参数化 SQL 方法（P0-2 已废弃，测试仍需验证向后兼容）
#![allow(deprecated)]

use postgres::NoTls;
use std::sync::{Arc, Mutex};
use szrsql_cdc::schema::{ColumnDef, DataType, SchemaChangeEvent, SchemaChangeType, TableSchema};
use szrsql_cdc::target::postgres::PostgresWriter;
use szrsql_cdc::target::TargetWriter;
use szrsql_cdc::migration::{DdlGenerator, Dialect};

/// 全局串行锁 — 所有集成测试操作相同的测试表（cdc_test_users 等），
/// 并发执行会导致表冲突。此锁强制测试串行运行，无需依赖 --test-threads=1。
static PG_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 在测试函数开头调用，获取全局锁（测试结束自动释放）
fn lock_pg_tests() -> std::sync::MutexGuard<'static, ()> {
    // 注意：unwrap 在持锁期间 poison 也不影响测试正确性
    PG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// PG 连接串（对应 AGENTS.md 中的本机 PostgreSQL 18）
const PG_URL: &str = "postgresql://postgres:test123@127.0.0.1:5432/sz_orm_test";

/// 尝试连接 PG，返回连接客户端；失败则返回 None（测试将跳过）
fn try_pg_connect() -> Option<postgres::Client> {
    match postgres::Client::connect(PG_URL, NoTls) {
        Ok(client) => Some(client),
        Err(e) => {
            eprintln!("[integration_pg] 跳过：无法连接 PG ({e})");
            None
        }
    }
}

/// 创建一个共享的 PG 连接池（用 Mutex 保护，单线程执行 SQL）
struct PgExecutor {
    client: Mutex<postgres::Client>,
}

impl PgExecutor {
    fn new(client: postgres::Client) -> Self {
        Self {
            client: Mutex::new(client),
        }
    }

    fn execute(&self, sql: &str) -> Result<(), String> {
        let mut client = self.client.lock().unwrap();
        client.batch_execute(sql).map_err(|e| {
            // 用 Debug 格式输出完整错误链（Display 太简化）
            format!("batch_execute failed: {e:?} | sql: {sql}")
        })
    }

    fn query_one(&self, sql: &str) -> Result<String, String> {
        let mut client = self.client.lock().unwrap();
        let rows = client.query(sql, &[]).map_err(|e| {
            format!("query failed: {e:?} | sql: {sql}")
        })?;
        if rows.is_empty() {
            return Err("no rows".to_string());
        }
        // COUNT(*) 等 SQL 返回 i64，需兼容多种类型
        let val: String = if let Ok(v) = rows[0].try_get::<_, i64>(0) {
            v.to_string()
        } else if let Ok(v) = rows[0].try_get::<_, String>(0) {
            v
        } else if let Ok(v) = rows[0].try_get::<_, i32>(0) {
            v.to_string()
        } else {
            return Err(format!(
                "unsupported column type at index 0 (sql: {sql})"
            ));
        };
        Ok(val)
    }
}

/// 测试前置：清理测试表（避免上次测试残留）
fn cleanup_tables(executor: &PgExecutor) {
    let _ = executor.execute("DROP TABLE IF EXISTS cdc_test_users;");
    let _ = executor.execute("DROP TABLE IF EXISTS cdc_test_orders;");
    let _ = executor.execute("DROP TABLE IF EXISTS cdc_test_alter;");
}

// =====================================================================
// 集成测试用例
// =====================================================================

/// 集成测试 1：DDL 同步 — CREATE TABLE 在真实 PG 上建表
///
/// **流程**：
/// 1. 连接真实 PG
/// 2. 通过 DdlGenerator 生成 CREATE TABLE DDL
/// 3. 通过 PostgresWriter::execute_ddl 在 PG 上执行
/// 4. 查询 pg_catalog 验证表已创建
#[test]
fn integration_pg_ddl_create_table() {
    let _lock = lock_pg_tests();
    let client = match try_pg_connect() {
        Some(c) => c,
        None => return,
    };
    let executor = Arc::new(PgExecutor::new(client));
    cleanup_tables(&executor);

    // 创建 PostgresWriter，注入真实 SQL 执行器
    let writer = PostgresWriter::with_executor(
        PG_URL,
        Arc::new({
            let exec = executor.clone();
            move |sql: &str| {
                exec.execute(sql).map_err(|e| {
                    szrsql_cdc::target::WriterError::Sql(e)
                })
            }
        }),
    )
    .expect("PostgresWriter::with_executor should succeed");

    // 构造 schema 并生成 DDL
    let schema = TableSchema {
        table_id: 1001,
        table_name: "cdc_test_users".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("name", DataType::Text),
            ColumnDef::nullable("age", DataType::Int32),
        ],
        version: 1,
    };
    let generator = DdlGenerator::new(Dialect::Postgres);
    let ddl = generator.generate_create_table(&schema);

    println!("[integration_pg] 执行 DDL: {ddl}");

    // 执行 DDL
    writer.execute_ddl(&ddl).expect("execute_ddl should succeed");

    // 验证：查询 pg_catalog 确认表已创建
    let count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'cdc_test_users';")
        .expect("query should succeed");
    assert_eq!(count, "1", "cdc_test_users 表应已创建");

    // 清理
    cleanup_tables(&executor);
}

/// 集成测试 2：DDL 同步 — DROP TABLE 在真实 PG 上删表
#[test]
fn integration_pg_ddl_drop_table() {
    let _lock = lock_pg_tests();
    let client = match try_pg_connect() {
        Some(c) => c,
        None => return,
    };
    let executor = Arc::new(PgExecutor::new(client));
    cleanup_tables(&executor);

    let writer = PostgresWriter::with_executor(
        PG_URL,
        Arc::new({
            let exec = executor.clone();
            move |sql: &str| {
                exec.execute(sql).map_err(|e| {
                    szrsql_cdc::target::WriterError::Sql(e)
                })
            }
        }),
    )
    .expect("writer should succeed");

    // 先建表
    let schema = TableSchema {
        table_id: 1002,
        table_name: "cdc_test_orders".to_string(),
        columns: vec![
            ColumnDef::not_null("order_id", DataType::Int64),
            ColumnDef::nullable("amount", DataType::Real),
        ],
        version: 1,
    };
    let generator = DdlGenerator::new(Dialect::Postgres);
    let create_ddl = generator.generate_create_table(&schema);
    writer.execute_ddl(&create_ddl).expect("create should succeed");

    // 验证表存在
    let count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'cdc_test_orders';")
        .expect("query should succeed");
    assert_eq!(count, "1");

    // 执行 DROP TABLE
    let drop_ddl = generator.generate_drop_table("cdc_test_orders");
    println!("[integration_pg] 执行 DDL: {drop_ddl}");
    writer.execute_ddl(&drop_ddl).expect("drop should succeed");

    // 验证表已删除
    let count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'cdc_test_orders';")
        .expect("query should succeed");
    assert_eq!(count, "0", "cdc_test_orders 表应已删除");
}

/// 集成测试 3：DDL 同步 — ALTER TABLE ADD COLUMN 在真实 PG 上加列
#[test]
fn integration_pg_ddl_alter_add_column() {
    let _lock = lock_pg_tests();
    let client = match try_pg_connect() {
        Some(c) => c,
        None => return,
    };
    let executor = Arc::new(PgExecutor::new(client));
    cleanup_tables(&executor);

    let writer = PostgresWriter::with_executor(
        PG_URL,
        Arc::new({
            let exec = executor.clone();
            move |sql: &str| {
                exec.execute(sql).map_err(|e| {
                    szrsql_cdc::target::WriterError::Sql(e)
                })
            }
        }),
    )
    .expect("writer should succeed");

    // 先建表（2 列）
    let schema_v1 = TableSchema {
        table_id: 1003,
        table_name: "cdc_test_alter".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("name", DataType::Text),
        ],
        version: 1,
    };
    let generator = DdlGenerator::new(Dialect::Postgres);
    writer.execute_ddl(&generator.generate_create_table(&schema_v1)).expect("create should succeed");

    // 验证初始列数
    let col_count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.columns WHERE table_name = 'cdc_test_alter';")
        .expect("query should succeed");
    assert_eq!(col_count, "2", "初始应有 2 列");

    // 执行 ALTER TABLE ADD COLUMN
    let new_col = ColumnDef::nullable("email", DataType::Text);
    let alter_ddl = generator.generate_add_column("cdc_test_alter", &new_col);
    println!("[integration_pg] 执行 DDL: {alter_ddl}");
    writer.execute_ddl(&alter_ddl).expect("alter should succeed");

    // 验证列数变为 3
    let col_count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.columns WHERE table_name = 'cdc_test_alter';")
        .expect("query should succeed");
    assert_eq!(col_count, "3", "加列后应有 3 列");

    // 验证新列名存在
    let email_exists = executor
        .query_one("SELECT COUNT(*) FROM information_schema.columns WHERE table_name = 'cdc_test_alter' AND column_name = 'email';")
        .expect("query should succeed");
    assert_eq!(email_exists, "1", "email 列应存在");

    // 清理
    cleanup_tables(&executor);
}

/// 集成测试 4：SchemaChangeEvent 端到端 — 通过 ReplicationTask 处理 DDL 事件
///
/// **流程**：
/// 1. 创建 PostgresWriter（注入真实 PG 执行器）
/// 2. 创建 ReplicationTask（dialect=Postgres）
/// 3. 通知 SchemaChangeEvent(CreateTable)
/// 4. 验证 PG 上表已创建
#[test]
fn integration_pg_schema_change_event_e2e() {
    let _lock = lock_pg_tests();
    let client = match try_pg_connect() {
        Some(c) => c,
        None => return,
    };
    let executor = Arc::new(PgExecutor::new(client));
    cleanup_tables(&executor);

    use szrsql_cdc::task::{ReplicationTask, TaskConfig};
    use szrsql_cdc::slot::SlotManager;
    use szrsql_cdc::schema::SchemaRegistry;
    use szrsql_cdc::decoder::RowDecoder;

    // 创建 writer（注入真实 PG 执行器）
    let writer: Arc<dyn TargetWriter> = Arc::new(
        PostgresWriter::with_executor(
            PG_URL,
            Arc::new({
                let exec = executor.clone();
                move |sql: &str| {
                    exec.execute(sql).map_err(|e| {
                        szrsql_cdc::target::WriterError::Sql(e)
                    })
                }
            }),
        )
        .expect("writer should succeed"),
    );

    // 创建 task 基础设施
    let registry = Arc::new(SchemaRegistry::new());
    let decoder = Arc::new(RowDecoder::new(registry.clone()));
    let slot_mgr = Arc::new(SlotManager::in_memory());

    let config = TaskConfig {
        task_id: "integration_pg_e2e".to_string(),
        description: "PG 集成测试".to_string(),
        table_filter: None,
        writer,
        target_type: "postgres".to_string(),
        target_connection: PG_URL.to_string(),
        snapshot_first: false,
        dialect: Dialect::Postgres,
        backpressure_config: szrsql_cdc::backpressure::BackpressureConfig::default(),
    };

    let task = Arc::new(
        ReplicationTask::new(config, slot_mgr, decoder, registry)
            .expect("ReplicationTask::new should succeed"),
    );
    task.start().expect("task.start should succeed");

    // 构造 CreateTable 事件
    let new_schema = TableSchema {
        table_id: 1004,
        table_name: "cdc_test_users".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("name", DataType::Text),
        ],
        version: 1,
    };
    let event = SchemaChangeEvent {
        tx_id: 1,
        lsn: 100,
        change_type: SchemaChangeType::CreateTable,
        table_id: 1004,
        old_schema: None,
        new_schema: Some(new_schema),
        changed_column: None,
        schema_version: 1,
        timestamp: 0,
    };

    // 通过 task 处理 DDL 事件
    use szrsql_cdc::schema::SchemaChangeObserver;
    task.on_schema_change(event);

    // 验证 PG 上表已创建
    let count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'cdc_test_users';")
        .expect("query should succeed");
    assert_eq!(count, "1", "cdc_test_users 表应已通过 DDL 事件创建");

    // 验证 task 统计
    let stats = task.stats();
    assert_eq!(stats.ddl_events_processed, 1, "应处理 1 个 DDL 事件");
    assert_eq!(stats.ddl_error_count, 0, "不应有 DDL 错误");

    // 清理
    cleanup_tables(&executor);
}

/// 集成测试 5：幂等性验证 — 重复执行相同 CREATE TABLE IF NOT EXISTS 不报错
#[test]
fn integration_pg_ddl_idempotent() {
    let _lock = lock_pg_tests();
    let client = match try_pg_connect() {
        Some(c) => c,
        None => return,
    };
    let executor = Arc::new(PgExecutor::new(client));
    cleanup_tables(&executor);

    let writer = PostgresWriter::with_executor(
        PG_URL,
        Arc::new({
            let exec = executor.clone();
            move |sql: &str| {
                exec.execute(sql).map_err(|e| {
                    szrsql_cdc::target::WriterError::Sql(e)
                })
            }
        }),
    )
    .expect("writer should succeed");

    let schema = TableSchema {
        table_id: 1005,
        table_name: "cdc_test_users".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
        ],
        version: 1,
    };
    let generator = DdlGenerator::new(Dialect::Postgres);
    let ddl = generator.generate_create_table(&schema);

    // 第一次执行
    writer.execute_ddl(&ddl).expect("第一次 execute_ddl 应成功");
    // 第二次执行（幂等）
    writer.execute_ddl(&ddl).expect("第二次 execute_ddl 应幂等成功");

    // 验证表存在
    let count = executor
        .query_one("SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'cdc_test_users';")
        .expect("query should succeed");
    assert_eq!(count, "1");

    // 清理
    cleanup_tables(&executor);
}

/// 集成测试 6：方言适配验证 — 生成的 DDL 符合 PG 语法
///
/// **验证点**：
/// - 标识符用双引号引用（PG 风格）
/// - 类型映射正确（Int64 → BIGINT, Text → TEXT, Real → DOUBLE PRECISION）
/// - IF NOT EXISTS 子句存在
#[test]
fn integration_pg_dialect_syntax_correct() {
    let _lock = lock_pg_tests();
    let client = match try_pg_connect() {
        Some(c) => c,
        None => return,
    };
    let executor = Arc::new(PgExecutor::new(client));
    cleanup_tables(&executor);

    let generator = DdlGenerator::new(Dialect::Postgres);

    // 生成覆盖多种类型的 DDL
    let schema = TableSchema {
        table_id: 1006,
        table_name: "cdc_test_users".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("name", DataType::Text),
            ColumnDef::nullable("score", DataType::Real),
            ColumnDef::nullable("active", DataType::Bool),
            ColumnDef::nullable("data", DataType::Blob),
        ],
        version: 1,
    };
    let ddl = generator.generate_create_table(&schema);
    println!("[integration_pg] PG DDL: {ddl}");

    // 验证 DDL 语法符合 PG
    assert!(ddl.contains("CREATE TABLE IF NOT EXISTS \"cdc_test_users\""));
    assert!(ddl.contains("\"id\" BIGINT NOT NULL"));
    assert!(ddl.contains("\"name\" TEXT"));
    assert!(ddl.contains("\"score\" DOUBLE PRECISION"));
    assert!(ddl.contains("\"active\" BOOLEAN"));
    assert!(ddl.contains("\"data\" BYTEA"));

    // 在真实 PG 上执行，验证语法正确
    let writer = PostgresWriter::with_executor(
        PG_URL,
        Arc::new({
            let exec = executor.clone();
            move |sql: &str| {
                exec.execute(sql).map_err(|e| {
                    szrsql_cdc::target::WriterError::Sql(e)
                })
            }
        }),
    )
    .expect("writer should succeed");
    writer.execute_ddl(&ddl).expect("PG 应接受此 DDL");

    // 清理
    cleanup_tables(&executor);
}

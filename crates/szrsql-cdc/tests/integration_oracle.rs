//! Oracle 目标端写入器集成测试（P5-1）
//!
//! # 测试策略
//!
//! 由于 Oracle Instant Client（OCI）在 Windows 上配置复杂，且需要额外的 native 依赖，
//! 本集成测试采用 `SqlExecutor` 闭包模式，验证 OracleWriter 在完整 CDC 流程中的行为：
//!
//! 1. **端到端 CDC 流程**：CdcEngine → ChangeEvent → OracleWriter → SQL 生成
//! 2. **SQL 语法验证**：验证生成的 Oracle SQL 符合 Oracle 方言语法
//! 3. **幂等性验证**：重复写入同一事件不应产生不同 SQL
//! 4. **多操作序列**：Insert → Update → Delete 完整生命周期
//! 5. **Schema 变更同步**：CREATE TABLE + ALTER TABLE DDL 执行
//! 6. **类型映射覆盖**：所有 SzValue 类型 → Oracle SQL 字面量
//!
//! # 真实 Oracle 集成测试
//!
//! 若要连接真实 Oracle 23ai（127.0.0.1:1521/freepdb1），需要：
//! 1. 在 Cargo.toml 添加 `oracle = "0.6"` 作为 dev-dependency
//! 2. 安装 Oracle Instant Client 并配置 PATH
//! 3. 将 SqlExecutor 闭包替换为真实 oracle::Connection 执行
//!
//! 当前测试已验证 SQL 生成正确性，真实连接测试待后续添加。

// 兼容旧闭包/非参数化 SQL 方法（P0-2 已废弃，测试仍需验证向后兼容）
#![allow(deprecated)]

use std::sync::{Arc, Mutex};
use szrsql_cdc::schema::{ColumnDef, DataType, TableSchema};
use szrsql_cdc::target::oracle::OracleWriter;
use szrsql_cdc::target::{TargetWriter, WriterError};
use szrsql_cdc::ChangeEvent;
use szrsql_types::value::Value as SzValue;

// =====================================================================
// 测试辅助
// =====================================================================

/// 创建测试 schema：users(id BIGINT NOT NULL, name VARCHAR2, age NUMBER)
fn make_users_schema() -> TableSchema {
    TableSchema {
        table_id: 1,
        table_name: "users".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("name", DataType::Text),
            ColumnDef::nullable("age", DataType::Int32),
        ],
        version: 1,
    }
}

/// 创建测试行
fn make_row(id: i64, name: &str, age: i32) -> szrsql_cdc::decoder::DecodedRow {
    szrsql_cdc::decoder::DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(id)),
            ("name".to_string(), SzValue::Text(name.to_string())),
            ("age".to_string(), SzValue::Int64(age as i64)),
        ],
    }
}

/// 创建收集执行的 SQL 的执行器
fn make_collecting_executor() -> (
    Arc<Mutex<Vec<String>>>,
    Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync>,
) {
    let sqls = Arc::new(Mutex::new(Vec::<String>::new()));
    let sqls_clone = sqls.clone();
    let executor: Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync> =
        Arc::new(move |sql| {
            sqls_clone.lock().unwrap().push(sql.to_string());
            Ok(())
        });
    (sqls, executor)
}

// =====================================================================
// 端到端 CDC 流程测试
// =====================================================================

#[test]
fn oracle_end_to_end_insert_update_delete() {
    // 端到端：Insert → Update → Delete 完整生命周期
    let (sqls, executor) = make_collecting_executor();
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();
    let schema = make_users_schema();

    // 1. Insert
    let insert_row = make_row(1, "Alice", 30);
    let insert_event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);
    writer
        .write_event(&insert_event, &schema, Some(&insert_row))
        .unwrap();

    // 2. Update
    let update_row = make_row(1, "Bob", 25);
    let update_event = ChangeEvent::update(100, 5001, 1, Vec::new(), Vec::new(), 1234567891);
    writer
        .write_event(&update_event, &schema, Some(&update_row))
        .unwrap();

    // 3. Delete
    let delete_row = make_row(1, "Bob", 25);
    let delete_event = ChangeEvent::delete(100, 5002, 1, Vec::new(), 1234567892);
    writer
        .write_event(&delete_event, &schema, Some(&delete_row))
        .unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 3);

    // 验证 SQL 类型
    assert!(
        collected[0].contains("MERGE INTO"),
        "first SQL should be MERGE: {}",
        collected[0]
    );
    assert!(
        collected[1].contains("UPDATE"),
        "second SQL should be UPDATE: {}",
        collected[1]
    );
    assert!(
        collected[2].contains("DELETE"),
        "third SQL should be DELETE: {}",
        collected[2]
    );

    // 验证写入计数
    assert_eq!(writer.write_count(), 3);
    assert_eq!(writer.fail_count(), 0);
}

#[test]
fn oracle_batch_write_multiple_inserts() {
    // 批量写入多个 Insert 事件
    let (_sqls, executor) = make_collecting_executor();
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();
    let schema = make_users_schema();
    let mut schemas = std::collections::HashMap::new();
    schemas.insert(1u32, schema.clone());

    let events: Vec<ChangeEvent> = (1..=5)
        .map(|i| ChangeEvent::insert(100, 5000 + i, 1, Vec::new(), 1234567890 + i))
        .collect();

    // write_batch 默认实现不传 row（None），OracleWriter 需要解码行数据
    let result = writer.write_batch(&events, &schemas);
    assert!(result.is_err(), "write_batch without rows should fail");
}

#[test]
fn oracle_ensure_table_then_write() {
    // 先建表再写入
    let (sqls, executor) = make_collecting_executor();
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();
    let schema = make_users_schema();

    // 1. 确保表存在
    writer.ensure_table(&schema).unwrap();

    // 2. 写入数据
    let row = make_row(42, "Alice", 30);
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);
    writer.write_event(&event, &schema, Some(&row)).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 2);

    // 第一条是 CREATE TABLE（PL/SQL 块）
    assert!(collected[0].contains("CREATE TABLE"));
    assert!(collected[0].contains("BEGIN EXECUTE IMMEDIATE"));

    // 第二条是 MERGE INTO
    assert!(collected[1].contains("MERGE INTO"));
}

#[test]
fn oracle_ensure_table_idempotent() {
    // 重复调用 ensure_table 不应重复建表
    let (sqls, executor) = make_collecting_executor();
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();
    let schema = make_users_schema();

    writer.ensure_table(&schema).unwrap();
    writer.ensure_table(&schema).unwrap();
    writer.ensure_table(&schema).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(
        collected.len(),
        1,
        "ensure_table should only generate one CREATE TABLE"
    );
}

// =====================================================================
// SQL 语法验证测试
// =====================================================================

#[test]
fn oracle_merge_into_sql_syntax_valid() {
    // 验证 MERGE INTO 语句语法符合 Oracle 规范
    let schema = make_users_schema();
    let row = make_row(42, "Alice", 30);
    let sql = OracleWriter::generate_insert_sql(&schema, &row, true).unwrap();

    // Oracle MERGE INTO 完整语法：
    // MERGE INTO target t USING (source) s ON (condition)
    // WHEN MATCHED THEN UPDATE SET ...
    // WHEN NOT MATCHED THEN INSERT (...) VALUES (...)
    assert!(sql.starts_with("MERGE INTO \"users\" t USING"));
    assert!(sql.contains("FROM DUAL"));
    assert!(sql.contains("ON (t.\"id\" = s.\"id\")"));
    assert!(sql.contains("WHEN MATCHED THEN UPDATE SET"));
    assert!(sql.contains("WHEN NOT MATCHED THEN INSERT"));
    assert!(sql.ends_with(';'));
}

#[test]
fn oracle_update_sql_syntax_valid() {
    let schema = make_users_schema();
    let row = make_row(42, "Bob", 25);
    let sql = OracleWriter::generate_update_sql(&schema, &row).unwrap();

    assert!(sql.starts_with("UPDATE \"users\" SET"));
    assert!(sql.contains("WHERE \"id\" = 42"));
    assert!(sql.ends_with(';'));
}

#[test]
fn oracle_delete_sql_syntax_valid() {
    let schema = make_users_schema();
    let row = make_row(42, "Alice", 30);
    let sql = OracleWriter::generate_delete_sql(&schema, &row).unwrap();

    assert!(sql.starts_with("DELETE FROM \"users\""));
    assert!(sql.contains("WHERE \"id\" = 42"));
    assert!(sql.ends_with(';'));
}

#[test]
fn oracle_create_table_uses_plsql_block() {
    // Oracle 不支持 CREATE TABLE IF NOT EXISTS，用 PL/SQL 块捕获 ORA-00955
    let schema = make_users_schema();
    let sql = OracleWriter::generate_create_table_sql(&schema);

    assert!(sql.starts_with("BEGIN EXECUTE IMMEDIATE '"));
    assert!(sql.contains("CREATE TABLE \"users\""));
    assert!(sql.contains("EXCEPTION WHEN OTHERS THEN"));
    assert!(sql.contains("SQLCODE != -955"));
    assert!(sql.ends_with("END;"));
}

// =====================================================================
// 类型映射覆盖测试
// =====================================================================

#[test]
fn oracle_all_data_types_in_create_table() {
    // 验证所有 DataType 都有 Oracle 类型映射
    let schema = TableSchema {
        table_id: 10,
        table_name: "all_types".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("int32_col", DataType::Int32),
            ColumnDef::nullable("int64_col", DataType::Int64),
            ColumnDef::nullable("text_col", DataType::Text),
            ColumnDef::nullable("blob_col", DataType::Blob),
            ColumnDef::nullable("real_col", DataType::Real),
            ColumnDef::nullable("bool_col", DataType::Bool),
            ColumnDef::nullable("date_col", DataType::Date),
            ColumnDef::nullable("timestamp_col", DataType::Timestamp),
            ColumnDef::nullable("json_col", DataType::Json),
            ColumnDef::nullable("uuid_col", DataType::Uuid),
        ],
        version: 1,
    };
    let sql = OracleWriter::generate_create_table_sql(&schema);

    assert!(sql.contains("NUMBER(19)")); // Int64
    assert!(sql.contains("NUMBER(10)")); // Int32
    assert!(sql.contains("VARCHAR2(4000)")); // Text
    assert!(sql.contains("BLOB")); // Blob
    assert!(sql.contains("BINARY_DOUBLE")); // Real
    assert!(sql.contains("NUMBER(1)")); // Bool
    assert!(sql.contains("DATE")); // Date
    assert!(sql.contains("TIMESTAMP")); // Timestamp
    assert!(sql.contains("CLOB")); // Json
    assert!(sql.contains("VARCHAR2(36)")); // Uuid
}

#[test]
fn oracle_value_types_in_merge() {
    // 验证各种 SzValue 类型在 MERGE INTO 中的 SQL 字面量
    let schema = TableSchema {
        table_id: 11,
        table_name: "type_test".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("text_val", DataType::Text),
            ColumnDef::nullable("bool_val", DataType::Bool),
            ColumnDef::nullable("blob_val", DataType::Blob),
            ColumnDef::nullable("float_val", DataType::Real),
        ],
        version: 1,
    };
    let row = szrsql_cdc::decoder::DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("text_val".to_string(), SzValue::Text("hello".to_string())),
            ("bool_val".to_string(), SzValue::Bool(true)),
            ("blob_val".to_string(), SzValue::Blob(vec![0xAB, 0xCD])),
            ("float_val".to_string(), SzValue::Float64(3.5)),
        ],
    };
    let sql = OracleWriter::generate_insert_sql(&schema, &row, true).unwrap();

    // 验证 Oracle 特定字面量
    assert!(sql.contains("1")); // Int64
    assert!(sql.contains("'hello'")); // Text
    assert!(sql.contains(", 1,")); // Bool → 1
    assert!(sql.contains("HEXTORAW('ABCD')")); // Blob
    assert!(sql.contains("3.5")); // Float64
}

// =====================================================================
// 幂等性验证测试
// =====================================================================

#[test]
fn oracle_idempotent_insert_same_sql() {
    // 重复写入同一事件应生成相同 SQL（幂等性前提）
    let writer = OracleWriter::new("oracle://localhost:1521/freepdb1").unwrap();
    let schema = make_users_schema();
    let row = make_row(1, "Alice", 30);
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);

    writer.write_event(&event, &schema, Some(&row)).unwrap();
    writer.write_event(&event, &schema, Some(&row)).unwrap();

    let sqls = writer.generated_sqls();
    assert_eq!(sqls.len(), 2);
    assert_eq!(
        sqls[0], sqls[1],
        "idempotent insert should generate identical SQL"
    );
}

// =====================================================================
// 健康检查测试
// =====================================================================

#[test]
fn oracle_health_check_uses_dual() {
    // Oracle 健康检查应使用 SELECT 1 FROM DUAL（不是 SELECT 1）
    let called = Arc::new(Mutex::new(String::new()));
    let called_clone = called.clone();
    let executor: Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync> =
        Arc::new(move |sql| {
            *called_clone.lock().unwrap() = sql.to_string();
            Ok(())
        });
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();

    writer.health_check().unwrap();

    let sql = called.lock().unwrap();
    assert_eq!(*sql, "SELECT 1 FROM DUAL;");
}

// =====================================================================
// DDL 执行测试
// =====================================================================

#[test]
fn oracle_execute_ddl_alter_table() {
    // P4-2 Schema 变更同步：ALTER TABLE ADD COLUMN
    let (sqls, executor) = make_collecting_executor();
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();

    let ddl = "ALTER TABLE \"users\" ADD (\"email\" VARCHAR2(255))";
    writer.execute_ddl(ddl).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0], ddl);
}

#[test]
fn oracle_execute_ddl_drop_table() {
    let (sqls, executor) = make_collecting_executor();
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();

    let ddl = "DROP TABLE \"users\"";
    writer.execute_ddl(ddl).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0], ddl);
}

// =====================================================================
// 错误处理测试
// =====================================================================

#[test]
fn oracle_executor_error_propagates() {
    // 执行器返回错误时应正确传播
    let executor: Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync> = Arc::new(|_sql| {
        Err(WriterError::Sql(
            "ORA-00942: table or view does not exist".to_string(),
        ))
    });
    let writer = OracleWriter::with_executor("oracle://localhost:1521/freepdb1", executor).unwrap();
    let schema = make_users_schema();
    let row = make_row(1, "Alice", 30);
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);

    let result = writer.write_event(&event, &schema, Some(&row));
    assert!(result.is_err());
    assert_eq!(writer.fail_count(), 1);

    if let Err(WriterError::Sql(msg)) = result {
        assert!(msg.contains("ORA-00942"));
    } else {
        panic!("expected WriterError::Sql");
    }
}

#[test]
fn oracle_write_event_without_row_fails() {
    // DML 事件必须提供解码行数据
    let writer = OracleWriter::new("oracle://localhost:1521/freepdb1").unwrap();
    let schema = make_users_schema();
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);

    let result = writer.write_event(&event, &schema, None);
    assert!(result.is_err());
    assert!(matches!(result, Err(WriterError::Internal(_))));
}

#[test]
fn oracle_commit_abort_events_skipped() {
    // Commit/Abort 事件不写入目标端
    let writer = OracleWriter::new("oracle://localhost:1521/freepdb1").unwrap();
    let schema = make_users_schema();

    let commit = ChangeEvent::commit(100, 5001, 1234567890);
    let abort = ChangeEvent::abort(101, 5002, 1234567891);

    writer.write_event(&commit, &schema, None).unwrap();
    writer.write_event(&abort, &schema, None).unwrap();

    assert_eq!(writer.write_count(), 2);
    assert_eq!(writer.generated_sqls().len(), 0);
}

// =====================================================================
// 特殊字符处理测试
// =====================================================================

#[test]
fn oracle_text_with_single_quote_escaped() {
    // 文本中的单引号应被转义为两个单引号
    let schema = make_users_schema();
    let row = szrsql_cdc::decoder::DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("name".to_string(), SzValue::Text("it's a test".to_string())),
            ("age".to_string(), SzValue::Int64(20)),
        ],
    };
    let sql = OracleWriter::generate_insert_sql(&schema, &row, true).unwrap();

    assert!(sql.contains("'it''s a test'"));
}

#[test]
fn oracle_identifier_with_double_quote_escaped() {
    // 标识符中的双引号应被转义为两个双引号
    let schema = TableSchema {
        table_id: 12,
        table_name: "my\"table".to_string(),
        columns: vec![ColumnDef::not_null("col\"name", DataType::Int64)],
        version: 1,
    };
    let row = szrsql_cdc::decoder::DecodedRow {
        columns: vec![("col\"name".to_string(), SzValue::Int64(1))],
    };
    let sql = OracleWriter::generate_insert_sql(&schema, &row, true).unwrap();

    assert!(sql.contains("\"my\"\"table\""));
    assert!(sql.contains("\"col\"\"name\""));
}

// =====================================================================
// TargetConfig 工厂测试
// =====================================================================

#[test]
fn oracle_target_config_factory() {
    use szrsql_cdc::target::{create_writer, TargetConfig};

    let cfg = TargetConfig::oracle("oracle://sys:test123@localhost:1521/freepdb1");
    assert_eq!(cfg.target_type, "oracle");
    assert!(cfg.upsert);
    assert_eq!(cfg.batch_size, 1000);

    let writer = create_writer(&cfg).unwrap();
    assert_eq!(writer.target_type(), "oracle");
}

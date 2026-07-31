//! SQL Server 目标端写入器集成测试（P5-2）
//!
//! # 测试策略
//!
//! 由于 SQL Server 客户端库（tiberius）在 Windows 上需要 native 依赖，
//! 本集成测试采用 `SqlExecutor` 闭包模式，验证 SqlServerWriter 在完整 CDC 流程中的行为：
//!
//! 1. **端到端 CDC 流程**：CdcEngine → ChangeEvent → SqlServerWriter → SQL 生成
//! 2. **SQL 语法验证**：验证生成的 T-SQL 符合 SQL Server 方言语法
//! 3. **幂等性验证**：重复写入同一事件不应产生不同 SQL
//! 4. **多操作序列**：Insert → Update → Delete 完整生命周期
//! 5. **Schema 变更同步**：CREATE TABLE + ALTER TABLE DDL 执行
//! 6. **类型映射覆盖**：所有 SzValue 类型 → T-SQL 字面量

// 兼容旧闭包/非参数化 SQL 方法（P0-2 已废弃，测试仍需验证向后兼容）
#![allow(deprecated)]

use szrsql_cdc::schema::{ColumnDef, DataType, TableSchema};
use szrsql_cdc::target::sqlserver::SqlServerWriter;
use szrsql_cdc::target::{TargetWriter, WriterError};
use szrsql_cdc::ChangeEvent;
use szrsql_types::value::Value as SzValue;
use std::sync::{Arc, Mutex};

// =====================================================================
// 测试辅助
// =====================================================================

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

fn make_row(id: i64, name: &str, age: i32) -> szrsql_cdc::decoder::DecodedRow {
    szrsql_cdc::decoder::DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(id)),
            ("name".to_string(), SzValue::Text(name.to_string())),
            ("age".to_string(), SzValue::Int64(age as i64)),
        ],
    }
}

fn make_collecting_executor() -> (Arc<Mutex<Vec<String>>>, Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync>) {
    let sqls = Arc::new(Mutex::new(Vec::<String>::new()));
    let sqls_clone = sqls.clone();
    let executor: Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync> = Arc::new(move |sql| {
        sqls_clone.lock().unwrap().push(sql.to_string());
        Ok(())
    });
    (sqls, executor)
}

// =====================================================================
// 端到端 CDC 流程测试
// =====================================================================

#[test]
fn sqlserver_end_to_end_insert_update_delete() {
    let (sqls, executor) = make_collecting_executor();
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();
    let schema = make_users_schema();

    let insert_row = make_row(1, "Alice", 30);
    let insert_event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);
    writer.write_event(&insert_event, &schema, Some(&insert_row)).unwrap();

    let update_row = make_row(1, "Bob", 25);
    let update_event = ChangeEvent::update(100, 5001, 1, Vec::new(), Vec::new(), 1234567891);
    writer.write_event(&update_event, &schema, Some(&update_row)).unwrap();

    let delete_row = make_row(1, "Bob", 25);
    let delete_event = ChangeEvent::delete(100, 5002, 1, Vec::new(), 1234567892);
    writer.write_event(&delete_event, &schema, Some(&delete_row)).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 3);
    assert!(collected[0].contains("MERGE"));
    assert!(collected[1].contains("UPDATE"));
    assert!(collected[2].contains("DELETE"));
    assert_eq!(writer.write_count(), 3);
    assert_eq!(writer.fail_count(), 0);
}

#[test]
fn sqlserver_ensure_table_then_write() {
    let (sqls, executor) = make_collecting_executor();
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();
    let schema = make_users_schema();

    writer.ensure_table(&schema).unwrap();

    let row = make_row(42, "Alice", 30);
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);
    writer.write_event(&event, &schema, Some(&row)).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 2);
    assert!(collected[0].contains("CREATE TABLE"));
    assert!(collected[0].contains("BEGIN TRY EXEC"));
    assert!(collected[1].contains("MERGE"));
}

#[test]
fn sqlserver_ensure_table_idempotent() {
    let (sqls, executor) = make_collecting_executor();
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();
    let schema = make_users_schema();

    writer.ensure_table(&schema).unwrap();
    writer.ensure_table(&schema).unwrap();
    writer.ensure_table(&schema).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 1);
}

// =====================================================================
// SQL 语法验证测试
// =====================================================================

#[test]
fn sqlserver_merge_sql_syntax_valid() {
    let schema = make_users_schema();
    let row = make_row(42, "Alice", 30);
    let sql = SqlServerWriter::generate_insert_sql(&schema, &row, true).unwrap();

    // T-SQL MERGE 语法：MERGE [target] AS t USING (...) AS s ON (...) WHEN MATCHED/NOT MATCHED
    assert!(sql.starts_with("MERGE [users] AS t USING"));
    // SQL Server 不需要 FROM DUAL（区别于 Oracle）
    assert!(!sql.contains("DUAL"));
    assert!(sql.contains("AS s ON"));
    assert!(sql.contains("WHEN MATCHED THEN UPDATE SET"));
    assert!(sql.contains("WHEN NOT MATCHED THEN INSERT"));
    assert!(sql.ends_with(';'));
}

#[test]
fn sqlserver_text_uses_n_prefix() {
    // T-SQL 使用 N 前缀表示 Unicode 字符串
    let schema = make_users_schema();
    let row = make_row(1, "hello", 30);
    let sql = SqlServerWriter::generate_insert_sql(&schema, &row, true).unwrap();
    assert!(sql.contains("N'hello'"));
}

#[test]
fn sqlserver_blob_uses_hex_prefix() {
    // T-SQL VARBINARY 使用 0x 前缀
    let schema = TableSchema {
        table_id: 2,
        table_name: "blob_test".to_string(),
        columns: vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::nullable("data", DataType::Blob),
        ],
        version: 1,
    };
    let row = szrsql_cdc::decoder::DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("data".to_string(), SzValue::Blob(vec![0xAB, 0xCD])),
        ],
    };
    let sql = SqlServerWriter::generate_insert_sql(&schema, &row, true).unwrap();
    assert!(sql.contains("0xABCD"));
}

#[test]
fn sqlserver_create_table_uses_try_catch() {
    // SQL Server 用 BEGIN TRY/CATCH 捕获错误 2714
    let schema = make_users_schema();
    let sql = SqlServerWriter::generate_create_table_sql(&schema);

    assert!(sql.starts_with("BEGIN TRY EXEC('"));
    assert!(sql.contains("CREATE TABLE [users]"));
    assert!(sql.contains("END TRY BEGIN CATCH"));
    assert!(sql.contains("ERROR_NUMBER() != 2714"));
    assert!(sql.ends_with("END CATCH;"));
}

// =====================================================================
// 类型映射覆盖测试
// =====================================================================

#[test]
fn sqlserver_all_data_types_in_create_table() {
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
    let sql = SqlServerWriter::generate_create_table_sql(&schema);

    assert!(sql.contains("BIGINT")); // Int64
    assert!(sql.contains("INT")); // Int32
    assert!(sql.contains("NVARCHAR(MAX)")); // Text
    assert!(sql.contains("VARBINARY(MAX)")); // Blob
    assert!(sql.contains("FLOAT(53)")); // Real
    assert!(sql.contains("BIT")); // Bool
    assert!(sql.contains("DATE")); // Date
    assert!(sql.contains("DATETIME2(7)")); // Timestamp
    assert!(sql.contains("NVARCHAR(MAX)")); // Json
    assert!(sql.contains("UNIQUEIDENTIFIER")); // Uuid
}

#[test]
fn sqlserver_value_types_in_merge() {
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
    let sql = SqlServerWriter::generate_insert_sql(&schema, &row, true).unwrap();

    assert!(sql.contains("1")); // Int64
    assert!(sql.contains("N'hello'")); // Text with N prefix
    assert!(sql.contains(", 1,")); // Bool → 1
    assert!(sql.contains("0xABCD")); // Blob
    assert!(sql.contains("3.5")); // Float64
}

// =====================================================================
// 幂等性验证测试
// =====================================================================

#[test]
fn sqlserver_idempotent_insert_same_sql() {
    let writer = SqlServerWriter::new("sqlserver://localhost:1433").unwrap();
    let schema = make_users_schema();
    let row = make_row(1, "Alice", 30);
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);

    writer.write_event(&event, &schema, Some(&row)).unwrap();
    writer.write_event(&event, &schema, Some(&row)).unwrap();

    let sqls = writer.generated_sqls();
    assert_eq!(sqls.len(), 2);
    assert_eq!(sqls[0], sqls[1]);
}

// =====================================================================
// 健康检查测试
// =====================================================================

#[test]
fn sqlserver_health_check_uses_select_1() {
    // SQL Server 健康检查用 SELECT 1（不需要 FROM DUAL）
    let called = Arc::new(Mutex::new(String::new()));
    let called_clone = called.clone();
    let executor: Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync> = Arc::new(move |sql| {
        *called_clone.lock().unwrap() = sql.to_string();
        Ok(())
    });
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();

    writer.health_check().unwrap();

    let sql = called.lock().unwrap();
    assert_eq!(*sql, "SELECT 1;");
}

// =====================================================================
// DDL 执行测试
// =====================================================================

#[test]
fn sqlserver_execute_ddl_alter_table() {
    let (sqls, executor) = make_collecting_executor();
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();

    let ddl = "ALTER TABLE [users] ADD [email] NVARCHAR(255)";
    writer.execute_ddl(ddl).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0], ddl);
}

#[test]
fn sqlserver_execute_ddl_drop_table() {
    let (sqls, executor) = make_collecting_executor();
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();

    let ddl = "DROP TABLE [users]";
    writer.execute_ddl(ddl).unwrap();

    let collected = sqls.lock().unwrap();
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0], ddl);
}

// =====================================================================
// 错误处理测试
// =====================================================================

#[test]
fn sqlserver_executor_error_propagates() {
    let executor: Arc<dyn Fn(&str) -> Result<(), WriterError> + Send + Sync> = Arc::new(|_sql| {
        Err(WriterError::Sql("Invalid object name 'users'".to_string()))
    });
    let writer = SqlServerWriter::with_executor("sqlserver://localhost:1433", executor).unwrap();
    let schema = make_users_schema();
    let row = make_row(1, "Alice", 30);
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);

    let result = writer.write_event(&event, &schema, Some(&row));
    assert!(result.is_err());
    assert_eq!(writer.fail_count(), 1);
}

#[test]
fn sqlserver_write_event_without_row_fails() {
    let writer = SqlServerWriter::new("sqlserver://localhost:1433").unwrap();
    let schema = make_users_schema();
    let event = ChangeEvent::insert(100, 5000, 1, Vec::new(), 1234567890);

    let result = writer.write_event(&event, &schema, None);
    assert!(result.is_err());
    assert!(matches!(result, Err(WriterError::Internal(_))));
}

#[test]
fn sqlserver_commit_abort_events_skipped() {
    let writer = SqlServerWriter::new("sqlserver://localhost:1433").unwrap();
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
fn sqlserver_text_with_single_quote_escaped() {
    let schema = make_users_schema();
    let row = szrsql_cdc::decoder::DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("name".to_string(), SzValue::Text("it's a test".to_string())),
            ("age".to_string(), SzValue::Int64(20)),
        ],
    };
    let sql = SqlServerWriter::generate_insert_sql(&schema, &row, true).unwrap();

    // T-SQL 使用 N 前缀 + 单引号转义为两个单引号
    assert!(sql.contains("N'it''s a test'"));
}

#[test]
fn sqlserver_identifier_with_right_bracket_escaped() {
    // T-SQL 标识符用方括号，右方括号 ] 转义为 ]]
    let schema = TableSchema {
        table_id: 12,
        table_name: "my]table".to_string(),
        columns: vec![ColumnDef::not_null("col]name", DataType::Int64)],
        version: 1,
    };
    let row = szrsql_cdc::decoder::DecodedRow {
        columns: vec![("col]name".to_string(), SzValue::Int64(1))],
    };
    let sql = SqlServerWriter::generate_insert_sql(&schema, &row, true).unwrap();

    assert!(sql.contains("[my]]table]"));
    assert!(sql.contains("[col]]name]"));
}

// =====================================================================
// TargetConfig 工厂测试
// =====================================================================

#[test]
fn sqlserver_target_config_factory() {
    use szrsql_cdc::target::{create_writer, TargetConfig};

    let cfg = TargetConfig::sqlserver("sqlserver://sa:pass@localhost:1433/master");
    assert_eq!(cfg.target_type, "sqlserver");
    assert!(cfg.upsert);
    assert_eq!(cfg.batch_size, 1000);

    let writer = create_writer(&cfg).unwrap();
    assert_eq!(writer.target_type(), "sqlserver");
}

#[test]
fn sqlserver_mssql_alias_supported() {
    // mssql 应作为 sqlserver 的别名
    use szrsql_cdc::target::{create_writer, TargetConfig};

    let cfg = TargetConfig {
        target_type: "mssql".to_string(),
        connection_string: "sqlserver://localhost:1433".to_string(),
        database: None,
        upsert: true,
        batch_size: 1000,
    };
    let writer = create_writer(&cfg).unwrap();
    assert_eq!(writer.target_type(), "sqlserver");
}

//! SQLite 兼容性集成测试 — 验证 L2 文件格式 + SQL 方言兼容性。
//!
//! 运行方式：
//! ```bash
//! cargo run --release --example sqlite_compat_check -p szrsql-sqlite-bridge
//! ```

use szrsql_sqlite_bridge::{SqliteAdapter, SqliteHeader, SqliteType};
use szrsql_types::value::Value;

fn main() {
    println!("=== SzRSQL SQLite Bridge 兼容性测试 ===");
    println!("时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!();

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<(String, String)> = Vec::new();

    macro_rules! ok {
        ($name:expr) => {{
            passed += 1;
            println!("  OK [{}]", $name);
        }};
    }

    macro_rules! fail {
        ($name:expr, $msg:expr) => {{
            failed += 1;
            errors.push(($name.to_string(), $msg.to_string()));
            println!("  FAIL [{}]: {}", $name, $msg);
        }};
    }

    let adapter = SqliteAdapter::new();

    // ============================================================
    // 1. SQL 方言转换测试
    // ============================================================
    println!("--- 1. SQL 方言转换测试 ---");

    // 1.1 简单 SELECT
    match adapter.convert_sql("SELECT 1") {
        Ok(_) => ok!("dialect_select_simple"),
        Err(e) => fail!("dialect_select_simple", format!("{e:?}")),
    }

    // 1.2 SQLite 风格 CREATE TABLE（INTEGER PRIMARY KEY）
    let sqlite_ddl = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)";
    match adapter.convert_sql(sqlite_ddl) {
        Ok(_) => ok!("dialect_create_table_sqlite"),
        Err(e) => fail!("dialect_create_table_sqlite", format!("{e:?}")),
    }

    // 1.3 SQLite AUTOINCREMENT
    let sqlite_autoinc = "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)";
    match adapter.convert_sql(sqlite_autoinc) {
        Ok(_) => ok!("dialect_autoincrement"),
        Err(e) => fail!("dialect_autoincrement", format!("{e:?}")),
    }

    // 1.4 SQLite WITHOUT ROWID
    let sqlite_wo_rowid = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT) WITHOUT ROWID";
    match adapter.convert_sql(sqlite_wo_rowid) {
        Ok(_) => ok!("dialect_without_rowid"),
        Err(e) => fail!("dialect_without_rowid", format!("{e:?}")),
    }

    // 1.5 SQLite PRAGMA
    let sqlite_pragma = "PRAGMA table_info(users)";
    match adapter.convert_sql(sqlite_pragma) {
        Ok(_) => ok!("dialect_pragma"),
        Err(e) => fail!("dialect_pragma", format!("{e:?}")),
    }

    // 1.6 SQLite INSERT
    let sqlite_insert = "INSERT INTO users (id, name, age) VALUES (1, 'Alice', 30)";
    match adapter.convert_sql(sqlite_insert) {
        Ok(_) => ok!("dialect_insert"),
        Err(e) => fail!("dialect_insert", format!("{e:?}")),
    }

    // 1.7 SQLite 复杂查询（JOIN + WHERE）
    let sqlite_complex = "SELECT u.name, COUNT(o.id) FROM users u LEFT JOIN orders o ON u.id = o.uid WHERE u.age > 18 GROUP BY u.name";
    match adapter.convert_sql(sqlite_complex) {
        Ok(_) => ok!("dialect_complex_query"),
        Err(e) => fail!("dialect_complex_query", format!("{e:?}")),
    }

    // 1.8 SQLite 类型亲和性（TEXT、INTEGER、REAL、BLOB、NUMERIC）
    let sqlite_affinity = "CREATE TABLE t (a TEXT, b INTEGER, c REAL, d BLOB, e NUMERIC)";
    match adapter.convert_sql(sqlite_affinity) {
        Ok(_) => ok!("dialect_type_affinity"),
        Err(e) => fail!("dialect_type_affinity", format!("{e:?}")),
    }

    // 1.9 SQLite 语法错误应失败
    match adapter.convert_sql("SELECT FROM WHERE") {
        Ok(_) => fail!("dialect_invalid_syntax", "应失败但成功了".to_string()),
        Err(_) => ok!("dialect_invalid_syntax"),
    }

    // ============================================================
    // 2. 文件格式兼容性测试
    // ============================================================
    println!("\n--- 2. 文件格式兼容性测试 ---");

    let tmp_dir = std::env::temp_dir();
    let test_db = tmp_dir.join("szrsql_sqlite_compat_test.db");

    // 2.1 导出空表
    let tables_empty: Vec<(String, Vec<Value>)> = Vec::new();
    match adapter.export_to_sqlite(&tables_empty, &test_db) {
        Ok(_) => ok!("export_empty"),
        Err(e) => fail!("export_empty", format!("{e:?}")),
    }

    // 2.2 验证导出文件的头部
    match std::fs::read(&test_db) {
        Ok(bytes) => {
            if bytes.len() >= 100 {
                // 校验魔数
                if &bytes[0..16] == szrsql_sqlite_bridge::MAGIC_HEADER {
                    ok!("export_magic_header");
                } else {
                    fail!("export_magic_header", "魔数不匹配".to_string());
                }

                // 校验页面大小
                let page_size = u16::from_be_bytes([bytes[16], bytes[17]]);
                if page_size == szrsql_sqlite_bridge::PAGE_SIZE_DEFAULT {
                    ok!("export_page_size");
                } else {
                    fail!("export_page_size", format!("expected {}, got {}", szrsql_sqlite_bridge::PAGE_SIZE_DEFAULT, page_size));
                }

                // 校验头部解码
                match SqliteHeader::decode(&bytes) {
                    Ok(header) => {
                        println!("    头部信息: page_size={}, text_encoding={}, db_size_pages={}",
                                 header.page_size, header.text_encoding, header.db_size_pages);
                        ok!("header_decode");
                    }
                    Err(e) => fail!("header_decode", format!("{e:?}")),
                }
            } else {
                fail!("export_empty", format!("文件过短: {} bytes", bytes.len()));
            }
        }
        Err(e) => fail!("export_empty", format!("读取失败: {e:?}")),
    }

    // 2.3 导入文件（当前版本返回空 Vec）
    match adapter.import_from_sqlite(&test_db) {
        Ok(data) => {
            if data.is_empty() {
                ok!("import_returns_empty");
            } else {
                fail!("import_returns_empty", format!("expected empty, got {} tables", data.len()));
            }
        }
        Err(e) => fail!("import_returns_empty", format!("{e:?}")),
    }

    // 2.4 导入非法文件应失败
    let bad_db = tmp_dir.join("szrsql_sqlite_bad_test.db");
    std::fs::write(&bad_db, b"this is not a sqlite file").unwrap();
    match adapter.import_from_sqlite(&bad_db) {
        Ok(_) => fail!("import_invalid_file", "应失败但成功了".to_string()),
        Err(_) => ok!("import_invalid_file"),
    }

    // 2.5 导出有数据的表（当前版本仅写头部）
    let tables_with_data: Vec<(String, Vec<Value>)> = vec![
        ("users".to_string(), vec![Value::Int64(1), Value::Text("Alice".to_string())]),
        ("orders".to_string(), vec![Value::Int64(100), Value::Float64(99.5)]),
    ];
    match adapter.export_to_sqlite(&tables_with_data, &test_db) {
        Ok(_) => ok!("export_with_data"),
        Err(e) => fail!("export_with_data", format!("{e:?}")),
    }

    // 清理临时文件
    let _ = std::fs::remove_file(&test_db);
    let _ = std::fs::remove_file(&bad_db);

    // ============================================================
    // 3. 类型映射测试
    // ============================================================
    println!("\n--- 3. 类型映射测试 ---");

    // 3.1 NULL 映射
    if SqliteType::from_value(&Value::Null) == SqliteType::Null {
        ok!("type_null");
    } else {
        fail!("type_null", "NULL 映射错误".to_string());
    }

    // 3.2 Int64 映射
    if SqliteType::from_value(&Value::Int64(42)) == SqliteType::Integer {
        ok!("type_int64");
    } else {
        fail!("type_int64", "Int64 映射错误".to_string());
    }

    // 3.3 Float64 映射
    if SqliteType::from_value(&Value::Float64(3.5)) == SqliteType::Float {
        ok!("type_float64");
    } else {
        fail!("type_float64", "Float64 映射错误".to_string());
    }

    // 3.4 Text 映射
    if SqliteType::from_value(&Value::Text("hello".to_string())) == SqliteType::Text {
        ok!("type_text");
    } else {
        fail!("type_text", "Text 映射错误".to_string());
    }

    // 3.5 Blob 映射
    if SqliteType::from_value(&Value::Blob(vec![1, 2, 3])) == SqliteType::Blob {
        ok!("type_blob");
    } else {
        fail!("type_blob", "Blob 映射错误".to_string());
    }

    // 3.6 Bool 映射（SQLite 无布尔类型，映射为 Integer）
    if SqliteType::from_value(&Value::Bool(true)) == SqliteType::Integer {
        ok!("type_bool_to_integer");
    } else {
        fail!("type_bool_to_integer", "Bool 应映射为 Integer".to_string());
    }

    // ============================================================
    // 汇总
    // ============================================================
    let total = passed + failed;
    println!("\n=== SQLite Bridge 兼容性测试汇总 ===");
    println!("总数: {}, 通过: {}, 失败: {}", total, passed, failed);
    if failed > 0 {
        println!("\n失败列表:");
        for (name, msg) in &errors {
            println!("  - {}: {}", name, msg);
        }
        std::process::exit(1);
    } else {
        println!("\n所有测试通过！");
        std::process::exit(0);
    }
}

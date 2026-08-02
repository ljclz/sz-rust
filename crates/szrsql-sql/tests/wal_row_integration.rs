//! P9-2 集成测试 — DML 行级 WAL 记录接入。
//!
//! 验证 Executor 的 DML 操作（INSERT/UPDATE/DELETE）在绑定 WalWriter 后，
//! 能将行级变更以 `WalOpType::Insert/Update/Delete` 记录写入 WAL 文件，
//! 并能通过 WalReader 读回且载荷正确。
//!
//! 事件流：
//!   Executor.mvcc_insert / execute_update / execute_delete
//!     → append_wal_row_insert / update / delete
//!     → WalWriter.append
//!     → WAL 文件
//!     → WalReader.read_next
//!
//! # 验收标准
//!
//! - INSERT 后 WAL 中存在 Insert 记录，载荷包含 new_payload
//! - UPDATE 后 WAL 中存在 Update 记录，载荷包含 old_payload + new_payload
//! - DELETE 后 WAL 中存在 Delete 记录，载荷包含 old_payload
//! - 未绑定 WalWriter 时 DML 静默跳过行级记录（旧行为兼容）
//! - 所有行级记录的 tx_id 与 executor 的 wal_tx_id 一致

use std::sync::Arc;

use szrsql_sql::executor::{Executor, InMemoryTable};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::ColumnType;

use szrsql_tx::wal::{WalOpType, WalReader, WalRowChange, WalWriter};

// =====================================================================
//  辅助函数
// =====================================================================

/// SQL → AST → LogicalPlan
fn plan_sql(sql: &str, catalog: &dyn szrsql_sql::plan::Catalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(
        stmts.len(),
        1,
        "expected exactly 1 statement, got {}",
        stmts.len()
    );
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

/// 构造测试 catalog：test_table(id INT, name TEXT)
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "test_table",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog
}

/// 构造测试表
fn make_table() -> InMemoryTable {
    InMemoryTable::with_columns(
        "test_table",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    )
}

/// 全局原子计数器，确保每个测试用例使用不同的 WAL 文件名（避免并行冲突）
static WAL_FILE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 创建临时 WAL 文件并返回 (Writer, Path)
fn make_wal_writer(test_name: &str) -> (Arc<WalWriter>, std::path::PathBuf) {
    let seq = WAL_FILE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "szrsql_p9_2_{}_{}_{}.wal",
        test_name,
        std::process::id(),
        seq
    ));
    std::fs::remove_file(&path).ok();
    let writer = Arc::new(WalWriter::create_new(&path).expect("create WAL failed"));
    (writer, path)
}

/// 读取 WAL 文件中的所有记录
fn read_all_records(path: &std::path::Path) -> Vec<szrsql_tx::wal::WalRecord> {
    let mut reader = WalReader::open(path).expect("open WAL failed");
    let mut records = Vec::new();
    while let Ok(Some(r)) = reader.read_next() {
        records.push(r);
    }
    records
}

// =====================================================================
//  P9-2 测试
// =====================================================================

#[test]
fn test_p9_2_wal_insert_row_record_written() {
    // 验证：绑定 WalWriter 后，INSERT 操作会在 WAL 中写入 Insert 记录
    let catalog = make_catalog();
    let mut table = make_table();
    let (writer, path) = make_wal_writer("insert");

    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_wal_writer(writer.clone());

    let plan = plan_sql(
        "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
        &catalog,
    );
    let result = executor
        .execute_insert(&plan, &mut table)
        .expect("insert failed");
    assert_eq!(result.affected_rows, 1);

    // flush 确保 OS 缓冲区写入文件
    writer.flush().expect("flush failed");
    drop(writer);

    let records = read_all_records(&path);
    // 应至少有 1 条 Insert 记录
    let inserts: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Insert)
        .collect();
    assert_eq!(
        inserts.len(),
        1,
        "expected 1 Insert WAL record, got {} (total records: {})",
        inserts.len(),
        records.len()
    );

    let record = inserts[0];
    assert_eq!(record.tx_id, 1, "autocommit mode tx_id should be 1");

    // 解码载荷验证 new_payload 非空
    let change = WalRowChange::decode_insert(record.page_id, &record.data)
        .expect("decode Insert payload failed");
    assert!(
        !change.new_payload.is_empty(),
        "Insert new_payload should be non-empty"
    );
    assert!(change.old_payload.is_empty());

    // new_payload 应为 serde_json 序列化的 Vec<Value>，包含 alice
    let new_payload_str = String::from_utf8_lossy(&change.new_payload);
    assert!(
        new_payload_str.contains("alice"),
        "new_payload should contain 'alice', got: {}",
        new_payload_str
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn test_p9_2_wal_update_row_record_written() {
    // 验证：UPDATE 操作会在 WAL 中写入 Update 记录，载荷包含 old + new
    let catalog = make_catalog();
    let mut table = make_table();
    let (writer, path) = make_wal_writer("update");

    // 先 INSERT 一行
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_wal_writer(writer.clone());
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    // UPDATE
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_wal_writer(writer.clone());
        let plan = plan_sql("UPDATE test_table SET name = 'bob' WHERE id = 1", &catalog);
        let result = executor
            .execute_update(&plan, &mut table)
            .expect("update failed");
        assert_eq!(result.affected_rows, 1);
    }

    writer.flush().expect("flush failed");
    drop(writer);

    let records = read_all_records(&path);
    let updates: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Update)
        .collect();
    assert_eq!(
        updates.len(),
        1,
        "expected 1 Update WAL record, got {}",
        updates.len()
    );

    let record = updates[0];
    let change = WalRowChange::decode_update(record.page_id, &record.data)
        .expect("decode Update payload failed");
    assert!(
        !change.old_payload.is_empty(),
        "old_payload should be non-empty"
    );
    assert!(
        !change.new_payload.is_empty(),
        "new_payload should be non-empty"
    );

    let old_str = String::from_utf8_lossy(&change.old_payload);
    let new_str = String::from_utf8_lossy(&change.new_payload);
    assert!(
        old_str.contains("alice"),
        "old_payload should contain 'alice'"
    );
    assert!(new_str.contains("bob"), "new_payload should contain 'bob'");

    std::fs::remove_file(&path).ok();
}

#[test]
fn test_p9_2_wal_delete_row_record_written() {
    // 验证：DELETE 操作会在 WAL 中写入 Delete 记录，载荷包含 old
    let catalog = make_catalog();
    let mut table = make_table();
    let (writer, path) = make_wal_writer("delete");

    // 先 INSERT 一行
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_wal_writer(writer.clone());
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    // DELETE
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_wal_writer(writer.clone());
        let plan = plan_sql("DELETE FROM test_table WHERE id = 1", &catalog);
        let result = executor
            .execute_delete(&plan, &mut table)
            .expect("delete failed");
        assert_eq!(result.affected_rows, 1);
    }

    writer.flush().expect("flush failed");
    drop(writer);

    let records = read_all_records(&path);
    let deletes: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Delete)
        .collect();
    assert_eq!(
        deletes.len(),
        1,
        "expected 1 Delete WAL record, got {}",
        deletes.len()
    );

    let record = deletes[0];
    let change = WalRowChange::decode_delete(record.page_id, &record.data)
        .expect("decode Delete payload failed");
    assert!(
        !change.old_payload.is_empty(),
        "old_payload should be non-empty"
    );
    assert!(change.new_payload.is_empty(), "new_payload should be empty");

    let old_str = String::from_utf8_lossy(&change.old_payload);
    assert!(
        old_str.contains("alice"),
        "old_payload should contain 'alice'"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn test_p9_2_wal_no_writer_no_records() {
    // 验证：未绑定 WalWriter 时，DML 不写入任何行级 WAL 记录（旧行为兼容）
    let catalog = make_catalog();
    let mut table = make_table();
    let (_, path) = make_wal_writer("no_writer");

    // 显式不绑定 WalWriter
    let executor = Executor::new().with_catalog(&catalog);

    let plan = plan_sql(
        "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
        &catalog,
    );
    let result = executor
        .execute_insert(&plan, &mut table)
        .expect("insert failed");
    assert_eq!(result.affected_rows, 1);

    // WAL 文件应为空（无记录写入）
    let records = read_all_records(&path);
    assert!(
        records.is_empty(),
        "expected 0 WAL records without WalWriter, got {}",
        records.len()
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn test_p9_2_wal_multiple_dml_records_lsn_monotonic() {
    // 验证：多次 DML 操作后，WAL 中的行级记录 LSN 单调递增
    let catalog = make_catalog();
    let mut table = make_table();
    let (writer, path) = make_wal_writer("multi");

    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_wal_writer(writer.clone());

    // 3 次 INSERT
    for i in 1..=3 {
        let sql = format!(
            "INSERT INTO test_table (id, name) VALUES ({}, 'user{}')",
            i, i
        );
        let plan = plan_sql(&sql, &catalog);
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    writer.flush().expect("flush failed");
    drop(writer);

    let records = read_all_records(&path);
    let inserts: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Insert)
        .collect();
    assert_eq!(inserts.len(), 3, "expected 3 Insert records");

    // LSN 单调递增
    assert!(inserts[0].lsn < inserts[1].lsn);
    assert!(inserts[1].lsn < inserts[2].lsn);

    std::fs::remove_file(&path).ok();
}

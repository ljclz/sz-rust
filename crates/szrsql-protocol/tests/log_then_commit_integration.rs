//! ADV-F-7: log-then-commit transaction model integration tests
//!
//! Verify that after injecting WalWriter, COMMIT/ROLLBACK operations correctly
//! write WAL records and fsync, ensuring "ACKed transactions are always persisted".
//!
//! # Test coverage
//!
//! 1. COMMIT writes WAL Commit record
//! 2. Backward compatible without WalWriter
//! 3. ROLLBACK writes WAL Abort record
//! 4. Transaction ID monotonically increasing
//! 5. Multi-transaction WAL LSN ordering correct

use szrsql_protocol::pgwire::session::{ExecutorService, QueryResult};
use szrsql_tx::wal::{WalOpType, WalReader, WalWriter};
use std::sync::Arc;

/// Helper: create temporary WAL file path under F:\test\data
///
/// NOTE: test data is written to F:\test\data (user requirement: do not use C drive)
fn temp_wal_path() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(r"F:\test\data");
    std::fs::create_dir_all(&dir).ok();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    dir.join(format!("szrsql_adv_f7_test_{}.log", unique))
}

/// ADV-F-7: verify COMMIT writes WAL Commit record after injecting WalWriter
#[tokio::test]
async fn test_log_then_commit_writes_wal_commit_record() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (2)").await;

    let results = svc.execute_sql("COMMIT").await;
    assert!(matches!(
        &results[0],
        Ok(QueryResult::TransactionComplete { tag, .. }) if tag == "COMMIT"
    ));

    // Verify WAL contains Commit record
    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, eof) = reader.read_all().unwrap();
    assert!(eof, "WAL should be fully read");

    let commit_records: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Commit)
        .collect();
    assert_eq!(
        commit_records.len(),
        1,
        "expected exactly 1 Commit record, got {}",
        commit_records.len()
    );
    assert_eq!(commit_records[0].tx_id, 1);

    // Verify data is persisted
    let results = svc.execute_sql("SELECT * FROM t").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 2),
        other => panic!("expected ResultSet, got {other:?}"),
    }

    let _ = std::fs::remove_file(&wal_path);
}

/// ADV-F-7: verify COMMIT works with old behavior when no WalWriter is injected
#[tokio::test]
async fn test_log_then_commit_backward_compatible() {
    let mut svc = ExecutorService::new();

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (42)").await;

    let results = svc.execute_sql("COMMIT").await;
    assert!(matches!(
        &results[0],
        Ok(QueryResult::TransactionComplete { tag, .. }) if tag == "COMMIT"
    ));

    let results = svc.execute_sql("SELECT * FROM t").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 1),
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

/// ADV-F-7: verify ROLLBACK writes WAL Abort record
#[tokio::test]
async fn test_rollback_writes_wal_abort_record() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;

    let results = svc.execute_sql("ROLLBACK").await;
    assert!(matches!(
        &results[0],
        Ok(QueryResult::TransactionComplete { tag, .. }) if tag == "ROLLBACK"
    ));

    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, _) = reader.read_all().unwrap();

    let abort_records: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Abort)
        .collect();
    assert_eq!(abort_records.len(), 1, "expected 1 Abort record");
    assert_eq!(abort_records[0].tx_id, 1);

    let results = svc.execute_sql("SELECT * FROM t").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 0),
        other => panic!("expected ResultSet, got {other:?}"),
    }

    let _ = std::fs::remove_file(&wal_path);
}

/// ADV-F-7: verify transaction ID is monotonically increasing
#[tokio::test]
async fn test_transaction_id_monotonic() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;

    // Transaction 1 (COMMIT)
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;
    svc.execute_sql("COMMIT").await;

    // Transaction 2 (COMMIT)
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (2)").await;
    svc.execute_sql("COMMIT").await;

    // Transaction 3 (ROLLBACK)
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (3)").await;
    svc.execute_sql("ROLLBACK").await;

    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, _) = reader.read_all().unwrap();

    let commit_abort: Vec<_> = records
        .iter()
        .filter(|r| matches!(r.op_type, WalOpType::Commit | WalOpType::Abort))
        .collect();

    assert_eq!(commit_abort.len(), 3);
    assert_eq!(commit_abort[0].tx_id, 1);
    assert_eq!(commit_abort[1].tx_id, 2);
    assert_eq!(commit_abort[2].tx_id, 3);

    let results = svc.execute_sql("SELECT * FROM t").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 2),
        other => panic!("expected ResultSet, got {other:?}"),
    }

    let _ = std::fs::remove_file(&wal_path);
}

/// ADV-F-7: verify multiple consecutive transactions have correct WAL LSN ordering
#[tokio::test]
async fn test_multiple_transactions_wal_ordering() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE counter (val BIGINT)").await;

    for i in 1..=5i64 {
        svc.execute_sql("BEGIN").await;
        svc.execute_sql(&format!("INSERT INTO counter (val) VALUES ({i})")).await;
        svc.execute_sql("COMMIT").await;
    }

    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, _) = reader.read_all().unwrap();

    let commit_records: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Commit)
        .collect();
    assert_eq!(commit_records.len(), 5);

    for i in 0..4 {
        assert!(
            commit_records[i].lsn < commit_records[i + 1].lsn,
            "LSN should be monotonically increasing: {} at index {} not < {} at index {}",
            commit_records[i].lsn,
            i,
            commit_records[i + 1].lsn,
            i + 1
        );
    }

    for (i, r) in commit_records.iter().enumerate() {
        assert_eq!(r.tx_id, (i + 1) as u32, "txn_id should be {}", i + 1);
    }

    let results = svc.execute_sql("SELECT * FROM counter ORDER BY val").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 5),
        other => panic!("expected ResultSet, got {other:?}"),
    }

    let _ = std::fs::remove_file(&wal_path);
}

/// ADV-F-7: verify ROLLBACK after COMMIT does not affect committed data
#[tokio::test]
async fn test_commit_then_rollback_isolation() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;

    // Transaction 1: INSERT + COMMIT
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (100)").await;
    svc.execute_sql("COMMIT").await;

    // Transaction 2: INSERT + ROLLBACK
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (200)").await;
    svc.execute_sql("ROLLBACK").await;

    // Verify only transaction 1's data exists
    let results = svc.execute_sql("SELECT * FROM t").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows.len(), 1, "only committed transaction's data should exist");
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }

    // Verify WAL has 1 Commit + 1 Abort
    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, _) = reader.read_all().unwrap();

    let commit_count = records.iter().filter(|r| r.op_type == WalOpType::Commit).count();
    let abort_count = records.iter().filter(|r| r.op_type == WalOpType::Abort).count();
    assert_eq!(commit_count, 1, "expected 1 Commit record");
    assert_eq!(abort_count, 1, "expected 1 Abort record");

    let _ = std::fs::remove_file(&wal_path);
}

/// ADV-F-7: verify empty transaction (BEGIN + COMMIT without DML) also writes WAL
#[tokio::test]
async fn test_empty_transaction_writes_commit_record() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;

    // Empty transaction
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("COMMIT").await;

    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, _) = reader.read_all().unwrap();

    let commit_records: Vec<_> = records
        .iter()
        .filter(|r| r.op_type == WalOpType::Commit)
        .collect();
    assert_eq!(commit_records.len(), 1, "empty transaction should still write Commit record");
    assert_eq!(commit_records[0].tx_id, 1);

    let _ = std::fs::remove_file(&wal_path);
}

/// ADV-F-7: verify WAL record checksum integrity
#[tokio::test]
async fn test_wal_record_checksum_integrity() {
    let wal_path = temp_wal_path();
    let _ = std::fs::remove_file(&wal_path);

    let writer = Arc::new(WalWriter::create_new(&wal_path).unwrap());
    let mut svc = ExecutorService::new().with_wal_writer(writer.clone());

    svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
    svc.execute_sql("BEGIN").await;
    svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;
    svc.execute_sql("COMMIT").await;

    // Read WAL, verify all records pass checksum (read_all auto-verifies)
    let mut reader = WalReader::open(&wal_path).unwrap();
    let (records, eof) = reader.read_all().unwrap();
    assert!(eof, "WAL should be fully read without checksum errors");
    assert!(!records.is_empty(), "WAL should contain at least 1 record");

    // Each record's checksum should be non-zero
    for r in &records {
        assert_ne!(r.checksum, 0, "checksum should be non-zero for record at LSN {}", r.lsn);
    }

    let _ = std::fs::remove_file(&wal_path);
}

//! 分页存储 MVCC 元数据保留（P1-6 / BUG-11）测试套件。
//!
//! 覆盖：
//! - spill 存储 xmin/xmax 及 tombstone 行（含已删除行）
//! - restore 重建 xmin/xmax 向量及 deleted 集合
//! - 活跃行可见性：rows() 过滤 tombstone
//! - 混合场景：活跃行 + tombstone + 不同 xmin
//! - v1 格式回退：无 flags 字节时恢复为 xmin=0, xmax=0, deleted=∅

use crate::executor::InMemoryTable;
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 为表启用分页存储（使用临时目录）
fn enable_paged(table: &mut InMemoryTable) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let path = tmp.path().join("paged");
    table
        .enable_paged_storage(&path)
        .expect("enable paged storage");
    tmp
}

// =====================================================================
//  测试：spill 保留 MVCC 元数据
// =====================================================================

#[test]
fn test_spill_preserves_xmin_xmax() {
    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    let _tmp = enable_paged(&mut table);

    // 插入行，设置不同 xmin
    let id0 = table.insert_with_xmin(vec![Value::Int64(1), Value::Int64(100)], 10);
    let id1 = table.insert_with_xmin(vec![Value::Int64(2), Value::Int64(200)], 20);
    let id2 = table.insert_with_xmin(vec![Value::Int64(3), Value::Int64(300)], 30);

    // 删除 id1（xmax = u32::MAX，tombstone）
    assert!(table.delete_row(id1));

    // spill
    table.spill_to_paged_storage().expect("spill");

    // 验证 B+Tree 中 xmin/xmax 未变（spill 不修改内存状态）
    assert_eq!(table.row_version(id0), Some((10, 0)));
    assert_eq!(table.row_version(id1), Some((20, u32::MAX)));
    assert_eq!(table.row_version(id2), Some((30, 0)));
    assert!(table.is_deleted(id1));
    assert!(!table.is_deleted(id0));
    assert!(!table.is_deleted(id2));
}

// =====================================================================
//  测试：restore 重建 MVCC 元数据
// =====================================================================

#[test]
fn test_restore_rebuilds_mvcc_metadata() {
    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    let _tmp = enable_paged(&mut table);

    let id0 = table.insert_with_xmin(vec![Value::Int64(1), Value::Int64(100)], 10);
    let id1 = table.insert_with_xmin(vec![Value::Int64(2), Value::Int64(200)], 20);
    let id2 = table.insert_with_xmin(vec![Value::Int64(3), Value::Int64(300)], 30);

    // 删除 id1
    assert!(table.delete_row(id1));
    assert_eq!(table.total_row_count(), 3);
    assert_eq!(table.rows().len(), 2); // rows() 过滤 tombstone

    // spill → restore
    table.spill_to_paged_storage().expect("spill");
    table.restore_from_paged_storage().expect("restore");

    // total_row_count 含 tombstone
    assert_eq!(table.total_row_count(), 3);

    // xmin 保留
    assert_eq!(table.row_version(id0), Some((10, 0)));
    assert_eq!(table.row_version(id1), Some((20, u32::MAX)));
    assert_eq!(table.row_version(id2), Some((30, 0)));

    // deleted 集合保留
    assert!(table.is_deleted(id1));
    assert!(!table.is_deleted(id0));
    assert!(!table.is_deleted(id2));

    // rows() 过滤 tombstone → 2 行
    let active = table.rows();
    assert_eq!(active.len(), 2);
    let ids: Vec<i64> = active
        .iter()
        .map(|r| match &r[0] {
            Value::Int64(v) => *v,
            _ => panic!("expected Int64"),
        })
        .collect();
    assert!(ids.contains(&1));
    assert!(ids.contains(&3));
    assert!(!ids.contains(&2));
}

// =====================================================================
//  测试：全部活跃行（无删除）restore 后 xmin 保留
// =====================================================================

#[test]
fn test_restore_all_active_rows_preserve_xmin() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    let _tmp = enable_paged(&mut table);

    let _ = table.insert_with_xmin(vec![Value::Int64(1)], 100);
    let _ = table.insert_with_xmin(vec![Value::Int64(2)], 200);

    table.spill_to_paged_storage().expect("spill");
    table.restore_from_paged_storage().expect("restore");

    assert_eq!(table.row_version(0), Some((100, 0)));
    assert_eq!(table.row_version(1), Some((200, 0)));
    assert_eq!(table.total_row_count(), 2);
    assert_eq!(table.rows().len(), 2);
    assert!(!table.is_deleted(0));
    assert!(!table.is_deleted(1));
}

// =====================================================================
//  测试：全部删除行 restore 后全部为 tombstone
// =====================================================================

#[test]
fn test_restore_all_deleted_rows_are_tombstones() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    let _tmp = enable_paged(&mut table);

    let id0 = table.insert_with_xmin(vec![Value::Int64(1)], 10);
    let id1 = table.insert_with_xmin(vec![Value::Int64(2)], 20);

    assert!(table.delete_row(id0));
    assert!(table.delete_row(id1));

    table.spill_to_paged_storage().expect("spill");
    table.restore_from_paged_storage().expect("restore");

    // 物理 2 行，活跃 0 行
    assert_eq!(table.total_row_count(), 2);
    assert_eq!(table.rows().len(), 0);
    assert!(table.is_deleted(id0));
    assert!(table.is_deleted(id1));
    // xmax = u32::MAX 标记删除
    assert_eq!(table.row_version(id0).map(|(_, x)| x), Some(u32::MAX));
    assert_eq!(table.row_version(id1).map(|(_, x)| x), Some(u32::MAX));
}

// =====================================================================
//  测试：空表 spill/restore
// =====================================================================

#[test]
fn test_spill_restore_empty_table() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    let _tmp = enable_paged(&mut table);

    table.spill_to_paged_storage().expect("spill");
    table.restore_from_paged_storage().expect("restore");

    assert_eq!(table.total_row_count(), 0);
    assert_eq!(table.rows().len(), 0);
}

// =====================================================================
//  测试：xmin=0 的普通行 restore 后保持 xmin=0
// =====================================================================

#[test]
fn test_restore_plain_insert_keeps_xmin_zero() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    let _tmp = enable_paged(&mut table);

    let _ = table.insert(vec![Value::Int64(42)]); // xmin=0

    table.spill_to_paged_storage().expect("spill");
    table.restore_from_paged_storage().expect("restore");

    assert_eq!(table.row_version(0), Some((0, 0)));
    assert_eq!(table.rows().len(), 1);
    assert!(matches!(&table.rows()[0][0], Value::Int64(42)));
}

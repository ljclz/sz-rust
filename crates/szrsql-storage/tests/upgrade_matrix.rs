//! 版本矩阵集成测试 — Phase 7e.7
//!
//! 对应 `SzRSQL实施进度.md` Phase 7e.7。
//!
//! # 验证目标
//!
//! 验证 SzRSQL 跨版本升级路径的正确性：
//!
//! ```text
//! v0.1.0 ──MINOR──→ v0.2.0 ──MINOR──→ v0.5.0 ──MAJOR──→ v1.0.0
//! ```
//!
//! # 测试矩阵
//!
//! | 升级路径 | 类型 | 数据量 | 验证点 |
//! |---------|------|-------|-------|
//! | v0.1.0 → v0.2.0 | MINOR | 10000 | 数据完整 |
//! | v0.2.0 → v0.5.0 | MINOR | 10000 | 数据完整 |
//! | v0.5.0 → v1.0.0 | MAJOR | 10000 | 数据迁移完整 |
//! | v0.1.0 → v1.0.0 | MAJOR | 10000 | 跨 3 版本 MAJOR 升级 |
//! | v0.1.0 → v0.2.0 → v0.5.0 → v1.0.0 | 连续 | 10000 | 3 次连续升级 |
//! | v0.1.0 → v0.2.0 PATCH 路径 | PATCH | 1000 | v0.1.0→v0.1.1 PATCH |
//! | v0.2.0 → v0.2.1 PATCH 路径 | PATCH | 1000 | PATCH 升级 |
//! | v1.0.0 → v1.0.1 PATCH 路径 | PATCH | 1000 | PATCH 升级 |
//! | v0.1.0 → v1.0.0 大数据量 | MAJOR | 100000 | 10 万行 MAJOR 升级 |
//! | v0.1.0 升级失败回滚 | PATCH | 10000 | 回滚零丢失 |
//! | 升级创建备份 | MINOR | 5000 | 备份计数 +1 |
//! | MajorUpgradeExecutor 直接执行 | MAJOR | 5000 | 直接调用成功 |

use szrsql_storage::upgrade::{
    classify_upgrade, generate_mock_rows, verify_rows_equal, MajorUpgradeExecutor, UpgradeContext,
    UpgradeKind, UpgradeOutcome, Version,
};

// =====================================================================
//  辅助函数
// =====================================================================

/// 生成有效的文件头字节（22 字节，小端序）
///
/// 布局：magic(4) + format_version(2) + flags(2) + created_at(8) + page_size(4) + reserved(2)
/// offset：0-3 magic | 4-5 format_version | 6-7 flags | 8-15 created_at | 16-19 page_size | 20-21 reserved
fn make_valid_header() -> [u8; 22] {
    let mut header = [0u8; 22];
    // magic = 0x42445A53 (小端序，磁盘字节 "SZDB")
    header[0] = 0x53;
    header[1] = 0x5A;
    header[2] = 0x44;
    header[3] = 0x42;
    // format_version = 4 (小端序)
    header[4] = 0x04;
    header[5] = 0x00;
    // flags = 0
    header[6] = 0x00;
    header[7] = 0x00;
    // created_at = 0 (offset 8-15，已由零初始化)
    // page_size = 4096 = 0x00001000 (小端序，offset 16-19)
    header[16] = 0x00;
    header[17] = 0x10;
    header[18] = 0x00;
    header[19] = 0x00;
    // reserved = 0 (offset 20-21，已由零初始化)
    header
}

// =====================================================================
//  版本矩阵测试 — MINOR 升级
// =====================================================================

/// 测试 v0.1.0 → v0.2.0 MINOR 升级
#[test]
fn matrix_v0_1_0_to_v0_2_0_minor_upgrade() {
    let from = Version::new(0, 1, 0);
    let to = Version::new(0, 2, 0);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Minor);

    let rows = generate_mock_rows(10000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_patch_minor_upgrade(&from, &to, &make_valid_header(), &rows)
        .expect("v0.1.0 → v0.2.0 MINOR 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Minor);
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.1.0 → v0.2.0 MINOR 升级失败: {:?}", reason);
        }
    }
}

/// 测试 v0.2.0 → v0.5.0 MINOR 升级
#[test]
fn matrix_v0_2_0_to_v0_5_0_minor_upgrade() {
    let from = Version::new(0, 2, 0);
    let to = Version::new(0, 5, 0);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Minor);

    let rows = generate_mock_rows(10000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_patch_minor_upgrade(&from, &to, &make_valid_header(), &rows)
        .expect("v0.2.0 → v0.5.0 MINOR 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Minor);
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.2.0 → v0.5.0 MINOR 升级失败: {:?}", reason);
        }
    }
}

// =====================================================================
//  版本矩阵测试 — MAJOR 升级
// =====================================================================

/// 测试 v0.5.0 → v1.0.0 MAJOR 升级
#[test]
fn matrix_v0_5_0_to_v1_0_0_major_upgrade() {
    let from = Version::new(0, 5, 0);
    let to = Version::new(1, 0, 0);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Major);

    let rows = generate_mock_rows(10000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_major_upgrade(from, to, &rows)
        .expect("v0.5.0 → v1.0.0 MAJOR 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Major);
            assert_eq!(result_rows.len(), rows.len());
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.5.0 → v1.0.0 MAJOR 升级失败: {:?}", reason);
        }
    }
}

/// 测试 v0.1.0 → v1.0.0 跨 3 个版本的 MAJOR 升级
#[test]
fn matrix_v0_1_0_to_v1_0_0_cross_3_versions_major_upgrade() {
    let from = Version::new(0, 1, 0);
    let to = Version::new(1, 0, 0);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Major);

    let rows = generate_mock_rows(10000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_major_upgrade(from, to, &rows)
        .expect("v0.1.0 → v1.0.0 跨版本 MAJOR 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Major);
            assert_eq!(result_rows.len(), rows.len());
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.1.0 → v1.0.0 跨版本 MAJOR 升级失败: {:?}", reason);
        }
    }
}

// =====================================================================
//  连续升级路径测试
// =====================================================================

/// 测试连续升级路径：v0.1.0 → v0.2.0 → v0.5.0 → v1.0.0
#[test]
fn matrix_sequential_upgrade_path_v0_1_0_to_v1_0_0() {
    let rows = generate_mock_rows(10000);
    let original_rows = rows.clone();
    let header = make_valid_header();

    // Step 1: v0.1.0 → v0.2.0 (MINOR)
    let mut ctx = UpgradeContext::new();
    let outcome1 = ctx
        .execute_patch_minor_upgrade(
            &Version::new(0, 1, 0),
            &Version::new(0, 2, 0),
            &header,
            &rows,
        )
        .expect("Step 1 v0.1.0 → v0.2.0 应成功");
    let rows = match outcome1 {
        UpgradeOutcome::Success { rows, .. } => rows,
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("Step 1 v0.1.0 → v0.2.0 失败: {:?}", reason);
        }
    };
    assert!(verify_rows_equal(&original_rows, &rows));

    // Step 2: v0.2.0 → v0.5.0 (MINOR)
    let outcome2 = ctx
        .execute_patch_minor_upgrade(
            &Version::new(0, 2, 0),
            &Version::new(0, 5, 0),
            &header,
            &rows,
        )
        .expect("Step 2 v0.2.0 → v0.5.0 应成功");
    let rows = match outcome2 {
        UpgradeOutcome::Success { rows, .. } => rows,
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("Step 2 v0.2.0 → v0.5.0 失败: {:?}", reason);
        }
    };
    assert!(verify_rows_equal(&original_rows, &rows));

    // Step 3: v0.5.0 → v1.0.0 (MAJOR)
    let outcome3 = ctx
        .execute_major_upgrade(Version::new(0, 5, 0), Version::new(1, 0, 0), &rows)
        .expect("Step 3 v0.5.0 → v1.0.0 应成功");
    let rows = match outcome3 {
        UpgradeOutcome::Success { rows, .. } => rows,
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("Step 3 v0.5.0 → v1.0.0 失败: {:?}", reason);
        }
    };
    assert_eq!(rows.len(), original_rows.len());
    assert!(verify_rows_equal(&original_rows, &rows));
}

// =====================================================================
//  PATCH 升级路径测试
// =====================================================================

/// 测试 v0.1.0 → v0.1.1 PATCH 升级
#[test]
fn matrix_v0_1_0_to_v0_1_1_patch_upgrade() {
    let from = Version::new(0, 1, 0);
    let to = Version::new(0, 1, 1);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Patch);

    let rows = generate_mock_rows(1000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_patch_minor_upgrade(&from, &to, &make_valid_header(), &rows)
        .expect("v0.1.0 → v0.1.1 PATCH 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Patch);
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.1.0 → v0.1.1 PATCH 升级失败: {:?}", reason);
        }
    }
}

/// 测试 v0.2.0 → v0.2.1 PATCH 升级
#[test]
fn matrix_v0_2_0_to_v0_2_1_patch_upgrade() {
    let from = Version::new(0, 2, 0);
    let to = Version::new(0, 2, 1);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Patch);

    let rows = generate_mock_rows(1000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_patch_minor_upgrade(&from, &to, &make_valid_header(), &rows)
        .expect("v0.2.0 → v0.2.1 PATCH 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Patch);
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.2.0 → v0.2.1 PATCH 升级失败: {:?}", reason);
        }
    }
}

/// 测试 v1.0.0 → v1.0.1 PATCH 升级
#[test]
fn matrix_v1_0_0_to_v1_0_1_patch_upgrade() {
    let from = Version::new(1, 0, 0);
    let to = Version::new(1, 0, 1);
    let kind = classify_upgrade(&from, &to);
    assert_eq!(kind, UpgradeKind::Patch);

    let rows = generate_mock_rows(1000);
    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_patch_minor_upgrade(&from, &to, &make_valid_header(), &rows)
        .expect("v1.0.0 → v1.0.1 PATCH 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Patch);
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v1.0.0 → v1.0.1 PATCH 升级失败: {:?}", reason);
        }
    }
}

// =====================================================================
//  大数据量测试
// =====================================================================

/// 测试 v0.1.0 → v1.0.0 大数据量 MAJOR 升级（100000 行）
#[test]
fn matrix_v0_1_0_to_v1_0_0_large_dataset_major_upgrade() {
    let from = Version::new(0, 1, 0);
    let to = Version::new(1, 0, 0);
    let rows = generate_mock_rows(100000);

    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .execute_major_upgrade(from, to, &rows)
        .expect("v0.1.0 → v1.0.0 大数据量 MAJOR 升级应成功");

    match outcome {
        UpgradeOutcome::Success {
            result,
            rows: result_rows,
            ..
        } => {
            assert!(result.success);
            assert_eq!(result.kind, UpgradeKind::Major);
            assert_eq!(result_rows.len(), rows.len());
            assert!(verify_rows_equal(&rows, &result_rows));
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("v0.1.0 → v1.0.0 大数据量 MAJOR 升级失败: {:?}", reason);
        }
    }
}

// =====================================================================
//  升级失败回滚测试
// =====================================================================

/// 测试升级失败回滚（使用 simulate_upgrade_failure 注入失败）
#[test]
fn matrix_upgrade_failure_rollback_zero_data_loss() {
    let current_version = Version::new(0, 1, 0);
    let rows = generate_mock_rows(10000);

    let mut ctx = UpgradeContext::new();
    let outcome = ctx
        .simulate_upgrade_failure(&current_version, &rows)
        .expect("simulate_upgrade_failure 应返回 FailedAndRolledBack");

    match outcome {
        UpgradeOutcome::FailedAndRolledBack {
            reason,
            rollback,
            rows: rolled_back_rows,
            ..
        } => {
            // 回滚结果应有内容
            assert!(
                !rollback.message.is_empty(),
                "回滚消息不应为空。原因: {:?}",
                reason
            );
            // 回滚后数据应与原始一致
            assert!(
                verify_rows_equal(&rows, &rolled_back_rows),
                "回滚后数据应与原始一致，但验证失败。原因: {:?}",
                reason
            );
        }
        UpgradeOutcome::Success { .. } => {
            panic!("注入失败后应返回 FailedAndRolledBack，但返回了 Success");
        }
    }
}

// =====================================================================
//  备份与直接执行测试
// =====================================================================

/// 测试升级过程中备份被正确创建
#[test]
fn matrix_upgrade_creates_backup() {
    let from = Version::new(0, 1, 0);
    let to = Version::new(0, 2, 0);
    let rows = generate_mock_rows(5000);

    let mut ctx = UpgradeContext::new();
    let initial_backup_count = ctx.backup_manager.backup_count();
    let outcome = ctx
        .execute_patch_minor_upgrade(&from, &to, &make_valid_header(), &rows)
        .expect("升级应成功");

    match outcome {
        UpgradeOutcome::Success { backup, .. } => {
            assert_eq!(backup.row_count, rows.len());
            assert_eq!(backup.kind, UpgradeKind::Minor);
            assert_eq!(ctx.backup_manager.backup_count(), initial_backup_count + 1);
        }
        UpgradeOutcome::FailedAndRolledBack { reason, .. } => {
            panic!("升级失败: {:?}", reason);
        }
    }
}

/// 测试 MAJOR 升级使用 MajorUpgradeExecutor 直接执行
#[test]
fn matrix_major_upgrade_executor_direct() {
    let from = Version::new(0, 1, 0);
    let to = Version::new(1, 0, 0);
    let rows = generate_mock_rows(5000);

    let executor = MajorUpgradeExecutor::new(from, to);
    let (result, migrated_rows) = executor.execute(&rows).expect("MAJOR 升级应成功");

    assert!(result.success);
    assert_eq!(result.kind, UpgradeKind::Major);
    assert_eq!(migrated_rows.len(), rows.len());
    assert!(verify_rows_equal(&rows, &migrated_rows));
}

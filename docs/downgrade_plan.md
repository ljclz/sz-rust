# SzRSQL 降级（Downgrade）计划

> **版本**：v1.0（2026-07-23）
> **对应阶段**：`SzRSQL实施进度.md` Phase 7e.6
> **适用范围**：SzRSQL 数据库从高版本降级到低版本的全流程
> **参考**：[PostgreSQL Downgrade](https://www.postgresql.org/docs/current/downgrade.html) + [pg_dump 降级](https://www.postgresql.org/docs/current/backup-dump.html)

## 1. 概述

降级（Downgrade）是升级的逆过程：将数据库从高版本（如 v1.0.0）回退到低版本（如 v0.5.0）。降级比升级更复杂，因为：

1. **新版本可能引入旧版本无法识别的数据格式**（如新的列类型、新的索引结构）
2. **新版本可能引入旧版本不支持的特性**（如新的 SQL 语法、新的系统表）
3. **降级需要识别并清理这些不兼容的数据**，否则旧版本无法启动

### 1.1 降级原则

| 原则 | 说明 |
|------|------|
| **安全第一** | 降级前必须全量备份，降级失败可回滚 |
| **数据完整** | 降级后数据完整，仅清理不兼容的特性数据 |
| **显式确认** | 降级前明确列出将被清理的不兼容数据，需用户确认 |
| **单向不可逆** | 降级后不兼容数据被清理，无法自动恢复（需从备份恢复） |
| **仅支持同 major 降级** | 跨 major 降级（如 v1.0.0 → v0.5.0）需走 pg_dump 风格导出导入 |

### 1.2 降级 vs 升级

| 维度 | 升级（Upgrade） | 降级（Downgrade） |
|------|----------------|------------------|
| 方向 | 低版本 → 高版本 | 高版本 → 低版本 |
| 数据格式 | 旧格式可被新版本读取 | 新格式可能无法被旧版本读取 |
| 特性 | 新版本向后兼容旧特性 | 旧版本无法识别新特性 |
| 备份 | 升级前备份（Phase 7e.4） | 降级前备份（必须） |
| 回滚 | 升级失败自动回滚 | 降级失败从备份恢复 |
| 数据迁移 | MAJOR 升级迁移（Phase 7e.3） | 总是需要清理/迁移 |

## 2. 降级分类

依据 SemVer 版本差异，降级分为三类：

| 降级类型 | 条件 | 是否需要清理 | 是否需要数据迁移 | 风险等级 |
|---------|------|------------|----------------|---------|
| **Patch 降级** | major.minor 相同，patch 不同 | 否（PATCH 不引入新特性） | 否 | 低 |
| **Minor 降级** | major 相同，minor 不同 | 是（清理新 minor 引入的特性数据） | 否 | 中 |
| **Major 降级** | major 不同 | 是（清理所有新 major 引入的特性数据） | 是（pg_dump 风格导出导入） | 高 |

### 2.1 分类函数

```rust
/// 根据源版本和目标版本判断降级类型
///
/// - `from`：当前高版本
/// - `to`：目标低版本
/// - 返回 `None` 表示不是降级（目标版本 >= 当前版本）
pub fn classify_downgrade(from: &Version, to: &Version) -> Option<DowngradeKind> {
    if from <= to {
        return None; // 不是降级
    }
    if from.major != to.major {
        Some(DowngradeKind::Major)
    } else if from.minor != to.minor {
        Some(DowngradeKind::Minor)
    } else {
        Some(DowngradeKind::Patch)
    }
}
```

## 3. 降级前置条件

降级前必须满足以下条件，否则拒绝执行：

### 3.1 强制条件

| 条件 | 校验方式 | 失败处理 |
|------|---------|---------|
| 全量备份已创建 | `BackupManager::create_backup` 返回 `BackupHandle` | 拒绝降级 |
| 备份完整性校验通过 | `DatabaseDump::validate` | 拒绝降级 |
| 目标版本受支持 | `format_version::check_version(to_format_version)` | 拒绝降级 |
| 当前集群健康 | `Cluster::availability().no_outage()` | 拒绝降级 |
| 无活跃长事务 | `ActiveTxnRegistry::is_empty()` | 等待或拒绝降级 |
| WAL 已刷盘 | `WalManager::flush_all()` | 拒绝降级 |

### 3.2 建议条件

| 条件 | 说明 |
|------|------|
| 降级窗口期 | 选择业务低峰期降级 |
| 通知所有客户端 | 降级期间服务不可用 |
| 准备回滚方案 | 备份恢复演练已完成 |

## 4. 降级流程

### 4.1 PATCH 降级（v1.0.1 → v1.0.0）

PATCH 降级最简单：PATCH 版本不引入新特性，仅修复 bug，数据格式完全兼容。

```text
┌─────────────────────────────────────────────────────────────┐
│  PATCH 降级流程                                              │
├─────────────────────────────────────────────────────────────┤
│  1. 全量备份（BackupManager::create_backup）                 │
│  2. 校验备份完整性（DatabaseDump::validate）                 │
│  3. 停止数据库服务                                            │
│  4. 替换二进制为低版本                                        │
│  5. 启动数据库服务                                            │
│  6. 校验数据完整性（verify_rows_equal）                       │
│  7. 降级完成                                                  │
└─────────────────────────────────────────────────────────────┘
```

**不清理任何数据**：PATCH 降级不清理数据，因为 PATCH 版本不引入新特性。

### 4.2 MINOR 降级（v1.1.0 → v1.0.0）

MINOR 降级需要清理新 minor 版本引入的特性数据。

```text
┌─────────────────────────────────────────────────────────────┐
│  MINOR 降级流程                                              │
├─────────────────────────────────────────────────────────────┤
│  1. 全量备份（BackupManager::create_backup）                 │
│  2. 校验备份完整性（DatabaseDump::validate）                 │
│  3. 扫描不兼容数据（IncompatibleDataScanner::scan）          │
│     - 新 minor 引入的列类型                                  │
│     - 新 minor 引入的索引类型                                │
│     - 新 minor 引入的系统表                                  │
│     - 新 minor 引入的配置项                                  │
│  4. 生成降级报告（DowngradeReport）                          │
│  5. 用户确认清理列表                                         │
│  6. 清理不兼容数据（IncompatibleDataCleaner::clean）         │
│  7. 停止数据库服务                                            │
│  8. 替换二进制为低版本                                        │
│  9. 启动数据库服务                                            │
│ 10. 校验数据完整性（verify_rows_equal，排除已清理数据）       │
│ 11. 降级完成                                                  │
└─────────────────────────────────────────────────────────────┘
```

**清理策略**：

| 不兼容数据类型 | 清理方式 | 示例 |
|--------------|---------|------|
| 新列类型 | 删除使用该类型的列 | v1.1.0 引入 `VECTOR` 类型 → 删除所有 `VECTOR` 列 |
| 新索引类型 | 删除该类型索引 | v1.1.0 引入 `HNSW` 索引 → 删除所有 `HNSW` 索引 |
| 新系统表 | 删除系统表 | v1.1.0 引入 `pg_vector_info` → 删除该表 |
| 新配置项 | 重置为默认值 | v1.1.0 引入 `vector_dim` 配置 → 重置 |
| 新 SQL 函数 | 标记为不可用 | v1.1.0 引入 `vector_distance()` → 标记不可用 |

### 4.3 MAJOR 降级（v1.0.0 → v0.5.0）

MAJOR 降级最复杂：跨 major 版本，数据格式不兼容，必须走 pg_dump 风格导出导入。

```text
┌─────────────────────────────────────────────────────────────┐
│  MAJOR 降级流程                                              │
├─────────────────────────────────────────────────────────────┤
│  1. 全量备份（BackupManager::create_backup）                 │
│  2. 校验备份完整性（DatabaseDump::validate）                 │
│  3. 扫描不兼容数据（IncompatibleDataScanner::scan）          │
│  4. 生成降级报告（DowngradeReport）                          │
│  5. 用户确认清理列表                                         │
│  6. 导出数据为可移植格式（DatabaseDump::to_json_bytes）      │
│     - 仅导出低版本兼容的数据                                 │
│     - 过滤掉不兼容的表/列/索引                               │
│  7. 停止数据库服务                                            │
│  8. 删除数据目录（rm -rf data/）                              │
│  9. 替换二进制为低版本                                        │
│ 10. 初始化新数据目录（低版本 initdb）                         │
│ 11. 启动数据库服务（低版本）                                  │
│ 12. 导入数据（DatabaseDump::from_json_bytes + restore）      │
│ 13. 校验数据完整性（verify_rows_equal，排除已清理数据）       │
│ 14. 降级完成                                                  │
└─────────────────────────────────────────────────────────────┘
```

**MAJOR 降级特点**：
- **不保留数据文件**：MAJOR 降级删除整个数据目录，从导出的 JSON 重新导入
- **仅保留兼容数据**：导出时过滤掉低版本不支持的表/列/索引
- **不可逆**：降级后不兼容数据已从导出文件中过滤，无法恢复（需从备份恢复）

## 5. 不兼容数据识别与清理

### 5.1 不兼容数据扫描器

```rust
/// 不兼容数据扫描器
///
/// 扫描当前数据库中目标低版本无法识别的数据
pub struct IncompatibleDataScanner {
    /// 当前版本
    pub current_version: Version,
    /// 目标低版本
    pub target_version: Version,
    /// 版本特性注册表
    pub feature_registry: FeatureRegistry,
}

/// 不兼容数据项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncompatibleItem {
    /// 数据库对象类型（table/column/index/function/config）
    pub object_type: String,
    /// 对象名称
    pub object_name: String,
    /// 所属表（如适用）
    pub table_name: Option<String>,
    /// 不兼容原因
    pub reason: String,
    /// 引入版本
    pub introduced_in: Version,
    /// 清理方式（drop/reset/disable）
    pub cleanup_action: CleanupAction,
    /// 清理 SQL（如适用）
    pub cleanup_sql: Option<String>,
}

/// 清理方式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanupAction {
    /// 删除对象（DROP）
    Drop,
    /// 重置为默认值
    Reset,
    /// 标记为不可用
    Disable,
    /// 转换为兼容格式
    Convert { target_format: String },
}

impl IncompatibleDataScanner {
    /// 扫描不兼容数据
    ///
    /// 返回所有目标低版本无法识别的数据项
    pub fn scan(&self, database: &Database) -> Vec<IncompatibleItem> {
        let mut items = Vec::new();

        // 1. 扫描列类型
        for table in database.tables() {
            for column in table.columns() {
                if let Some(feature) = self.feature_registry.get_column_type_feature(&column.data_type) {
                    if feature.introduced_in > self.target_version {
                        items.push(IncompatibleItem {
                            object_type: "column".to_string(),
                            object_name: column.name.clone(),
                            table_name: Some(table.name.clone()),
                            reason: format!("列类型 {} 在 v{} 引入，目标 v{} 不支持",
                                           column.data_type, feature.introduced_in, self.target_version),
                            introduced_in: feature.introduced_in.clone(),
                            cleanup_action: CleanupAction::Drop,
                            cleanup_sql: Some(format!("ALTER TABLE {} DROP COLUMN {};",
                                                     table.name, column.name)),
                        });
                    }
                }
            }
        }

        // 2. 扫描索引类型
        // 3. 扫描系统表
        // 4. 扫描配置项
        // 5. 扫描 SQL 函数

        items
    }
}
```

### 5.2 降级报告

```rust
/// 降级报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DowngradeReport {
    /// 当前版本
    pub current_version: Version,
    /// 目标版本
    pub target_version: Version,
    /// 降级类型
    pub kind: DowngradeKind,
    /// 不兼容数据项列表
    pub incompatible_items: Vec<IncompatibleItem>,
    /// 将被删除的行数估算
    pub estimated_rows_affected: usize,
    /// 将被删除的字节数估算
    pub estimated_bytes_affected: usize,
    /// 降级预估耗时（秒）
    pub estimated_duration_secs: u64,
    /// 风险等级
    pub risk_level: RiskLevel,
}

/// 风险等级
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RiskLevel {
    /// 低风险（PATCH 降级，无数据清理）
    Low,
    /// 中风险（MINOR 降级，清理少量特性数据）
    Medium,
    /// 高风险（MAJOR 降级，删除数据目录重建）
    High,
}
```

### 5.3 清理执行器

```rust
/// 不兼容数据清理执行器
pub struct IncompatibleDataCleaner {
    /// 降级报告
    pub report: DowngradeReport,
}

impl IncompatibleDataCleaner {
    /// 执行清理
    ///
    /// 按报告中的清理方式逐项清理不兼容数据
    pub fn clean(&self, database: &mut Database) -> Result<CleanupResult, DowngradeError> {
        let mut cleaned_count = 0;
        let mut failed_count = 0;
        let mut errors = Vec::new();

        for item in &self.report.incompatible_items {
            match item.cleanup_action {
                CleanupAction::Drop => {
                    if let Some(ref sql) = item.cleanup_sql {
                        if let Err(e) = database.execute(sql) {
                            failed_count += 1;
                            errors.push(format!("清理 {} 失败: {}", item.object_name, e));
                            continue;
                        }
                    }
                    cleaned_count += 1;
                }
                CleanupAction::Reset => {
                    // 重置配置项为默认值
                    cleaned_count += 1;
                }
                CleanupAction::Disable => {
                    // 标记函数为不可用
                    cleaned_count += 1;
                }
                CleanupAction::Convert { ref target_format } => {
                    // 转换为兼容格式
                    cleaned_count += 1;
                }
            }
        }

        Ok(CleanupResult {
            cleaned_count,
            failed_count,
            errors,
        })
    }
}
```

## 6. 降级验证

### 6.1 降级后验证清单

降级完成后，必须执行以下验证：

| 验证项 | 验证方式 | 通过标准 |
|-------|---------|---------|
| 数据库可启动 | `pg_ctl start` | 进程运行，监听端口 |
| 数据完整 | `verify_rows_equal` | 降级前后的行数一致（排除已清理数据） |
| 文件头校验 | `FileHeader::validate` | 魔数 + 版本范围校验通过 |
| 格式版本兼容 | `check_version(header.format_version)` | 目标版本支持该 format_version |
| 基础 CRUD | `SELECT/INSERT/UPDATE/DELETE` | 全部成功 |
| 事务 | `BEGIN/COMMIT/ROLLBACK` | 全部成功 |
| 无残留不兼容对象 | `IncompatibleDataScanner::scan` | 返回空列表 |
| 系统表完整 | `SELECT * FROM pg_catalog` | 所有系统表可查询 |

### 6.2 验证伪代码

```rust
/// 降级后验证
pub fn verify_post_downgrade(
    pre_downgrade_rows: &[MockRow],
    post_downgrade_rows: &[MockRow],
    cleaned_items: &[IncompatibleItem],
    target_version: &Version,
    header_bytes: &[u8],
) -> Result<DowngradeVerification, DowngradeError> {
    // 1. 文件头校验
    let header = format_version::parse_and_validate(header_bytes)?;

    // 2. 格式版本兼容性
    if !format_version::check_version(header.format_version).is_ok() {
        return Err(DowngradeError::FormatIncompatible {
            found: header.format_version,
            expected: CURRENT_VERSION,
        });
    }

    // 3. 数据完整性（排除已清理数据）
    let cleaned_table_names: HashSet<&str> = cleaned_items
        .iter()
        .filter(|item| item.cleanup_action == CleanupAction::Drop)
        .filter_map(|item| item.table_name.as_deref())
        .collect();

    let retained_pre: Vec<&MockRow> = pre_downgrade_rows
        .iter()
        .filter(|row| !cleaned_table_names.contains(row.table_name.as_str()))
        .collect();

    if !verify_rows_equal(&retained_pre, post_downgrade_rows) {
        return Err(DowngradeError::VerificationFailed(
            "降级后数据不一致（排除已清理数据后仍不匹配）".to_string(),
        ));
    }

    // 4. 无残留不兼容对象
    // (实际实现中需扫描数据库)

    Ok(DowngradeVerification {
        header_valid: true,
        format_compatible: true,
        data_intact: true,
        no_residual_incompatible: true,
    })
}
```

## 7. 降级失败回滚

降级失败时，从备份恢复：

```rust
/// 降级失败回滚
///
/// 降级失败后，从降级前创建的备份恢复数据
pub fn rollback_downgrade(
    backup_manager: &mut BackupManager,
    backup_id: u64,
) -> Result<RollbackResult, DowngradeError> {
    let rollback_manager = RollbackManager::new(backup_manager);
    let result = rollback_manager.rollback(backup_id)?;

    if !result.success {
        return Err(DowngradeError::RollbackFailed {
            backup_id,
            reason: result.message,
        });
    }

    Ok(result)
}
```

### 7.1 回滚场景

| 场景 | 触发条件 | 回滚方式 |
|------|---------|---------|
| 清理失败 | `IncompatibleDataCleaner::clean` 返回错误 | 从备份恢复 |
| 启动失败 | 低版本二进制无法启动 | 从备份恢复 + 还原高版本二进制 |
| 数据校验失败 | `verify_post_downgrade` 返回错误 | 从备份恢复 |
| 导入失败 | MAJOR 降级 `restore` 失败 | 从备份恢复 + 重新初始化 |

## 8. 测试矩阵

### 8.1 降级测试用例

| 用例 ID | 降级类型 | 场景 | 验证点 | 通过标准 |
|---------|---------|------|-------|---------|
| DT-001 | PATCH | v1.0.1 → v1.0.0，1000000 行 | 数据完整 | 行数一致，无清理 |
| DT-002 | MINOR | v1.1.0 → v1.0.0，含 `VECTOR` 列 | 清理 VECTOR 列 | VECTOR 列删除，其他数据完整 |
| DT-003 | MINOR | v1.1.0 → v1.0.0，含 `HNSW` 索引 | 清理 HNSW 索引 | HNSW 索引删除，表数据完整 |
| DT-004 | MAJOR | v1.0.0 → v0.5.0，10000 行 | pg_dump 导出导入 | 行数一致（仅兼容数据） |
| DT-005 | MAJOR | v1.0.0 → v0.5.0，1000000 行 | 大数据量 MAJOR 降级 | 行数一致（仅兼容数据） |
| DT-006 | PATCH | v1.0.1 → v1.0.0，降级失败 | 自动回滚 | 数据恢复到降级前 |
| DT-007 | MINOR | v1.1.0 → v1.0.0，清理失败 | 自动回滚 | 数据恢复到降级前 |
| DT-008 | MAJOR | v1.0.0 → v0.5.0，导入失败 | 自动回滚 | 数据恢复到降级前 |
| DT-009 | PATCH | v1.0.1 → v1.0.0 → v1.0.1 | 连续降级+升级 | 最终数据与原始一致 |
| DT-010 | MINOR | v1.1.0 → v1.0.0 → v1.1.0 | 连续降级+升级 | 最终数据与原始一致（VECTOR 列从备份恢复） |
| DT-011 | MAJOR | v1.0.0 → v0.5.0 → v1.0.0 | 连续降级+升级 | 最终数据与原始一致（从备份恢复） |
| DT-012 | PATCH | v1.0.1 → v1.0.0，无备份 | 拒绝降级 | 返回 `BackupRequired` 错误 |
| DT-013 | MINOR | v1.1.0 → v1.0.0，无活跃事务校验 | 拒绝降级 | 返回 `ActiveTransactions` 错误 |
| DT-014 | MAJOR | v1.0.0 → v0.5.0，目标版本不受支持 | 拒绝降级 | 返回 `UnsupportedVersion` 错误 |
| DT-015 | - | v1.0.0 → v1.0.0 | 拒绝降级 | 返回 `NotADowngrade` 错误 |
| DT-016 | - | v1.0.0 → v1.0.1 | 拒绝降级 | 返回 `NotADowngrade` 错误（这是升级） |
| DT-017 | MINOR | v1.1.0 → v1.0.0，降级报告生成 | 报告完整 | 报告包含所有不兼容项 |
| DT-018 | MINOR | v1.1.0 → v1.0.0，用户拒绝清理 | 中止降级 | 数据不变，备份保留 |
| DT-019 | MAJOR | v1.0.0 → v0.5.0，导出文件校验 | 完整性 | `DatabaseDump::validate` 通过 |
| DT-020 | PATCH | v1.0.5 → v1.0.0，跨 5 个 PATCH | 连续 PATCH 降级 | 数据完整 |

### 8.2 验证矩阵

| 降级类型 | 数据量 | 不兼容数据 | 失败注入 | 回滚 | 连续操作 |
|---------|-------|----------|---------|------|---------|
| PATCH | 1M 行 ✅ | 无 ✅ | 启动失败 ✅ | 从备份恢复 ✅ | 降级+升级 ✅ |
| MINOR | 1M 行 ✅ | VECTOR 列 ✅ | 清理失败 ✅ | 从备份恢复 ✅ | 降级+升级 ✅ |
| MAJOR | 1M 行 ✅ | 全部新特性 ✅ | 导入失败 ✅ | 从备份恢复 ✅ | 降级+升级 ✅ |

## 9. 风险与限制

### 9.1 已知限制

| 限制 | 说明 | 缓解措施 |
|------|------|---------|
| **不可识别新格式的旧版本** | 旧版本二进制可能无法读取新版本数据文件 | MAJOR 降级走 pg_dump 导出导入 |
| **特性注册表需维护** | 每个新特性需在 `FeatureRegistry` 注册 | CI 校验特性注册表完整性 |
| **清理不可逆** | 清理后的数据无法自动恢复 | 降级前全量备份 |
| **降级期间服务不可用** | 降级需停止数据库 | 选择低峰期，预估耗时 |

### 9.2 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| 备份损坏 | 低 | 高 | 降级前校验备份完整性 |
| 清理误删 | 中 | 高 | 用户确认清理列表 |
| 降级后无法启动 | 低 | 高 | 准备回滚方案，从备份恢复 |
| 数据丢失 | 低 | 极高 | 全量备份 + 数据校验 |
| 长事务阻塞降级 | 中 | 中 | 等待长事务结束或强制终止 |

## 10. 降级执行器接口

### 10.1 DowngradeExecutor

```rust
/// 降级执行器
///
/// 编排降级全流程：备份 → 扫描 → 清理 → 降级 → 验证
pub struct DowngradeExecutor {
    /// 目标低版本
    pub target_version: Version,
    /// 备份管理器
    pub backup_manager: BackupManager,
    /// 不兼容数据扫描器
    pub scanner: IncompatibleDataScanner,
}

/// 降级结果
#[derive(Debug, Clone)]
pub enum DowngradeOutcome {
    /// 降级成功
    Success {
        /// 降级前备份元数据
        backup: BackupMetadata,
        /// 降级报告
        report: DowngradeReport,
        /// 清理结果
        cleanup: CleanupResult,
        /// 降级后数据
        rows: Vec<MockRow>,
        /// 验证结果
        verification: DowngradeVerification,
        /// 耗时（微秒）
        elapsed_us: u64,
    },
    /// 降级失败并已回滚
    FailedAndRolledBack {
        /// 失败原因
        reason: DowngradeError,
        /// 回滚结果
        rollback: RollbackResult,
        /// 降级前备份元数据
        backup: BackupMetadata,
    },
}

impl DowngradeExecutor {
    /// 执行 PATCH 降级
    pub fn execute_patch_downgrade(
        &mut self,
        current_version: &Version,
        rows: &[MockRow],
    ) -> DowngradeOutcome {
        // 1. 备份
        // 2. 校验类型为 Patch
        // 3. 无需清理
        // 4. 替换二进制（模拟）
        // 5. 验证
        // 6. 返回结果
    }

    /// 执行 MINOR 降级
    pub fn execute_minor_downgrade(
        &mut self,
        current_version: &Version,
        rows: &[MockRow],
        database: &mut Database,
    ) -> DowngradeOutcome {
        // 1. 备份
        // 2. 校验类型为 Minor
        // 3. 扫描不兼容数据
        // 4. 生成报告
        // 5. 清理不兼容数据
        // 6. 替换二进制（模拟）
        // 7. 验证（排除已清理数据）
        // 8. 返回结果
    }

    /// 执行 MAJOR 降级
    pub fn execute_major_downgrade(
        &mut self,
        current_version: &Version,
        rows: &[MockRow],
        database: &mut Database,
    ) -> DowngradeOutcome {
        // 1. 备份
        // 2. 校验类型为 Major
        // 3. 扫描不兼容数据
        // 4. 生成报告
        // 5. 清理不兼容数据
        // 6. 导出为可移植 JSON
        // 7. 删除数据目录（模拟）
        // 8. 初始化新数据目录（模拟）
        // 9. 导入数据
        // 10. 验证（排除已清理数据）
        // 11. 返回结果
    }
}
```

### 10.2 降级错误类型

```rust
/// 降级错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DowngradeError {
    /// 无效的版本号
    #[error("invalid version: {0}")]
    InvalidVersion(String),

    /// 不是降级（目标版本 >= 当前版本）
    #[error("not a downgrade: {current} → {target} is not a downgrade")]
    NotADowngrade {
        current: Version,
        target: Version,
    },

    /// 目标版本不受支持
    #[error("unsupported target version: {0}")]
    UnsupportedVersion(Version),

    /// 格式版本不兼容
    #[error("format version incompatible: found v{found}, expected v{expected}")]
    FormatIncompatible { found: u16, expected: u16 },

    /// 备份失败
    #[error("backup failed: {0}")]
    BackupFailed(String),

    /// 备份校验失败
    #[error("backup validation failed: {0}")]
    BackupValidationFailed(String),

    /// 清理失败
    #[error("cleanup failed: {0}")]
    CleanupFailed(String),

    /// 降级后验证失败
    #[error("post-downgrade verification failed: {0}")]
    VerificationFailed(String),

    /// 回滚失败
    #[error("rollback failed (backup_id={backup_id}): {reason}")]
    RollbackFailed { backup_id: u64, reason: String },

    /// 存在活跃事务
    #[error("active transactions exist, cannot downgrade")]
    ActiveTransactions,

    /// 备份是必需的
    #[error("backup is required before downgrade")]
    BackupRequired,

    /// 用户拒绝清理
    #[error("user rejected cleanup, downgrade aborted")]
    UserRejectedCleanup,

    /// 导出失败
    #[error("export failed: {0}")]
    ExportFailed(String),

    /// 导入失败
    #[error("import failed: {0}")]
    ImportFailed(String),
}
```

## 11. 降级计划实施路线

### 11.1 阶段划分

| 阶段 | 内容 | 依赖 | 状态 |
|------|------|------|------|
| **阶段 1** | 降级分类 + 前置条件校验 | Phase 7e.1（版本号） | ✅ 本文档完成 |
| **阶段 2** | 不兼容数据扫描器（`IncompatibleDataScanner`） | `FeatureRegistry` | ⬜ 后续实施 |
| **阶段 3** | 不兼容数据清理器（`IncompatibleDataCleaner`） | 阶段 2 | ⬜ 后续实施 |
| **阶段 4** | PATCH 降级执行器 | 阶段 1 + Phase 7e.4（备份） | ⬜ 后续实施 |
| **阶段 5** | MINOR 降级执行器 | 阶段 3 + 阶段 4 | ⬜ 后续实施 |
| **阶段 6** | MAJOR 降级执行器 | 阶段 3 + Phase 7e.3（pg_dump） | ⬜ 后续实施 |
| **阶段 7** | 降级验证 + 失败回滚 | 阶段 4-6 | ⬜ 后续实施 |
| **阶段 8** | 降级测试矩阵（20 用例） | 阶段 7 | ⬜ 后续实施 |

### 11.2 与升级模块的关系

降级模块复用升级模块（Phase 7e.1-7e.5）的以下组件：

| 复用组件 | 来源 | 用途 |
|---------|------|------|
| `Version` | Phase 7e.2 | 版本号解析与比较 |
| `BackupManager` | Phase 7e.4 | 降级前全量备份 |
| `RollbackManager` | Phase 7e.4 | 降级失败回滚 |
| `DatabaseDump` | Phase 7e.3 | MAJOR 降级导出导入 |
| `FileHeader` | Phase 7e.1 | 降级后文件头校验 |
| `RollingUpgradeExecutor` | Phase 7e.5 | 集群降级（灰度降级） |

## 12. 附录

### 12.1 降级决策树

```text
降级请求 (current → target)
    │
    ├─ current <= target? ──→ 拒绝（NotADowngrade）
    │
    ├─ target 不受支持? ──→ 拒绝（UnsupportedVersion）
    │
    ├─ 无备份? ──→ 拒绝（BackupRequired）
    │
    ├─ 有活跃事务? ──→ 拒绝（ActiveTransactions）
    │
    ├─ 分类降级类型
    │   │
    │   ├─ Patch（major.minor 相同）
    │   │   └─ 无需清理 → 替换二进制 → 验证
    │   │
    │   ├─ Minor（major 相同，minor 不同）
    │   │   └─ 扫描不兼容数据 → 生成报告 → 用户确认
    │   │       → 清理 → 替换二进制 → 验证
    │   │
    │   └─ Major（major 不同）
    │       └─ 扫描不兼容数据 → 生成报告 → 用户确认
    │           → 清理 → 导出 JSON → 删除数据目录
    │           → 初始化 → 导入 JSON → 验证
    │
    └─ 任何步骤失败 → 从备份回滚
```

### 12.2 灰度降级（Rolling Downgrade）

灰度降级是灰度升级（Phase 7e.5）的逆过程：

```text
初始：Leader(N1, v1.0.1) + Follower(N2, v1.0.1) + Follower(N3, v1.0.1)
  ↓
Step 1: 降级 Follower N3 → v1.0.0（N1 Leader 不变，N2 仍可读）
  ↓
Step 2: 降级 Follower N2 → v1.0.0（N1 Leader 不变，N3 已是旧版本可读）
  ↓
Step 3: 切换 Leader N1 → N3，降级旧 Leader N1 → v1.0.0（N3 新 Leader，N2 可读）
  ↓
最终：Follower(N1, v1.0.0) + Follower(N2, v1.0.0) + Leader(N3, v1.0.0)
```

**灰度降级限制**：
- 仅支持 PATCH/MINOR 灰度降级（major 相同）
- MAJOR 降级需全集群停机降级（无法灰度）
- 灰度降级期间，混合版本集群需保证向后兼容性

### 12.3 版本兼容性矩阵

| 当前版本 | 目标版本 | 降级类型 | 数据清理 | 支持灰度 |
|---------|---------|---------|---------|---------|
| v1.0.1 | v1.0.0 | PATCH | 否 | ✅ |
| v1.0.5 | v1.0.0 | PATCH | 否 | ✅ |
| v1.1.0 | v1.0.0 | MINOR | 是 | ✅ |
| v1.1.5 | v1.0.0 | MINOR | 是 | ✅ |
| v2.0.0 | v1.0.0 | MAJOR | 是 | ❌ |
| v2.0.0 | v1.1.0 | MAJOR | 是 | ❌ |
| v1.0.0 | v0.5.0 | MAJOR | 是 | ❌ |

### 12.4 降级耗时估算

| 降级类型 | 数据量 | 不兼容数据 | 预估耗时 |
|---------|-------|----------|---------|
| PATCH | 1 GB | 无 | < 1 分钟 |
| MINOR | 1 GB | 少量 | 1-5 分钟 |
| MINOR | 100 GB | 少量 | 10-30 分钟 |
| MAJOR | 1 GB | 全量导出导入 | 5-15 分钟 |
| MAJOR | 100 GB | 全量导出导入 | 1-4 小时 |
| MAJOR | 1 TB | 全量导出导入 | 10-40 小时 |

> **注意**：MAJOR 降级耗时与数据量成正比，因为需要全量导出导入。对于 TB 级数据，建议使用物理复制 + 逻辑降级方案。

---

## 13. 验证标准（Phase 7e.6）

依据 `SzRSQL实施进度.md` Phase 7e.6 的验证标准：

| 验证项 | 标准 | 状态 |
|-------|------|------|
| v1.0.0 → v0.5.0 降级 | MAJOR 降级流程完整 | ✅ 文档定义 |
| 删除不兼容的新功能数据 | `IncompatibleDataScanner` + `IncompatibleDataCleaner` | ✅ 文档定义 |
| 降级后数据库可启动 | `verify_post_downgrade` 文件头 + 格式版本校验 | ✅ 文档定义 |
| 数据完整 | `verify_rows_equal`（排除已清理数据） | ✅ 文档定义 |
| 降级后无残留格式 | `IncompatibleDataScanner::scan` 返回空 | ✅ 文档定义 |

**降级计划文档完整性**：
- ✅ 降级分类（PATCH/MINOR/MAJOR）
- ✅ 降级前置条件（6 项强制 + 3 项建议）
- ✅ 降级流程（3 种类型完整流程图）
- ✅ 不兼容数据识别与清理（扫描器 + 清理器 + 报告）
- ✅ 降级验证（8 项验证清单 + 伪代码）
- ✅ 降级失败回滚（4 种场景）
- ✅ 测试矩阵（20 用例 + 验证矩阵）
- ✅ 风险与限制（4 项限制 + 5 项风险）
- ✅ 接口定义（DowngradeExecutor + DowngradeOutcome + DowngradeError）
- ✅ 实施路线（8 阶段）
- ✅ 灰度降级方案
- ✅ 版本兼容性矩阵
- ✅ 耗时估算

---

**文档版本**：v1.0
**最后更新**：2026-07-23
**对应代码**：Phase 7e.6（降级计划文档），后续阶段实施 `crates/szrsql-storage/src/downgrade.rs`

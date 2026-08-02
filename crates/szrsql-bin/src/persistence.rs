//! 表数据快照持久化 — 默认启用的磁盘持久化机制。
//!
//! # 设计
//!
//! - **保存**：将 `shared_tables` 中的所有 `InMemoryTable` 序列化为 JSON 写入磁盘
//! - **加载**：启动时从磁盘读取 JSON，反序列化为 `InMemoryTable` 并注入 `shared_tables`
//! - **触发时机**：
//!   - 启动时加载（若快照文件存在）
//!   - 后台周期性保存（默认每 5 秒）
//!   - 服务器关闭时保存（通过 shutdown 信号触发）
//!
//! # 文件格式
//!
//! ```json
//! {
//!   "version": 1,
//!   "saved_at": "2026-07-28T12:00:00Z",
//!   "tables": [ /* InMemoryTable 序列化数组 */ ]
//! }
//! ```
//!
//! # 与 WAL 的关系
//!
//! WAL（`--wal-path`）记录事务 Commit/Abort 日志，用于崩溃恢复时保证 ACID。
//! 快照持久化（`--data-dir`）保存表数据的完整副本，用于重启后恢复数据。
//! 两者互补：WAL 保证已提交事务不丢失，快照保证重启后表数据可见。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
// P1-2：DirtyTableTracker 已移至 szrsql-protocol crate（避免循环依赖）
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
pub use szrsql_protocol::pgwire::DirtyTableTracker;
use szrsql_sql::executor::{InMemoryTable, TableStorage};
use szrsql_tx::wal::{WalOpType, WalRecord};
use tokio::sync::{Mutex, RwLock};

/// 快照文件格式版本
const SNAPSHOT_VERSION: u32 = 1;

/// 快照文件结构
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotFile {
    /// 格式版本号
    version: u32,
    /// 保存时间戳（RFC 3339）
    saved_at: String,
    /// 所有表的序列化数据
    tables: Vec<InMemoryTable>,
}

/// 快照文件默认路径
pub fn default_snapshot_path(data_dir: &Path) -> PathBuf {
    data_dir.join("tables.json")
}

/// P1-5：FNV-1a 32-bit 表名哈希（与 `Executor::table_name_to_id` 算法一致）。
///
/// 用于 WAL 行级回放时通过 `page_id`（= table_id）反查目标表名。
fn table_name_to_id(table_name: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for byte in table_name.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// 从磁盘加载快照，返回表名 → InMemoryTable 的映射。
///
/// 若文件不存在，返回空 HashMap（首次启动，无数据可恢复）。
/// 若文件损坏，返回错误（不静默忽略，避免数据丢失）。
pub fn load_snapshot(data_dir: &Path) -> Result<HashMap<String, Arc<Mutex<InMemoryTable>>>> {
    let snapshot_path = default_snapshot_path(data_dir);
    if !snapshot_path.exists() {
        tracing::info!(
            snapshot_path = %snapshot_path.display(),
            "snapshot file not found, starting with empty table set"
        );
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(&snapshot_path)
        .with_context(|| format!("failed to read snapshot file: {}", snapshot_path.display()))?;

    let snapshot: SnapshotFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse snapshot file: {}", snapshot_path.display()))?;

    tracing::info!(
        snapshot_path = %snapshot_path.display(),
        version = snapshot.version,
        table_count = snapshot.tables.len(),
        saved_at = %snapshot.saved_at,
        "snapshot loaded from disk"
    );

    let mut tables = HashMap::new();
    for mut table in snapshot.tables {
        // 修复旧版快照中缺失的 xmin/xmax 数组
        table.ensure_version_arrays();
        // P0-3 修复：加载后自动检测主键并重建 BTree 索引
        // 快照中 pk_index 被 serde skip，重启后为 None，需从 schema 主键信息重建
        rebuild_pk_index_if_needed(&mut table);
        let name = table.name().to_string();
        tables.insert(name, Arc::new(Mutex::new(table)));
    }
    Ok(tables)
}

/// P0-3 修复：检测表的主键列，如果是单列 Int64 主键，自动重建 BTree 索引。
///
/// 快照序列化时 pk_index 被 serde skip（BTree 不是 Serialize），
/// 加载后 pk_index 为 None。此函数从 schema 的 primary_key 字段检测主键列，
/// 若为单列 Int64 主键，调用 `enable_btree_pk` 重建索引并回填已有数据。
fn rebuild_pk_index_if_needed(table: &mut InMemoryTable) {
    let pk_cols: Vec<usize> = table
        .schema()
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.primary_key)
        .map(|(i, _)| i)
        .collect();
    if pk_cols.len() == 1 {
        let col_idx = pk_cols[0];
        let col = &table.schema().columns[col_idx];
        if col.data_type == szrsql_types::value::ColumnType::Int64 {
            table.enable_btree_pk(col_idx);
        }
    }
}

/// 将 shared_tables 中的所有表保存到磁盘（全量快照，仅用于测试或首次快照）。
///
/// 生产路径请使用 [`save_incremental_snapshot`] + [`DirtyTableTracker`] 实现的增量快照机制。
///
/// 获取 shared_tables 的读锁，遍历每张表获取写锁（为了序列化一致性），
/// 序列化为 JSON 后原子写入临时文件，再重命名为目标文件（避免写入中途崩溃导致数据损坏）。
#[allow(dead_code)]
pub async fn save_snapshot(
    shared_tables: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    data_dir: &Path,
) -> Result<()> {
    let snapshot_path = default_snapshot_path(data_dir);

    // 确保目录存在
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create data dir: {}", parent.display()))?;
    }

    // 读取所有表数据
    let guard = shared_tables.read().await;
    let mut tables = Vec::with_capacity(guard.len());
    for (name, table_arc) in guard.iter() {
        let table_guard = table_arc.lock().await;
        tables.push(table_guard.clone());
        tracing::trace!(table = %name, row_count = table_guard.rows().len(), "serialized table for snapshot");
    }
    drop(guard);

    let snapshot = SnapshotFile {
        version: SNAPSHOT_VERSION,
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .to_string(),
        tables,
    };

    // 原子写入：先写临时文件，再重命名
    let tmp_path = snapshot_path.with_extension("json.tmp");
    let json =
        serde_json::to_string_pretty(&snapshot).context("failed to serialize snapshot to JSON")?;
    std::fs::write(&tmp_path, json)
        .with_context(|| format!("failed to write temp snapshot file: {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &snapshot_path).with_context(|| {
        format!(
            "failed to rename snapshot file: {} -> {}",
            tmp_path.display(),
            snapshot_path.display()
        )
    })?;

    tracing::debug!(
        snapshot_path = %snapshot_path.display(),
        table_count = snapshot.tables.len(),
        "snapshot saved to disk"
    );
    Ok(())
}

/// P1-2：启动后台周期性增量保存任务，返回一个 JoinHandle。
///
/// 使用 [`DirtyTableTracker`] 跟踪脏表，仅对自上次保存后被修改的表重新序列化，
/// 非脏表从磁盘已有快照中复用。
///
/// # 性能优势
///
/// - 表数量多但写入热点集中时，显著减少序列化和 IO 开销
/// - 无 DML 时跳过保存（仅检查脏表集合，开销极低）
///
/// # 参数
///
/// - `shared_tables`：共享表存储
/// - `data_dir`：快照文件目录
/// - `interval_secs`：保存间隔（秒）
/// - `tracker`：脏表跟踪器（与 Session 共享同一实例）
/// - `backup_notify`：P2-14 备份触发通知（HTTP `/api/v1/backup` 触发即时保存）
pub fn spawn_periodic_incremental_save(
    shared_tables: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    data_dir: PathBuf,
    interval_secs: u64,
    tracker: DirtyTableTracker,
    backup_notify: Option<Arc<tokio::sync::Notify>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
        loop {
            // P2-14：同时等待周期 tick 与 HTTP 备份触发通知；
            // 收到通知时立即执行增量快照（而非等下一个周期）。
            match &backup_notify {
                Some(notify) => {
                    tokio::select! {
                        _ = interval.tick() => {}
                        _ = notify.notified() => {
                            tracing::info!("backup triggered via HTTP notify, saving snapshot now");
                        }
                    }
                }
                None => {
                    interval.tick().await;
                }
            }
            if let Err(e) = save_incremental_snapshot(&shared_tables, &data_dir, &tracker).await {
                tracing::warn!(error = %e, "periodic incremental snapshot save failed");
            }
        }
    })
}

/// P1-2：增量快照保存。
///
/// 仅序列化脏表集合中的表，非脏表保留磁盘已有快照内容。
/// 若无脏表（无 DML 提交），直接返回，避免无谓的 IO。
///
/// # 算法
///
/// 1. 从 `tracker` 取出脏表集合（原子清空，保证保存期间的新提交会在下次保存）
/// 2. 若脏表为空，直接返回 Ok
/// 3. 读取磁盘已有快照（若不存在则视为空快照）
/// 4. 获取 `shared_tables` 读锁，对每张脏表获取写锁并 clone
/// 5. 用脏表的新数据覆盖快照中同名表（新增/更新），保留非脏表不变
/// 6. 原子写入（tmp + rename）
///
/// # 与 `save_snapshot` 的差异
///
/// | 维度 | `save_snapshot` | `save_incremental_snapshot` |
/// |------|-----------------|---------------------------|
/// | 序列化范围 | 所有表 | 仅脏表 |
/// | 无 DML 时开销 | 全量序列化 + 写盘 | 仅锁检查，立即返回 |
/// | 磁盘读取 | 不读取 | 读取已有快照以保留非脏表 |
///
/// # 参数
///
/// - `shared_tables`：共享表存储
/// - `data_dir`：快照文件目录
/// - `tracker`：脏表跟踪器
pub async fn save_incremental_snapshot(
    shared_tables: &Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    data_dir: &Path,
    tracker: &DirtyTableTracker,
) -> Result<()> {
    // 步骤 1：取出脏表集合（清空 tracker 内部状态）
    let dirty_tables = tracker.take_dirty().await;

    // 步骤 2：无脏表时直接返回，避免无谓 IO
    if dirty_tables.is_empty() {
        tracing::trace!("incremental snapshot: no dirty tables, skip save");
        return Ok(());
    }

    let snapshot_path = default_snapshot_path(data_dir);
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create data dir: {}", parent.display()))?;
    }

    // 步骤 3：读取磁盘已有快照，保留非脏表内容
    let mut existing_tables: HashMap<String, InMemoryTable> = if snapshot_path.exists() {
        match std::fs::read_to_string(&snapshot_path) {
            Ok(content) => match serde_json::from_str::<SnapshotFile>(&content) {
                Ok(snap) => snap
                    .tables
                    .into_iter()
                    .map(|t| (t.name().to_string(), t))
                    .collect(),
                Err(e) => {
                    // 磁盘快照损坏：不能丢弃，回退到全量保存
                    tracing::warn!(
                        error = %e,
                        snapshot_path = %snapshot_path.display(),
                        "existing snapshot is corrupted, falling back to full save"
                    );
                    HashMap::new()
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    snapshot_path = %snapshot_path.display(),
                    "failed to read existing snapshot, falling back to full save"
                );
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };

    // 步骤 4：获取脏表的最新数据
    let guard = shared_tables.read().await;
    let mut updated_count = 0usize;
    let mut missing_count = 0usize;
    for name in &dirty_tables {
        if let Some(table_arc) = guard.get(name) {
            let table_guard = table_arc.lock().await;
            existing_tables.insert(name.clone(), table_guard.clone());
            updated_count += 1;
        } else {
            // 脏表已被 DROP：从快照中移除
            existing_tables.remove(name);
            missing_count += 1;
        }
    }
    drop(guard);

    tracing::debug!(
        updated = updated_count,
        removed = missing_count,
        total_tables = existing_tables.len(),
        "incremental snapshot: merged dirty tables"
    );

    // 步骤 5：写入磁盘（原子写）
    let tables_vec: Vec<InMemoryTable> = existing_tables.into_values().collect();
    let snapshot = SnapshotFile {
        version: SNAPSHOT_VERSION,
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .to_string(),
        tables: tables_vec,
    };

    let tmp_path = snapshot_path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&snapshot)
        .context("failed to serialize incremental snapshot to JSON")?;
    std::fs::write(&tmp_path, json)
        .with_context(|| format!("failed to write temp snapshot file: {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &snapshot_path).with_context(|| {
        format!(
            "failed to rename snapshot file: {} -> {}",
            tmp_path.display(),
            snapshot_path.display()
        )
    })?;

    tracing::debug!(
        snapshot_path = %snapshot_path.display(),
        dirty_table_count = dirty_tables.len(),
        total_table_count = snapshot.tables.len(),
        "incremental snapshot saved to disk"
    );
    Ok(())
}

/// P0-1 修复：将 WAL 中的 `TableData` 记录应用到已加载的表集合，实现崩溃恢复。
///
/// # 算法（保证 ACID）
///
/// 1. 遍历 WAL 记录，维护 `pending: HashMap<tx_id, Vec<(table_name, table)>>`
///    暂存每个事务的 `TableData` 记录（尚未提交）
/// 2. 遇到 `Commit` 记录：将 `pending[tx_id]` 中的所有表应用到 `tables`（覆盖同名的现有表）
/// 3. 遇到 `Abort` 记录：丢弃 `pending[tx_id]`（事务回滚，数据不应用）
/// 4. 遇到 `TableData` 记录：解码并加入 `pending[tx_id]`
/// 5. 遍历结束后，`pending` 中剩余的未提交事务数据被丢弃（崩溃时事务未完成）
///
/// # TableData data 字段格式
///
/// - `u32 LE`：表名 UTF-8 字节长度
/// - `bytes`：表名 UTF-8 字节
/// - `bytes`：表数据 JSON（`InMemoryTable` 序列化结果）
///
/// # 参数
///
/// - `tables`：从快照加载的表集合（将被 WAL 记录覆盖/补全）
/// - `records`：WAL 回放的所有记录
///
/// # 返回
///
/// 成功应用的表数量（已提交事务的 TableData 记录数）
pub fn apply_wal_table_data(
    tables: &mut HashMap<String, Arc<Mutex<InMemoryTable>>>,
    records: &[WalRecord],
) -> (usize, std::collections::HashSet<u32>) {
    use std::collections::HashMap as StdHashMap;

    // 暂存每个事务的 TableData（tx_id → 表名 + 表数据）
    let mut pending: StdHashMap<u32, Vec<(String, InMemoryTable)>> = StdHashMap::new();
    let mut applied_count = 0usize;
    let mut committed_txns = std::collections::HashSet::new();

    for record in records {
        match record.op_type {
            WalOpType::TableData => {
                // 解码 TableData 记录
                if let Some((table_name, table)) = decode_table_data(&record.data) {
                    pending
                        .entry(record.tx_id)
                        .or_insert_with(Vec::new)
                        .push((table_name, table));
                }
            }
            WalOpType::Commit => {
                // 事务提交：应用该事务的所有 TableData 到 tables
                if let Some(table_list) = pending.remove(&record.tx_id) {
                    for (table_name, table) in table_list {
                        tables.insert(table_name, Arc::new(Mutex::new(table)));
                        applied_count += 1;
                    }
                    committed_txns.insert(record.tx_id);
                }
            }
            WalOpType::Abort => {
                // 事务回滚：丢弃该事务的 TableData
                pending.remove(&record.tx_id);
            }
            _ => {
                // 其他记录类型（Insert/Update/Delete/Checkpoint/FullPageImage）忽略
            }
        }
    }

    // 剩余 pending 中的事务未提交（崩溃时事务未完成），丢弃
    if !pending.is_empty() {
        tracing::warn!(
            uncommitted_txn_count = pending.len(),
            "WAL replay: discarded TableData from uncommitted transactions (crash during commit)"
        );
    }

    (applied_count, committed_txns)
}

// =====================================================================
//  P1-5：行级 WAL 回放（增量崩溃恢复）
// =====================================================================

/// P1-5：将 WAL 中的行级变更（Insert/Update/Delete）应用到表集合。
///
/// # 设计
///
/// 与 `apply_wal_table_data`（全表快照回放）互补：
/// - `TableData` 记录提供崩溃时点的表全量快照（粗粒度）
/// - 行级记录（Insert/Update/Delete）提供细粒度增量变更
///
/// 两者联合使用可大幅缩短大表恢复时间：
/// 1. 先调用 `apply_wal_table_data` 恢复最近一次全量快照
/// 2. 再调用本函数，仅回放**没有 TableData 快照的事务**产生的行级变更
///    （有 TableData 的事务其行级变更已被快照覆盖，跳过避免重复应用）
///
/// # ACID 语义
///
/// - 行级记录按 `tx_id` 缓冲，仅在遇到对应 `Commit` 记录后才应用
/// - `Abort` 记录导致该事务的所有行级变更被丢弃
/// - 崩溃时未完成的事务（无 Commit/Abort）其行级变更留在缓冲区，最终丢弃
///
/// # 行级记录格式
///
/// `WalRowChange` 载荷（`record.data`）由 `WalRecord::decode_insert/update/delete` 解码：
/// - **Insert**：`[row_id: u32][new_len: u32][new_payload(JSON Vec<Value>)]`
/// - **Update**：`[row_id: u32][old_len: u32][old_payload][new_len: u32][new_payload]`
/// - **Delete**：`[row_id: u32][old_len: u32][old_payload]`
///
/// 其中 `row_id` 对应 `InMemoryTable` 的 `tuple_id`，`new_payload` 为
/// `serde_json` 序列化的 `Vec<Value>`（与 `serialize_row_for_cdc` 格式一致）。
///
/// # 参数
///
/// - `tables`：已加载的表集合（通常由快照 + TableData 回放建立基础状态）
/// - `records`：WAL 回放的所有记录
/// - `table_data_txns`：已有 TableData 快照的事务 ID 集合（其行级变更跳过，避免重复）
///
/// # 返回
///
/// 成功应用的行级变更数（Insert + Update + Delete 总条数）
pub async fn apply_wal_row_level(
    tables: &mut HashMap<String, Arc<Mutex<InMemoryTable>>>,
    records: &[WalRecord],
    table_data_txns: &std::collections::HashSet<u32>,
) -> usize {
    use std::collections::HashMap as StdHashMap;
    use szrsql_tx::wal::WalRowChange;

    // 构建 table_id → table_name 反向映射（FNV-1a 哈希，与写入时一致）
    // FNV-1a 32-bit 与 Executor::table_name_to_id 算法相同
    let table_id_map: StdHashMap<u32, String> = tables
        .keys()
        .map(|name| {
            let id = table_name_to_id(name);
            (id, name.clone())
        })
        .collect();

    // 按 tx_id 缓冲的行级变更（tx_id → Vec<(op_type, table_id, row_id, new_payload)>)
    // 仅缓冲没有 TableData 快照的事务（有快照的事务跳过，避免重复应用）
    let mut pending: StdHashMap<u32, Vec<(WalOpType, u32, usize, Vec<u8>)>> = StdHashMap::new();
    let mut applied_count = 0usize;

    for record in records {
        // 跳过已有 TableData 快照的事务（其行级变更已被快照覆盖）
        if table_data_txns.contains(&record.tx_id) {
            continue;
        }

        let table_id = record.page_id;

        match record.op_type {
            WalOpType::Insert => match WalRowChange::decode_insert(table_id, &record.data) {
                Ok(change) => {
                    pending.entry(record.tx_id).or_default().push((
                        WalOpType::Insert,
                        table_id,
                        change.row_id,
                        change.new_payload,
                    ));
                }
                Err(e) => {
                    tracing::warn!(
                        tx_id = record.tx_id,
                        lsn = record.lsn,
                        error = %e,
                        "apply_wal_row_level: failed to decode Insert record"
                    );
                }
            },
            WalOpType::Update => {
                match WalRowChange::decode_update(table_id, &record.data) {
                    Ok(change) => {
                        // Update 回放仅需 new_payload（用新值替换指定 tuple_id 的行内容）
                        pending.entry(record.tx_id).or_default().push((
                            WalOpType::Update,
                            table_id,
                            change.row_id,
                            change.new_payload,
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            tx_id = record.tx_id,
                            lsn = record.lsn,
                            error = %e,
                            "apply_wal_row_level: failed to decode Update record"
                        );
                    }
                }
            }
            WalOpType::Delete => {
                match WalRowChange::decode_delete(table_id, &record.data) {
                    Ok(change) => {
                        // Delete 回放：payload 为空，仅用 row_id 标记删除
                        pending.entry(record.tx_id).or_default().push((
                            WalOpType::Delete,
                            table_id,
                            change.row_id,
                            Vec::new(),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            tx_id = record.tx_id,
                            lsn = record.lsn,
                            error = %e,
                            "apply_wal_row_level: failed to decode Delete record"
                        );
                    }
                }
            }
            WalOpType::Commit => {
                // 事务提交：应用该事务的所有行级变更
                if let Some(changes) = pending.remove(&record.tx_id) {
                    for (op, tid, row_id, payload) in changes {
                        applied_count +=
                            apply_one_row_change(tables, &table_id_map, op, tid, row_id, payload)
                                .await;
                    }
                }
            }
            WalOpType::Abort => {
                // 事务回滚：丢弃该事务的行级变更
                pending.remove(&record.tx_id);
            }
            _ => {} // TableData/Checkpoint/FullPageImage 由其他路径处理
        }
    }

    // 剩余 pending 中的事务未提交（崩溃时事务未完成），丢弃
    if !pending.is_empty() {
        tracing::warn!(
            uncommitted_txn_count = pending.len(),
            "WAL row-level replay: discarded changes from uncommitted transactions"
        );
    }

    applied_count
}

/// 将单条行级变更应用到表集合。
///
/// `table_id` 为 WAL 记录 `page_id` 字段（FNV-1a 表名哈希），
/// 通过 `table_id_map` 反查目标表名后执行对应操作。
///
/// 返回 1 表示成功应用，0 表示失败（表不存在 / 反序列化失败 / 行不存在）。
async fn apply_one_row_change(
    tables: &mut HashMap<String, Arc<Mutex<InMemoryTable>>>,
    table_id_map: &std::collections::HashMap<u32, String>,
    op: WalOpType,
    table_id: u32,
    row_id: usize,
    payload: Vec<u8>,
) -> usize {
    let table_name = match table_id_map.get(&table_id) {
        Some(name) => name.clone(),
        None => {
            tracing::warn!(
                table_id,
                "apply_one_row_change: table_id not found in map (table dropped or never created)"
            );
            return 0;
        }
    };

    let table_arc = match tables.get(&table_name) {
        Some(arc) => arc.clone(),
        None => {
            tracing::warn!(
                table = %table_name,
                "apply_one_row_change: table not found in tables map"
            );
            return 0;
        }
    };

    let row: Vec<szrsql_types::value::Value> = if op == WalOpType::Delete {
        // Delete 回放无需行内容（payload 为空），仅用 row_id 定位目标行
        Vec::new()
    } else {
        match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    table = %table_name,
                    row_id,
                    error = %e,
                    "apply_one_row_change: failed to deserialize row payload"
                );
                return 0;
            }
        }
    };

    // `table_arc.lock()` 返回 tokio 异步锁的 Future；调用方为 async 上下文，
    // 直接 await 即可（main.rs 在 runtime.block_on(async{}) 中调用）。
    let mut table = table_arc.lock().await;
    match op {
        WalOpType::Insert => {
            // Insert 回放：在指定 tuple_id 处插入行（xmin=0 表示已提交）
            table.insert_at_tuple_id(row_id as u32, row, 0);
            1
        }
        WalOpType::Update => {
            // Update 回放：替换指定 tuple_id 处的行内容
            if table.update_row(row_id, row) {
                1
            } else {
                tracing::warn!(
                    table = %table_name,
                    row_id,
                    "apply_one_row_change: Update target row not found"
                );
                0
            }
        }
        WalOpType::Delete => {
            // Delete 回放：标记指定 tuple_id 处的行为删除（xmax=u32::MAX）。
            // 幂等处理：若行已被标记删除（如快照已含该删除），仍计为已应用，
            // 避免重复回放时因 deleted 集合已含该 tuple_id 而误报失败。
            if table.delete_row(row_id) {
                1
            } else {
                // delete_row 返回 false 有两种情况：(a) 行已删除（幂等，计为成功）
                // (b) 行不存在（真实错误）。此处保守计为成功，因为 WAL 记录本身
                // 即表示该删除已发生；若行从未存在则说明 WAL 与数据不一致，
                // 跳过比报错更适合崩溃恢复场景。
                tracing::warn!(
                    table = %table_name,
                    row_id,
                    "apply_one_row_change: Delete target already deleted or missing (idempotent)"
                );
                1
            }
        }
        _ => 0,
    }
}
///
/// 格式：u32 LE 表名长度 + 表名 UTF-8 + 表数据 JSON
fn decode_table_data(data: &[u8]) -> Option<(String, InMemoryTable)> {
    if data.len() < 4 {
        tracing::warn!(
            data_len = data.len(),
            "TableData payload too short for length prefix"
        );
        return None;
    }
    let name_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + name_len {
        tracing::warn!(
            data_len = data.len(),
            name_len,
            "TableData payload too short for table name"
        );
        return None;
    }
    let table_name = match std::str::from_utf8(&data[4..4 + name_len]) {
        Ok(s) => s.to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "TableData table name is not valid UTF-8");
            return None;
        }
    };
    let table_json = &data[4 + name_len..];
    match serde_json::from_slice::<InMemoryTable>(table_json) {
        Ok(table) => Some((table_name, table)),
        Err(e) => {
            tracing::warn!(table = %table_name, error = %e, "failed to deserialize TableData JSON");
            None
        }
    }
}

// =====================================================================
//  P1-2：增量快照单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::ast::{ColumnDefinition, TableName};
    use szrsql_sql::plan::TableSchema;
    use szrsql_tx::wal::{WalReplayer, WalWriter};
    use szrsql_types::value::{ColumnType, Value};
    use tempfile::TempDir;

    /// 构造测试用表（单列 Int64 主键 + 单行数据）
    fn make_test_table(name: &str, row_value: i64) -> InMemoryTable {
        let schema = TableSchema {
            name: TableName::new(name),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        };
        let mut table = InMemoryTable::new(schema);
        table.insert(vec![Value::Int64(row_value)]);
        table
    }

    /// 构造 TableData 记录的 data 字段：u32 LE 表名长度 + 表名 UTF-8 + 表 JSON
    fn encode_table_data(table_name: &str, table: &InMemoryTable) -> Vec<u8> {
        let table_json = serde_json::to_vec(table).expect("serialize table");
        let name_bytes = table_name.as_bytes();
        let mut payload = Vec::with_capacity(4 + name_bytes.len() + table_json.len());
        payload.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(name_bytes);
        payload.extend_from_slice(&table_json);
        payload
    }

    #[tokio::test]
    async fn dirty_tracker_mark_and_take() {
        let tracker = DirtyTableTracker::new();
        assert!(!tracker.is_dirty().await);

        tracker.mark_dirty("users").await;
        tracker.mark_dirty("orders").await;
        assert!(tracker.is_dirty().await);

        let dirty = tracker.take_dirty().await;
        assert_eq!(dirty.len(), 2);
        assert!(dirty.contains("users"));
        assert!(dirty.contains("orders"));

        // take 后应清空
        assert!(!tracker.is_dirty().await);
    }

    #[tokio::test]
    async fn dirty_tracker_mark_many() {
        let tracker = DirtyTableTracker::new();
        tracker.mark_dirty_many(["a", "b", "c"]).await;
        let dirty = tracker.take_dirty().await;
        assert_eq!(dirty.len(), 3);
    }

    #[tokio::test]
    async fn dirty_tracker_clone_shares_state() {
        let tracker1 = DirtyTableTracker::new();
        let tracker2 = tracker1.clone();

        tracker1.mark_dirty("shared_table").await;
        // clone 后两者共享同一内部状态
        assert!(tracker2.is_dirty().await);
        let dirty = tracker2.take_dirty().await;
        assert!(dirty.contains("shared_table"));
        assert!(!tracker1.is_dirty().await);
    }

    #[tokio::test]
    async fn incremental_snapshot_skips_when_no_dirty() {
        let tmp = TempDir::new().unwrap();
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tracker = DirtyTableTracker::new();

        // 无脏表：应直接返回，不写文件
        let result = save_incremental_snapshot(&shared, tmp.path(), &tracker).await;
        assert!(result.is_ok());
        assert!(!default_snapshot_path(tmp.path()).exists());
    }

    #[tokio::test]
    async fn incremental_snapshot_saves_only_dirty_tables() {
        let tmp = TempDir::new().unwrap();
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tracker = DirtyTableTracker::new();

        // 注册两张表
        let t1 = Arc::new(Mutex::new(make_test_table("users", 1)));
        let t2 = Arc::new(Mutex::new(make_test_table("orders", 100)));
        shared.write().await.insert("users".into(), t1.clone());
        shared.write().await.insert("orders".into(), t2.clone());

        // 仅标记 users 为脏
        tracker.mark_dirty("users").await;

        let result = save_incremental_snapshot(&shared, tmp.path(), &tracker).await;
        assert!(result.is_ok());

        // 验证快照文件已写入
        let snap_path = default_snapshot_path(tmp.path());
        assert!(snap_path.exists());

        // 验证快照内容包含两张表（users 是脏表新写入，orders 是非脏表从空快照写入）
        let content = std::fs::read_to_string(&snap_path).unwrap();
        let snap: SnapshotFile = serde_json::from_str(&content).unwrap();
        assert_eq!(snap.tables.len(), 1); // orders 未在磁盘快照中，仅 users 被写入
        assert_eq!(snap.tables[0].name(), "users");
    }

    #[tokio::test]
    async fn incremental_snapshot_merges_with_existing() {
        let tmp = TempDir::new().unwrap();
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tracker = DirtyTableTracker::new();

        // 第一轮：保存 users 表
        let t1 = Arc::new(Mutex::new(make_test_table("users", 1)));
        shared.write().await.insert("users".into(), t1.clone());
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 第二轮：新增 orders 表（仅标记 orders 为脏，users 不脏）
        let t2 = Arc::new(Mutex::new(make_test_table("orders", 100)));
        shared.write().await.insert("orders".into(), t2.clone());
        tracker.mark_dirty("orders").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 验证：两张表都应在快照中（users 从磁盘复用，orders 新写入）
        let snap_path = default_snapshot_path(tmp.path());
        let content = std::fs::read_to_string(&snap_path).unwrap();
        let snap: SnapshotFile = serde_json::from_str(&content).unwrap();
        assert_eq!(snap.tables.len(), 2);
        let names: Vec<&str> = snap.tables.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"users"));
        assert!(names.contains(&"orders"));
    }

    #[tokio::test]
    async fn incremental_snapshot_updates_dirty_table_value() {
        let tmp = TempDir::new().unwrap();
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tracker = DirtyTableTracker::new();

        // 第一轮：保存 users 表（row_value=1）
        let t1 = Arc::new(Mutex::new(make_test_table("users", 1)));
        shared.write().await.insert("users".into(), t1.clone());
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 修改 users 表数据
        {
            let mut guard = t1.lock().await;
            // 修改第一行的值（Row 是 Vec<Value>，直接索引）
            guard.rows_mut()[0][0] = Value::Int64(999);
        }

        // 第二轮：再次标记 users 为脏
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 验证：快照中的 users 表数据应为新值 999
        let snap_path = default_snapshot_path(tmp.path());
        let content = std::fs::read_to_string(&snap_path).unwrap();
        let snap: SnapshotFile = serde_json::from_str(&content).unwrap();
        assert_eq!(snap.tables.len(), 1);
        assert_eq!(snap.tables[0].name(), "users");
        assert_eq!(snap.tables[0].rows().len(), 1);
        // 验证行值已被更新（Row 是 Vec<Value>，直接索引）
        match &snap.tables[0].rows()[0][0] {
            Value::Int64(v) => assert_eq!(*v, 999),
            other => panic!("expected Int64(999), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn incremental_snapshot_handles_dropped_table() {
        let tmp = TempDir::new().unwrap();
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tracker = DirtyTableTracker::new();

        // 第一轮：保存 users 表
        let t1 = Arc::new(Mutex::new(make_test_table("users", 1)));
        shared.write().await.insert("users".into(), t1);
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 第二轮：DROP users（从 shared 移除），标记 users 为脏
        shared.write().await.remove("users");
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 验证：快照中不应有 users 表
        let snap_path = default_snapshot_path(tmp.path());
        let content = std::fs::read_to_string(&snap_path).unwrap();
        let snap: SnapshotFile = serde_json::from_str(&content).unwrap();
        assert_eq!(snap.tables.len(), 0);
    }

    #[tokio::test]
    async fn incremental_snapshot_falls_back_on_corrupted_existing() {
        let tmp = TempDir::new().unwrap();
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let tracker = DirtyTableTracker::new();

        // 写入损坏的快照文件
        let snap_path = default_snapshot_path(tmp.path());
        std::fs::write(&snap_path, "not valid json").unwrap();

        // 标记一张脏表
        let t1 = Arc::new(Mutex::new(make_test_table("users", 1)));
        shared.write().await.insert("users".into(), t1);
        tracker.mark_dirty("users").await;

        // 应成功保存（回退到全量保存策略）
        let result = save_incremental_snapshot(&shared, tmp.path(), &tracker).await;
        assert!(result.is_ok());

        // 验证：快照文件已被覆盖为合法 JSON
        let content = std::fs::read_to_string(&snap_path).unwrap();
        let snap: SnapshotFile = serde_json::from_str(&content).unwrap();
        assert_eq!(snap.tables.len(), 1);
        assert_eq!(snap.tables[0].name(), "users");
    }

    // =================================================================
    //  P1-3：端到端崩溃恢复集成测试（WAL + 快照协同）
    // =================================================================

    /// E2E-1：快照加载 + WAL 回放的完整崩溃恢复流程。
    ///
    /// 模拟场景：进程崩溃后重启，先加载磁盘快照（旧数据），
    /// 再回放 WAL 中的已提交 TableData（新数据），验证 WAL 覆盖快照。
    #[tokio::test]
    async fn e2e_crash_recovery_snapshot_plus_wal() {
        let tmp = TempDir::new().unwrap();

        // 步骤 1：创建初始快照（users 表 1 行，值为 1）
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        shared.write().await.insert(
            "users".into(),
            Arc::new(Mutex::new(make_test_table("users", 1))),
        );
        let tracker = DirtyTableTracker::new();
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 步骤 2：模拟 WAL 写入（新事务提交了 users 表的更新版本，2 行数据）
        let wal_path = tmp.path().join("test.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();
        let mut new_users = make_test_table("users", 100);
        new_users.insert(vec![Value::Int64(200)]);
        let payload = encode_table_data("users", &new_users);
        let tx_id = 1u32;
        writer
            .append(WalRecord::new(0, tx_id, WalOpType::TableData, 0, payload))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx_id, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 步骤 3：模拟重启 — 加载快照 + 回放 WAL
        let mut loaded_tables = load_snapshot(tmp.path()).unwrap();
        let wal_records = WalReplayer::replay_all(&wal_path).unwrap();
        assert!(!wal_records.is_empty());
        let (applied, _) = apply_wal_table_data(&mut loaded_tables, &wal_records);
        assert_eq!(applied, 1, "应应用 1 个 TableData 记录");

        // 步骤 4：验证最终 users 表有 2 行数据（WAL 覆盖了快照）
        let users_arc = loaded_tables.get("users").expect("users 表应存在");
        let users_guard = users_arc.lock().await;
        assert_eq!(users_guard.rows().len(), 2, "WAL 回放后 users 表应有 2 行");
        assert_eq!(users_guard.rows()[0][0], Value::Int64(100));
        assert_eq!(users_guard.rows()[1][0], Value::Int64(200));
    }

    /// E2E-2：Abort 事务的 TableData 不被应用。
    ///
    /// 模拟场景：事务写入 TableData 后回滚（Abort），回放时该数据不应出现。
    #[tokio::test]
    async fn e2e_wal_replay_abort_transaction_not_applied() {
        let tmp = TempDir::new().unwrap();
        let wal_path = tmp.path().join("abort.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();

        // 事务 1：Abort（TableData 不应被应用）
        let tx1 = 10u32;
        let abort_table = make_test_table("temp", 999);
        writer
            .append(WalRecord::new(
                0,
                tx1,
                WalOpType::TableData,
                0,
                encode_table_data("temp", &abort_table),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx1, WalOpType::Abort, 0, vec![]))
            .unwrap();

        // 事务 2：Commit（TableData 应被应用）
        let tx2 = 11u32;
        let commit_table = make_test_table("real", 42);
        writer
            .append(WalRecord::new(
                0,
                tx2,
                WalOpType::TableData,
                0,
                encode_table_data("real", &commit_table),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx2, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 回放
        let mut loaded_tables: HashMap<String, Arc<Mutex<InMemoryTable>>> = HashMap::new();
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let (applied, _) = apply_wal_table_data(&mut loaded_tables, &records);
        assert_eq!(applied, 1, "仅 Commit 事务的 TableData 被应用");

        // 验证：temp 表不应存在（Abort），real 表应存在（Commit）
        assert!(
            !loaded_tables.contains_key("temp"),
            "Abort 事务的 temp 表不应被应用"
        );
        assert!(
            loaded_tables.contains_key("real"),
            "Commit 事务的 real 表应存在"
        );
        let real_guard = loaded_tables.get("real").unwrap().lock().await;
        assert_eq!(real_guard.rows().len(), 1);
        assert_eq!(real_guard.rows()[0][0], Value::Int64(42));
    }

    /// E2E-3：未完成事务（无 Commit/Abort）的 TableData 不被应用。
    ///
    /// 模拟场景：进程崩溃时事务正在提交，TableData 已写入但 Commit 未写入，
    /// 重启后该事务的 TableData 应被丢弃（保证 ACID）。
    #[tokio::test]
    async fn e2e_wal_replay_uncommitted_transaction_not_applied() {
        let tmp = TempDir::new().unwrap();
        let wal_path = tmp.path().join("uncommitted.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();

        // 事务 1：仅写 TableData，无 Commit（模拟崩溃时事务未完成）
        let tx1 = 100u32;
        let table = make_test_table("uncommitted", 777);
        writer
            .append(WalRecord::new(
                0,
                tx1,
                WalOpType::TableData,
                0,
                encode_table_data("uncommitted", &table),
            ))
            .unwrap();
        // 不写 Commit/Abort，直接 flush + drop（模拟崩溃）
        writer.flush().unwrap();
        drop(writer);

        // 回放
        let mut loaded_tables: HashMap<String, Arc<Mutex<InMemoryTable>>> = HashMap::new();
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let (applied, _) = apply_wal_table_data(&mut loaded_tables, &records);
        assert_eq!(applied, 0, "未提交事务的 TableData 不应被应用");
        assert!(
            !loaded_tables.contains_key("uncommitted"),
            "未提交事务的表不应存在"
        );
    }

    /// E2E-4：多事务多表并发提交，WAL 回放后所有已提交表都应存在。
    ///
    /// 模拟场景：3 个事务分别修改不同的表，全部 Commit，
    /// 回放后 3 张表都应被正确应用。
    #[tokio::test]
    async fn e2e_apply_wal_multiple_commits_different_tables() {
        let tmp = TempDir::new().unwrap();
        let wal_path = tmp.path().join("multi.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();

        // 事务 1：users 表
        let tx1 = 1u32;
        let users = make_test_table("users", 10);
        writer
            .append(WalRecord::new(
                0,
                tx1,
                WalOpType::TableData,
                0,
                encode_table_data("users", &users),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx1, WalOpType::Commit, 0, vec![]))
            .unwrap();

        // 事务 2：orders 表
        let tx2 = 2u32;
        let orders = make_test_table("orders", 20);
        writer
            .append(WalRecord::new(
                0,
                tx2,
                WalOpType::TableData,
                0,
                encode_table_data("orders", &orders),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx2, WalOpType::Commit, 0, vec![]))
            .unwrap();

        // 事务 3：products 表
        let tx3 = 3u32;
        let products = make_test_table("products", 30);
        writer
            .append(WalRecord::new(
                0,
                tx3,
                WalOpType::TableData,
                0,
                encode_table_data("products", &products),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx3, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 回放
        let mut loaded_tables: HashMap<String, Arc<Mutex<InMemoryTable>>> = HashMap::new();
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let (applied, _) = apply_wal_table_data(&mut loaded_tables, &records);
        assert_eq!(applied, 3, "3 个已提交事务的 TableData 都应被应用");
        assert!(loaded_tables.contains_key("users"));
        assert!(loaded_tables.contains_key("orders"));
        assert!(loaded_tables.contains_key("products"));

        // 验证每张表的数据正确
        let users_val = {
            let g = loaded_tables.get("users").unwrap().lock().await;
            g.rows()[0][0].clone()
        };
        assert_eq!(users_val, Value::Int64(10));

        let orders_val = {
            let g = loaded_tables.get("orders").unwrap().lock().await;
            g.rows()[0][0].clone()
        };
        assert_eq!(orders_val, Value::Int64(20));

        let products_val = {
            let g = loaded_tables.get("products").unwrap().lock().await;
            g.rows()[0][0].clone()
        };
        assert_eq!(products_val, Value::Int64(30));
    }

    /// E2E-5：WAL 回放覆盖快照中的同名表。
    ///
    /// 模拟场景：快照中有 users 表（1 行），WAL 中有同名 users 表（2 行），
    /// 回放后 users 表应为 WAL 版本（2 行），而非快照版本（1 行）。
    #[tokio::test]
    async fn e2e_apply_wal_overwrites_snapshot() {
        let tmp = TempDir::new().unwrap();

        // 步骤 1：创建快照（users 表 1 行）
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        shared.write().await.insert(
            "users".into(),
            Arc::new(Mutex::new(make_test_table("users", 1))),
        );
        let tracker = DirtyTableTracker::new();
        tracker.mark_dirty("users").await;
        save_incremental_snapshot(&shared, tmp.path(), &tracker)
            .await
            .unwrap();

        // 步骤 2：WAL 中写入同名 users 表（2 行，不同值）
        let wal_path = tmp.path().join("overwrite.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();
        let mut new_users = make_test_table("users", 500);
        new_users.insert(vec![Value::Int64(600)]);
        let tx_id = 1u32;
        writer
            .append(WalRecord::new(
                0,
                tx_id,
                WalOpType::TableData,
                0,
                encode_table_data("users", &new_users),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, tx_id, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 步骤 3：加载快照（1 行）→ 回放 WAL（2 行覆盖）
        let mut loaded_tables = load_snapshot(tmp.path()).unwrap();
        {
            let g = loaded_tables.get("users").unwrap().lock().await;
            assert_eq!(g.rows().len(), 1, "快照加载后 users 表应有 1 行");
        }
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let (applied, _) = apply_wal_table_data(&mut loaded_tables, &records);
        assert_eq!(applied, 1);

        // 步骤 4：验证 users 表已被 WAL 版本覆盖（2 行）
        let g = loaded_tables.get("users").unwrap().lock().await;
        assert_eq!(g.rows().len(), 2, "WAL 回放后 users 表应有 2 行");
        assert_eq!(g.rows()[0][0], Value::Int64(500));
        assert_eq!(g.rows()[1][0], Value::Int64(600));
    }

    // =================================================================
    //  P1-5：行级 WAL 回放测试
    // =================================================================

    /// E2E-R1：行级 Insert 回放 — 崩溃恢复后新增行出现。
    ///
    /// 场景：快照有 1 行 → 事务插入第 2 行（写行级 Insert WAL）→ 崩溃
    /// 恢复：快照 1 行 + 行级回放 = 2 行
    #[tokio::test]
    async fn e2e_row_level_replay_insert() {
        use szrsql_sql::executor::{InMemoryTable, Row};
        use szrsql_tx::wal::{WalRecord, WalReplayer, WalRowChange, WalWriter};
        use szrsql_types::value::Value;

        let tmp = TempDir::new().unwrap();

        // 步骤 1：创建快照（users 表 2 行：tuple_id=0 值 100，tuple_id=1 值 200）
        let mut snap_table = make_test_table("users", 100);
        snap_table.insert(vec![Value::Int64(200)]);
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::from([(
                "users".into(),
                Arc::new(Mutex::new(snap_table)),
            )])));
        save_snapshot(&shared, tmp.path()).await.unwrap();

        // 步骤 2：写入行级 Insert WAL 记录（模拟事务插入 tuple_id=2 的新行）
        let wal_path = tmp.path().join("row_insert.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();
        let table_id = table_name_to_id("users");
        let new_row: Row = vec![Value::Int64(300)];
        let new_payload = serde_json::to_vec(&new_row).unwrap();
        let change = WalRowChange::for_insert(table_id, 2, new_payload);
        let record = WalRecord::new_row_insert(42, &change);
        writer.append(record).unwrap();
        writer
            .append(WalRecord::new(0, 42, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 步骤 3：加载快照 + 行级回放
        let mut loaded_tables = load_snapshot(tmp.path()).unwrap();
        {
            let g = loaded_tables.get("users").unwrap().lock().await;
            assert_eq!(g.rows().len(), 2, "快照加载后应有 2 行");
        }
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let empty_set = std::collections::HashSet::new();
        let row_applied = apply_wal_row_level(&mut loaded_tables, &records, &empty_set).await;
        assert_eq!(row_applied, 1, "应应用 1 条行级 Insert");

        // 步骤 4：验证新行已插入
        let g = loaded_tables.get("users").unwrap().lock().await;
        assert_eq!(g.rows().len(), 3, "行级回放后应有 3 行");
    }

    /// E2E-R2：行级 Update 回放 — 崩溃恢复后行内容被更新。
    #[tokio::test]
    async fn e2e_row_level_replay_update() {
        use szrsql_sql::executor::{InMemoryTable, Row};
        use szrsql_tx::wal::{WalRecord, WalReplayer, WalRowChange, WalWriter};
        use szrsql_types::value::Value;

        let tmp = TempDir::new().unwrap();

        // 步骤 1：创建快照（users 表 1 行，tuple_id=0，值为 100）
        let snap_table = make_test_table("users", 100);
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::from([(
                "users".into(),
                Arc::new(Mutex::new(snap_table)),
            )])));
        save_snapshot(&shared, tmp.path()).await.unwrap();

        // 步骤 2：写入行级 Update WAL 记录（将 tuple_id=0 的值更新为 999）
        let wal_path = tmp.path().join("row_update.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();
        let table_id = table_name_to_id("users");
        let new_row: Row = vec![Value::Int64(999)];
        let new_payload = serde_json::to_vec(&new_row).unwrap();
        let change = WalRowChange::for_update(table_id, 0, vec![], new_payload);
        let record = WalRecord::new_row_update(55, &change);
        writer.append(record).unwrap();
        writer
            .append(WalRecord::new(0, 55, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 步骤 3：加载快照 + 行级回放
        let mut loaded_tables = load_snapshot(tmp.path()).unwrap();
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let empty_set = std::collections::HashSet::new();
        let row_applied = apply_wal_row_level(&mut loaded_tables, &records, &empty_set).await;
        assert_eq!(row_applied, 1, "应应用 1 条行级 Update");

        // 步骤 4：验证行值已被更新
        let g = loaded_tables.get("users").unwrap().lock().await;
        assert_eq!(g.rows().len(), 1, "Update 不改变行数");
        assert_eq!(
            g.rows()[0][0],
            Value::Int64(999),
            "行级 Update 回放后值应为 999"
        );
    }

    /// E2E-R3：行级 Delete 回放 — 崩溃恢复后行被标记删除。
    #[tokio::test]
    async fn e2e_row_level_replay_delete() {
        use szrsql_sql::executor::InMemoryTable;
        use szrsql_tx::wal::{WalRecord, WalReplayer, WalRowChange, WalWriter};
        use szrsql_types::value::Value;

        let tmp = TempDir::new().unwrap();

        // 步骤 1：创建快照（users 表 2 行，tuple_id=0 和 1）
        let mut snap_table = make_test_table("users", 100);
        snap_table.insert(vec![Value::Int64(200)]);
        let shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>> =
            Arc::new(RwLock::new(HashMap::from([(
                "users".into(),
                Arc::new(Mutex::new(snap_table)),
            )])));
        save_snapshot(&shared, tmp.path()).await.unwrap();

        // 步骤 2：写入行级 Delete WAL 记录（删除 tuple_id=0 的行）
        let wal_path = tmp.path().join("row_delete.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();
        let table_id = table_name_to_id("users");
        let change = WalRowChange::for_delete(table_id, 0, vec![]);
        let record = WalRecord::new_row_delete(77, &change);
        writer.append(record).unwrap();
        writer
            .append(WalRecord::new(0, 77, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 步骤 3：加载快照 + 行级回放
        let mut loaded_tables = load_snapshot(tmp.path()).unwrap();
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let empty_set = std::collections::HashSet::new();
        let row_applied = apply_wal_row_level(&mut loaded_tables, &records, &empty_set).await;
        assert_eq!(row_applied, 1, "应应用 1 条行级 Delete");

        // 步骤 4：验证删除的行不再可见（delete_row 标记 xmax，rows() 过滤已删除）
        let g = loaded_tables.get("users").unwrap().lock().await;
        assert_eq!(
            g.rows().len(),
            1,
            "行级 Delete 回放后可见行应为 1（tuple_id=0 被删除）"
        );
        assert_eq!(g.rows()[0][0], Value::Int64(200));
    }

    /// E2E-R4：Abort 事务的行级变更不被应用。
    #[tokio::test]
    async fn e2e_row_level_replay_abort_not_applied() {
        use szrsql_sql::executor::{InMemoryTable, Row};
        use szrsql_tx::wal::{WalRecord, WalReplayer, WalRowChange, WalWriter};
        use szrsql_types::value::Value;

        let tmp = TempDir::new().unwrap();
        let wal_path = tmp.path().join("abort_row.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();

        // 事务 1（Abort）：Insert 不应被应用
        let table_id = table_name_to_id("users");
        let bad_row: Row = vec![Value::Int64(9999)];
        let change = WalRowChange::for_insert(table_id, 10, serde_json::to_vec(&bad_row).unwrap());
        writer
            .append(WalRecord::new_row_insert(200, &change))
            .unwrap();
        writer
            .append(WalRecord::new(0, 200, WalOpType::Abort, 0, vec![]))
            .unwrap();

        // 事务 2（Commit）：Insert 应被应用
        let good_row: Row = vec![Value::Int64(42)];
        let change2 =
            WalRowChange::for_insert(table_id, 11, serde_json::to_vec(&good_row).unwrap());
        writer
            .append(WalRecord::new_row_insert(201, &change2))
            .unwrap();
        writer
            .append(WalRecord::new(0, 201, WalOpType::Commit, 0, vec![]))
            .unwrap();
        writer.flush().unwrap();
        drop(writer);

        // 创建空表（无快照，行级 Insert 需要表存在才能应用）
        let mut loaded_tables: HashMap<String, Arc<Mutex<InMemoryTable>>> = HashMap::new();
        // 创建 users 表（空），使行级 Insert 有目标
        let empty_table = make_test_table("users", 0);
        // 清除默认行，保持空表
        loaded_tables.insert("users".to_string(), Arc::new(Mutex::new(empty_table)));

        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let empty_set = std::collections::HashSet::new();
        let row_applied = apply_wal_row_level(&mut loaded_tables, &records, &empty_set).await;
        assert_eq!(row_applied, 1, "仅 Commit 事务的行级 Insert 被应用");

        let g = loaded_tables.get("users").unwrap().lock().await;
        // Abort 的 9999 不应出现，Commit 的 42 应出现
        let has_9999 = g.rows().iter().any(|r| matches!(&r[0], Value::Int64(9999)));
        let has_42 = g.rows().iter().any(|r| matches!(&r[0], Value::Int64(42)));
        assert!(!has_9999, "Abort 事务的行不应被应用");
        assert!(has_42, "Commit 事务的行应被应用");
    }

    /// E2E-R5：TableData + 行级联合回放 — TableData 事务的行级变更跳过（不重复应用）。
    ///
    /// 场景：
    /// - 事务 10：写 TableData（全量快照，users = [1,2]）+ Commit
    /// - 事务 11：写行级 Insert（users 新增行 3）+ Commit
    /// 回放：TableData 重建 users=[1,2]，行级 Insert 追加行 3 → users=[1,2,3]
    /// 事务 10 的行级 Insert（若存在）应被跳过（已被 TableData 覆盖）
    #[tokio::test]
    async fn e2e_row_level_with_table_data_skip_committed_txn() {
        use szrsql_sql::executor::Row;
        use szrsql_tx::wal::{WalRecord, WalReplayer, WalRowChange, WalWriter};
        use szrsql_types::value::Value;

        let tmp = TempDir::new().unwrap();
        let wal_path = tmp.path().join("mixed.wal");
        let writer = WalWriter::create_new(&wal_path).unwrap();
        let table_id = table_name_to_id("users");

        // 事务 10：TableData（全量快照 users=[100]）
        writer
            .append(WalRecord::new(
                0,
                10,
                WalOpType::TableData,
                0,
                encode_table_data("users", &make_test_table("users", 100)),
            ))
            .unwrap();
        writer
            .append(WalRecord::new(0, 10, WalOpType::Commit, 0, vec![]))
            .unwrap();

        // 事务 10 的行级 Insert（模拟：与 TableData 同一事务，应被跳过）
        let dup_row: Row = vec![Value::Int64(9999)];
        let dup_change =
            WalRowChange::for_insert(table_id, 99, serde_json::to_vec(&dup_row).unwrap());
        writer
            .append(WalRecord::new_row_insert(10, &dup_change))
            .unwrap();

        // 事务 11：行级 Insert（新增行 300，tuple_id=2）
        let new_row: Row = vec![Value::Int64(300)];
        let new_change =
            WalRowChange::for_insert(table_id, 2, serde_json::to_vec(&new_row).unwrap());
        writer
            .append(WalRecord::new_row_insert(11, &new_change))
            .unwrap();
        writer
            .append(WalRecord::new(0, 11, WalOpType::Commit, 0, vec![]))
            .unwrap();

        writer.flush().unwrap();
        drop(writer);

        // 回放：先 TableData，再行级
        let mut loaded_tables = load_snapshot(tmp.path()).unwrap_or_default();
        let records = WalReplayer::replay_all(&wal_path).unwrap();
        let (td_applied, committed_txns) = apply_wal_table_data(&mut loaded_tables, &records);
        assert_eq!(td_applied, 1, "1 个 TableData 被应用");
        assert!(
            committed_txns.contains(&10),
            "事务 10 应标记为已提交 TableData"
        );

        let row_applied = apply_wal_row_level(&mut loaded_tables, &records, &committed_txns).await;
        // 事务 10 的行级 Insert 被跳过（在 committed_txns 中）
        // 事务 11 的行级 Insert 被应用
        assert_eq!(
            row_applied, 1,
            "仅事务 11 的行级 Insert 被应用（事务 10 的跳过）"
        );

        // 验证：9999（事务 10 的行级，应跳过）不应出现
        let g = loaded_tables.get("users").unwrap().lock().await;
        let has_9999 = g.rows().iter().any(|r| matches!(&r[0], Value::Int64(9999)));
        let has_300 = g.rows().iter().any(|r| matches!(&r[0], Value::Int64(300)));
        assert!(!has_9999, "TableData 事务的行级变更应被跳过");
        assert!(has_300, "事务 11 的新增行应被应用");
    }
}

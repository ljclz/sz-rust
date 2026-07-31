//! 物理备份 + WAL 归档 — Phase 7a.3
//!
//! # 设计
//!
//! - **`BackupManager`** — 物理备份管理器，支持全量备份、WAL 归档、恢复 + WAL 回放
//! - **`BackupManifest`** — 备份元数据（JSON 序列化），含 pages_checksum 校验
//! - **`WalArchiveMeta`** — WAL 归档元数据（JSON 序列化），含 checksum 校验
//! - **`ReplayStats`** — WAL 回放统计（应用/跳过/更新/创建）
//!
//! # 备份目录结构
//!
//! ```text
//! <backup_dir>/
//!   <backup_id>/
//!     manifest.json              # BackupManifest (JSON)
//!     pages.bin                  # 页数据：[page_id u32 LE][page_len u32 LE][page_bytes N]
//!     wal_archives/
//!       <archive_id>.wal         # WAL 记录：concatenated WalRecord::encode()
//!       <archive_id>.meta.json   # WalArchiveMeta (JSON)
//! ```
//!
//! # WAL 回放语义
//!
//! 物理日志（physical logging）：
//! - `Insert` / `Update` / `Delete` / `FullPageImage`：`data` = 页后镜像（after-image），替换整页
//! - `Commit` / `Abort` / `Checkpoint`：不修改页，跳过
//!
//! 对应 `SzRSQL实施进度.md` Phase 7a.3。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use szrsql_tx::wal::{WalError, WalOpType, WalRecord, WAL_HEADER_SIZE, WAL_TRAILER_SIZE};
use thiserror::Error;
#[allow(unused_imports)]
use tracing::{debug, info, instrument, trace, warn};

// =====================================================================
//  常量
// =====================================================================

const MANIFEST_FILE: &str = "manifest.json";
const PAGES_FILE: &str = "pages.bin";
const WAL_ARCHIVE_DIR: &str = "wal_archives";

/// 页数据列表类型别名：`(page_id, page_bytes)` 的集合
pub type Pages = Vec<(u32, Vec<u8>)>;

// =====================================================================
//  BackupError
// =====================================================================

/// 物理备份错误类型
#[derive(Debug, Error)]
pub enum BackupError {
    /// I/O 错误
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON 序列化/反序列化错误
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// WAL 错误
    #[error("WAL error: {0}")]
    Wal(#[from] WalError),
    /// 备份不存在
    #[error("backup not found: {0}")]
    BackupNotFound(String),
    /// WAL 归档不存在
    #[error("WAL archive not found: {0}")]
    ArchiveNotFound(String),
    /// 校验和不匹配
    #[error("checksum mismatch: expected {expected:#010x}, actual {actual:#010x}")]
    ChecksumMismatch { expected: u32, actual: u32 },
    /// 备份 ID 为空
    #[error("backup id cannot be empty")]
    EmptyBackupId,
    /// 归档 ID 为空
    #[error("archive id cannot be empty")]
    EmptyArchiveId,
}

// =====================================================================
//  BackupManifest — 备份元数据
// =====================================================================

/// 全量备份元数据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    /// 备份 ID
    pub backup_id: String,
    /// 创建时间（Unix epoch 秒）
    pub created_at: u64,
    /// 起始 LSN（备份对应的 WAL 起始点）
    pub start_lsn: u64,
    /// 结束 LSN（备份时刻的 WAL 末尾，恢复时从此 LSN 之后开始回放）
    pub end_lsn: u64,
    /// 页数量
    pub page_count: u32,
    /// 页数据 CRC32C 校验和
    pub pages_checksum: u32,
}

// =====================================================================
//  WalArchiveMeta — WAL 归档元数据
// =====================================================================

/// WAL 归档元数据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalArchiveMeta {
    /// 归档 ID
    pub archive_id: String,
    /// 起始 LSN
    pub start_lsn: u64,
    /// 结束 LSN
    pub end_lsn: u64,
    /// 记录数
    pub record_count: u64,
    /// WAL 数据 CRC32C 校验和
    pub checksum: u32,
    /// 创建时间（Unix epoch 秒），用于 PITR 时间点恢复筛选
    #[serde(default)]
    pub created_at: u64,
}

// =====================================================================
//  ReplayStats — WAL 回放统计
// =====================================================================

/// WAL 回放统计
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayStats {
    /// 总记录数
    pub records_total: usize,
    /// 已应用记录数（Insert/Update/Delete/FullPageImage）
    pub records_applied: usize,
    /// 已跳过记录数（Commit/Abort/Checkpoint）
    pub records_skipped: usize,
    /// 更新已有页数
    pub pages_updated: usize,
    /// 新建页数
    pub pages_created: usize,
}

// =====================================================================
//  DiffBackupMeta — 差异备份元数据（Phase 7a.4）
// =====================================================================

/// 差异备份元数据
///
/// 基于基准全量备份的签名清单，仅包含签名变更的页。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffBackupMeta {
    /// 差异备份 ID
    pub diff_id: String,
    /// 基准全量备份 ID
    pub backup_id: String,
    /// 创建时间（Unix epoch 秒）
    pub created_at: u64,
    /// 变更页数量
    pub changed_page_count: u32,
    /// 变更页数据 CRC32C 校验和
    pub pages_checksum: u32,
}

// =====================================================================
//  BackupManager — 物理备份管理器
// =====================================================================

/// 物理备份管理器
///
/// 管理全量备份、WAL 归档、恢复 + WAL 回放的完整生命周期。
///
/// # 示例
///
/// ```
/// use szrsql_replication::backup::BackupManager;
/// use szrsql_tx::wal::{WalRecord, WalOpType};
///
/// // 1. 创建备份管理器
/// let dir = std::env::temp_dir().join("szrsql_backup_doctest");
/// let mgr = BackupManager::new(&dir).unwrap();
///
/// // 2. 全量备份
/// let pages = vec![(0u32, vec![0xAA; 8192])];
/// let manifest = mgr.create_full_backup("bk1", &pages, 100).unwrap();
/// assert_eq!(manifest.page_count, 1);
///
/// // 3. WAL 归档
/// let records = vec![WalRecord::new(101, 1, WalOpType::FullPageImage, 0, vec![0xBB; 8192])];
/// let meta = mgr.archive_wal("bk1", "ar1", &records).unwrap();
/// assert_eq!(meta.record_count, 1);
///
/// // 4. 恢复 + WAL 回放
/// let (restored, _, replay_stats) = mgr.restore_with_wal("bk1").unwrap();
/// assert_eq!(restored.len(), 1);
/// assert_eq!(restored[0].1, vec![0xBB; 8192]); // WAL 覆盖了备份
/// assert_eq!(replay_stats[0].records_applied, 1);
///
/// // 清理
/// mgr.delete_backup("bk1").unwrap();
/// ```
pub struct BackupManager {
    backup_dir: PathBuf,
}

impl BackupManager {
    /// 创建备份管理器，自动创建备份根目录
    pub fn new<P: AsRef<Path>>(backup_dir: P) -> Result<Self, BackupError> {
        let backup_dir = backup_dir.as_ref().to_path_buf();
        fs::create_dir_all(&backup_dir)?;
        Ok(Self { backup_dir })
    }

    // -----------------------------------------------------------------
    //  路径辅助方法
    // -----------------------------------------------------------------

    fn backup_path(&self, backup_id: &str) -> PathBuf {
        self.backup_dir.join(backup_id)
    }

    fn manifest_path(&self, backup_id: &str) -> PathBuf {
        self.backup_path(backup_id).join(MANIFEST_FILE)
    }

    fn pages_path(&self, backup_id: &str) -> PathBuf {
        self.backup_path(backup_id).join(PAGES_FILE)
    }

    fn wal_archive_dir(&self, backup_id: &str) -> PathBuf {
        self.backup_path(backup_id).join(WAL_ARCHIVE_DIR)
    }

    fn wal_archive_path(&self, backup_id: &str, archive_id: &str) -> PathBuf {
        self.wal_archive_dir(backup_id)
            .join(format!("{}.wal", archive_id))
    }

    fn wal_archive_meta_path(&self, backup_id: &str, archive_id: &str) -> PathBuf {
        self.wal_archive_dir(backup_id)
            .join(format!("{}.meta.json", archive_id))
    }

    // -----------------------------------------------------------------
    //  全量备份
    // -----------------------------------------------------------------

    /// 创建全量备份
    ///
    /// 将所有页数据序列化到 `pages.bin`，并生成 `manifest.json` 元数据。
    ///
    /// # 参数
    /// - `backup_id` — 备份 ID（用作子目录名）
    /// - `pages` — 页数据列表 `(page_id, page_bytes)`
    /// - `end_lsn` — 备份时刻的 WAL 末尾 LSN（恢复时从此 LSN 之后开始回放）
    ///
    /// # 返回
    /// 备份元数据 `BackupManifest`
    #[tracing::instrument(skip(self, pages))]
    pub fn create_full_backup(
        &self,
        backup_id: &str,
        pages: &[(u32, Vec<u8>)],
        end_lsn: u64,
    ) -> Result<BackupManifest, BackupError> {
        if backup_id.is_empty() {
            tracing::warn!("create_full_backup called with empty backup_id");
            return Err(BackupError::EmptyBackupId);
        }

        let backup_path = self.backup_path(backup_id);
        fs::create_dir_all(&backup_path)?;

        // 写入页数据
        let pages_path = self.pages_path(backup_id);
        let mut writer = BufWriter::new(File::create(&pages_path)?);
        let mut checksum_buf = Vec::new();
        for (page_id, page_bytes) in pages {
            writer.write_all(&page_id.to_le_bytes())?;
            writer.write_all(&(page_bytes.len() as u32).to_le_bytes())?;
            writer.write_all(page_bytes)?;
            checksum_buf.extend_from_slice(&page_id.to_le_bytes());
            checksum_buf.extend_from_slice(&(page_bytes.len() as u32).to_le_bytes());
            checksum_buf.extend_from_slice(page_bytes);
        }
        writer.flush()?;
        let pages_checksum = crc32c::crc32c(&checksum_buf);

        // 创建元数据
        let manifest = BackupManifest {
            backup_id: backup_id.to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            start_lsn: 0,
            end_lsn,
            page_count: pages.len() as u32,
            pages_checksum,
        };

        // 写入元数据
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        fs::write(self.manifest_path(backup_id), manifest_json)?;

        // 创建 WAL 归档子目录
        fs::create_dir_all(self.wal_archive_dir(backup_id))?;

        tracing::debug!(
            page_count = manifest.page_count,
            pages_checksum = manifest.pages_checksum,
            end_lsn = manifest.end_lsn,
            "full backup created"
        );
        Ok(manifest)
    }

    // -----------------------------------------------------------------
    //  恢复全量备份
    // -----------------------------------------------------------------

    /// 从全量备份恢复页数据
    ///
    /// 读取 `pages.bin` 并校验 CRC32C 校验和。
    ///
    /// # 返回
    /// `(pages, manifest)` — 页数据列表 + 元数据
    #[tracing::instrument(skip(self))]
    pub fn restore_full_backup(
        &self,
        backup_id: &str,
    ) -> Result<(Pages, BackupManifest), BackupError> {
        let manifest_path = self.manifest_path(backup_id);
        if !manifest_path.exists() {
            tracing::warn!(backup_id, "restore_full_backup: backup not found");
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }

        // 读取元数据
        let manifest_str = fs::read_to_string(&manifest_path)?;
        let manifest: BackupManifest = serde_json::from_str(&manifest_str)?;

        // 读取页数据
        let pages_path = self.pages_path(backup_id);
        let mut reader = BufReader::new(File::open(&pages_path)?);
        let mut pages = Vec::with_capacity(manifest.page_count as usize);
        let mut checksum_buf = Vec::new();

        for _ in 0..manifest.page_count {
            let mut id_buf = [0u8; 4];
            reader.read_exact(&mut id_buf)?;
            let page_id = u32::from_le_bytes(id_buf);

            let mut len_buf = [0u8; 4];
            reader.read_exact(&mut len_buf)?;
            let page_len = u32::from_le_bytes(len_buf) as usize;

            let mut page_bytes = vec![0u8; page_len];
            reader.read_exact(&mut page_bytes)?;

            checksum_buf.extend_from_slice(&id_buf);
            checksum_buf.extend_from_slice(&len_buf);
            checksum_buf.extend_from_slice(&page_bytes);

            pages.push((page_id, page_bytes));
        }

        // 校验 checksum
        let actual_checksum = crc32c::crc32c(&checksum_buf);
        if actual_checksum != manifest.pages_checksum {
            tracing::warn!(
                backup_id,
                expected = manifest.pages_checksum,
                actual = actual_checksum,
                "restore_full_backup: pages checksum mismatch"
            );
            return Err(BackupError::ChecksumMismatch {
                expected: manifest.pages_checksum,
                actual: actual_checksum,
            });
        }

        tracing::debug!(
            backup_id,
            page_count = manifest.page_count,
            "full backup restored"
        );
        Ok((pages, manifest))
    }

    /// 获取备份元数据（不读取页数据）
    pub fn get_manifest(&self, backup_id: &str) -> Result<BackupManifest, BackupError> {
        let manifest_path = self.manifest_path(backup_id);
        if !manifest_path.exists() {
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }
        let manifest_str = fs::read_to_string(&manifest_path)?;
        let manifest: BackupManifest = serde_json::from_str(&manifest_str)?;
        Ok(manifest)
    }

    /// 列出所有备份 ID（按字母序）
    pub fn list_backups(&self) -> Result<Vec<String>, BackupError> {
        let mut backups = Vec::new();
        if !self.backup_dir.exists() {
            return Ok(backups);
        }
        for entry in fs::read_dir(&self.backup_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if self.manifest_path(name).exists() {
                        backups.push(name.to_string());
                    }
                }
            }
        }
        backups.sort();
        Ok(backups)
    }

    /// 删除备份（含 WAL 归档）
    pub fn delete_backup(&self, backup_id: &str) -> Result<(), BackupError> {
        let backup_path = self.backup_path(backup_id);
        if !backup_path.exists() {
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }
        fs::remove_dir_all(&backup_path)?;
        Ok(())
    }

    // -----------------------------------------------------------------
    //  WAL 归档
    // -----------------------------------------------------------------

    /// 归档 WAL 记录
    ///
    /// 将 WAL 记录序列化到 `<archive_id>.wal`，并生成 `<archive_id>.meta.json`。
    ///
    /// # 参数
    /// - `backup_id` — 所属备份 ID
    /// - `archive_id` — 归档 ID
    /// - `records` — WAL 记录列表
    ///
    /// # 返回
    /// WAL 归档元数据 `WalArchiveMeta`
    pub fn archive_wal(
        &self,
        backup_id: &str,
        archive_id: &str,
        records: &[WalRecord],
    ) -> Result<WalArchiveMeta, BackupError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.archive_wal_with_timestamp(backup_id, archive_id, records, now)
    }

    /// 归档 WAL 记录（指定创建时间戳，用于 PITR 测试）
    ///
    /// 与 `archive_wal` 相同，但允许显式指定 `created_at` 时间戳，
    /// 便于 PITR 时间点恢复测试构造特定时间序列。
    #[tracing::instrument(skip(self, records))]
    pub fn archive_wal_with_timestamp(
        &self,
        backup_id: &str,
        archive_id: &str,
        records: &[WalRecord],
        created_at: u64,
    ) -> Result<WalArchiveMeta, BackupError> {
        if archive_id.is_empty() {
            tracing::warn!("archive_wal_with_timestamp called with empty archive_id");
            return Err(BackupError::EmptyArchiveId);
        }

        let archive_dir = self.wal_archive_dir(backup_id);
        if !archive_dir.exists() {
            tracing::warn!(backup_id, "archive_wal: backup dir not found");
            return Err(BackupError::BackupNotFound(backup_id.to_string()));
        }

        let archive_path = self.wal_archive_path(backup_id, archive_id);
        let meta_path = self.wal_archive_meta_path(backup_id, archive_id);

        // 写入 WAL 记录
        let mut writer = BufWriter::new(File::create(&archive_path)?);
        let mut checksum_buf = Vec::new();
        let mut start_lsn = u64::MAX;
        let mut end_lsn = 0u64;

        for record in records {
            let encoded = record.encode();
            writer.write_all(&encoded)?;
            checksum_buf.extend_from_slice(&encoded);
            if record.lsn < start_lsn {
                start_lsn = record.lsn;
            }
            if record.lsn > end_lsn {
                end_lsn = record.lsn;
            }
        }
        writer.flush()?;

        let checksum = crc32c::crc32c(&checksum_buf);
        let start_lsn = if records.is_empty() {
            0
        } else {
            start_lsn
        };

        let meta = WalArchiveMeta {
            archive_id: archive_id.to_string(),
            start_lsn,
            end_lsn,
            record_count: records.len() as u64,
            checksum,
            created_at,
        };

        // 写入元数据
        let meta_json = serde_json::to_string_pretty(&meta)?;
        fs::write(&meta_path, meta_json)?;

        tracing::debug!(
            backup_id,
            archive_id,
            record_count = meta.record_count,
            start_lsn = meta.start_lsn,
            end_lsn = meta.end_lsn,
            "WAL archive written"
        );
        Ok(meta)
    }

    /// 读取 WAL 归档
    ///
    /// 读取 `<archive_id>.wal` 并校验 CRC32C 校验和。
    ///
    /// # 返回
    /// `(records, meta)` — WAL 记录列表 + 元数据
    #[tracing::instrument(skip(self))]
    pub fn read_wal_archive(
        &self,
        backup_id: &str,
        archive_id: &str,
    ) -> Result<(Vec<WalRecord>, WalArchiveMeta), BackupError> {
        let archive_path = self.wal_archive_path(backup_id, archive_id);
        let meta_path = self.wal_archive_meta_path(backup_id, archive_id);

        if !archive_path.exists() || !meta_path.exists() {
            tracing::warn!(backup_id, archive_id, "read_wal_archive: archive not found");
            return Err(BackupError::ArchiveNotFound(archive_id.to_string()));
        }

        // 读取元数据
        let meta_str = fs::read_to_string(&meta_path)?;
        let meta: WalArchiveMeta = serde_json::from_str(&meta_str)?;

        // 读取记录
        let mut reader = BufReader::new(File::open(&archive_path)?);
        let mut records = Vec::with_capacity(meta.record_count as usize);
        let mut checksum_buf = Vec::new();

        for _ in 0..meta.record_count {
            // 读取 header（21 字节）
            let mut header = [0u8; WAL_HEADER_SIZE];
            reader.read_exact(&mut header)?;

            // 解析 data_len
            let data_len = u32::from_le_bytes(header[17..21].try_into().unwrap()) as usize;

            // 读取 data + checksum
            let mut tail = vec![0u8; data_len + WAL_TRAILER_SIZE];
            reader.read_exact(&mut tail)?;

            // 拼接完整 record 并解码
            let mut full = Vec::with_capacity(WAL_HEADER_SIZE + tail.len());
            full.extend_from_slice(&header);
            full.extend_from_slice(&tail);

            let record = WalRecord::decode(&full)?;
            records.push(record);

            checksum_buf.extend_from_slice(&full);
        }

        // 校验 checksum
        let actual_checksum = crc32c::crc32c(&checksum_buf);
        if actual_checksum != meta.checksum {
            tracing::warn!(
                backup_id,
                archive_id,
                expected = meta.checksum,
                actual = actual_checksum,
                "read_wal_archive: WAL checksum mismatch"
            );
            return Err(BackupError::ChecksumMismatch {
                expected: meta.checksum,
                actual: actual_checksum,
            });
        }

        tracing::debug!(
            backup_id,
            archive_id,
            record_count = meta.record_count,
            "WAL archive read"
        );
        Ok((records, meta))
    }

    /// 列出某备份下的所有 WAL 归档 ID（按字母序）
    pub fn list_wal_archives(&self, backup_id: &str) -> Result<Vec<String>, BackupError> {
        let archive_dir = self.wal_archive_dir(backup_id);
        if !archive_dir.exists() {
            return Ok(Vec::new());
        }
        let mut archives = Vec::new();
        for entry in fs::read_dir(&archive_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wal") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    archives.push(stem.to_string());
                }
            }
        }
        archives.sort();
        Ok(archives)
    }

    // -----------------------------------------------------------------
    //  WAL 回放
    // -----------------------------------------------------------------

    /// 在页数据上回放 WAL 记录
    ///
    /// 物理日志语义：
    /// - `Insert` / `Update` / `Delete` / `FullPageImage`：`data` = 页后镜像，替换整页
    /// - `Commit` / `Abort` / `Checkpoint`：跳过
    ///
    /// # 参数
    /// - `pages` — 可变的页数据列表（回放会修改/新增页）
    /// - `records` — WAL 记录列表（按 LSN 升序）
    ///
    /// # 返回
    /// 回放统计 `ReplayStats`
    #[tracing::instrument(skip(self, pages, records))]
    pub fn replay_wal(
        &self,
        pages: &mut Vec<(u32, Vec<u8>)>,
        records: &[WalRecord],
    ) -> Result<ReplayStats, BackupError> {
        let mut stats = ReplayStats {
            records_total: records.len(),
            records_applied: 0,
            records_skipped: 0,
            pages_updated: 0,
            pages_created: 0,
        };

        // 构建 page_id → index 的哈希索引
        let mut page_index: HashMap<u32, usize> = HashMap::new();
        for (i, (page_id, _)) in pages.iter().enumerate() {
            page_index.insert(*page_id, i);
        }

        for record in records {
            tracing::trace!(lsn = record.lsn, page_id = record.page_id, "replaying WAL record");
            match record.op_type {
                WalOpType::Insert
                | WalOpType::Update
                | WalOpType::Delete
                | WalOpType::FullPageImage
                | WalOpType::TableData => {
                    if let Some(&idx) = page_index.get(&record.page_id) {
                        // 更新已有页
                        pages[idx].1 = record.data.clone();
                        stats.pages_updated += 1;
                    } else {
                        // 新建页
                        page_index.insert(record.page_id, pages.len());
                        pages.push((record.page_id, record.data.clone()));
                        stats.pages_created += 1;
                    }
                    stats.records_applied += 1;
                }
                WalOpType::Commit | WalOpType::Abort | WalOpType::Checkpoint => {
                    stats.records_skipped += 1;
                }
            }
        }

        tracing::debug!(
            records_total = stats.records_total,
            records_applied = stats.records_applied,
            records_skipped = stats.records_skipped,
            pages_updated = stats.pages_updated,
            pages_created = stats.pages_created,
            "WAL replay completed"
        );
        Ok(stats)
    }

    // -----------------------------------------------------------------
    //  一键恢复（全量备份 + 所有 WAL 归档回放）
    // -----------------------------------------------------------------

    /// 一键恢复：全量备份 + 按序回放所有 WAL 归档
    ///
    /// 1. 从全量备份恢复页数据
    /// 2. 按字母序读取所有 WAL 归档
    /// 3. 逐个归档回放 WAL 记录
    ///
    /// # 返回
    /// `(pages, manifest, replay_stats_list)` — 最终页数据 + 备份元数据 + 每个归档的回放统计
    #[tracing::instrument(skip(self))]
    pub fn restore_with_wal(
        &self,
        backup_id: &str,
    ) -> Result<(Pages, BackupManifest, Vec<ReplayStats>), BackupError> {
        // 1. 恢复全量备份
        let (mut pages, manifest) = self.restore_full_backup(backup_id)?;

        // 2. 列出并排序 WAL 归档
        let mut archives = self.list_wal_archives(backup_id)?;
        archives.sort();

        // 3. 逐个归档回放
        let mut all_stats = Vec::new();
        for archive_id in &archives {
            let (records, _) = self.read_wal_archive(backup_id, archive_id)?;
            let stats = self.replay_wal(&mut pages, &records)?;
            all_stats.push(stats);
        }

        Ok((pages, manifest, all_stats))
    }

    // =================================================================
    //  Phase 7a.5：PITR 时间点恢复
    //
    //  设计：
    //  - 基于 WAL 归档的 created_at 时间戳筛选
    //  - 恢复到指定时间戳：全量恢复 → 按字母序回放 created_at <= target 的归档
    //  - 秒级精度：created_at 为 Unix epoch 秒
    // =================================================================

    /// PITR 时间点恢复
    ///
    /// 恢复全量备份，然后按字母序回放所有 `created_at <= target_timestamp` 的 WAL 归档。
    ///
    /// # 参数
    /// - `backup_id` — 全量备份 ID
    /// - `target_timestamp` — 目标时间点（Unix epoch 秒）
    ///
    /// # 返回
    /// `(pages, manifest, replay_stats_list, applied_archive_ids)` — 最终页数据 + 备份元数据 + 每个归档的回放统计 + 已应用的归档 ID 列表
    pub fn restore_to_timestamp(
        &self,
        backup_id: &str,
        target_timestamp: u64,
    ) -> Result<(Pages, BackupManifest, Vec<ReplayStats>, Vec<String>), BackupError> {
        // 1. 恢复全量备份
        let (mut pages, manifest) = self.restore_full_backup(backup_id)?;

        // 2. 列出并排序 WAL 归档（按字母序，确保 LSN 单调递增）
        let mut archives = self.list_wal_archives(backup_id)?;
        archives.sort();

        // 3. 逐个归档回放（仅 created_at <= target_timestamp）
        let mut all_stats = Vec::new();
        let mut applied = Vec::new();
        for archive_id in &archives {
            let (records, meta) = self.read_wal_archive(backup_id, archive_id)?;
            if meta.created_at > target_timestamp {
                continue; // 跳过目标时间点之后的归档
            }
            let stats = self.replay_wal(&mut pages, &records)?;
            all_stats.push(stats);
            applied.push(archive_id.clone());
        }

        Ok((pages, manifest, all_stats, applied))
    }

    /// 列出所有 WAL 归档的元数据（按 archive_id 字母序）
    ///
    /// 用于 PITR 查看可用的时间点。
    pub fn list_wal_archive_metas(
        &self,
        backup_id: &str,
    ) -> Result<Vec<(String, WalArchiveMeta)>, BackupError> {
        let mut archives = self.list_wal_archives(backup_id)?;
        archives.sort();
        let mut metas = Vec::with_capacity(archives.len());
        for archive_id in &archives {
            let (_, meta) = self.read_wal_archive(backup_id, archive_id)?;
            metas.push((archive_id.clone(), meta));
        }
        Ok(metas)
    }

    // =================================================================
    //  Phase 7a.4：差异备份（签名对比）
    //
    //  设计：
    //  - 页签名 = 页字节流的 CRC32C（4 字节）
    //  - 签名清单（SignatureManifest）= 全量备份时刻所有页的签名快照
    //  - 差异备份（DiffBackup）= 比较当前页签名与签名清单，仅备份签名变更的页
    //  - 恢复 = 全量恢复 → 按字母序应用差异备份（按 page_id 替换）
    // =================================================================

    fn signatures_path(&self, backup_id: &str) -> PathBuf {
        self.backup_path(backup_id).join("signatures.json")
    }

    fn diff_dir(&self, backup_id: &str, diff_id: &str) -> PathBuf {
        self.backup_path(backup_id).join("diffs").join(diff_id)
    }

    fn diff_manifest_path(&self, backup_id: &str, diff_id: &str) -> PathBuf {
        self.diff_dir(backup_id, diff_id).join("diff.json")
    }

    fn diff_pages_path(&self, backup_id: &str, diff_id: &str) -> PathBuf {
        self.diff_dir(backup_id, diff_id).join("pages.bin")
    }

    /// 计算单页签名（CRC32C）
    pub fn page_signature(page_bytes: &[u8]) -> u32 {
        crc32c::crc32c(page_bytes)
    }

    /// 生成页数据列表的签名清单
    pub fn build_signature_manifest(pages: &Pages) -> Vec<(u32, u32)> {
        let mut sigs: Vec<(u32, u32)> = pages
            .iter()
            .map(|(page_id, page_bytes)| (*page_id, Self::page_signature(page_bytes)))
            .collect();
        sigs.sort_by_key(|(pid, _)| *pid);
        sigs
    }

    /// 创建全量备份时同步生成签名清单
    ///
    /// 在 `create_full_backup` 之后调用，将页签名快照写入 `signatures.json`。
    /// 后续 `create_diff_backup` 将以此清单为基准检测变更页。
    pub fn create_signature_manifest(
        &self,
        backup_id: &str,
        pages: &Pages,
    ) -> Result<Vec<(u32, u32)>, BackupError> {
        let sigs = Self::build_signature_manifest(pages);
        let json = serde_json::to_string_pretty(&sigs)?;
        fs::write(self.signatures_path(backup_id), json)?;
        Ok(sigs)
    }

    /// 读取签名清单
    pub fn load_signature_manifest(&self, backup_id: &str) -> Result<Vec<(u32, u32)>, BackupError> {
        let path = self.signatures_path(backup_id);
        if !path.exists() {
            return Err(BackupError::BackupNotFound(format!(
                "signatures.json not found for backup {}",
                backup_id
            )));
        }
        let json = fs::read_to_string(&path)?;
        let sigs: Vec<(u32, u32)> = serde_json::from_str(&json)?;
        Ok(sigs)
    }

    /// 创建差异备份
    ///
    /// 比较当前页签名与签名清单，仅备份签名变更的页。
    ///
    /// # 参数
    /// - `backup_id` — 基准全量备份 ID（必须已存在签名清单）
    /// - `diff_id` — 差异备份 ID
    /// - `current_pages` — 当前页数据列表
    ///
    /// # 返回
    /// 差异备份元数据 `DiffBackupMeta`
    pub fn create_diff_backup(
        &self,
        backup_id: &str,
        diff_id: &str,
        current_pages: &Pages,
    ) -> Result<DiffBackupMeta, BackupError> {
        if diff_id.is_empty() {
            return Err(BackupError::EmptyBackupId);
        }

        // 1. 加载基准签名清单
        let base_sigs = self.load_signature_manifest(backup_id)?;
        let base_sig_map: HashMap<u32, u32> = base_sigs.iter().copied().collect();

        // 2. 检测变更页
        let mut changed_pages: Vec<(u32, Vec<u8>)> = Vec::new();
        for (page_id, page_bytes) in current_pages {
            let current_sig = Self::page_signature(page_bytes);
            let is_changed = match base_sig_map.get(page_id) {
                Some(&base_sig) => base_sig != current_sig,
                None => true, // 新页
            };
            if is_changed {
                changed_pages.push((*page_id, page_bytes.clone()));
            }
        }

        // 3. 写入变更页数据
        let diff_dir = self.diff_dir(backup_id, diff_id);
        fs::create_dir_all(&diff_dir)?;

        let mut checksum_buf = Vec::new();
        {
            let mut writer =
                BufWriter::new(File::create(self.diff_pages_path(backup_id, diff_id))?);
            for (page_id, page_bytes) in &changed_pages {
                writer.write_all(&page_id.to_le_bytes())?;
                writer.write_all(&(page_bytes.len() as u32).to_le_bytes())?;
                writer.write_all(page_bytes)?;
                checksum_buf.extend_from_slice(&page_id.to_le_bytes());
                checksum_buf.extend_from_slice(&(page_bytes.len() as u32).to_le_bytes());
                checksum_buf.extend_from_slice(page_bytes);
            }
            writer.flush()?;
        }
        let pages_checksum = crc32c::crc32c(&checksum_buf);

        // 4. 写入元数据
        let meta = DiffBackupMeta {
            diff_id: diff_id.to_string(),
            backup_id: backup_id.to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            changed_page_count: changed_pages.len() as u32,
            pages_checksum,
        };
        let json = serde_json::to_string_pretty(&meta)?;
        fs::write(self.diff_manifest_path(backup_id, diff_id), json)?;

        Ok(meta)
    }

    /// 读取差异备份
    ///
    /// # 返回
    /// `(changed_pages, meta)` — 变更页数据 + 元数据
    pub fn read_diff_backup(
        &self,
        backup_id: &str,
        diff_id: &str,
    ) -> Result<(Pages, DiffBackupMeta), BackupError> {
        let meta_path = self.diff_manifest_path(backup_id, diff_id);
        if !meta_path.exists() {
            return Err(BackupError::BackupNotFound(format!(
                "diff {} not found for backup {}",
                diff_id, backup_id
            )));
        }

        let json = fs::read_to_string(&meta_path)?;
        let meta: DiffBackupMeta = serde_json::from_str(&json)?;

        let mut reader = BufReader::new(File::open(self.diff_pages_path(backup_id, diff_id))?);
        let mut pages: Pages = Vec::with_capacity(meta.changed_page_count as usize);
        let mut checksum_buf = Vec::new();

        for _ in 0..meta.changed_page_count {
            let mut id_buf = [0u8; 4];
            reader.read_exact(&mut id_buf)?;
            let page_id = u32::from_le_bytes(id_buf);

            let mut len_buf = [0u8; 4];
            reader.read_exact(&mut len_buf)?;
            let page_len = u32::from_le_bytes(len_buf) as usize;

            let mut page_bytes = vec![0u8; page_len];
            reader.read_exact(&mut page_bytes)?;

            checksum_buf.extend_from_slice(&id_buf);
            checksum_buf.extend_from_slice(&len_buf);
            checksum_buf.extend_from_slice(&page_bytes);

            pages.push((page_id, page_bytes));
        }

        let actual_checksum = crc32c::crc32c(&checksum_buf);
        if actual_checksum != meta.pages_checksum {
            return Err(BackupError::ChecksumMismatch {
                expected: meta.pages_checksum,
                actual: actual_checksum,
            });
        }

        Ok((pages, meta))
    }

    /// 列出所有差异备份 ID（按字母序）
    pub fn list_diff_backups(&self, backup_id: &str) -> Result<Vec<String>, BackupError> {
        let mut diffs = Vec::new();
        let diffs_root = self.backup_path(backup_id).join("diffs");
        if !diffs_root.exists() {
            return Ok(diffs);
        }
        for entry in fs::read_dir(&diffs_root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if self.diff_manifest_path(backup_id, name).exists() {
                        diffs.push(name.to_string());
                    }
                }
            }
        }
        diffs.sort();
        Ok(diffs)
    }

    /// 删除差异备份
    pub fn delete_diff_backup(&self, backup_id: &str, diff_id: &str) -> Result<(), BackupError> {
        let diff_dir = self.diff_dir(backup_id, diff_id);
        if !diff_dir.exists() {
            return Err(BackupError::BackupNotFound(format!(
                "diff {} not found for backup {}",
                diff_id, backup_id
            )));
        }
        fs::remove_dir_all(&diff_dir)?;
        Ok(())
    }

    /// 应用差异备份到页数据（按 page_id 替换）
    ///
    /// 返回被替换的页数。
    pub fn apply_diff(pages: &mut Pages, diff_pages: &Pages) -> usize {
        let mut page_index: HashMap<u32, usize> = HashMap::new();
        for (i, (page_id, _)) in pages.iter().enumerate() {
            page_index.insert(*page_id, i);
        }

        let mut replaced = 0;
        for (page_id, page_bytes) in diff_pages {
            if let Some(&idx) = page_index.get(page_id) {
                pages[idx].1 = page_bytes.clone();
                replaced += 1;
            } else {
                page_index.insert(*page_id, pages.len());
                pages.push((*page_id, page_bytes.clone()));
            }
        }
        replaced
    }

    /// 全量恢复 + 按字母序应用所有差异备份
    ///
    /// # 返回
    /// `(final_pages, full_manifest, applied_diff_ids)` — 最终页数据 + 全量备份元数据 + 已应用的差异备份 ID 列表
    pub fn restore_full_with_diffs(
        &self,
        backup_id: &str,
    ) -> Result<(Pages, BackupManifest, Vec<String>), BackupError> {
        // 1. 全量恢复
        let (mut pages, manifest) = self.restore_full_backup(backup_id)?;

        // 2. 列出差异备份（按字母序）
        let mut diffs = self.list_diff_backups(backup_id)?;
        diffs.sort();

        // 3. 逐个应用差异备份
        let mut applied = Vec::new();
        for diff_id in &diffs {
            let (diff_pages, _) = self.read_diff_backup(backup_id, diff_id)?;
            Self::apply_diff(&mut pages, &diff_pages);
            applied.push(diff_id.clone());
        }

        Ok((pages, manifest, applied))
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use szrsql_tx::wal::{WalOpType, WalRecord};

    // -----------------------------------------------------------------
    //  测试辅助函数
    // -----------------------------------------------------------------

    /// 创建唯一的测试目录（基于时间戳 + 进程 ID）
    fn create_test_dir(test_name: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "szrsql_backup_{}_{}_{}",
            test_name,
            std::process::id(),
            ts
        ));
        if dir.exists() {
            let _ = fs::remove_dir_all(&dir);
        }
        dir
    }

    /// 清理测试目录
    fn cleanup_test_dir(dir: &Path) {
        if dir.exists() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// 生成测试页数据
    fn make_test_pages(count: u32, page_size: usize) -> Vec<(u32, Vec<u8>)> {
        (0..count)
            .map(|i| {
                let bytes = vec![(i as u8).wrapping_mul(0xAA); page_size];
                (i, bytes)
            })
            .collect()
    }

    // -----------------------------------------------------------------
    //  BackupManager 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_new_creates_backup_dir() {
        let dir = create_test_dir("new_creates");
        assert!(!dir.exists());
        let _mgr = BackupManager::new(&dir).unwrap();
        assert!(dir.exists());
        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_new_with_existing_dir() {
        let dir = create_test_dir("new_existing");
        fs::create_dir_all(&dir).unwrap();
        let _mgr = BackupManager::new(&dir).unwrap();
        assert!(dir.exists());
        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  全量备份 + 恢复测试
    // -----------------------------------------------------------------

    #[test]
    fn test_create_and_restore_full_backup() {
        let dir = create_test_dir("create_restore");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(10, 8192);
        let manifest = mgr.create_full_backup("bk1", &pages, 100).unwrap();

        assert_eq!(manifest.backup_id, "bk1");
        assert_eq!(manifest.page_count, 10);
        assert_eq!(manifest.end_lsn, 100);
        assert!(manifest.pages_checksum != 0);

        let (restored, restored_manifest) = mgr.restore_full_backup("bk1").unwrap();
        assert_eq!(restored.len(), 10);
        assert_eq!(restored, pages);
        assert_eq!(restored_manifest, manifest);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_create_backup_empty_pages() {
        let dir = create_test_dir("empty_pages");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Vec<(u32, Vec<u8>)> = vec![];
        let manifest = mgr.create_full_backup("bk1", &pages, 50).unwrap();
        assert_eq!(manifest.page_count, 0);
        assert_eq!(manifest.pages_checksum, crc32c::crc32c(&[]));

        let (restored, _) = mgr.restore_full_backup("bk1").unwrap();
        assert!(restored.is_empty());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_create_backup_empty_id_error() {
        let dir = create_test_dir("empty_id");
        let mgr = BackupManager::new(&dir).unwrap();

        let result = mgr.create_full_backup("", &make_test_pages(1, 8192), 0);
        assert!(matches!(result, Err(BackupError::EmptyBackupId)));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_restore_nonexistent_backup() {
        let dir = create_test_dir("restore_nonexistent");
        let mgr = BackupManager::new(&dir).unwrap();

        let result = mgr.restore_full_backup("nonexistent");
        assert!(matches!(result, Err(BackupError::BackupNotFound(_))));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_restore_checksum_verification() {
        let dir = create_test_dir("checksum_verify");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(5, 4096);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();

        // 篡改 pages.bin
        let pages_path = mgr.pages_path("bk1");
        let mut content = fs::read(&pages_path).unwrap();
        // 修改最后一个字节
        let last = content.len() - 1;
        content[last] ^= 0xFF;
        fs::write(&pages_path, content).unwrap();

        let result = mgr.restore_full_backup("bk1");
        assert!(matches!(result, Err(BackupError::ChecksumMismatch { .. })));

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  元数据 + 列表 + 删除测试
    // -----------------------------------------------------------------

    #[test]
    fn test_get_manifest() {
        let dir = create_test_dir("get_manifest");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(3, 8192);
        let manifest = mgr.create_full_backup("bk1", &pages, 42).unwrap();

        let fetched = mgr.get_manifest("bk1").unwrap();
        assert_eq!(fetched, manifest);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_list_backups() {
        let dir = create_test_dir("list_backups");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk3", &make_test_pages(1, 8192), 0)
            .unwrap();
        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();
        mgr.create_full_backup("bk2", &make_test_pages(1, 8192), 0)
            .unwrap();

        let list = mgr.list_backups().unwrap();
        assert_eq!(list, vec!["bk1", "bk2", "bk3"]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_delete_backup() {
        let dir = create_test_dir("delete_backup");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();
        assert!(mgr.backup_path("bk1").exists());

        mgr.delete_backup("bk1").unwrap();
        assert!(!mgr.backup_path("bk1").exists());

        let result = mgr.delete_backup("bk1");
        assert!(matches!(result, Err(BackupError::BackupNotFound(_))));

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  WAL 归档测试
    // -----------------------------------------------------------------

    #[test]
    fn test_archive_and_read_wal() {
        let dir = create_test_dir("archive_read_wal");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 10)
            .unwrap();

        let records = vec![
            WalRecord::new(11, 1, WalOpType::Insert, 0, vec![0xAA; 8192]),
            WalRecord::new(12, 1, WalOpType::Update, 0, vec![0xBB; 8192]),
            WalRecord::new(13, 1, WalOpType::Commit, 0, vec![]),
        ];

        let meta = mgr.archive_wal("bk1", "ar1", &records).unwrap();
        assert_eq!(meta.archive_id, "ar1");
        assert_eq!(meta.start_lsn, 11);
        assert_eq!(meta.end_lsn, 13);
        assert_eq!(meta.record_count, 3);
        assert!(meta.checksum != 0);

        let (read_records, read_meta) = mgr.read_wal_archive("bk1", "ar1").unwrap();
        assert_eq!(read_records.len(), 3);
        assert_eq!(read_records[0].lsn, 11);
        assert_eq!(read_records[1].lsn, 12);
        assert_eq!(read_records[2].lsn, 13);
        assert_eq!(read_records[0].op_type, WalOpType::Insert);
        assert_eq!(read_records[1].op_type, WalOpType::Update);
        assert_eq!(read_records[2].op_type, WalOpType::Commit);
        assert_eq!(read_meta, meta);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_archive_empty_records() {
        let dir = create_test_dir("archive_empty");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();

        let records: Vec<WalRecord> = vec![];
        let meta = mgr.archive_wal("bk1", "ar1", &records).unwrap();
        assert_eq!(meta.record_count, 0);
        assert_eq!(meta.start_lsn, 0);
        assert_eq!(meta.end_lsn, 0);
        assert_eq!(meta.checksum, crc32c::crc32c(&[]));

        let (read_records, _) = mgr.read_wal_archive("bk1", "ar1").unwrap();
        assert!(read_records.is_empty());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_archive_empty_id_error() {
        let dir = create_test_dir("archive_empty_id");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();

        let result = mgr.archive_wal("bk1", "", &[]);
        assert!(matches!(result, Err(BackupError::EmptyArchiveId)));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_archive_nonexistent_backup() {
        let dir = create_test_dir("archive_nonexistent");
        let mgr = BackupManager::new(&dir).unwrap();

        let result = mgr.archive_wal("nonexistent", "ar1", &[]);
        assert!(matches!(result, Err(BackupError::BackupNotFound(_))));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_read_nonexistent_archive() {
        let dir = create_test_dir("read_nonexistent_archive");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();

        let result = mgr.read_wal_archive("bk1", "nonexistent");
        assert!(matches!(result, Err(BackupError::ArchiveNotFound(_))));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_list_wal_archives() {
        let dir = create_test_dir("list_archives");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();

        let records = vec![WalRecord::new(1, 1, WalOpType::Insert, 0, vec![0xAA; 8192])];
        mgr.archive_wal("bk1", "ar3", &records).unwrap();
        mgr.archive_wal("bk1", "ar1", &records).unwrap();
        mgr.archive_wal("bk1", "ar2", &records).unwrap();

        let list = mgr.list_wal_archives("bk1").unwrap();
        assert_eq!(list, vec!["ar1", "ar2", "ar3"]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_wal_archive_checksum_verification() {
        let dir = create_test_dir("wal_checksum");
        let mgr = BackupManager::new(&dir).unwrap();

        mgr.create_full_backup("bk1", &make_test_pages(1, 8192), 0)
            .unwrap();

        let records = vec![WalRecord::new(1, 1, WalOpType::Insert, 0, vec![0xAA; 8192])];
        mgr.archive_wal("bk1", "ar1", &records).unwrap();

        // 篡改 WAL 文件
        let wal_path = mgr.wal_archive_path("bk1", "ar1");
        let mut content = fs::read(&wal_path).unwrap();
        content[0] ^= 0xFF;
        fs::write(&wal_path, content).unwrap();

        let result = mgr.read_wal_archive("bk1", "ar1");
        assert!(matches!(result, Err(BackupError::ChecksumMismatch { .. })));

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  WAL 回放测试
    // -----------------------------------------------------------------

    #[test]
    fn test_replay_wal_update_existing_page() {
        let dir = create_test_dir("replay_update");
        let mgr = BackupManager::new(&dir).unwrap();

        let mut pages = vec![(0u32, vec![0x11; 4096])];
        let records = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0x22; 4096])];

        let stats = mgr.replay_wal(&mut pages, &records).unwrap();

        assert_eq!(stats.records_total, 1);
        assert_eq!(stats.records_applied, 1);
        assert_eq!(stats.records_skipped, 0);
        assert_eq!(stats.pages_updated, 1);
        assert_eq!(stats.pages_created, 0);
        assert_eq!(pages[0].1, vec![0x22; 4096]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_replay_wal_create_new_page() {
        let dir = create_test_dir("replay_create");
        let mgr = BackupManager::new(&dir).unwrap();

        let mut pages: Vec<(u32, Vec<u8>)> = vec![];
        let records = vec![WalRecord::new(
            1,
            1,
            WalOpType::FullPageImage,
            5,
            vec![0x33; 8192],
        )];

        let stats = mgr.replay_wal(&mut pages, &records).unwrap();

        assert_eq!(stats.records_applied, 1);
        assert_eq!(stats.pages_created, 1);
        assert_eq!(stats.pages_updated, 0);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].0, 5);
        assert_eq!(pages[0].1, vec![0x33; 8192]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_replay_wal_skip_commit_abort_checkpoint() {
        let dir = create_test_dir("replay_skip");
        let mgr = BackupManager::new(&dir).unwrap();

        let mut pages = vec![(0u32, vec![0x11; 4096])];
        let records = vec![
            WalRecord::new(1, 1, WalOpType::Commit, 0, vec![]),
            WalRecord::new(2, 1, WalOpType::Abort, 0, vec![]),
            WalRecord::new(3, 1, WalOpType::Checkpoint, 0, vec![]),
        ];

        let stats = mgr.replay_wal(&mut pages, &records).unwrap();

        assert_eq!(stats.records_total, 3);
        assert_eq!(stats.records_applied, 0);
        assert_eq!(stats.records_skipped, 3);
        assert_eq!(stats.pages_updated, 0);
        assert_eq!(stats.pages_created, 0);
        assert_eq!(pages[0].1, vec![0x11; 4096]); // 未修改

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_replay_wal_mixed_operations() {
        let dir = create_test_dir("replay_mixed");
        let mgr = BackupManager::new(&dir).unwrap();

        let mut pages = vec![(0u32, vec![0x00; 4096])];
        let records = vec![
            // 修改 page 0
            WalRecord::new(1, 1, WalOpType::Insert, 0, vec![0x01; 4096]),
            // Commit（跳过）
            WalRecord::new(2, 1, WalOpType::Commit, 0, vec![]),
            // 新建 page 1
            WalRecord::new(3, 2, WalOpType::FullPageImage, 1, vec![0x02; 4096]),
            // 修改 page 0 再次
            WalRecord::new(4, 2, WalOpType::Update, 0, vec![0x03; 4096]),
            // Checkpoint（跳过）
            WalRecord::new(5, 2, WalOpType::Checkpoint, 0, vec![]),
            // 新建 page 2
            WalRecord::new(6, 3, WalOpType::Delete, 2, vec![0x04; 4096]),
        ];

        let stats = mgr.replay_wal(&mut pages, &records).unwrap();

        assert_eq!(stats.records_total, 6);
        assert_eq!(stats.records_applied, 4);
        assert_eq!(stats.records_skipped, 2);
        assert_eq!(stats.pages_updated, 2); // page 0 被修改 2 次
        assert_eq!(stats.pages_created, 2); // page 1 + page 2

        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].0, 0);
        assert_eq!(pages[0].1, vec![0x03; 4096]); // 最后一次更新
        assert_eq!(pages[1].0, 1);
        assert_eq!(pages[1].1, vec![0x02; 4096]);
        assert_eq!(pages[2].0, 2);
        assert_eq!(pages[2].1, vec![0x04; 4096]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_replay_wal_empty_records() {
        let dir = create_test_dir("replay_empty");
        let mgr = BackupManager::new(&dir).unwrap();

        let mut pages = vec![(0u32, vec![0x11; 4096])];
        let records: Vec<WalRecord> = vec![];

        let stats = mgr.replay_wal(&mut pages, &records).unwrap();

        assert_eq!(stats.records_total, 0);
        assert_eq!(stats.records_applied, 0);
        assert_eq!(pages.len(), 1);

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  一键恢复（restore_with_wal）测试
    // -----------------------------------------------------------------

    #[test]
    fn test_restore_with_wal_single_archive() {
        let dir = create_test_dir("restore_single");
        let mgr = BackupManager::new(&dir).unwrap();

        // 全量备份：1 个 page
        let pages = vec![(0u32, vec![0x00; 4096])];
        mgr.create_full_backup("bk1", &pages, 10).unwrap();

        // WAL 归档：修改 page 0 + 新建 page 1
        let records = vec![
            WalRecord::new(11, 1, WalOpType::Update, 0, vec![0xAA; 4096]),
            WalRecord::new(12, 1, WalOpType::FullPageImage, 1, vec![0xBB; 4096]),
            WalRecord::new(13, 1, WalOpType::Commit, 0, vec![]),
        ];
        mgr.archive_wal("bk1", "ar1", &records).unwrap();

        // 一键恢复
        let (restored, manifest, stats_list) = mgr.restore_with_wal("bk1").unwrap();

        assert_eq!(manifest.end_lsn, 10);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].0, 0);
        assert_eq!(restored[0].1, vec![0xAA; 4096]); // WAL 更新
        assert_eq!(restored[1].0, 1);
        assert_eq!(restored[1].1, vec![0xBB; 4096]); // WAL 新建
        assert_eq!(stats_list.len(), 1);
        assert_eq!(stats_list[0].records_applied, 2);
        assert_eq!(stats_list[0].records_skipped, 1);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_restore_with_wal_multiple_archives() {
        let dir = create_test_dir("restore_multi");
        let mgr = BackupManager::new(&dir).unwrap();

        // 全量备份
        let pages = vec![(0u32, vec![0x00; 4096])];
        mgr.create_full_backup("bk1", &pages, 5).unwrap();

        // 归档 1：修改 page 0
        let records1 = vec![WalRecord::new(6, 1, WalOpType::Update, 0, vec![0x11; 4096])];
        mgr.archive_wal("bk1", "ar1", &records1).unwrap();

        // 归档 2：新建 page 1 + 修改 page 0
        let records2 = vec![
            WalRecord::new(7, 2, WalOpType::FullPageImage, 1, vec![0x22; 4096]),
            WalRecord::new(8, 2, WalOpType::Update, 0, vec![0x33; 4096]),
        ];
        mgr.archive_wal("bk1", "ar2", &records2).unwrap();

        // 一键恢复（按 ar1 → ar2 顺序回放）
        let (restored, _, stats_list) = mgr.restore_with_wal("bk1").unwrap();

        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].0, 0);
        assert_eq!(restored[0].1, vec![0x33; 4096]); // ar2 的最后一次更新
        assert_eq!(restored[1].0, 1);
        assert_eq!(restored[1].1, vec![0x22; 4096]);
        assert_eq!(stats_list.len(), 2);
        assert_eq!(stats_list[0].records_applied, 1); // ar1
        assert_eq!(stats_list[1].records_applied, 2); // ar2

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_restore_with_wal_no_archives() {
        let dir = create_test_dir("restore_no_archives");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(5, 4096);
        mgr.create_full_backup("bk1", &pages, 10).unwrap();

        let (restored, _, stats_list) = mgr.restore_with_wal("bk1").unwrap();

        assert_eq!(restored, pages);
        assert!(stats_list.is_empty());

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  Phase 7a.3 集成测试：全量备份 → INSERT 1000000 行 → WAL 归档 → 删除 → 恢复 → 回放 → 验证
    // -----------------------------------------------------------------

    #[test]
    fn test_7a3_integration_full_backup_wal_archive_restore_replay() {
        let dir = create_test_dir("7a3_integration");
        let mgr = BackupManager::new(&dir).unwrap();

        // --- 参数 ---
        const TOTAL_ROWS: u64 = 1_000_000;
        const ROW_SIZE: usize = 8; // 8 bytes per row (u64 row_id)
        const PAGE_SIZE: usize = 8192; // 8KB per page
        const ROWS_PER_PAGE: usize = PAGE_SIZE / ROW_SIZE; // 1024 rows/page
        const INITIAL_PAGES: u32 = 100;
        const INITIAL_END_LSN: u64 = 1000;

        // 计算需要的总页数
        let total_pages = (TOTAL_ROWS as usize).div_ceil(ROWS_PER_PAGE);
        // 1000000 / 1024 = 976.5625 → 977 pages
        assert_eq!(total_pages, 977);

        // --- 步骤 1：创建初始页（100 个空页）---
        let initial_pages: Vec<(u32, Vec<u8>)> = (0..INITIAL_PAGES)
            .map(|i| (i, vec![0u8; PAGE_SIZE]))
            .collect();

        // --- 步骤 2：全量备份 ---
        let manifest = mgr
            .create_full_backup("bk_7a3", &initial_pages, INITIAL_END_LSN)
            .unwrap();
        assert_eq!(manifest.page_count, INITIAL_PAGES);
        assert_eq!(manifest.end_lsn, INITIAL_END_LSN);

        // --- 步骤 3：模拟 INSERT 1000000 行 ---
        // 生成 977 个页的最终状态（含 1000000 行数据）
        // 每个 row = 8 字节 (row_id as u64 LE)
        let mut final_pages: Vec<(u32, Vec<u8>)> = Vec::with_capacity(total_pages);

        for page_idx in 0..total_pages {
            let page_id = page_idx as u32;
            let mut page_bytes = vec![0u8; PAGE_SIZE];
            let start_row = page_idx * ROWS_PER_PAGE;
            let end_row = std::cmp::min(start_row + ROWS_PER_PAGE, TOTAL_ROWS as usize);

            for row_idx in start_row..end_row {
                let offset = (row_idx - start_row) * ROW_SIZE;
                let row_id = row_idx as u64;
                page_bytes[offset..offset + ROW_SIZE].copy_from_slice(&row_id.to_le_bytes());
            }

            final_pages.push((page_id, page_bytes));
        }

        // 确认数据完整性：最后一页应有 1000000 - 976*1024 = 576 行
        let last_page_rows = TOTAL_ROWS as usize - (total_pages - 1) * ROWS_PER_PAGE;
        assert_eq!(last_page_rows, 576);

        // --- 步骤 4：为每个页生成 WAL 记录（FullPageImage 物理日志）---
        let mut wal_records: Vec<WalRecord> = Vec::with_capacity(total_pages);

        for (lsn, (page_id, page_bytes)) in (INITIAL_END_LSN + 1..).zip(final_pages.iter()) {
            // 每页一条 FullPageImage 记录，data = 完整页内容（后镜像）
            let mut record = WalRecord::new(
                lsn,
                1, // tx_id
                WalOpType::FullPageImage,
                *page_id,
                page_bytes.clone(),
            );
            record.update_checksum();
            wal_records.push(record);
        }

        assert_eq!(wal_records.len(), total_pages); // 977 条 WAL 记录

        // --- 步骤 5：WAL 归档 ---
        let archive_meta = mgr.archive_wal("bk_7a3", "ar_7a3", &wal_records).unwrap();
        assert_eq!(archive_meta.record_count, total_pages as u64);
        assert_eq!(archive_meta.start_lsn, INITIAL_END_LSN + 1);
        assert_eq!(archive_meta.end_lsn, INITIAL_END_LSN + total_pages as u64);

        // --- 步骤 6：删除数据（模拟数据丢失）---
        drop(initial_pages);
        drop(final_pages);
        drop(wal_records);

        // --- 步骤 7：全量恢复 + WAL 回放 ---
        let (restored_pages, restored_manifest, replay_stats_list) =
            mgr.restore_with_wal("bk_7a3").unwrap();

        // 验证恢复的元数据
        assert_eq!(restored_manifest.page_count, INITIAL_PAGES);
        assert_eq!(restored_manifest.end_lsn, INITIAL_END_LSN);

        // 验证回放统计
        assert_eq!(replay_stats_list.len(), 1);
        let stats = &replay_stats_list[0];
        assert_eq!(stats.records_total, total_pages);
        assert_eq!(stats.records_applied, total_pages);
        assert_eq!(stats.records_skipped, 0);
        // 前 100 页是更新（backup 恢复了 100 个空页），后 877 页是新建
        assert_eq!(stats.pages_updated, INITIAL_PAGES as usize);
        assert_eq!(stats.pages_created, total_pages - INITIAL_PAGES as usize);

        // --- 步骤 8：验证数据完全恢复 ---
        // 8.1 页数正确
        assert_eq!(restored_pages.len(), total_pages);

        // 8.2 逐页验证内容
        for (page_idx, (page_id, page_bytes)) in restored_pages.iter().enumerate() {
            assert_eq!(*page_id, page_idx as u32);

            let start_row = page_idx * ROWS_PER_PAGE;
            let end_row = std::cmp::min(start_row + ROWS_PER_PAGE, TOTAL_ROWS as usize);

            // 验证每一行的数据
            for row_idx in start_row..end_row {
                let offset = (row_idx - start_row) * ROW_SIZE;
                let expected_row_id = row_idx as u64;
                let actual_row_id =
                    u64::from_le_bytes(page_bytes[offset..offset + ROW_SIZE].try_into().unwrap());
                assert_eq!(
                    actual_row_id, expected_row_id,
                    "page {} row {}: expected {}, got {}",
                    page_idx, row_idx, expected_row_id, actual_row_id
                );
            }

            // 验证页末尾未使用区域仍为 0
            let used_bytes = (end_row - start_row) * ROW_SIZE;
            for (i, &byte) in page_bytes
                .iter()
                .enumerate()
                .take(PAGE_SIZE)
                .skip(used_bytes)
            {
                assert_eq!(
                    byte, 0,
                    "page {} offset {}: expected 0, got {}",
                    page_idx, i, byte
                );
            }
        }

        // 8.3 验证总行数（按页结构计算，避免 row_id=0 全零被误判为空行）
        let total_rows_verified: u64 = restored_pages
            .iter()
            .enumerate()
            .map(|(page_idx, _)| {
                let start_row = page_idx * ROWS_PER_PAGE;
                let end_row = std::cmp::min(start_row + ROWS_PER_PAGE, TOTAL_ROWS as usize);
                (end_row - start_row) as u64
            })
            .sum();
        assert_eq!(total_rows_verified, TOTAL_ROWS);

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  端到端生命周期测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7a3_e2e_backup_lifecycle() {
        let dir = create_test_dir("7a3_lifecycle");
        let mgr = BackupManager::new(&dir).unwrap();

        // 1. 创建备份
        let pages1 = make_test_pages(3, 4096);
        let m1 = mgr.create_full_backup("bk1", &pages1, 10).unwrap();
        assert_eq!(m1.page_count, 3);

        // 2. 创建第二个备份
        let pages2 = make_test_pages(5, 4096);
        let m2 = mgr.create_full_backup("bk2", &pages2, 20).unwrap();
        assert_eq!(m2.page_count, 5);

        // 3. 列出备份
        let list = mgr.list_backups().unwrap();
        assert_eq!(list, vec!["bk1", "bk2"]);

        // 4. 为 bk1 归档 WAL
        let records = vec![
            WalRecord::new(11, 1, WalOpType::Insert, 10, vec![0xFF; 4096]),
            WalRecord::new(12, 1, WalOpType::Commit, 0, vec![]),
        ];
        mgr.archive_wal("bk1", "ar1", &records).unwrap();

        // 5. 列出 bk1 的归档
        let archives = mgr.list_wal_archives("bk1").unwrap();
        assert_eq!(archives, vec!["ar1"]);

        // 6. 恢复 bk1 + WAL
        let (restored, _, stats) = mgr.restore_with_wal("bk1").unwrap();
        assert_eq!(restored.len(), 4); // 3 原始 + 1 新建 (page 10)
        assert_eq!(stats[0].records_applied, 1);
        assert_eq!(stats[0].records_skipped, 1);

        // 7. 验证 page 10 的内容
        let page10 = restored.iter().find(|(id, _)| *id == 10).unwrap();
        assert_eq!(page10.1, vec![0xFF; 4096]);

        // 8. 删除 bk1
        mgr.delete_backup("bk1").unwrap();
        let list_after = mgr.list_backups().unwrap();
        assert_eq!(list_after, vec!["bk2"]);

        // 9. bk1 的归档也被删除
        let archives_after = mgr.list_wal_archives("bk1").unwrap();
        assert!(archives_after.is_empty());

        cleanup_test_dir(&dir);
    }

    // -----------------------------------------------------------------
    //  边界 + 极端情况测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7a3_large_page_size() {
        let dir = create_test_dir("7a3_large_page");
        let mgr = BackupManager::new(&dir).unwrap();

        // 1MB 页
        let large_page = vec![0x42; 1024 * 1024];
        let pages = vec![(0u32, large_page.clone())];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let (restored, _) = mgr.restore_full_backup("bk1").unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].1, large_page);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a3_many_small_pages() {
        let dir = create_test_dir("7a3_many_small");
        let mgr = BackupManager::new(&dir).unwrap();

        // 1000 个 64 字节小页
        let pages: Vec<(u32, Vec<u8>)> =
            (0..1000).map(|i| (i, vec![(i % 256) as u8; 64])).collect();
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let (restored, _) = mgr.restore_full_backup("bk1").unwrap();
        assert_eq!(restored.len(), 1000);
        assert_eq!(restored, pages);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a3_wal_multiple_operations_same_page() {
        let dir = create_test_dir("7a3_multi_ops_same");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        // 同一页 3 次修改
        let records = vec![
            WalRecord::new(1, 1, WalOpType::Update, 0, vec![0x01; 1024]),
            WalRecord::new(2, 1, WalOpType::Update, 0, vec![0x02; 1024]),
            WalRecord::new(3, 1, WalOpType::Update, 0, vec![0x03; 1024]),
        ];
        mgr.archive_wal("bk1", "ar1", &records).unwrap();

        let (restored, _, stats) = mgr.restore_with_wal("bk1").unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].1, vec![0x03; 1024]); // 最后一次更新生效
        assert_eq!(stats[0].pages_updated, 3);
        assert_eq!(stats[0].pages_created, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a3_overwrite_backup() {
        let dir = create_test_dir("7a3_overwrite");
        let mgr = BackupManager::new(&dir).unwrap();

        // 第一次备份
        let pages1 = make_test_pages(3, 4096);
        mgr.create_full_backup("bk1", &pages1, 10).unwrap();

        // 覆盖备份（同一 backup_id）
        let pages2 = make_test_pages(5, 4096);
        mgr.create_full_backup("bk1", &pages2, 20).unwrap();

        let (restored, manifest) = mgr.restore_full_backup("bk1").unwrap();
        assert_eq!(restored, pages2);
        assert_eq!(manifest.page_count, 5);
        assert_eq!(manifest.end_lsn, 20);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a3_wal_archive_ordering() {
        let dir = create_test_dir("7a3_archive_order");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        // 按非字母序创建归档
        let records_c = vec![WalRecord::new(5, 1, WalOpType::Update, 0, vec![0xCC; 1024])];
        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let records_b = vec![WalRecord::new(3, 1, WalOpType::Update, 0, vec![0xBB; 1024])];

        mgr.archive_wal("bk1", "ar_c", &records_c).unwrap();
        mgr.archive_wal("bk1", "ar_a", &records_a).unwrap();
        mgr.archive_wal("bk1", "ar_b", &records_b).unwrap();

        // 恢复时按字母序回放：ar_a → ar_b → ar_c
        let (restored, _, stats) = mgr.restore_with_wal("bk1").unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].1, vec![0xCC; 1024]); // ar_c 最后回放
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[0].records_applied, 1); // ar_a
        assert_eq!(stats[1].records_applied, 1); // ar_b
        assert_eq!(stats[2].records_applied, 1); // ar_c

        cleanup_test_dir(&dir);
    }

    // =================================================================
    //  Phase 7a.4：差异备份（签名对比）测试
    // =================================================================

    #[test]
    fn test_7a4_page_signature_deterministic() {
        let page_a = vec![0x11; 8192];
        let page_b = vec![0x11; 8192];
        let page_c = vec![0x22; 8192];

        assert_eq!(
            BackupManager::page_signature(&page_a),
            BackupManager::page_signature(&page_b)
        );
        assert_ne!(
            BackupManager::page_signature(&page_a),
            BackupManager::page_signature(&page_c)
        );
    }

    #[test]
    fn test_7a4_build_signature_manifest_sorted() {
        let pages: Pages = vec![
            (2u32, vec![0xAA; 100]),
            (0u32, vec![0xBB; 100]),
            (1u32, vec![0xCC; 100]),
        ];
        let sigs = BackupManager::build_signature_manifest(&pages);
        assert_eq!(sigs.len(), 3);
        assert_eq!(sigs[0].0, 0);
        assert_eq!(sigs[1].0, 1);
        assert_eq!(sigs[2].0, 2);
    }

    #[test]
    fn test_7a4_create_and_load_signature_manifest() {
        let dir = create_test_dir("7a4_sig_manifest");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(5, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();

        let created_sigs = mgr.create_signature_manifest("bk1", &pages).unwrap();
        assert_eq!(created_sigs.len(), 5);

        let loaded_sigs = mgr.load_signature_manifest("bk1").unwrap();
        assert_eq!(loaded_sigs, created_sigs);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_load_signature_manifest_not_found() {
        let dir = create_test_dir("7a4_sig_not_found");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &pages, 50).unwrap();
        // 没有调用 create_signature_manifest
        let result = mgr.load_signature_manifest("bk1");
        assert!(result.is_err());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_zero_changes() {
        // 修改 0 行 → 差异备份 0 数据
        let dir = create_test_dir("7a4_diff_zero");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(10, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &pages).unwrap();

        // 当前页与备份完全相同
        let meta = mgr.create_diff_backup("bk1", "diff1", &pages).unwrap();
        assert_eq!(meta.changed_page_count, 0);
        assert_eq!(meta.pages_checksum, crc32c::crc32c(&[] as &[u8]));

        // 读取验证
        let (diff_pages, loaded_meta) = mgr.read_diff_backup("bk1", "diff1").unwrap();
        assert!(diff_pages.is_empty());
        assert_eq!(loaded_meta.changed_page_count, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_all_pages_changed() {
        let dir = create_test_dir("7a4_diff_all");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(10, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        // 修改所有页
        let changed_pages: Pages = base_pages
            .iter()
            .map(|(pid, _)| (*pid, vec![0xFF; 8192]))
            .collect();

        let meta = mgr
            .create_diff_backup("bk1", "diff1", &changed_pages)
            .unwrap();
        assert_eq!(meta.changed_page_count, 10);

        // 读取验证
        let (diff_pages, _) = mgr.read_diff_backup("bk1", "diff1").unwrap();
        assert_eq!(diff_pages.len(), 10);
        for (_, bytes) in &diff_pages {
            assert_eq!(bytes, &vec![0xFF; 8192]);
        }

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_partial_changes() {
        // 修改部分页：仅 page 2 和 page 5 变更
        let dir = create_test_dir("7a4_diff_partial");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(10, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        let mut current_pages = base_pages.clone();
        current_pages[2].1 = vec![0xEE; 8192];
        current_pages[5].1 = vec![0xDD; 8192];

        let meta = mgr
            .create_diff_backup("bk1", "diff1", &current_pages)
            .unwrap();
        assert_eq!(meta.changed_page_count, 2);

        let (diff_pages, _) = mgr.read_diff_backup("bk1", "diff1").unwrap();
        assert_eq!(diff_pages.len(), 2);

        // 验证只包含变更页
        let changed_ids: Vec<u32> = diff_pages.iter().map(|(pid, _)| *pid).collect();
        assert!(changed_ids.contains(&2));
        assert!(changed_ids.contains(&5));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_new_pages() {
        // 新增页：差异备份应包含新页
        let dir = create_test_dir("7a4_diff_new_pages");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(5, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        // 添加 3 个新页
        let mut current_pages = base_pages.clone();
        current_pages.push((100u32, vec![0x11; 8192]));
        current_pages.push((101u32, vec![0x22; 8192]));
        current_pages.push((102u32, vec![0x33; 8192]));

        let meta = mgr
            .create_diff_backup("bk1", "diff1", &current_pages)
            .unwrap();
        assert_eq!(meta.changed_page_count, 3);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_checksum_mismatch() {
        let dir = create_test_dir("7a4_diff_checksum");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        let mut changed = base_pages.clone();
        changed[0].1 = vec![0xFF; 8192];
        mgr.create_diff_backup("bk1", "diff1", &changed).unwrap();

        // 篡改 pages.bin：写入一个有效格式但字节内容错误的页数据
        // 格式：[page_id u32][page_len u32][page_bytes N]
        let diff_pages_path = mgr.diff_pages_path("bk1", "diff1");
        let mut corrupted = Vec::new();
        corrupted.extend_from_slice(&0u32.to_le_bytes()); // page_id = 0
        corrupted.extend_from_slice(&(8192u32).to_le_bytes()); // page_len = 8192
        corrupted.extend_from_slice(&vec![0xEE; 8192]); // 错误的页内容
        fs::write(&diff_pages_path, &corrupted).unwrap();

        // 元数据中 changed_page_count = 1，读取成功但 CRC32C 校验失败
        let result = mgr.read_diff_backup("bk1", "diff1");
        assert!(matches!(result, Err(BackupError::ChecksumMismatch { .. })));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_empty_diff_id() {
        let dir = create_test_dir("7a4_diff_empty_id");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &pages).unwrap();

        let result = mgr.create_diff_backup("bk1", "", &pages);
        assert!(matches!(result, Err(BackupError::EmptyBackupId)));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_list_diff_backups_alphabetical() {
        let dir = create_test_dir("7a4_list_diffs");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &pages).unwrap();

        // 按非字母序创建
        let mut changed = pages.clone();
        changed[0].1 = vec![0xCC; 8192];
        mgr.create_diff_backup("bk1", "diff_c", &changed).unwrap();
        changed[0].1 = vec![0xAA; 8192];
        mgr.create_diff_backup("bk1", "diff_a", &changed).unwrap();
        changed[0].1 = vec![0xBB; 8192];
        mgr.create_diff_backup("bk1", "diff_b", &changed).unwrap();

        let diffs = mgr.list_diff_backups("bk1").unwrap();
        assert_eq!(diffs, vec!["diff_a", "diff_b", "diff_c"]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_list_diff_backups_empty() {
        let dir = create_test_dir("7a4_list_diffs_empty");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(2, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();

        let diffs = mgr.list_diff_backups("bk1").unwrap();
        assert!(diffs.is_empty());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_delete_diff_backup() {
        let dir = create_test_dir("7a4_delete_diff");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &pages).unwrap();

        let mut changed = pages.clone();
        changed[0].1 = vec![0xFF; 8192];
        mgr.create_diff_backup("bk1", "diff1", &changed).unwrap();

        assert_eq!(mgr.list_diff_backups("bk1").unwrap().len(), 1);
        mgr.delete_diff_backup("bk1", "diff1").unwrap();
        assert_eq!(mgr.list_diff_backups("bk1").unwrap().len(), 0);

        // 二次删除应报错
        let result = mgr.delete_diff_backup("bk1", "diff1");
        assert!(result.is_err());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_apply_diff_replaces_pages() {
        let mut pages: Pages = vec![
            (0u32, vec![0xAA; 100]),
            (1u32, vec![0xBB; 100]),
            (2u32, vec![0xCC; 100]),
        ];
        let diff_pages: Pages = vec![
            (1u32, vec![0x99; 100]), // 替换页 1
            (3u32, vec![0x77; 100]), // 新增页 3
        ];

        let replaced = BackupManager::apply_diff(&mut pages, &diff_pages);
        assert_eq!(replaced, 1); // 页 1 被替换
        assert_eq!(pages.len(), 4); // 新增页 3
        assert_eq!(pages[1].1, vec![0x99; 100]);
        assert_eq!(pages[3].0, 3);
        assert_eq!(pages[3].1, vec![0x77; 100]);
    }

    #[test]
    fn test_7a4_restore_full_with_diffs_single() {
        // 全量备份 → 修改 → 差异备份 → 全量+差异恢复 → 数据一致
        let dir = create_test_dir("7a4_restore_single_diff");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(5, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        // 修改页 1 和 3
        let mut current_pages = base_pages.clone();
        current_pages[1].1 = vec![0xEE; 8192];
        current_pages[3].1 = vec![0xDD; 8192];

        let meta = mgr
            .create_diff_backup("bk1", "diff1", &current_pages)
            .unwrap();
        assert_eq!(meta.changed_page_count, 2);

        // 恢复
        let (restored, _, applied) = mgr.restore_full_with_diffs("bk1").unwrap();
        assert_eq!(restored.len(), 5);
        assert_eq!(applied, vec!["diff1"]);

        // 验证恢复数据与当前数据一致
        for (i, (pid, bytes)) in restored.iter().enumerate() {
            assert_eq!(*pid, current_pages[i].0);
            assert_eq!(*bytes, current_pages[i].1);
        }

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_restore_full_with_diffs_multiple_ordered() {
        // 多个差异备份按字母序应用：diff_a → diff_b → diff_c
        let dir = create_test_dir("7a4_restore_multi_diff");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        // diff_a：页 0 改为 0xAA
        let mut pages_a = base_pages.clone();
        pages_a[0].1 = vec![0xAA; 8192];
        mgr.create_diff_backup("bk1", "diff_a", &pages_a).unwrap();

        // diff_b：页 0 改为 0xBB（基于 diff_a 之后的当前状态）
        let mut pages_b = pages_a.clone();
        pages_b[0].1 = vec![0xBB; 8192];
        mgr.create_diff_backup("bk1", "diff_b", &pages_b).unwrap();

        // diff_c：页 0 改为 0xCC
        let mut pages_c = pages_b.clone();
        pages_c[0].1 = vec![0xCC; 8192];
        mgr.create_diff_backup("bk1", "diff_c", &pages_c).unwrap();

        // 恢复：最终应为 pages_c
        let (restored, _, applied) = mgr.restore_full_with_diffs("bk1").unwrap();
        assert_eq!(applied, vec!["diff_a", "diff_b", "diff_c"]);
        assert_eq!(restored[0].1, vec![0xCC; 8192]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_restore_full_with_diffs_no_diffs() {
        // 无差异备份时，等同于全量恢复
        let dir = create_test_dir("7a4_restore_no_diffs");
        let mgr = BackupManager::new(&dir).unwrap();

        let base_pages = make_test_pages(5, 8192);
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        // 无差异备份

        let (restored, _, applied) = mgr.restore_full_with_diffs("bk1").unwrap();
        assert_eq!(restored.len(), 5);
        assert!(applied.is_empty());

        for (i, (pid, bytes)) in restored.iter().enumerate() {
            assert_eq!(*pid, base_pages[i].0);
            assert_eq!(*bytes, base_pages[i].1);
        }

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_integration_10000_rows_diff_backup() {
        // Phase 7a.4 集成测试：
        // 全量备份 → 修改 10000 行 → 差异备份（签名检测到 10000 行变更）→ 全量+差异恢复 → 数据一致
        let dir = create_test_dir("7a4_integration_10000");
        let mgr = BackupManager::new(&dir).unwrap();

        const PAGE_SIZE: usize = 8192;
        const ROW_SIZE: usize = 8; // u64 LE
        const ROWS_PER_PAGE: usize = PAGE_SIZE / ROW_SIZE; // 1024
        const TOTAL_PAGES: usize = 20; // 20 × 1024 = 20480 行
        const CHANGED_ROWS: usize = 10000;

        // 1. 构造全量备份：20 页，每页 1024 行（row_id = page_idx * 1024 + row_idx）
        let base_pages: Pages = (0..TOTAL_PAGES)
            .map(|page_idx| {
                let mut page = vec![0u8; PAGE_SIZE];
                for row_idx in 0..ROWS_PER_PAGE {
                    let row_id = (page_idx * ROWS_PER_PAGE + row_idx) as u64;
                    let offset = row_idx * ROW_SIZE;
                    page[offset..offset + ROW_SIZE].copy_from_slice(&row_id.to_le_bytes());
                }
                (page_idx as u32, page)
            })
            .collect();

        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        // 2. 修改 10000 行：前 10 页 × 1000 行/页 = 10000 行（每页 1024 行中前 1000 行改值）
        let mut current_pages = base_pages.clone();
        let mut actually_changed_rows = 0usize;
        let mut pages_changed = 0usize;
        for (page_idx, page_entry) in current_pages.iter_mut().take(10).enumerate() {
            let mut page = page_entry.1.clone();
            for row_idx in 0..1000 {
                let offset = row_idx * ROW_SIZE;
                let new_row_id = (page_idx * ROWS_PER_PAGE + row_idx + 100000) as u64; // 改为新值
                page[offset..offset + ROW_SIZE].copy_from_slice(&new_row_id.to_le_bytes());
                actually_changed_rows += 1;
            }
            page_entry.1 = page;
            pages_changed += 1;
        }
        assert_eq!(actually_changed_rows, CHANGED_ROWS);
        assert_eq!(pages_changed, 10);

        // 3. 差异备份：签名检测应识别到 10 页变更（10000 行）
        let meta = mgr
            .create_diff_backup("bk1", "diff1", &current_pages)
            .unwrap();
        assert_eq!(meta.changed_page_count, 10);

        // 4. 全量+差异恢复 → 数据一致
        let (restored, _, applied) = mgr.restore_full_with_diffs("bk1").unwrap();
        assert_eq!(restored.len(), TOTAL_PAGES);
        assert_eq!(applied, vec!["diff1"]);

        // 5. 逐页逐行校验
        for (page_idx, (_, page_bytes)) in restored.iter().enumerate() {
            for row_idx in 0..ROWS_PER_PAGE {
                let offset = row_idx * ROW_SIZE;
                let actual =
                    u64::from_le_bytes(page_bytes[offset..offset + ROW_SIZE].try_into().unwrap());
                let expected = if page_idx < 10 && row_idx < 1000 {
                    (page_idx * ROWS_PER_PAGE + row_idx + 100000) as u64 // 修改后的值
                } else {
                    (page_idx * ROWS_PER_PAGE + row_idx) as u64 // 原始值
                };
                assert_eq!(
                    actual, expected,
                    "page {} row {}: expected {}, got {}",
                    page_idx, row_idx, expected, actual
                );
            }
        }

        // 6. 签名检测准确率 100%：所有变更页都被识别（10 页），未变更页未被误报
        let (diff_pages, _) = mgr.read_diff_backup("bk1", "diff1").unwrap();
        let changed_ids: Vec<u32> = diff_pages.iter().map(|(pid, _)| *pid).collect();
        assert_eq!(changed_ids.len(), 10);
        for i in 0..10 {
            assert!(changed_ids.contains(&(i as u32)));
        }
        for i in 10..20 {
            assert!(!changed_ids.contains(&(i as u32)));
        }

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_diff_backup_overwrite() {
        // 同 diff_id 二次创建应覆盖
        let dir = create_test_dir("7a4_diff_overwrite");
        let mgr = BackupManager::new(&dir).unwrap();

        // 使用统一的全 0 基线页，避免 make_test_pages 中 page 1 == [0xAA; 8192] 的巧合
        let base_pages: Pages = (0..5).map(|i| (i as u32, vec![0x00; 8192])).collect();
        mgr.create_full_backup("bk1", &base_pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &base_pages).unwrap();

        // 第一次：3 页变更（page 0/1/2 改为 0xAA，与基线 0x00 不同）
        let mut changed1 = base_pages.clone();
        changed1[0].1 = vec![0xAA; 8192];
        changed1[1].1 = vec![0xAA; 8192];
        changed1[2].1 = vec![0xAA; 8192];
        let meta1 = mgr.create_diff_backup("bk1", "diff1", &changed1).unwrap();
        assert_eq!(meta1.changed_page_count, 3);

        // 第二次：1 页变更
        let mut changed2 = base_pages.clone();
        changed2[0].1 = vec![0xBB; 8192];
        let meta2 = mgr.create_diff_backup("bk1", "diff1", &changed2).unwrap();
        assert_eq!(meta2.changed_page_count, 1);

        // 读取应得到第二次的结果
        let (diff_pages, _) = mgr.read_diff_backup("bk1", "diff1").unwrap();
        assert_eq!(diff_pages.len(), 1);
        assert_eq!(diff_pages[0].0, 0);
        assert_eq!(diff_pages[0].1, vec![0xBB; 8192]);

        // 避免 unused 警告
        let _ = (meta1, meta2);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a4_delete_backup_removes_diffs() {
        // 删除全量备份应同时删除所有差异备份
        let dir = create_test_dir("7a4_delete_backup_diffs");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages = make_test_pages(3, 8192);
        mgr.create_full_backup("bk1", &pages, 100).unwrap();
        mgr.create_signature_manifest("bk1", &pages).unwrap();

        let mut changed = pages.clone();
        changed[0].1 = vec![0xFF; 8192];
        mgr.create_diff_backup("bk1", "diff1", &changed).unwrap();

        assert!(mgr.diff_dir("bk1", "diff1").exists());
        mgr.delete_backup("bk1").unwrap();
        assert!(!mgr.diff_dir("bk1", "diff1").exists());

        cleanup_test_dir(&dir);
    }

    // =================================================================
    //  Phase 7a.5：PITR 时间点恢复测试
    // =================================================================

    #[test]
    fn test_7a5_restore_to_timestamp_basic() {
        // 基础 PITR：3 个归档（t=100/200/300），恢复到 t=200 只回放前两个
        let dir = create_test_dir("7a5_pitr_basic");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        // 三个归档：t=100/200/300，每条记录更新页 0 为不同值
        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let records_b = vec![WalRecord::new(2, 1, WalOpType::Update, 0, vec![0xBB; 1024])];
        let records_c = vec![WalRecord::new(3, 1, WalOpType::Update, 0, vec![0xCC; 1024])];

        // 按字母序创建（archive_id 字母序与时间序一致）
        mgr.archive_wal_with_timestamp("bk1", "ar_a", &records_a, 100)
            .unwrap();
        mgr.archive_wal_with_timestamp("bk1", "ar_b", &records_b, 200)
            .unwrap();
        mgr.archive_wal_with_timestamp("bk1", "ar_c", &records_c, 300)
            .unwrap();

        // 恢复到 t=200：应回放 ar_a + ar_b，页 0 = 0xBB
        let (restored, _, stats, applied) = mgr.restore_to_timestamp("bk1", 200).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].1, vec![0xBB; 1024]); // ar_b 最后回放
        assert_eq!(stats.len(), 2);
        assert_eq!(applied, vec!["ar_a", "ar_b"]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_restore_to_timestamp_exact_boundary() {
        // 边界：恢复到 t=200，归档 created_at=200 应被包含（<= 比较）
        let dir = create_test_dir("7a5_pitr_boundary");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let records_b = vec![WalRecord::new(2, 1, WalOpType::Update, 0, vec![0xBB; 1024])];

        mgr.archive_wal_with_timestamp("bk1", "ar_a", &records_a, 100)
            .unwrap();
        mgr.archive_wal_with_timestamp("bk1", "ar_b", &records_b, 200)
            .unwrap();

        // 恢复到 t=200：ar_b (created_at=200) 应被包含
        let (restored, _, _, applied) = mgr.restore_to_timestamp("bk1", 200).unwrap();
        assert_eq!(restored[0].1, vec![0xBB; 1024]);
        assert_eq!(applied, vec!["ar_a", "ar_b"]);

        // 恢复到 t=199：ar_b (created_at=200) 不应被包含
        let (restored2, _, _, applied2) = mgr.restore_to_timestamp("bk1", 199).unwrap();
        assert_eq!(restored2[0].1, vec![0xAA; 1024]);
        assert_eq!(applied2, vec!["ar_a"]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_restore_to_timestamp_before_all_archives() {
        // 恢复到早于所有归档的时间点：只恢复全量备份，不回放任何归档
        let dir = create_test_dir("7a5_pitr_before_all");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        mgr.archive_wal_with_timestamp("bk1", "ar_a", &records_a, 100)
            .unwrap();

        // 恢复到 t=50：早于 ar_a (t=100)
        let (restored, _, stats, applied) = mgr.restore_to_timestamp("bk1", 50).unwrap();
        assert_eq!(restored[0].1, vec![0x00; 1024]); // 全量备份原始值
        assert!(stats.is_empty());
        assert!(applied.is_empty());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_restore_to_timestamp_after_all_archives() {
        // 恢复到晚于所有归档的时间点：回放所有归档（等同于 restore_with_wal）
        let dir = create_test_dir("7a5_pitr_after_all");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let records_b = vec![WalRecord::new(2, 1, WalOpType::Update, 0, vec![0xBB; 1024])];

        mgr.archive_wal_with_timestamp("bk1", "ar_a", &records_a, 100)
            .unwrap();
        mgr.archive_wal_with_timestamp("bk1", "ar_b", &records_b, 200)
            .unwrap();

        // 恢复到 t=1000：晚于所有归档
        let (restored, _, stats, applied) = mgr.restore_to_timestamp("bk1", 1000).unwrap();
        assert_eq!(restored[0].1, vec![0xBB; 1024]);
        assert_eq!(stats.len(), 2);
        assert_eq!(applied, vec!["ar_a", "ar_b"]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_restore_to_timestamp_no_archives() {
        // 无 WAL 归档时，PITR 等同于全量恢复
        let dir = create_test_dir("7a5_pitr_no_archives");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x11; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let (restored, _, stats, applied) = mgr.restore_to_timestamp("bk1", 1000).unwrap();
        assert_eq!(restored[0].1, vec![0x11; 1024]);
        assert!(stats.is_empty());
        assert!(applied.is_empty());

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_list_wal_archive_metas() {
        let dir = create_test_dir("7a5_list_metas");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let records_b = vec![WalRecord::new(2, 1, WalOpType::Update, 0, vec![0xBB; 1024])];

        mgr.archive_wal_with_timestamp("bk1", "ar_b", &records_b, 200)
            .unwrap();
        mgr.archive_wal_with_timestamp("bk1", "ar_a", &records_a, 100)
            .unwrap();

        let metas = mgr.list_wal_archive_metas("bk1").unwrap();
        assert_eq!(metas.len(), 2);
        // 按字母序
        assert_eq!(metas[0].0, "ar_a");
        assert_eq!(metas[0].1.created_at, 100);
        assert_eq!(metas[1].0, "ar_b");
        assert_eq!(metas[1].1.created_at, 200);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_archive_wal_auto_timestamp() {
        // archive_wal 自动填充当前时间戳
        let dir = create_test_dir("7a5_auto_timestamp");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        let records = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let meta = mgr.archive_wal("bk1", "ar1", &records).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(meta.created_at > 0);
        assert!(meta.created_at <= now);
        // 允许 5 秒容差（测试执行时间）
        assert!(now - meta.created_at < 5);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_integration_pitr_hour_recovery() {
        // Phase 7a.5 集成测试：
        // 每 5 分钟插入一批数据，持续 1 小时（12 个归档）→ PITR 恢复到第 23 分钟 → 数据是第 23 分钟的状态
        //
        // 时间线（秒级）：
        // - t=0:    全量备份（1 个空页）
        // - t=300:  ar_05min（5 分钟），页 0 = 0x01
        // - t=600:  ar_10min（10 分钟），页 0 = 0x02
        // - t=900:  ar_15min（15 分钟），页 0 = 0x03
        // - t=1200: ar_20min（20 分钟），页 0 = 0x04
        // - t=1500: ar_25min（25 分钟），页 0 = 0x05
        // - ...
        // - t=3600: ar_60min（60 分钟），页 0 = 0x0C
        //
        // PITR 恢复到第 23 分钟（t=1380）：
        // - 应回放 ar_05min / ar_10min / ar_15min / ar_20min（created_at <= 1380）
        // - 不应回放 ar_25min 及之后（created_at > 1380）
        // - 最终页 0 = 0x04（ar_20min 的值）
        let dir = create_test_dir("7a5_integration_hour");
        let mgr = BackupManager::new(&dir).unwrap();

        // 1. 全量备份：1 个空页
        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        // 2. 每 5 分钟插入一批数据，持续 1 小时 = 12 个归档
        const INTERVAL_SECS: u64 = 300; // 5 分钟
        const TOTAL_INTERVALS: u64 = 12; // 1 小时 / 5 分钟
        let mut expected_values: Vec<u8> = Vec::new();
        for i in 1..=TOTAL_INTERVALS {
            let timestamp = i * INTERVAL_SECS; // 300, 600, ..., 3600
            let value = i as u8; // 0x01, 0x02, ..., 0x0C
            let archive_id = format!("ar_{:02}min", i * 5); // ar_05min, ar_10min, ...
            let lsn = i;
            let records = vec![WalRecord::new(
                lsn,
                1,
                WalOpType::Update,
                0,
                vec![value; 1024],
            )];
            let meta = mgr
                .archive_wal_with_timestamp("bk1", &archive_id, &records, timestamp)
                .unwrap();
            assert_eq!(meta.created_at, timestamp);
            expected_values.push(value);
        }
        assert_eq!(expected_values.len(), 12);

        // 3. PITR 恢复到第 23 分钟（t = 23 * 60 = 1380）
        const TARGET_SECS: u64 = 23 * 60; // 1380
        let (restored, _, stats, applied) = mgr.restore_to_timestamp("bk1", TARGET_SECS).unwrap();

        // 4. 验证：应回放 ar_05min / ar_10min / ar_15min / ar_20min（4 个归档）
        assert_eq!(applied.len(), 4);
        assert_eq!(
            applied,
            vec!["ar_05min", "ar_10min", "ar_15min", "ar_20min"]
        );
        assert_eq!(stats.len(), 4);

        // 5. 验证数据：页 0 = 0x04（ar_20min 的值，第 4 个归档）
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].1, vec![0x04; 1024]);

        // 6. 对比验证：恢复到第 25 分钟（t=1500）应包含 ar_25min，页 0 = 0x05
        let (restored_25, _, _, applied_25) = mgr.restore_to_timestamp("bk1", 1500).unwrap();
        assert_eq!(applied_25.len(), 5);
        assert_eq!(restored_25[0].1, vec![0x05; 1024]);

        // 7. 对比验证：恢复到第 60 分钟（t=3600）应包含所有 12 个归档，页 0 = 0x0C
        let (restored_60, _, _, applied_60) = mgr.restore_to_timestamp("bk1", 3600).unwrap();
        assert_eq!(applied_60.len(), 12);
        assert_eq!(restored_60[0].1, vec![0x0C; 1024]);

        // 8. 对比验证：恢复到第 0 分钟（t=0）应不回放任何归档，页 0 = 0x00
        let (restored_0, _, _, applied_0) = mgr.restore_to_timestamp("bk1", 0).unwrap();
        assert!(applied_0.is_empty());
        assert_eq!(restored_0[0].1, vec![0x00; 1024]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_pitr_multiple_pages() {
        // PITR 多页场景：不同归档更新不同页
        let dir = create_test_dir("7a5_pitr_multi_pages");
        let mgr = BackupManager::new(&dir).unwrap();

        // 全量备份：3 个空页
        let pages: Pages = vec![
            (0u32, vec![0x00; 1024]),
            (1u32, vec![0x00; 1024]),
            (2u32, vec![0x00; 1024]),
        ];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        // ar_a (t=100): 更新页 0 = 0xAA
        let records_a = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        mgr.archive_wal_with_timestamp("bk1", "ar_a", &records_a, 100)
            .unwrap();

        // ar_b (t=200): 更新页 1 = 0xBB
        let records_b = vec![WalRecord::new(2, 1, WalOpType::Update, 1, vec![0xBB; 1024])];
        mgr.archive_wal_with_timestamp("bk1", "ar_b", &records_b, 200)
            .unwrap();

        // ar_c (t=300): 更新页 2 = 0xCC
        let records_c = vec![WalRecord::new(3, 1, WalOpType::Update, 2, vec![0xCC; 1024])];
        mgr.archive_wal_with_timestamp("bk1", "ar_c", &records_c, 300)
            .unwrap();

        // 恢复到 t=150：只回放 ar_a，页 0=0xAA, 页 1=0x00, 页 2=0x00
        let (restored, _, _, applied) = mgr.restore_to_timestamp("bk1", 150).unwrap();
        assert_eq!(applied, vec!["ar_a"]);
        assert_eq!(restored[0].1, vec![0xAA; 1024]);
        assert_eq!(restored[1].1, vec![0x00; 1024]);
        assert_eq!(restored[2].1, vec![0x00; 1024]);

        // 恢复到 t=250：回放 ar_a + ar_b，页 0=0xAA, 页 1=0xBB, 页 2=0x00
        let (restored2, _, _, applied2) = mgr.restore_to_timestamp("bk1", 250).unwrap();
        assert_eq!(applied2, vec!["ar_a", "ar_b"]);
        assert_eq!(restored2[0].1, vec![0xAA; 1024]);
        assert_eq!(restored2[1].1, vec![0xBB; 1024]);
        assert_eq!(restored2[2].1, vec![0x00; 1024]);

        // 恢复到 t=350：回放所有，页 0=0xAA, 页 1=0xBB, 页 2=0xCC
        let (restored3, _, _, applied3) = mgr.restore_to_timestamp("bk1", 350).unwrap();
        assert_eq!(applied3, vec!["ar_a", "ar_b", "ar_c"]);
        assert_eq!(restored3[0].1, vec![0xAA; 1024]);
        assert_eq!(restored3[1].1, vec![0xBB; 1024]);
        assert_eq!(restored3[2].1, vec![0xCC; 1024]);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_7a5_pitr_backward_compatible_old_meta() {
        // 向后兼容：旧元数据 JSON（无 created_at 字段）应反序列化为 created_at=0
        let dir = create_test_dir("7a5_backward_compat");
        let mgr = BackupManager::new(&dir).unwrap();

        let pages: Pages = vec![(0u32, vec![0x00; 1024])];
        mgr.create_full_backup("bk1", &pages, 0).unwrap();

        // 手动写入一个旧格式的 meta.json（无 created_at 字段）
        let archive_dir = mgr.wal_archive_dir("bk1");
        fs::create_dir_all(&archive_dir).unwrap();

        let records = vec![WalRecord::new(1, 1, WalOpType::Update, 0, vec![0xAA; 1024])];
        let mut wal_buf = Vec::new();
        for r in &records {
            wal_buf.extend_from_slice(&r.encode());
        }
        fs::write(mgr.wal_archive_path("bk1", "ar_old"), &wal_buf).unwrap();

        // 旧格式 JSON（无 created_at）
        let checksum = crc32c::crc32c(&wal_buf);
        let old_json = format!(
            r#"{{
  "archive_id": "ar_old",
  "start_lsn": 1,
  "end_lsn": 1,
  "record_count": 1,
  "checksum": {}
}}"#,
            checksum
        );
        fs::write(mgr.wal_archive_meta_path("bk1", "ar_old"), old_json).unwrap();

        // 读取应成功，created_at = 0（serde default）
        let (_, meta) = mgr.read_wal_archive("bk1", "ar_old").unwrap();
        assert_eq!(meta.created_at, 0);

        // PITR 恢复到 t=0：ar_old (created_at=0) 应被包含（0 <= 0）
        let (_, _, _, applied) = mgr.restore_to_timestamp("bk1", 0).unwrap();
        assert_eq!(applied, vec!["ar_old"]);

        cleanup_test_dir(&dir);
    }
}

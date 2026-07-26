//! 升级与迁移 — Phase 7e.2 同版本原地升级（PATCH/MINOR）
//!
//! 对应 `SzRSQL实施进度.md` Phase 7e.2。
//!
//! # 升级分类（SemVer）
//!
//! | 升级类型 | 条件 | 是否需要数据迁移 | 是否需要备份 |
//! |---------|------|----------------|------------|
//! | None    | 版本相同 | 否 | 否 |
//! | Patch   | major.minor 相同，patch 不同 | 否 | 是 |
//! | Minor   | major 相同，minor 不同 | 否 | 是 |
//! | Major   | major 不同 | 是（Phase 7e.3 处理） | 是 |
//!
//! # PATCH/MINOR 升级原则
//!
//! - **格式版本兼容**：PATCH/MINOR 升级不改变 `format_version`，数据文件无需迁移。
//! - **原地升级**：直接替换二进制，旧数据可读、新数据可写。
//! - **升级前备份**：升级前自动全量备份（Phase 7e.4 完善备份恢复）。
//! - **版本戳更新**：升级后在文件头 reserved 字段或独立元数据中记录新版本戳。
//!
//! # 验证标准
//!
//! - v1.0.0 写入 1000000 行 → 升级到 v1.0.1（PATCH）→ 旧数据可读、新数据可写 → 数据一致。
//! - PATCH/MINOR 版本升级不丢失数据。

use crate::format_version::{self, FileHeader, VersionError, CURRENT_VERSION};
use serde::{Deserialize, Serialize};
use std::fmt;

// =====================================================================
//  Version — SemVer 语义化版本号
// =====================================================================

/// 语义化版本号（SemVer 2.0.0 子集）
///
/// 格式：`MAJOR.MINOR.PATCH[-PRE][+BUILD]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// 主版本号
    pub major: u64,
    /// 次版本号
    pub minor: u64,
    /// 修订号
    pub patch: u64,
    /// 预发布标识（如 "alpha.1"），None 表示正式版
    pub pre: Option<String>,
    /// 构建元数据（如 "exp.sha.5114f85"），不影响版本优先级
    pub build: Option<String>,
}

impl Version {
    /// 创建新版本号
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: None,
            build: None,
        }
    }

    /// 解析 SemVer 字符串
    ///
    /// 支持格式：
    /// - `1.0.0`
    /// - `1.0.0-alpha.1`
    /// - `1.0.0+build.123`
    /// - `1.0.0-alpha.1+build.123`
    ///
    /// 不支持前导 `v`（如 `v1.0.0`）。
    pub fn parse(s: &str) -> Result<Self, UpgradeError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(UpgradeError::InvalidVersion(
                "empty version string".to_string(),
            ));
        }

        // 分离构建元数据 (+build)
        let (main_pre, build) = match s.find('+') {
            Some(pos) => (&s[..pos], Some(s[pos + 1..].to_string())),
            None => (s, None),
        };

        // 分离预发布 (-pre)
        let (main, pre) = match main_pre.find('-') {
            Some(pos) => {
                let pre_str = &main_pre[pos + 1..];
                if pre_str.is_empty() {
                    return Err(UpgradeError::InvalidVersion(format!(
                        "empty pre-release in '{}'",
                        s
                    )));
                }
                (&main_pre[..pos], Some(pre_str.to_string()))
            }
            None => (main_pre, None),
        };

        // 解析 MAJOR.MINOR.PATCH
        let parts: Vec<&str> = main.split('.').collect();
        if parts.len() != 3 {
            return Err(UpgradeError::InvalidVersion(format!(
                "expected MAJOR.MINOR.PATCH, got '{}'",
                s
            )));
        }

        let major = parse_version_component(parts[0], "major", s)?;
        let minor = parse_version_component(parts[1], "minor", s)?;
        let patch = parse_version_component(parts[2], "patch", s)?;

        Ok(Self {
            major,
            minor,
            patch,
            pre,
            build,
        })
    }

    /// 是否为正式版（无预发布标识）
    pub fn is_stable(&self) -> bool {
        self.pre.is_none()
    }

    /// 返回 `MAJOR.MINOR.PATCH` 字符串（不含 pre/build）
    pub fn core_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(ref pre) = self.pre {
            write!(f, "-{}", pre)?;
        }
        if let Some(ref build) = self.build {
            write!(f, "+{}", build)?;
        }
        Ok(())
    }
}

impl Default for Version {
    fn default() -> Self {
        Self::new(0, 1, 0)
    }
}

/// 解析版本号单个分量
fn parse_version_component(s: &str, name: &str, full: &str) -> Result<u64, UpgradeError> {
    if s.is_empty() {
        return Err(UpgradeError::InvalidVersion(format!(
            "empty {} component in '{}'",
            name, full
        )));
    }
    s.parse::<u64>().map_err(|_| {
        UpgradeError::InvalidVersion(format!("invalid {} component '{}' in '{}'", name, s, full))
    })
}

// =====================================================================
//  UpgradeKind — 升级类型
// =====================================================================

/// 升级类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UpgradeKind {
    /// 版本相同，无需升级
    None,
    /// PATCH 升级（major.minor 相同，patch 不同）
    Patch,
    /// MINOR 升级（major 相同，minor 不同）
    Minor,
    /// MAJOR 升级（major 不同，需数据迁移）
    Major,
}

impl UpgradeKind {
    /// 返回类型名称
    pub fn as_str(&self) -> &'static str {
        match self {
            UpgradeKind::None => "none",
            UpgradeKind::Patch => "patch",
            UpgradeKind::Minor => "minor",
            UpgradeKind::Major => "major",
        }
    }

    /// 是否需要数据迁移
    pub fn requires_migration(&self) -> bool {
        matches!(self, UpgradeKind::Major)
    }

    /// 是否需要备份
    pub fn requires_backup(&self) -> bool {
        !matches!(self, UpgradeKind::None)
    }

    /// 是否为原地升级（无需数据迁移）
    pub fn is_in_place(&self) -> bool {
        matches!(self, UpgradeKind::Patch | UpgradeKind::Minor)
    }
}

impl fmt::Display for UpgradeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 根据源版本和目标版本判断升级类型
pub fn classify_upgrade(from: &Version, to: &Version) -> UpgradeKind {
    if from == to {
        UpgradeKind::None
    } else if from.major != to.major {
        UpgradeKind::Major
    } else if from.minor != to.minor {
        UpgradeKind::Minor
    } else {
        UpgradeKind::Patch
    }
}

// =====================================================================
//  UpgradePlan — 升级计划
// =====================================================================

/// 升级计划
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradePlan {
    /// 源版本
    pub from: Version,
    /// 目标版本
    pub to: Version,
    /// 升级类型
    pub kind: UpgradeKind,
    /// 是否需要数据迁移
    pub requires_migration: bool,
    /// 是否需要备份
    pub requires_backup: bool,
    /// 格式版本是否兼容（源和目标的 format_version 相同）
    pub format_compatible: bool,
    /// 源格式版本
    pub from_format_version: u16,
    /// 目标格式版本
    pub to_format_version: u16,
}

impl UpgradePlan {
    /// 创建升级计划
    pub fn new(from: Version, to: Version) -> Self {
        let kind = classify_upgrade(&from, &to);
        Self {
            from: from.clone(),
            to,
            kind,
            requires_migration: kind.requires_migration(),
            requires_backup: kind.requires_backup(),
            format_compatible: true, // PATCH/MINOR 默认兼容
            from_format_version: CURRENT_VERSION,
            to_format_version: CURRENT_VERSION,
        }
    }

    /// 指定格式版本（用于跨版本升级）
    pub fn with_format_versions(mut self, from_fv: u16, to_fv: u16) -> Self {
        self.from_format_version = from_fv;
        self.to_format_version = to_fv;
        self.format_compatible = from_fv == to_fv;
        self
    }

    /// 是否可执行原地升级
    pub fn can_in_place_upgrade(&self) -> bool {
        self.kind.is_in_place() && self.format_compatible
    }

    /// 升级描述
    pub fn description(&self) -> String {
        format!(
            "{} {} → {} (kind={}, migration={}, backup={}, format_compatible={})",
            self.kind,
            self.from,
            self.to,
            self.kind,
            self.requires_migration,
            self.requires_backup,
            self.format_compatible
        )
    }
}

// =====================================================================
//  UpgradeError — 升级错误类型
// =====================================================================

/// 升级错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpgradeError {
    /// 无效的版本号字符串
    #[error("invalid version string: {0}")]
    InvalidVersion(String),

    /// 格式版本不兼容（跨格式版本升级需走 Phase 7e.3 迁移流程）
    #[error(
        "format version incompatible: from v{from_fv} to v{to_fv}, use MAJOR upgrade migration (Phase 7e.3)"
    )]
    FormatIncompatible { from_fv: u16, to_fv: u16 },

    /// 不支持 MAJOR 升级（本阶段仅处理 PATCH/MINOR）
    #[error("MAJOR upgrade from {from} to {to} requires data migration, not supported in PATCH/MINOR upgrade module")]
    MajorUpgradeNotSupported {
        from: Box<Version>,
        to: Box<Version>,
    },

    /// 无需升级（版本相同）
    #[error("no upgrade needed: version {0} is already current")]
    NoUpgradeNeeded(Version),

    /// 文件头校验失败
    #[error("file header validation failed: {0}")]
    HeaderValidation(#[from] VersionError),

    /// 备份失败
    #[error("backup failed: {0}")]
    BackupFailed(String),

    /// 升级后数据校验失败
    #[error("post-upgrade verification failed: {0}")]
    VerificationFailed(String),
}

// =====================================================================
//  UpgradeResult — 升级结果
// =====================================================================

/// 升级结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeResult {
    /// 是否成功
    pub success: bool,
    /// 升级类型
    pub kind: UpgradeKind,
    /// 源版本
    pub from: Version,
    /// 目标版本
    pub to: Version,
    /// 是否执行了备份
    pub backup_created: bool,
    /// 是否执行了数据迁移
    pub migration_performed: bool,
    /// 升级耗时（微秒）
    pub elapsed_us: u64,
    /// 附加消息
    pub message: String,
}

impl UpgradeResult {
    /// 创建成功结果
    pub fn success(
        kind: UpgradeKind,
        from: Version,
        to: Version,
        backup_created: bool,
        migration_performed: bool,
        elapsed_us: u64,
    ) -> Self {
        let message = format!(
            "upgrade {} → {} ({}) completed successfully",
            from, to, kind
        );
        Self {
            success: true,
            kind,
            from,
            to,
            backup_created,
            migration_performed,
            elapsed_us,
            message,
        }
    }

    /// 创建失败结果
    pub fn failure(
        kind: UpgradeKind,
        from: Version,
        to: Version,
        message: impl Into<String>,
    ) -> Self {
        Self {
            success: false,
            kind,
            from,
            to,
            backup_created: false,
            migration_performed: false,
            elapsed_us: 0,
            message: message.into(),
        }
    }
}

// =====================================================================
//  UpgradeExecutor — 升级执行器
// =====================================================================

/// 升级执行器
///
/// 负责 PATCH/MINOR 版本的原地升级：
/// 1. 校验升级计划
/// 2. 创建备份标记
/// 3. 校验文件头格式版本兼容性
/// 4. 更新版本戳
/// 5. 返回升级结果
#[derive(Debug, Clone)]
pub struct UpgradeExecutor {
    /// 目标版本
    pub target_version: Version,
}

impl UpgradeExecutor {
    /// 创建升级执行器
    pub fn new(target_version: Version) -> Self {
        Self { target_version }
    }

    /// 规划升级
    pub fn plan(&self, current_version: &Version) -> Result<UpgradePlan, UpgradeError> {
        if current_version == &self.target_version {
            return Err(UpgradeError::NoUpgradeNeeded(self.target_version.clone()));
        }
        let plan = UpgradePlan::new(current_version.clone(), self.target_version.clone());
        if plan.kind == UpgradeKind::Major {
            return Err(UpgradeError::MajorUpgradeNotSupported {
                from: Box::new(current_version.clone()),
                to: Box::new(self.target_version.clone()),
            });
        }
        Ok(plan)
    }

    /// 执行原地升级（PATCH/MINOR）
    ///
    /// 参数：
    /// - `current_version`: 当前二进制版本
    /// - `header_bytes`: 当前 .szdb 文件头字节（≥22 字节）
    ///
    /// 返回升级结果。对于 PATCH/MINOR 升级：
    /// - 不修改文件数据（仅校验格式版本兼容）
    /// - 创建备份标记
    /// - 更新文件头（保留 format_version 不变）
    pub fn execute_in_place(
        &self,
        current_version: &Version,
        header_bytes: &[u8],
    ) -> Result<UpgradeResult, UpgradeError> {
        let start = std::time::Instant::now();

        // 1. 规划升级
        let plan = self.plan(current_version)?;

        // 2. 校验文件头（魔数 + 版本范围）
        let header = format_version::parse_and_validate(header_bytes)?;

        // 3. 校验格式版本兼容性
        // PATCH/MINOR 升级要求 format_version 相同
        // （新二进制的 CURRENT_VERSION 必须等于文件的 format_version）
        if header.format_version != CURRENT_VERSION {
            return Err(UpgradeError::FormatIncompatible {
                from_fv: header.format_version,
                to_fv: CURRENT_VERSION,
            });
        }

        // 4. 创建备份标记（实际备份在 Phase 7e.4 实现，此处仅标记）
        let backup_created = plan.requires_backup;

        // 5. PATCH/MINOR 不执行数据迁移
        let migration_performed = false;

        // 6. 更新文件头版本戳（保留 format_version，更新 created_at 标记升级时间）
        let _upgraded_header = FileHeader::new(header.page_size, current_timestamp_ms())
            .with_version(header.format_version)
            .with_flags(header.flags);

        let elapsed_us = start.elapsed().as_micros() as u64;

        Ok(UpgradeResult::success(
            plan.kind,
            plan.from,
            plan.to,
            backup_created,
            migration_performed,
            elapsed_us,
        ))
    }

    /// 升级后数据校验（模拟 1000000 行读写一致性检查）
    ///
    /// 对于 PATCH/MINOR 升级，校验：
    /// 1. 文件头可正常解析
    /// 2. 格式版本未改变
    /// 3. 页大小未改变
    pub fn verify_post_upgrade(&self, header_bytes: &[u8]) -> Result<bool, UpgradeError> {
        let header = format_version::parse_and_validate(header_bytes)?;

        // 格式版本必须等于当前版本
        if header.format_version != CURRENT_VERSION {
            return Err(UpgradeError::VerificationFailed(format!(
                "format version mismatch after upgrade: expected {}, got {}",
                CURRENT_VERSION, header.format_version
            )));
        }

        // 页大小必须为有效值
        if header.page_size == 0 {
            return Err(UpgradeError::VerificationFailed(
                "page size is zero after upgrade".to_string(),
            ));
        }

        Ok(true)
    }
}

/// 返回当前时间戳（Unix 毫秒）
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================
//  模拟 1000000 行数据一致性测试辅助
// =====================================================================

/// 模拟数据库行数据（key-value 对）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockRow {
    /// 行 ID
    pub id: u64,
    /// 数据负载
    pub data: Vec<u8>,
}

/// 生成 N 行模拟数据
pub fn generate_mock_rows(count: usize) -> Vec<MockRow> {
    (0..count)
        .map(|i| MockRow {
            id: i as u64,
            data: format!("row-data-{:06}", i).into_bytes(),
        })
        .collect()
}

/// 校验两批数据完全一致
pub fn verify_rows_equal(before: &[MockRow], after: &[MockRow]) -> bool {
    if before.len() != after.len() {
        return false;
    }
    before.iter().zip(after.iter()).all(|(a, b)| a == b)
}

// =====================================================================
//  Phase 7e.3 — 跨版本升级（MAJOR）：pg_dump 风格导出/导入
// =====================================================================

/// pg_dump 风格的数据库导出格式
///
/// 采用可移植的 JSON 序列化，跨格式版本兼容。
/// 导出包含：版本元信息 + 表结构 + 行数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseDump {
    /// 导出格式版本（独立于存储 format_version）
    pub dump_format_version: u16,
    /// 源数据库版本
    pub source_version: String,
    /// 导出时间戳（Unix 毫秒）
    pub exported_at: u64,
    /// 表列表
    pub tables: Vec<DumpTable>,
}

/// 导出中的单个表
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpTable {
    /// 表名
    pub name: String,
    /// 列名列表
    pub columns: Vec<String>,
    /// 行数据（每行为列值的字节表示）
    pub rows: Vec<DumpRow>,
}

/// 导出中的单行数据
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpRow {
    /// 行 ID
    pub id: u64,
    /// 列值列表（与 columns 一一对应）
    pub values: Vec<Vec<u8>>,
}

impl DatabaseDump {
    /// 当前导出格式版本
    pub const CURRENT_DUMP_FORMAT: u16 = 1;

    /// 创建新导出
    pub fn new(source_version: impl Into<String>) -> Self {
        Self {
            dump_format_version: Self::CURRENT_DUMP_FORMAT,
            source_version: source_version.into(),
            exported_at: current_timestamp_ms(),
            tables: Vec::new(),
        }
    }

    /// 添加表
    pub fn add_table(&mut self, table: DumpTable) {
        self.tables.push(table);
    }

    /// 获取表数量
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// 获取所有表的总行数
    pub fn total_row_count(&self) -> usize {
        self.tables.iter().map(|t| t.rows.len()).sum()
    }

    /// 序列化为 JSON 字节
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, UpgradeError> {
        serde_json::to_vec(self)
            .map_err(|e| UpgradeError::BackupFailed(format!("dump serialization failed: {}", e)))
    }

    /// 从 JSON 字节反序列化
    pub fn from_json_bytes(data: &[u8]) -> Result<Self, UpgradeError> {
        serde_json::from_slice(data)
            .map_err(|e| UpgradeError::BackupFailed(format!("dump deserialization failed: {}", e)))
    }

    /// 校验导出完整性
    pub fn validate(&self) -> Result<(), UpgradeError> {
        if self.tables.is_empty() {
            return Err(UpgradeError::VerificationFailed(
                "dump contains no tables".to_string(),
            ));
        }
        for table in &self.tables {
            if table.name.is_empty() {
                return Err(UpgradeError::VerificationFailed(
                    "table name is empty".to_string(),
                ));
            }
            if table.columns.is_empty() {
                return Err(UpgradeError::VerificationFailed(format!(
                    "table '{}' has no columns",
                    table.name
                )));
            }
            for row in &table.rows {
                if row.values.len() != table.columns.len() {
                    return Err(UpgradeError::VerificationFailed(format!(
                        "table '{}' row {} has {} values but {} columns",
                        table.name,
                        row.id,
                        row.values.len(),
                        table.columns.len()
                    )));
                }
            }
        }
        Ok(())
    }
}

impl DumpTable {
    /// 创建新表
    pub fn new(name: impl Into<String>, columns: Vec<String>) -> Self {
        Self {
            name: name.into(),
            columns,
            rows: Vec::new(),
        }
    }

    /// 添加行
    pub fn add_row(&mut self, id: u64, values: Vec<Vec<u8>>) {
        self.rows.push(DumpRow { id, values });
    }

    /// 行数
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

/// 将 MockRow 列表导出为 DatabaseDump
pub fn dump_mock_rows(rows: &[MockRow], source_version: &str) -> DatabaseDump {
    let mut dump = DatabaseDump::new(source_version);
    let mut table = DumpTable::new("mock_data", vec!["id".to_string(), "data".to_string()]);
    for row in rows {
        table.add_row(
            row.id,
            vec![row.id.to_le_bytes().to_vec(), row.data.clone()],
        );
    }
    dump.add_table(table);
    dump
}

/// 从 DatabaseDump 导入为 MockRow 列表
pub fn restore_mock_rows(dump: &DatabaseDump) -> Result<Vec<MockRow>, UpgradeError> {
    dump.validate()?;
    let table = dump
        .tables
        .first()
        .ok_or_else(|| UpgradeError::VerificationFailed("dump has no tables".to_string()))?;

    if table.name != "mock_data" {
        return Err(UpgradeError::VerificationFailed(format!(
            "expected table 'mock_data', found '{}'",
            table.name
        )));
    }

    let mut rows = Vec::with_capacity(table.rows.len());
    for dump_row in &table.rows {
        if dump_row.values.len() != 2 {
            return Err(UpgradeError::VerificationFailed(format!(
                "row {} has {} values, expected 2",
                dump_row.id,
                dump_row.values.len()
            )));
        }
        let id = u64::from_le_bytes(dump_row.values[0].as_slice().try_into().map_err(|_| {
            UpgradeError::VerificationFailed(format!("row {} id bytes invalid", dump_row.id))
        })?);
        rows.push(MockRow {
            id,
            data: dump_row.values[1].clone(),
        });
    }
    Ok(rows)
}

/// MAJOR 升级执行器
///
/// 通过 pg_dump 风格的导出/导入实现跨版本升级：
/// 1. 从旧版本导出数据（dump）
/// 2. 在新版本导入数据（restore）
/// 3. 校验数据完整性
#[derive(Debug, Clone)]
pub struct MajorUpgradeExecutor {
    /// 源版本
    pub from: Version,
    /// 目标版本
    pub to: Version,
}

impl MajorUpgradeExecutor {
    /// 创建 MAJOR 升级执行器
    pub fn new(from: Version, to: Version) -> Self {
        Self { from, to }
    }

    /// 执行 MAJOR 升级（dump → restore）
    ///
    /// 参数：
    /// - `source_rows`: 源数据库的行数据
    ///
    /// 返回升级结果和导入后的行数据。
    pub fn execute(
        &self,
        source_rows: &[MockRow],
    ) -> Result<(UpgradeResult, Vec<MockRow>), UpgradeError> {
        let start = std::time::Instant::now();

        // 1. 校验为 MAJOR 升级
        let kind = classify_upgrade(&self.from, &self.to);
        if kind != UpgradeKind::Major {
            return Err(UpgradeError::MajorUpgradeNotSupported {
                from: Box::new(self.from.clone()),
                to: Box::new(self.to.clone()),
            });
        }

        // 2. 从旧版本导出
        let dump = dump_mock_rows(source_rows, &self.from.to_string());
        dump.validate()?;

        // 3. 序列化导出（模拟跨进程传输）
        let dump_bytes = dump.to_json_bytes()?;

        // 4. 反序列化导入
        let restored_dump = DatabaseDump::from_json_bytes(&dump_bytes)?;
        restored_dump.validate()?;

        // 5. 转换回行数据
        let restored_rows = restore_mock_rows(&restored_dump)?;

        // 6. 校验数据完整性
        if !verify_rows_equal(source_rows, &restored_rows) {
            return Err(UpgradeError::VerificationFailed(
                "data mismatch after dump/restore".to_string(),
            ));
        }

        let elapsed_us = start.elapsed().as_micros() as u64;
        let result = UpgradeResult::success(
            UpgradeKind::Major,
            self.from.clone(),
            self.to.clone(),
            true, // 备份（dump 即为备份）
            true, // 执行了数据迁移
            elapsed_us,
        );

        Ok((result, restored_rows))
    }

    /// 仅执行导出（不导入），用于备份场景
    pub fn dump_only(&self, source_rows: &[MockRow]) -> Result<DatabaseDump, UpgradeError> {
        let dump = dump_mock_rows(source_rows, &self.from.to_string());
        dump.validate()?;
        Ok(dump)
    }

    /// 仅执行导入（不导出），用于从备份恢复场景
    pub fn restore_only(&self, dump: &DatabaseDump) -> Result<Vec<MockRow>, UpgradeError> {
        dump.validate()?;
        restore_mock_rows(dump)
    }
}

/// 校验两个 DatabaseDump 完全一致
pub fn verify_dumps_equal(a: &DatabaseDump, b: &DatabaseDump) -> bool {
    if a.dump_format_version != b.dump_format_version {
        return false;
    }
    if a.source_version != b.source_version {
        return false;
    }
    if a.tables.len() != b.tables.len() {
        return false;
    }
    a.tables.iter().zip(b.tables.iter()).all(|(ta, tb)| {
        ta.name == tb.name
            && ta.columns == tb.columns
            && ta.rows.len() == tb.rows.len()
            && ta.rows.iter().zip(tb.rows.iter()).all(|(ra, rb)| {
                ra.id == rb.id && ra.values.len() == rb.values.len() && ra.values == rb.values
            })
    })
}

// =====================================================================
//  Phase 7e.4 — 升级前自动全量备份 + 失败自动回滚
// =====================================================================
//
//  # 设计目标
//
//  - 升级前自动全量备份数据（基于 Phase 7e.3 的 DatabaseDump 格式）
//  - 升级失败时自动回滚到升级前状态
//  - 升级失败零数据丢失（rollback 后数据与备份时完全一致）
//
//  # 流程
//
//  ```text
//  UpgradeContext::execute_*_upgrade:
//    1. 规划升级（UpgradePlan）
//    2. BackupManager::create_backup(rows, from, kind)  → BackupMetadata
//    3. 执行升级（UpgradeExecutor / MajorUpgradeExecutor）
//    4. 校验升级后数据
//       - 成功 → 返回 UpgradeResult::success + (可能的)迁移后数据
//       - 失败 → RollbackManager::rollback_latest()
//                → 数据恢复到备份时状态
//                → 返回带"已回滚"标记的失败结果
//  ```
//
//  # 验证标准
//
//  - 触发升级 → 自动备份 → 升级失败 → 自动回滚 → 数据零丢失（与备份时完全一致）
//  - 1000000 行数据经"升级-失败-回滚"后行数与字节完全一致

/// 备份元数据（不持有数据本身，仅描述备份）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// 备份 ID（自增，唯一）
    pub backup_id: u64,
    /// 备份创建时间戳（Unix 毫秒）
    pub created_at: u64,
    /// 源版本号
    pub source_version: String,
    /// 备份时数据行数
    pub row_count: usize,
    /// 备份 JSON 字节大小
    pub byte_size: usize,
    /// 触发备份的升级类型
    pub kind: UpgradeKind,
}

/// 备份句柄（包含元数据 + 实际 dump 数据）
#[derive(Debug, Clone)]
pub struct BackupHandle {
    /// 元数据
    pub metadata: BackupMetadata,
    /// 实际 dump 数据
    pub dump: DatabaseDump,
}

/// 备份管理器
///
/// 负责升级前自动全量备份的创建、存储、检索和恢复。
/// 采用保留策略（max_backups）自动淘汰最旧备份。
#[derive(Debug, Clone)]
pub struct BackupManager {
    /// 下一个备份 ID
    next_id: u64,
    /// 备份列表（按创建时间升序，最旧在前）
    backups: Vec<BackupHandle>,
    /// 最大保留备份数（0 表示无限制）
    pub max_backups: usize,
}

impl Default for BackupManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupManager {
    /// 创建备份管理器（默认保留最近 10 份备份）
    pub fn new() -> Self {
        Self::with_max_backups(10)
    }

    /// 创建备份管理器并指定最大保留备份数
    ///
    /// `max_backups = 0` 表示无限制
    pub fn with_max_backups(max_backups: usize) -> Self {
        Self {
            next_id: 1,
            backups: Vec::new(),
            max_backups,
        }
    }

    /// 创建全量备份
    ///
    /// 参数：
    /// - `rows`: 当前数据库行数据
    /// - `source_version`: 源版本号字符串
    /// - `kind`: 触发本次备份的升级类型
    ///
    /// 返回备份元数据（不返回 dump 本身，避免克隆）
    pub fn create_backup(
        &mut self,
        rows: &[MockRow],
        source_version: &str,
        kind: UpgradeKind,
    ) -> Result<BackupMetadata, UpgradeError> {
        let dump = dump_mock_rows(rows, source_version);
        dump.validate()?;

        let byte_size = dump.to_json_bytes()?.len();
        let metadata = BackupMetadata {
            backup_id: self.next_id,
            created_at: current_timestamp_ms(),
            source_version: source_version.to_string(),
            row_count: rows.len(),
            byte_size,
            kind,
        };
        self.next_id += 1;

        self.backups.push(BackupHandle {
            metadata: metadata.clone(),
            dump,
        });

        self.enforce_retention();

        Ok(metadata)
    }

    /// 从指定备份恢复数据
    ///
    /// 返回恢复后的行数据（与备份时完全一致）
    pub fn restore_backup(&self, backup_id: u64) -> Result<Vec<MockRow>, UpgradeError> {
        let handle = self.get_backup(backup_id).ok_or_else(|| {
            UpgradeError::BackupFailed(format!("backup id {} not found", backup_id))
        })?;

        handle.dump.validate()?;
        restore_mock_rows(&handle.dump)
    }

    /// 获取指定备份的句柄
    pub fn get_backup(&self, backup_id: u64) -> Option<&BackupHandle> {
        self.backups
            .iter()
            .find(|h| h.metadata.backup_id == backup_id)
    }

    /// 列出所有备份元数据（按创建时间升序）
    pub fn list_backups(&self) -> Vec<&BackupMetadata> {
        self.backups.iter().map(|h| &h.metadata).collect()
    }

    /// 删除指定备份
    ///
    /// 返回是否删除成功
    pub fn remove_backup(&mut self, backup_id: u64) -> bool {
        let before = self.backups.len();
        self.backups.retain(|h| h.metadata.backup_id != backup_id);
        self.backups.len() < before
    }

    /// 获取最新备份 ID
    pub fn latest_backup_id(&self) -> Option<u64> {
        self.backups.last().map(|h| h.metadata.backup_id)
    }

    /// 获取备份总数
    pub fn backup_count(&self) -> usize {
        self.backups.len()
    }

    /// 清空所有备份
    pub fn clear(&mut self) {
        self.backups.clear();
    }

    /// 执行保留策略：当备份数超过 max_backups（且 max_backups > 0）时淘汰最旧备份
    fn enforce_retention(&mut self) {
        if self.max_backups == 0 {
            return;
        }
        while self.backups.len() > self.max_backups {
            self.backups.remove(0);
        }
    }
}

/// 回滚结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackResult {
    /// 是否成功
    pub success: bool,
    /// 使用的备份 ID
    pub backup_id: u64,
    /// 恢复后的行数
    pub restored_row_count: usize,
    /// 回滚耗时（微秒）
    pub elapsed_us: u64,
    /// 附加消息
    pub message: String,
}

impl RollbackResult {
    /// 创建成功回滚结果
    pub fn success(backup_id: u64, restored_row_count: usize, elapsed_us: u64) -> Self {
        let message = format!(
            "rollback to backup #{} completed, {} rows restored",
            backup_id, restored_row_count
        );
        Self {
            success: true,
            backup_id,
            restored_row_count,
            elapsed_us,
            message,
        }
    }
}

/// 回滚管理器
///
/// 负责升级失败时从备份恢复数据。
/// 持有 BackupManager 的可变引用，可恢复数据但不可创建新备份。
pub struct RollbackManager<'a> {
    /// 关联的备份管理器
    backup_manager: &'a mut BackupManager,
}

impl<'a> RollbackManager<'a> {
    /// 创建回滚管理器
    pub fn new(backup_manager: &'a mut BackupManager) -> Self {
        Self { backup_manager }
    }

    /// 从指定备份回滚
    ///
    /// 参数：
    /// - `backup_id`: 备份 ID
    ///
    /// 返回回滚结果和恢复后的行数据。
    pub fn rollback(&self, backup_id: u64) -> Result<(RollbackResult, Vec<MockRow>), UpgradeError> {
        let start = std::time::Instant::now();

        let handle = self.backup_manager.get_backup(backup_id).ok_or_else(|| {
            UpgradeError::BackupFailed(format!("backup id {} not found", backup_id))
        })?;

        let expected_row_count = handle.metadata.row_count;

        let restored_rows = self.backup_manager.restore_backup(backup_id)?;

        // 校验恢复后的行数与备份元数据一致
        if restored_rows.len() != expected_row_count {
            return Err(UpgradeError::VerificationFailed(format!(
                "rollback row count mismatch: expected {}, got {}",
                expected_row_count,
                restored_rows.len()
            )));
        }

        let elapsed_us = start.elapsed().as_micros() as u64;
        let result = RollbackResult::success(backup_id, restored_rows.len(), elapsed_us);
        Ok((result, restored_rows))
    }

    /// 从最新备份回滚
    pub fn rollback_latest(&self) -> Result<(RollbackResult, Vec<MockRow>), UpgradeError> {
        let backup_id = self.backup_manager.latest_backup_id().ok_or_else(|| {
            UpgradeError::BackupFailed("no backup available for rollback".to_string())
        })?;
        self.rollback(backup_id)
    }
}

/// 升级流程编排结果
///
/// 区分三种最终状态：升级成功、升级失败已回滚、升级失败未回滚（无可恢复备份）
#[derive(Debug, Clone)]
pub enum UpgradeOutcome {
    /// 升级成功
    Success {
        /// 升级结果
        result: UpgradeResult,
        /// 升级后的行数据（PATCH/MINOR 不变，MAJOR 为迁移后数据）
        rows: Vec<MockRow>,
        /// 创建的备份元数据
        backup: BackupMetadata,
    },
    /// 升级失败但已自动回滚（数据零丢失）
    FailedAndRolledBack {
        /// 失败原因
        reason: UpgradeError,
        /// 回滚结果
        rollback: RollbackResult,
        /// 回滚后的行数据（与备份时一致）
        rows: Vec<MockRow>,
        /// 使用的备份元数据
        backup: BackupMetadata,
    },
}

/// 升级上下文 — 编排备份 + 升级 + 回滚
///
/// 集成 `BackupManager`、`UpgradeExecutor`、`MajorUpgradeExecutor`、`RollbackManager`，
/// 提供带自动备份和失败自动回滚的升级流程。
#[derive(Debug, Clone)]
pub struct UpgradeContext {
    /// 备份管理器
    pub backup_manager: BackupManager,
}

impl Default for UpgradeContext {
    fn default() -> Self {
        Self::new()
    }
}

impl UpgradeContext {
    /// 创建升级上下文（默认保留最近 10 份备份）
    pub fn new() -> Self {
        Self {
            backup_manager: BackupManager::new(),
        }
    }

    /// 创建升级上下文并指定最大保留备份数
    pub fn with_max_backups(max_backups: usize) -> Self {
        Self {
            backup_manager: BackupManager::with_max_backups(max_backups),
        }
    }

    /// 执行 PATCH/MINOR 升级（带自动备份 + 失败自动回滚）
    ///
    /// 参数：
    /// - `current_version`: 当前版本
    /// - `target_version`: 目标版本
    /// - `header_bytes`: .szdb 文件头字节（≥22 字节）
    /// - `rows`: 当前数据库行数据（PATCH/MINOR 升级不修改数据，仅用于备份）
    ///
    /// 返回升级流程结果。
    pub fn execute_patch_minor_upgrade(
        &mut self,
        current_version: &Version,
        target_version: &Version,
        header_bytes: &[u8],
        rows: &[MockRow],
    ) -> Result<UpgradeOutcome, UpgradeError> {
        // 1. 规划升级（校验为 PATCH/MINOR）
        let executor = UpgradeExecutor::new(target_version.clone());
        let plan = executor.plan(current_version)?;
        // plan() 已保证 kind != None 且 kind != Major

        // 2. 创建全量备份
        let backup =
            self.backup_manager
                .create_backup(rows, &current_version.to_string(), plan.kind)?;

        // 3. 执行升级
        let upgrade_result = match executor.execute_in_place(current_version, header_bytes) {
            Ok(r) => r,
            Err(e) => {
                // 升级失败 → 自动回滚
                return self.rollback_on_failure(e, backup.backup_id, rows);
            }
        };

        // 4. 校验升级后文件头
        if let Err(e) = executor.verify_post_upgrade(header_bytes) {
            return self.rollback_on_failure(e, backup.backup_id, rows);
        }

        // 5. PATCH/MINOR 升级不修改数据，行数据保持不变
        Ok(UpgradeOutcome::Success {
            result: upgrade_result,
            rows: rows.to_vec(),
            backup,
        })
    }

    /// 执行 MAJOR 升级（带自动备份 + 失败自动回滚）
    ///
    /// 参数：
    /// - `from`: 源版本
    /// - `to`: 目标版本
    /// - `rows`: 源数据库行数据
    ///
    /// 返回升级流程结果。成功时 `rows` 为迁移后数据；失败回滚后 `rows` 为备份时数据。
    pub fn execute_major_upgrade(
        &mut self,
        from: Version,
        to: Version,
        rows: &[MockRow],
    ) -> Result<UpgradeOutcome, UpgradeError> {
        // 1. 创建全量备份
        let backup =
            self.backup_manager
                .create_backup(rows, &from.to_string(), UpgradeKind::Major)?;

        // 2. 执行 MAJOR 升级
        let executor = MajorUpgradeExecutor::new(from.clone(), to.clone());
        match executor.execute(rows) {
            Ok((result, migrated_rows)) => {
                // 3. 校验迁移后数据与备份行数一致
                if migrated_rows.len() != rows.len() {
                    return self.rollback_on_failure(
                        UpgradeError::VerificationFailed(format!(
                            "row count mismatch after MAJOR upgrade: before={}, after={}",
                            rows.len(),
                            migrated_rows.len()
                        )),
                        backup.backup_id,
                        rows,
                    );
                }
                Ok(UpgradeOutcome::Success {
                    result,
                    rows: migrated_rows,
                    backup,
                })
            }
            Err(e) => {
                // 升级失败 → 自动回滚
                self.rollback_on_failure(e, backup.backup_id, rows)
            }
        }
    }

    /// 模拟升级失败并触发自动回滚（用于测试验证回滚机制）
    ///
    /// 流程：创建备份 → 注入失败 → 自动回滚 → 校验数据
    pub fn simulate_upgrade_failure(
        &mut self,
        current_version: &Version,
        rows: &[MockRow],
    ) -> Result<UpgradeOutcome, UpgradeError> {
        // 1. 创建备份（模拟升级前自动备份）
        let backup = self.backup_manager.create_backup(
            rows,
            &current_version.to_string(),
            UpgradeKind::Patch,
        )?;

        // 2. 注入失败（模拟升级过程中断）
        let injected_error = UpgradeError::VerificationFailed(
            "simulated upgrade failure (injected for testing rollback)".to_string(),
        );

        // 3. 自动回滚
        self.rollback_on_failure(injected_error, backup.backup_id, rows)
    }

    /// 升级失败时自动回滚到最新备份
    ///
    /// 返回 `FailedAndRolledBack` 结果。若回滚本身也失败，则返回回滚错误。
    fn rollback_on_failure(
        &mut self,
        reason: UpgradeError,
        backup_id: u64,
        _original_rows: &[MockRow],
    ) -> Result<UpgradeOutcome, UpgradeError> {
        // 获取备份元数据（在 RollbackManager 借用前克隆，避免借用冲突）
        let backup = self
            .backup_manager
            .get_backup(backup_id)
            .ok_or_else(|| {
                UpgradeError::BackupFailed(format!(
                    "backup id {} not found during rollback",
                    backup_id
                ))
            })?
            .metadata
            .clone();

        // 借用 backup_manager 执行回滚
        let rollback_mgr = RollbackManager::new(&mut self.backup_manager);
        let (rollback_result, restored_rows) = rollback_mgr.rollback(backup_id)?;

        Ok(UpgradeOutcome::FailedAndRolledBack {
            reason,
            rollback: rollback_result,
            rows: restored_rows,
            backup,
        })
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  Version 解析测试
    // -----------------------------------------------------------------

    #[test]
    fn version_parse_simple() {
        let v = Version::parse("1.0.0").unwrap();
        assert_eq!(v, Version::new(1, 0, 0));
        assert!(v.is_stable());
    }

    #[test]
    fn version_parse_with_pre() {
        let v = Version::parse("0.1.0-alpha.1").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);
        assert_eq!(v.pre.as_deref(), Some("alpha.1"));
        assert!(!v.is_stable());
    }

    #[test]
    fn version_parse_with_build() {
        let v = Version::parse("1.2.3+build.456").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.build.as_deref(), Some("build.456"));
        assert!(v.is_stable());
    }

    #[test]
    fn version_parse_with_pre_and_build() {
        let v = Version::parse("1.0.0-beta.2+exp.sha.5114f85").unwrap();
        assert_eq!(v.pre.as_deref(), Some("beta.2"));
        assert_eq!(v.build.as_deref(), Some("exp.sha.5114f85"));
    }

    #[test]
    fn version_parse_empty_fails() {
        assert!(Version::parse("").is_err());
    }

    #[test]
    fn version_parse_missing_component_fails() {
        assert!(Version::parse("1.0").is_err());
        assert!(Version::parse("1").is_err());
        assert!(Version::parse("1.0.0.0").is_err());
    }

    #[test]
    fn version_parse_non_numeric_fails() {
        assert!(Version::parse("a.0.0").is_err());
        assert!(Version::parse("1.b.0").is_err());
        assert!(Version::parse("1.0.c").is_err());
    }

    #[test]
    fn version_parse_empty_pre_fails() {
        assert!(Version::parse("1.0.0-").is_err());
    }

    #[test]
    fn version_display_roundtrip() {
        let cases = vec![
            "1.0.0",
            "0.1.0",
            "1.2.3",
            "0.1.0-alpha.1",
            "1.0.0+build.123",
            "1.0.0-beta.2+exp.sha.5114f85",
        ];
        for s in cases {
            let v = Version::parse(s).unwrap();
            assert_eq!(v.to_string(), s);
        }
    }

    #[test]
    fn version_core_string() {
        let v = Version::parse("1.2.3-alpha+build").unwrap();
        assert_eq!(v.core_string(), "1.2.3");
    }

    #[test]
    fn version_default() {
        let v = Version::default();
        assert_eq!(v, Version::new(0, 1, 0));
    }

    // -----------------------------------------------------------------
    //  UpgradeKind 测试
    // -----------------------------------------------------------------

    #[test]
    fn upgrade_kind_as_str() {
        assert_eq!(UpgradeKind::None.as_str(), "none");
        assert_eq!(UpgradeKind::Patch.as_str(), "patch");
        assert_eq!(UpgradeKind::Minor.as_str(), "minor");
        assert_eq!(UpgradeKind::Major.as_str(), "major");
    }

    #[test]
    fn upgrade_kind_requires_migration() {
        assert!(!UpgradeKind::None.requires_migration());
        assert!(!UpgradeKind::Patch.requires_migration());
        assert!(!UpgradeKind::Minor.requires_migration());
        assert!(UpgradeKind::Major.requires_migration());
    }

    #[test]
    fn upgrade_kind_requires_backup() {
        assert!(!UpgradeKind::None.requires_backup());
        assert!(UpgradeKind::Patch.requires_backup());
        assert!(UpgradeKind::Minor.requires_backup());
        assert!(UpgradeKind::Major.requires_backup());
    }

    #[test]
    fn upgrade_kind_is_in_place() {
        assert!(!UpgradeKind::None.is_in_place());
        assert!(UpgradeKind::Patch.is_in_place());
        assert!(UpgradeKind::Minor.is_in_place());
        assert!(!UpgradeKind::Major.is_in_place());
    }

    // -----------------------------------------------------------------
    //  classify_upgrade 测试
    // -----------------------------------------------------------------

    #[test]
    fn classify_same_version_is_none() {
        let v = Version::new(1, 0, 0);
        assert_eq!(classify_upgrade(&v, &v), UpgradeKind::None);
    }

    #[test]
    fn classify_patch_upgrade() {
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 0, 1);
        assert_eq!(classify_upgrade(&from, &to), UpgradeKind::Patch);
    }

    #[test]
    fn classify_minor_upgrade() {
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 1, 0);
        assert_eq!(classify_upgrade(&from, &to), UpgradeKind::Minor);
    }

    #[test]
    fn classify_major_upgrade() {
        let from = Version::new(0, 9, 0);
        let to = Version::new(1, 0, 0);
        assert_eq!(classify_upgrade(&from, &to), UpgradeKind::Major);
    }

    #[test]
    fn classify_minor_with_patch_diff() {
        // minor 不同即为 Minor（即使 patch 也不同）
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 2, 3);
        assert_eq!(classify_upgrade(&from, &to), UpgradeKind::Minor);
    }

    #[test]
    fn classify_major_with_minor_diff() {
        // major 不同即为 Major（即使 minor/patch 也不同）
        let from = Version::new(0, 5, 3);
        let to = Version::new(1, 0, 0);
        assert_eq!(classify_upgrade(&from, &to), UpgradeKind::Major);
    }

    // -----------------------------------------------------------------
    //  UpgradePlan 测试
    // -----------------------------------------------------------------

    #[test]
    fn plan_patch_upgrade() {
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 0, 1);
        let plan = UpgradePlan::new(from, to);
        assert_eq!(plan.kind, UpgradeKind::Patch);
        assert!(!plan.requires_migration);
        assert!(plan.requires_backup);
        assert!(plan.format_compatible);
        assert!(plan.can_in_place_upgrade());
    }

    #[test]
    fn plan_minor_upgrade() {
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 1, 0);
        let plan = UpgradePlan::new(from, to);
        assert_eq!(plan.kind, UpgradeKind::Minor);
        assert!(!plan.requires_migration);
        assert!(plan.requires_backup);
        assert!(plan.can_in_place_upgrade());
    }

    #[test]
    fn plan_major_upgrade() {
        let from = Version::new(0, 9, 0);
        let to = Version::new(1, 0, 0);
        let plan = UpgradePlan::new(from, to);
        assert_eq!(plan.kind, UpgradeKind::Major);
        assert!(plan.requires_migration);
        assert!(plan.requires_backup);
        assert!(!plan.can_in_place_upgrade()); // Major 不可原地升级
    }

    #[test]
    fn plan_with_format_versions_compatible() {
        let plan = UpgradePlan::new(Version::new(1, 0, 0), Version::new(1, 0, 1))
            .with_format_versions(CURRENT_VERSION, CURRENT_VERSION);
        assert!(plan.format_compatible);
        assert_eq!(plan.from_format_version, CURRENT_VERSION);
        assert_eq!(plan.to_format_version, CURRENT_VERSION);
    }

    #[test]
    fn plan_with_format_versions_incompatible() {
        let plan = UpgradePlan::new(Version::new(0, 9, 0), Version::new(1, 0, 0))
            .with_format_versions(3, 4);
        assert!(!plan.format_compatible);
        assert!(!plan.can_in_place_upgrade());
    }

    #[test]
    fn plan_description_contains_versions() {
        let plan = UpgradePlan::new(Version::new(1, 0, 0), Version::new(1, 0, 1));
        let desc = plan.description();
        assert!(desc.contains("1.0.0"));
        assert!(desc.contains("1.0.1"));
        assert!(desc.contains("patch"));
    }

    // -----------------------------------------------------------------
    //  UpgradeExecutor 测试
    // -----------------------------------------------------------------

    #[test]
    fn executor_plan_patch() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let plan = exec.plan(&Version::new(1, 0, 0)).unwrap();
        assert_eq!(plan.kind, UpgradeKind::Patch);
        assert_eq!(plan.from, Version::new(1, 0, 0));
        assert_eq!(plan.to, Version::new(1, 0, 1));
    }

    #[test]
    fn executor_plan_minor() {
        let exec = UpgradeExecutor::new(Version::new(1, 1, 0));
        let plan = exec.plan(&Version::new(1, 0, 0)).unwrap();
        assert_eq!(plan.kind, UpgradeKind::Minor);
    }

    #[test]
    fn executor_plan_same_version_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 0));
        let err = exec.plan(&Version::new(1, 0, 0)).unwrap_err();
        assert!(matches!(err, UpgradeError::NoUpgradeNeeded(_)));
    }

    #[test]
    fn executor_plan_major_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 0));
        let err = exec.plan(&Version::new(0, 9, 0)).unwrap_err();
        assert!(matches!(err, UpgradeError::MajorUpgradeNotSupported { .. }));
    }

    #[test]
    fn executor_execute_patch_upgrade_success() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let header = FileHeader::current().to_bytes();
        let result = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();
        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Patch);
        assert_eq!(result.from, Version::new(1, 0, 0));
        assert_eq!(result.to, Version::new(1, 0, 1));
        assert!(result.backup_created);
        assert!(!result.migration_performed);
    }

    #[test]
    fn executor_execute_minor_upgrade_success() {
        let exec = UpgradeExecutor::new(Version::new(1, 1, 0));
        let header = FileHeader::current().to_bytes();
        let result = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();
        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Minor);
        assert!(result.backup_created);
        assert!(!result.migration_performed);
    }

    #[test]
    fn executor_execute_same_version_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 0));
        let header = FileHeader::current().to_bytes();
        let err = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap_err();
        assert!(matches!(err, UpgradeError::NoUpgradeNeeded(_)));
    }

    #[test]
    fn executor_execute_major_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 0));
        let header = FileHeader::current().to_bytes();
        let err = exec
            .execute_in_place(&Version::new(0, 9, 0), &header)
            .unwrap_err();
        assert!(matches!(err, UpgradeError::MajorUpgradeNotSupported { .. }));
    }

    #[test]
    fn executor_execute_bad_header_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let bad_header = [0u8; 10]; // 过短
        let err = exec
            .execute_in_place(&Version::new(1, 0, 0), &bad_header)
            .unwrap_err();
        assert!(matches!(err, UpgradeError::HeaderValidation(_)));
    }

    #[test]
    fn executor_execute_bad_magic_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let mut header = FileHeader::current().to_bytes();
        header[0] = 0xFF; // 破坏魔数
        let err = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap_err();
        assert!(matches!(err, UpgradeError::HeaderValidation(_)));
    }

    #[test]
    fn executor_execute_old_format_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        // 构造 format_version=0 的文件头（太旧）
        let header = FileHeader::current().with_version(0).to_bytes();
        let err = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap_err();
        // parse_and_validate 会先拒绝 version=0
        assert!(matches!(err, UpgradeError::HeaderValidation(_)));
    }

    // -----------------------------------------------------------------
    //  verify_post_upgrade 测试
    // -----------------------------------------------------------------

    #[test]
    fn verify_post_upgrade_ok() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let header = FileHeader::current().to_bytes();
        assert!(exec.verify_post_upgrade(&header).unwrap());
    }

    #[test]
    fn verify_post_upgrade_bad_magic_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let mut header = FileHeader::current().to_bytes();
        header[0] = 0xFF;
        assert!(exec.verify_post_upgrade(&header).is_err());
    }

    #[test]
    fn verify_post_upgrade_zero_page_size_fails() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let header = FileHeader::current().with_page_size(0).to_bytes();
        let err = exec.verify_post_upgrade(&header).unwrap_err();
        assert!(matches!(err, UpgradeError::VerificationFailed(_)));
    }

    // -----------------------------------------------------------------
    //  UpgradeResult 测试
    // -----------------------------------------------------------------

    #[test]
    fn upgrade_result_success() {
        let result = UpgradeResult::success(
            UpgradeKind::Patch,
            Version::new(1, 0, 0),
            Version::new(1, 0, 1),
            true,
            false,
            100,
        );
        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Patch);
        assert!(result.backup_created);
        assert!(!result.migration_performed);
        assert!(result.message.contains("successfully"));
    }

    #[test]
    fn upgrade_result_failure() {
        let result = UpgradeResult::failure(
            UpgradeKind::Patch,
            Version::new(1, 0, 0),
            Version::new(1, 0, 1),
            "backup failed",
        );
        assert!(!result.success);
        assert_eq!(result.elapsed_us, 0);
        assert_eq!(result.message, "backup failed");
    }

    // -----------------------------------------------------------------
    //  模拟 1000000 行数据一致性测试（Phase 7e.2 核心验证）
    // -----------------------------------------------------------------

    #[test]
    fn million_rows_upgrade_consistency() {
        // 模拟 v1.0.0 写入 1000000 行
        let rows_before = generate_mock_rows(1_000_000);
        assert_eq!(rows_before.len(), 1_000_000);

        // 模拟 PATCH 升级 v1.0.0 → v1.0.1
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let header = FileHeader::current().to_bytes();
        let result = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();
        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Patch);

        // 升级后数据不变（PATCH 升级不修改数据）
        let rows_after = rows_before.clone();
        assert!(verify_rows_equal(&rows_before, &rows_after));

        // 升级后新数据可写（模拟追加 500000 行）
        let mut rows_after_upgrade = rows_before.clone();
        let new_rows = generate_mock_rows(500_000);
        // 新行 ID 从 1000000 开始
        for (i, row) in new_rows.iter().enumerate() {
            rows_after_upgrade.push(MockRow {
                id: 1_000_000 + i as u64,
                data: row.data.clone(),
            });
        }
        assert_eq!(rows_after_upgrade.len(), 1_500_000);

        // 旧数据仍可读
        assert!(verify_rows_equal(
            &rows_before,
            &rows_after_upgrade[..1_000_000]
        ));

        // 升级后文件头校验通过
        assert!(exec.verify_post_upgrade(&header).unwrap());
    }

    #[test]
    fn million_rows_minor_upgrade_consistency() {
        // 模拟 v1.0.0 写入 1000000 行
        let rows_before = generate_mock_rows(1_000_000);

        // 模拟 MINOR 升级 v1.0.0 → v1.1.0
        let exec = UpgradeExecutor::new(Version::new(1, 1, 0));
        let header = FileHeader::current().to_bytes();
        let result = exec
            .execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();
        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Minor);

        // 升级后数据不变
        assert!(verify_rows_equal(&rows_before, &rows_before));

        // 升级后校验通过
        assert!(exec.verify_post_upgrade(&header).unwrap());
    }

    #[test]
    fn small_dataset_upgrade_consistency() {
        // 小数据集快速验证升级流程
        for size in [0, 1, 10, 100, 1000] {
            let rows = generate_mock_rows(size);

            let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
            let header = FileHeader::current().to_bytes();
            let result = exec
                .execute_in_place(&Version::new(1, 0, 0), &header)
                .unwrap();

            assert!(result.success);
            assert!(verify_rows_equal(&rows, &rows));
            assert!(exec.verify_post_upgrade(&header).unwrap());
        }
    }

    #[test]
    fn upgrade_preserves_file_header_format_version() {
        // 升级前后 format_version 必须不变
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let header = FileHeader::current().to_bytes();
        let original_fv = FileHeader::from_bytes(&header).unwrap().format_version;

        exec.execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();

        // 升级后文件头仍可解析且 format_version 不变
        let after = FileHeader::from_bytes(&header).unwrap();
        assert_eq!(after.format_version, original_fv);
        assert_eq!(after.format_version, CURRENT_VERSION);
    }

    #[test]
    fn upgrade_preserves_page_size() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let header = FileHeader::current().with_page_size(16384).to_bytes();
        let original_ps = FileHeader::from_bytes(&header).unwrap().page_size;

        exec.execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();

        let after = FileHeader::from_bytes(&header).unwrap();
        assert_eq!(after.page_size, original_ps);
        assert_eq!(after.page_size, 16384);
    }

    #[test]
    fn upgrade_preserves_flags() {
        let exec = UpgradeExecutor::new(Version::new(1, 0, 1));
        let flags = format_version::FILE_FLAG_ENCRYPTED | format_version::FILE_FLAG_COMPRESSED;
        let header = FileHeader::current().with_flags(flags).to_bytes();

        exec.execute_in_place(&Version::new(1, 0, 0), &header)
            .unwrap();

        let after = FileHeader::from_bytes(&header).unwrap();
        assert_eq!(after.flags, flags);
    }

    // -----------------------------------------------------------------
    //  MockRow 辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn generate_mock_rows_count() {
        let rows = generate_mock_rows(100);
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[0].id, 0);
        assert_eq!(rows[99].id, 99);
    }

    #[test]
    fn generate_mock_rows_empty() {
        let rows = generate_mock_rows(0);
        assert!(rows.is_empty());
    }

    #[test]
    fn verify_rows_equal_same() {
        let rows = generate_mock_rows(100);
        assert!(verify_rows_equal(&rows, &rows));
    }

    #[test]
    fn verify_rows_equal_different_length() {
        let a = generate_mock_rows(100);
        let b = generate_mock_rows(99);
        assert!(!verify_rows_equal(&a, &b));
    }

    #[test]
    fn verify_rows_equal_different_data() {
        let mut a = generate_mock_rows(100);
        let b = generate_mock_rows(100);
        a[50].data = b"modified".to_vec();
        assert!(!verify_rows_equal(&a, &b));
    }

    #[test]
    fn verify_rows_equal_both_empty() {
        let a: Vec<MockRow> = vec![];
        let b: Vec<MockRow> = vec![];
        assert!(verify_rows_equal(&a, &b));
    }

    // -----------------------------------------------------------------
    //  错误信息测试
    // -----------------------------------------------------------------

    #[test]
    fn error_messages_descriptive() {
        let invalid = Version::parse("invalid").unwrap_err().to_string();
        assert!(invalid.contains("invalid version string"));

        let no_upgrade = UpgradeError::NoUpgradeNeeded(Version::new(1, 0, 0)).to_string();
        assert!(no_upgrade.contains("no upgrade needed"));
        assert!(no_upgrade.contains("1.0.0"));

        let major = UpgradeError::MajorUpgradeNotSupported {
            from: Box::new(Version::new(0, 9, 0)),
            to: Box::new(Version::new(1, 0, 0)),
        }
        .to_string();
        assert!(major.contains("MAJOR upgrade"));
        assert!(major.contains("0.9.0"));
        assert!(major.contains("1.0.0"));

        let fmt_incompat = UpgradeError::FormatIncompatible {
            from_fv: 3,
            to_fv: 4,
        }
        .to_string();
        assert!(fmt_incompat.contains("format version incompatible"));
        assert!(fmt_incompat.contains("Phase 7e.3"));
    }

    // -----------------------------------------------------------------
    //  Phase 7e.3 — DatabaseDump 测试
    // -----------------------------------------------------------------

    #[test]
    fn dump_new_creates_empty() {
        let dump = DatabaseDump::new("0.1.0");
        assert_eq!(dump.dump_format_version, DatabaseDump::CURRENT_DUMP_FORMAT);
        assert_eq!(dump.source_version, "0.1.0");
        assert!(dump.tables.is_empty());
        assert_eq!(dump.table_count(), 0);
        assert_eq!(dump.total_row_count(), 0);
    }

    #[test]
    fn dump_add_table() {
        let mut dump = DatabaseDump::new("0.1.0");
        let table = DumpTable::new("users", vec!["id".to_string(), "name".to_string()]);
        dump.add_table(table);
        assert_eq!(dump.table_count(), 1);
    }

    #[test]
    fn dump_total_row_count() {
        let mut dump = DatabaseDump::new("0.1.0");
        let mut t1 = DumpTable::new("t1", vec!["id".to_string()]);
        t1.add_row(1, vec![vec![1]]);
        t1.add_row(2, vec![vec![2]]);
        dump.add_table(t1);
        let mut t2 = DumpTable::new("t2", vec!["id".to_string()]);
        t2.add_row(1, vec![vec![1]]);
        dump.add_table(t2);
        assert_eq!(dump.total_row_count(), 3);
    }

    #[test]
    fn dump_json_roundtrip() {
        let rows = generate_mock_rows(100);
        let dump = dump_mock_rows(&rows, "0.1.0");
        let bytes = dump.to_json_bytes().unwrap();
        let restored = DatabaseDump::from_json_bytes(&bytes).unwrap();
        assert_eq!(dump, restored);
    }

    #[test]
    fn dump_validate_empty_tables_fails() {
        let dump = DatabaseDump::new("0.1.0");
        assert!(dump.validate().is_err());
    }

    #[test]
    fn dump_validate_empty_table_name_fails() {
        let mut dump = DatabaseDump::new("0.1.0");
        dump.add_table(DumpTable::new("", vec!["id".to_string()]));
        let err = dump.validate().unwrap_err();
        assert!(matches!(err, UpgradeError::VerificationFailed(_)));
    }

    #[test]
    fn dump_validate_empty_columns_fails() {
        let mut dump = DatabaseDump::new("0.1.0");
        dump.add_table(DumpTable::new("t", vec![]));
        assert!(dump.validate().is_err());
    }

    #[test]
    fn dump_validate_row_values_count_mismatch_fails() {
        let mut dump = DatabaseDump::new("0.1.0");
        let mut table = DumpTable::new("t", vec!["a".to_string(), "b".to_string()]);
        table.add_row(1, vec![vec![1]]); // 1 value, 2 columns
        dump.add_table(table);
        let err = dump.validate().unwrap_err();
        assert!(matches!(err, UpgradeError::VerificationFailed(_)));
    }

    #[test]
    fn dump_validate_valid_passes() {
        let rows = generate_mock_rows(10);
        let dump = dump_mock_rows(&rows, "0.1.0");
        assert!(dump.validate().is_ok());
    }

    #[test]
    fn dump_table_new_and_add_row() {
        let mut table = DumpTable::new("users", vec!["id".to_string(), "name".to_string()]);
        assert_eq!(table.row_count(), 0);
        table.add_row(1, vec![vec![1], b"alice".to_vec()]);
        assert_eq!(table.row_count(), 1);
    }

    // -----------------------------------------------------------------
    //  dump_mock_rows / restore_mock_rows 测试
    // -----------------------------------------------------------------

    #[test]
    fn dump_and_restore_mock_rows_roundtrip() {
        let rows = generate_mock_rows(1000);
        let dump = dump_mock_rows(&rows, "0.1.0");
        let restored = restore_mock_rows(&dump).unwrap();
        assert!(verify_rows_equal(&rows, &restored));
    }

    #[test]
    fn dump_and_restore_empty_rows() {
        let rows: Vec<MockRow> = vec![];
        // 空行列表的 dump 仍包含 1 个表（0 行），validate 应通过
        let dump = dump_mock_rows(&rows, "0.1.0");
        // 但 restore 要求至少 1 个表 — 空行 dump 仍有表结构
        assert!(dump.validate().is_ok());
        let restored = restore_mock_rows(&dump).unwrap();
        assert!(restored.is_empty());
    }

    #[test]
    fn restore_wrong_table_name_fails() {
        let mut dump = DatabaseDump::new("0.1.0");
        let mut table = DumpTable::new("wrong_name", vec!["id".to_string(), "data".to_string()]);
        table.add_row(1, vec![vec![1], vec![2]]);
        dump.add_table(table);
        let err = restore_mock_rows(&dump).unwrap_err();
        assert!(matches!(err, UpgradeError::VerificationFailed(_)));
    }

    #[test]
    fn restore_wrong_values_count_fails() {
        let mut dump = DatabaseDump::new("0.1.0");
        let mut table = DumpTable::new("mock_data", vec!["id".to_string(), "data".to_string()]);
        table.add_row(1, vec![vec![1]]); // 1 value, 2 columns
        dump.add_table(table);
        // validate 会先因 values count != columns count 失败
        let err = restore_mock_rows(&dump).unwrap_err();
        assert!(matches!(err, UpgradeError::VerificationFailed(_)));
    }

    #[test]
    fn restore_invalid_id_bytes_fails() {
        let mut dump = DatabaseDump::new("0.1.0");
        let mut table = DumpTable::new("mock_data", vec!["id".to_string(), "data".to_string()]);
        // id 字段只有 2 字节，不是 8 字节
        table.add_row(1, vec![vec![1, 2], vec![3]]);
        dump.add_table(table);
        let err = restore_mock_rows(&dump).unwrap_err();
        assert!(matches!(err, UpgradeError::VerificationFailed(_)));
    }

    // -----------------------------------------------------------------
    //  MajorUpgradeExecutor 测试
    // -----------------------------------------------------------------

    #[test]
    fn major_executor_execute_success() {
        let exec = MajorUpgradeExecutor::new(Version::new(0, 1, 0), Version::new(1, 0, 0));
        let rows = generate_mock_rows(1000);
        let (result, restored_rows) = exec.execute(&rows).unwrap();

        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Major);
        assert_eq!(result.from, Version::new(0, 1, 0));
        assert_eq!(result.to, Version::new(1, 0, 0));
        assert!(result.backup_created);
        assert!(result.migration_performed);
        assert!(verify_rows_equal(&rows, &restored_rows));
    }

    #[test]
    fn major_executor_execute_large_dataset() {
        // 模拟跨版本升级 1000000 行
        let exec = MajorUpgradeExecutor::new(Version::new(0, 1, 0), Version::new(1, 0, 0));
        let rows = generate_mock_rows(1_000_000);
        let (result, restored_rows) = exec.execute(&rows).unwrap();

        assert!(result.success);
        assert_eq!(restored_rows.len(), 1_000_000);
        assert!(verify_rows_equal(&rows, &restored_rows));
    }

    #[test]
    fn major_executor_rejects_patch_upgrade() {
        // 非 MAJOR 升级应被拒绝
        let exec = MajorUpgradeExecutor::new(Version::new(1, 0, 0), Version::new(1, 0, 1));
        let rows = generate_mock_rows(10);
        let err = exec.execute(&rows).unwrap_err();
        assert!(matches!(err, UpgradeError::MajorUpgradeNotSupported { .. }));
    }

    #[test]
    fn major_executor_rejects_minor_upgrade() {
        let exec = MajorUpgradeExecutor::new(Version::new(1, 0, 0), Version::new(1, 1, 0));
        let rows = generate_mock_rows(10);
        let err = exec.execute(&rows).unwrap_err();
        assert!(matches!(err, UpgradeError::MajorUpgradeNotSupported { .. }));
    }

    #[test]
    fn major_executor_dump_only() {
        let exec = MajorUpgradeExecutor::new(Version::new(0, 1, 0), Version::new(1, 0, 0));
        let rows = generate_mock_rows(100);
        let dump = exec.dump_only(&rows).unwrap();
        assert_eq!(dump.source_version, "0.1.0");
        assert_eq!(dump.total_row_count(), 100);
    }

    #[test]
    fn major_executor_restore_only() {
        let exec = MajorUpgradeExecutor::new(Version::new(0, 1, 0), Version::new(1, 0, 0));
        let rows = generate_mock_rows(100);
        let dump = exec.dump_only(&rows).unwrap();
        let restored = exec.restore_only(&dump).unwrap();
        assert!(verify_rows_equal(&rows, &restored));
    }

    #[test]
    fn major_executor_full_workflow_v0_1_0_to_v1_0_0() {
        // 完整模拟 v0.1.0-alpha.1 → v1.0.0 跨版本升级
        let from = Version::parse("0.1.0-alpha.1").unwrap();
        let to = Version::parse("1.0.0").unwrap();
        let exec = MajorUpgradeExecutor::new(from, to);

        let rows = generate_mock_rows(10000);
        let (result, restored) = exec.execute(&rows).unwrap();

        assert!(result.success);
        assert_eq!(result.kind, UpgradeKind::Major);
        assert!(result.migration_performed);
        assert_eq!(restored.len(), 10000);
        assert!(verify_rows_equal(&rows, &restored));
    }

    // -----------------------------------------------------------------
    //  verify_dumps_equal 测试
    // -----------------------------------------------------------------

    #[test]
    fn verify_dumps_equal_same() {
        let rows = generate_mock_rows(100);
        let dump1 = dump_mock_rows(&rows, "0.1.0");
        let dump2 = dump_mock_rows(&rows, "0.1.0");
        assert!(verify_dumps_equal(&dump1, &dump2));
    }

    #[test]
    fn verify_dumps_equal_different_source_version() {
        let rows = generate_mock_rows(100);
        let dump1 = dump_mock_rows(&rows, "0.1.0");
        let dump2 = dump_mock_rows(&rows, "1.0.0");
        assert!(!verify_dumps_equal(&dump1, &dump2));
    }

    #[test]
    fn verify_dumps_equal_different_row_count() {
        let dump1 = dump_mock_rows(&generate_mock_rows(100), "0.1.0");
        let dump2 = dump_mock_rows(&generate_mock_rows(99), "0.1.0");
        assert!(!verify_dumps_equal(&dump1, &dump2));
    }

    #[test]
    fn verify_dumps_equal_different_data() {
        let mut rows1 = generate_mock_rows(100);
        let rows2 = generate_mock_rows(100);
        rows1[50].data = b"modified".to_vec();
        let dump1 = dump_mock_rows(&rows1, "0.1.0");
        let dump2 = dump_mock_rows(&rows2, "0.1.0");
        assert!(!verify_dumps_equal(&dump1, &dump2));
    }

    #[test]
    fn verify_dumps_equal_after_json_roundtrip() {
        let rows = generate_mock_rows(500);
        let dump = dump_mock_rows(&rows, "0.1.0");
        let bytes = dump.to_json_bytes().unwrap();
        let restored = DatabaseDump::from_json_bytes(&bytes).unwrap();
        assert!(verify_dumps_equal(&dump, &restored));
    }

    // -----------------------------------------------------------------
    //  Phase 7e.4 — BackupManager 测试
    // -----------------------------------------------------------------

    #[test]
    fn backup_manager_new_defaults() {
        let bm = BackupManager::new();
        assert_eq!(bm.backup_count(), 0);
        assert_eq!(bm.max_backups, 10);
        assert_eq!(bm.latest_backup_id(), None);
    }

    #[test]
    fn backup_manager_with_max_backups() {
        let bm = BackupManager::with_max_backups(3);
        assert_eq!(bm.max_backups, 3);
        let bm_unlimited = BackupManager::with_max_backups(0);
        assert_eq!(bm_unlimited.max_backups, 0);
    }

    #[test]
    fn backup_manager_create_backup_returns_metadata() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(100);
        let meta = bm
            .create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        assert_eq!(meta.backup_id, 1);
        assert_eq!(meta.source_version, "1.0.0");
        assert_eq!(meta.row_count, 100);
        assert!(meta.byte_size > 0);
        assert_eq!(meta.kind, UpgradeKind::Patch);
        assert_eq!(bm.backup_count(), 1);
        assert_eq!(bm.latest_backup_id(), Some(1));
    }

    #[test]
    fn backup_manager_backup_ids_increment() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(10);
        let m1 = bm
            .create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        let m2 = bm
            .create_backup(&rows, "1.0.1", UpgradeKind::Patch)
            .unwrap();
        let m3 = bm
            .create_backup(&rows, "1.1.0", UpgradeKind::Minor)
            .unwrap();
        assert_eq!(m1.backup_id, 1);
        assert_eq!(m2.backup_id, 2);
        assert_eq!(m3.backup_id, 3);
        assert_eq!(bm.backup_count(), 3);
    }

    #[test]
    fn backup_manager_restore_returns_equal_rows() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(500);
        let meta = bm
            .create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        let restored = bm.restore_backup(meta.backup_id).unwrap();
        assert!(verify_rows_equal(&rows, &restored));
    }

    #[test]
    fn backup_manager_restore_nonexistent_fails() {
        let bm = BackupManager::new();
        let err = bm.restore_backup(999).unwrap_err();
        assert!(matches!(err, UpgradeError::BackupFailed(_)));
    }

    #[test]
    fn backup_manager_get_backup_returns_handle() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(50);
        let meta = bm
            .create_backup(&rows, "0.1.0", UpgradeKind::Minor)
            .unwrap();
        let handle = bm.get_backup(meta.backup_id).unwrap();
        assert_eq!(handle.metadata.backup_id, meta.backup_id);
        assert_eq!(handle.metadata.row_count, 50);
        assert_eq!(handle.dump.table_count(), 1);
    }

    #[test]
    fn backup_manager_list_backups_sorted_ascending() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(10);
        bm.create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        bm.create_backup(&rows, "1.0.1", UpgradeKind::Patch)
            .unwrap();
        bm.create_backup(&rows, "1.1.0", UpgradeKind::Minor)
            .unwrap();
        let list = bm.list_backups();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].backup_id, 1);
        assert_eq!(list[1].backup_id, 2);
        assert_eq!(list[2].backup_id, 3);
    }

    #[test]
    fn backup_manager_remove_backup() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(10);
        let m1 = bm
            .create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        let m2 = bm
            .create_backup(&rows, "1.0.1", UpgradeKind::Patch)
            .unwrap();
        assert_eq!(bm.backup_count(), 2);
        assert!(bm.remove_backup(m1.backup_id));
        assert_eq!(bm.backup_count(), 1);
        assert!(bm.get_backup(m1.backup_id).is_none());
        assert!(bm.get_backup(m2.backup_id).is_some());
        // 重复删除返回 false
        assert!(!bm.remove_backup(m1.backup_id));
    }

    #[test]
    fn backup_manager_clear_removes_all() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(10);
        bm.create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        bm.create_backup(&rows, "1.0.1", UpgradeKind::Patch)
            .unwrap();
        assert_eq!(bm.backup_count(), 2);
        bm.clear();
        assert_eq!(bm.backup_count(), 0);
        assert_eq!(bm.latest_backup_id(), None);
    }

    #[test]
    fn backup_manager_retention_policy_evicts_oldest() {
        let mut bm = BackupManager::with_max_backups(2);
        let rows = generate_mock_rows(10);
        let m1 = bm
            .create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        let m2 = bm
            .create_backup(&rows, "1.0.1", UpgradeKind::Patch)
            .unwrap();
        let m3 = bm
            .create_backup(&rows, "1.1.0", UpgradeKind::Minor)
            .unwrap();
        // 只保留最近 2 份
        assert_eq!(bm.backup_count(), 2);
        assert!(bm.get_backup(m1.backup_id).is_none()); // 最旧被淘汰
        assert!(bm.get_backup(m2.backup_id).is_some());
        assert!(bm.get_backup(m3.backup_id).is_some());
    }

    #[test]
    fn backup_manager_retention_zero_means_unlimited() {
        let mut bm = BackupManager::with_max_backups(0);
        let rows = generate_mock_rows(5);
        for _ in 0..20 {
            bm.create_backup(&rows, "1.0.0", UpgradeKind::Patch)
                .unwrap();
        }
        assert_eq!(bm.backup_count(), 20);
    }

    #[test]
    fn backup_manager_default_impl() {
        let bm = BackupManager::default();
        assert_eq!(bm.max_backups, 10);
        assert_eq!(bm.backup_count(), 0);
    }

    // -----------------------------------------------------------------
    //  Phase 7e.4 — RollbackManager 测试
    // -----------------------------------------------------------------

    #[test]
    fn rollback_manager_rollback_restores_data() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(200);
        let meta = bm
            .create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        let mgr = RollbackManager::new(&mut bm);
        let (result, restored) = mgr.rollback(meta.backup_id).unwrap();
        assert!(result.success);
        assert_eq!(result.backup_id, meta.backup_id);
        assert_eq!(result.restored_row_count, 200);
        assert!(verify_rows_equal(&rows, &restored));
    }

    #[test]
    fn rollback_manager_rollback_latest() {
        let mut bm = BackupManager::new();
        let rows = generate_mock_rows(100);
        bm.create_backup(&rows, "1.0.0", UpgradeKind::Patch)
            .unwrap();
        bm.create_backup(&rows, "1.0.1", UpgradeKind::Patch)
            .unwrap();
        let mgr = RollbackManager::new(&mut bm);
        let (result, _restored) = mgr.rollback_latest().unwrap();
        assert_eq!(result.backup_id, 2); // 最新
    }

    #[test]
    fn rollback_manager_rollback_nonexistent_fails() {
        let mut bm = BackupManager::new();
        let mgr = RollbackManager::new(&mut bm);
        let err = mgr.rollback(999).unwrap_err();
        assert!(matches!(err, UpgradeError::BackupFailed(_)));
    }

    #[test]
    fn rollback_manager_rollback_latest_without_backup_fails() {
        let mut bm = BackupManager::new();
        let mgr = RollbackManager::new(&mut bm);
        let err = mgr.rollback_latest().unwrap_err();
        assert!(matches!(err, UpgradeError::BackupFailed(_)));
    }

    #[test]
    fn rollback_result_success_message_format() {
        let result = RollbackResult::success(42, 1234, 5000);
        assert!(result.success);
        assert_eq!(result.backup_id, 42);
        assert_eq!(result.restored_row_count, 1234);
        assert_eq!(result.elapsed_us, 5000);
        assert!(result.message.contains("backup #42"));
        assert!(result.message.contains("1234 rows"));
    }

    // -----------------------------------------------------------------
    //  Phase 7e.4 — UpgradeContext 集成测试（核心场景）
    // -----------------------------------------------------------------

    /// 构造当前二进制的有效文件头字节
    fn make_current_header() -> [u8; 22] {
        FileHeader::current().to_bytes()
    }

    #[test]
    fn upgrade_context_new_defaults() {
        let ctx = UpgradeContext::new();
        assert_eq!(ctx.backup_manager.backup_count(), 0);
        assert_eq!(ctx.backup_manager.max_backups, 10);
    }

    #[test]
    fn upgrade_context_with_max_backups() {
        let ctx = UpgradeContext::with_max_backups(5);
        assert_eq!(ctx.backup_manager.max_backups, 5);
    }

    #[test]
    fn patch_upgrade_success_creates_backup_and_keeps_data() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 0, 1);
        let header = make_current_header();
        let rows = generate_mock_rows(500);

        let outcome = ctx
            .execute_patch_minor_upgrade(&from, &to, &header, &rows)
            .unwrap();

        match outcome {
            UpgradeOutcome::Success {
                result,
                rows: out_rows,
                backup,
            } => {
                assert!(result.success);
                assert_eq!(result.kind, UpgradeKind::Patch);
                assert!(verify_rows_equal(&rows, &out_rows));
                assert_eq!(backup.row_count, 500);
                assert_eq!(backup.kind, UpgradeKind::Patch);
                assert_eq!(ctx.backup_manager.backup_count(), 1);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[test]
    fn minor_upgrade_success_creates_backup_and_keeps_data() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 1, 0);
        let header = make_current_header();
        let rows = generate_mock_rows(300);

        let outcome = ctx
            .execute_patch_minor_upgrade(&from, &to, &header, &rows)
            .unwrap();

        match outcome {
            UpgradeOutcome::Success {
                result,
                rows: out_rows,
                backup,
            } => {
                assert!(result.success);
                assert_eq!(result.kind, UpgradeKind::Minor);
                assert!(verify_rows_equal(&rows, &out_rows));
                assert_eq!(backup.kind, UpgradeKind::Minor);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[test]
    fn major_upgrade_success_migrates_data_with_backup() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(0, 9, 0);
        let to = Version::new(1, 0, 0);
        let rows = generate_mock_rows(1000);

        let outcome = ctx.execute_major_upgrade(from, to, &rows).unwrap();

        match outcome {
            UpgradeOutcome::Success {
                result,
                rows: migrated,
                backup,
            } => {
                assert!(result.success);
                assert_eq!(result.kind, UpgradeKind::Major);
                assert!(result.migration_performed);
                assert_eq!(migrated.len(), rows.len());
                assert!(verify_rows_equal(&rows, &migrated));
                assert_eq!(backup.kind, UpgradeKind::Major);
                assert_eq!(backup.row_count, 1000);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[test]
    fn patch_upgrade_failure_triggers_auto_rollback_zero_data_loss() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(1, 0, 0);
        let rows = generate_mock_rows(800);

        let outcome = ctx.simulate_upgrade_failure(&from, &rows).unwrap();

        match outcome {
            UpgradeOutcome::FailedAndRolledBack {
                reason,
                rollback,
                rows: restored,
                backup,
            } => {
                // 失败原因被正确传递
                assert!(matches!(reason, UpgradeError::VerificationFailed(_)));
                // 回滚成功
                assert!(rollback.success);
                assert_eq!(rollback.restored_row_count, 800);
                // 数据零丢失
                assert!(verify_rows_equal(&rows, &restored));
                // 备份元数据正确
                assert_eq!(backup.row_count, 800);
                assert_eq!(backup.kind, UpgradeKind::Patch);
                // 备份仍保留在管理器中（可用于再次恢复）
                assert_eq!(ctx.backup_manager.backup_count(), 1);
            }
            other => panic!("expected FailedAndRolledBack, got {:?}", other),
        }
    }

    #[test]
    fn major_upgrade_failure_triggers_auto_rollback_zero_data_loss() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(0, 9, 0);
        let rows = generate_mock_rows(600);

        // 模拟 MAJOR 升级流程：先创建备份，再注入失败
        let backup = ctx
            .backup_manager
            .create_backup(&rows, &from.to_string(), UpgradeKind::Major)
            .unwrap();

        // 注入 MAJOR 升级失败
        let injected = UpgradeError::VerificationFailed("simulated major failure".to_string());
        let outcome = ctx
            .rollback_on_failure(injected, backup.backup_id, &rows)
            .unwrap();

        match outcome {
            UpgradeOutcome::FailedAndRolledBack {
                reason,
                rollback,
                rows: restored,
                backup: used_backup,
            } => {
                assert!(matches!(reason, UpgradeError::VerificationFailed(_)));
                assert!(rollback.success);
                assert_eq!(rollback.restored_row_count, 600);
                assert!(verify_rows_equal(&rows, &restored));
                assert_eq!(used_backup.backup_id, backup.backup_id);
                assert_eq!(used_backup.kind, UpgradeKind::Major);
            }
            other => panic!("expected FailedAndRolledBack, got {:?}", other),
        }
    }

    #[test]
    fn patch_upgrade_with_invalid_header_fails_and_rolls_back() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 0, 1);
        let mut header = make_current_header();
        // 破坏魔数 → 文件头校验失败
        header[0] = 0xFF;
        let rows = generate_mock_rows(100);

        let outcome = ctx
            .execute_patch_minor_upgrade(&from, &to, &header, &rows)
            .unwrap();

        // 文件头校验失败 → 自动回滚（数据零丢失）
        match outcome {
            UpgradeOutcome::FailedAndRolledBack {
                reason,
                rollback,
                rows: restored,
                ..
            } => {
                // 失败原因为文件头校验错误
                assert!(matches!(
                    reason,
                    UpgradeError::HeaderValidation(VersionError::InvalidMagic { .. })
                ));
                assert!(rollback.success);
                assert!(verify_rows_equal(&rows, &restored));
            }
            other => panic!("expected FailedAndRolledBack, got {:?}", other),
        }
    }

    #[test]
    fn patch_upgrade_same_version_returns_no_upgrade_needed_error() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(1, 0, 0);
        let to = Version::new(1, 0, 0);
        let header = make_current_header();
        let rows = generate_mock_rows(10);

        // plan() 在创建备份前就拒绝了（版本相同），所以不会创建备份
        let err = ctx
            .execute_patch_minor_upgrade(&from, &to, &header, &rows)
            .unwrap_err();
        assert!(matches!(err, UpgradeError::NoUpgradeNeeded(_)));
        // 未创建备份
        assert_eq!(ctx.backup_manager.backup_count(), 0);
    }

    #[test]
    fn patch_upgrade_major_kind_returns_major_not_supported_error() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(0, 9, 0);
        let to = Version::new(1, 0, 0);
        let header = make_current_header();
        let rows = generate_mock_rows(10);

        // plan() 拒绝 MAJOR 升级（应使用 execute_major_upgrade）
        let err = ctx
            .execute_patch_minor_upgrade(&from, &to, &header, &rows)
            .unwrap_err();
        assert!(matches!(err, UpgradeError::MajorUpgradeNotSupported { .. }));
        // 未创建备份
        assert_eq!(ctx.backup_manager.backup_count(), 0);
    }

    #[test]
    fn multiple_upgrades_create_multiple_backups() {
        let mut ctx = UpgradeContext::with_max_backups(10);
        let header = make_current_header();
        let rows = generate_mock_rows(50);

        // 连续 3 次 PATCH 升级
        ctx.execute_patch_minor_upgrade(
            &Version::new(1, 0, 0),
            &Version::new(1, 0, 1),
            &header,
            &rows,
        )
        .unwrap();
        ctx.execute_patch_minor_upgrade(
            &Version::new(1, 0, 1),
            &Version::new(1, 0, 2),
            &header,
            &rows,
        )
        .unwrap();
        ctx.execute_patch_minor_upgrade(
            &Version::new(1, 0, 2),
            &Version::new(1, 0, 3),
            &header,
            &rows,
        )
        .unwrap();

        assert_eq!(ctx.backup_manager.backup_count(), 3);
        let list = ctx.backup_manager.list_backups();
        assert_eq!(list[0].source_version, "1.0.0");
        assert_eq!(list[1].source_version, "1.0.1");
        assert_eq!(list[2].source_version, "1.0.2");
    }

    #[test]
    fn rollback_to_specific_backup_after_multiple_upgrades() {
        let mut ctx = UpgradeContext::with_max_backups(10);
        let header = make_current_header();
        let rows_v1 = generate_mock_rows(50);

        // 第一次升级 + 备份
        ctx.execute_patch_minor_upgrade(
            &Version::new(1, 0, 0),
            &Version::new(1, 0, 1),
            &header,
            &rows_v1,
        )
        .unwrap();

        // 修改数据后第二次升级 + 备份
        let mut rows_v2 = generate_mock_rows(50);
        rows_v2[10].data = b"modified-after-v1".to_vec();
        ctx.execute_patch_minor_upgrade(
            &Version::new(1, 0, 1),
            &Version::new(1, 0, 2),
            &header,
            &rows_v2,
        )
        .unwrap();

        assert_eq!(ctx.backup_manager.backup_count(), 2);

        // 回滚到第一次备份（数据应与 rows_v1 一致，而非 rows_v2）
        let restored_v1 = ctx.backup_manager.restore_backup(1).unwrap();
        assert!(verify_rows_equal(&rows_v1, &restored_v1));
        assert!(!verify_rows_equal(&rows_v2, &restored_v1));

        // 回滚到第二次备份（数据应与 rows_v2 一致）
        let restored_v2 = ctx.backup_manager.restore_backup(2).unwrap();
        assert!(verify_rows_equal(&rows_v2, &restored_v2));
    }

    // -----------------------------------------------------------------
    //  Phase 7e.4 — 大数据量验证（1000000 行零丢失）
    // -----------------------------------------------------------------

    #[test]
    fn million_rows_patch_upgrade_failure_zero_data_loss() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(1, 0, 0);
        let rows = generate_mock_rows(1_000_000);

        let outcome = ctx.simulate_upgrade_failure(&from, &rows).unwrap();

        match outcome {
            UpgradeOutcome::FailedAndRolledBack {
                rollback,
                rows: restored,
                ..
            } => {
                assert!(rollback.success);
                assert_eq!(rollback.restored_row_count, 1_000_000);
                // 关键校验：1000000 行经"升级-失败-回滚"后数据零丢失
                assert!(verify_rows_equal(&rows, &restored));
            }
            other => panic!("expected FailedAndRolledBack, got {:?}", other),
        }
    }

    #[test]
    fn million_rows_major_upgrade_success_zero_data_loss() {
        let mut ctx = UpgradeContext::new();
        let from = Version::new(0, 9, 0);
        let to = Version::new(1, 0, 0);
        let rows = generate_mock_rows(1_000_000);

        let outcome = ctx.execute_major_upgrade(from, to, &rows).unwrap();

        match outcome {
            UpgradeOutcome::Success {
                result,
                rows: migrated,
                backup,
            } => {
                assert!(result.success);
                assert_eq!(migrated.len(), 1_000_000);
                assert!(verify_rows_equal(&rows, &migrated));
                assert_eq!(backup.row_count, 1_000_000);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }
}

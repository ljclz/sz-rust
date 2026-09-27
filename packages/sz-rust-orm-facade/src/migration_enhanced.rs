//! 数据库迁移健壮性（v1.5.0 P1-3）
//!
//! 增强迁移幂等性、校验和验证、回滚、dry-run、并发锁。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_orm_facade::migration_enhanced::{MigrationEnhanced, Migration};
//! use sz_orm_core::DbType;
//! use std::sync::Arc;
//!
//! let migrations = vec![
//!     Migration::new("001", "create_users", "CREATE TABLE users (id INT PRIMARY KEY)", "DROP TABLE users"),
//! ];
//! let mgr = MigrationEnhanced::new(Arc::new(pool), migrations, DbType::MySQL);
//! let report = mgr.migrate().await?;
//! ```
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use sz_orm_core::{DbError, DbType, Migration, Pool, PoolError, PooledConnection, Value};

/// 迁移错误
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// 校验和不匹配（spec 5.3.1 规则2）
    #[error("checksum mismatch for migration version {0}")]
    ChecksumMismatch(String),

    /// 并发迁移锁冲突（spec 5.3.1 规则8）
    #[error("migration lock conflict")]
    LockConflict,

    /// 迁移未找到
    #[error("migration not found: {0}")]
    NotFound(String),

    /// 数据库连接错误（spec 5.3.1 规则5）
    #[error("migration connection error: {0}")]
    ConnectionError(String),

    /// 数据库错误
    #[error(transparent)]
    Db(#[from] DbError),

    /// 连接池错误
    #[error(transparent)]
    Pool(#[from] PoolError),
}

/// 迁移记录
#[derive(Debug, Clone)]
pub struct MigrationRecord {
    /// 版本号
    pub version: String,
    /// 迁移名称
    pub name: String,
    /// 校验和
    pub checksum: String,
}

/// 迁移失败记录
#[derive(Debug, Clone)]
pub struct MigrationFailure {
    /// 版本号
    pub version: String,
    /// 错误信息
    pub error: String,
}

/// 迁移报告
#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// 已应用的迁移
    pub applied: Vec<MigrationRecord>,
    /// 跳过的迁移（已应用）
    pub skipped: Vec<MigrationRecord>,
    /// 失败的迁移
    pub failed: Vec<MigrationFailure>,
}

impl MigrationReport {
    fn new() -> Self {
        Self {
            applied: Vec::new(),
            skipped: Vec::new(),
            failed: Vec::new(),
        }
    }
}

/// 校验和验证器
struct ChecksumVerifier;

impl ChecksumVerifier {
    /// 计算 SQL 的 SHA-256 校验和（hex 编码，64 字符）
    fn compute(sql: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(sql.as_bytes());
        let result = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for byte in result {
            hex.push_str(&format!("{:02x}", byte));
        }
        hex
    }
}

/// 迁移并发锁
struct MigrationLock {
    db_type: DbType,
    acquired: bool,
}

impl MigrationLock {
    const LOCK_NAME: &'static str = "sz_migration_lock";
    const LOCK_KEY_PG: i64 = 770077;

    async fn acquire(conn: &mut PooledConnection, db_type: DbType) -> Result<Self, MigrationError> {
        let sql = match db_type {
            DbType::MySQL | DbType::OceanBase => {
                format!("SELECT GET_LOCK('{}', 0) AS result", Self::LOCK_NAME)
            }
            DbType::PostgreSQL | DbType::Kingbase => {
                format!(
                    "SELECT pg_try_advisory_lock({}) AS result",
                    Self::LOCK_KEY_PG
                )
            }
            _ => {
                return Ok(Self {
                    db_type,
                    acquired: false,
                })
            }
        };

        let rows = conn.query(&sql).await?;
        let acquired = rows
            .first()
            .and_then(|row| row.get("result"))
            .map(|v| matches!(v, Value::I64(1) | Value::I32(1) | Value::Bool(true)))
            .unwrap_or(false);

        if !acquired {
            return Err(MigrationError::LockConflict);
        }

        Ok(Self {
            db_type,
            acquired: true,
        })
    }

    async fn release(&mut self, conn: &mut PooledConnection) -> Result<(), MigrationError> {
        if !self.acquired {
            return Ok(());
        }

        let sql = match self.db_type {
            DbType::MySQL | DbType::OceanBase => {
                format!("SELECT RELEASE_LOCK('{}') AS result", Self::LOCK_NAME)
            }
            DbType::PostgreSQL | DbType::Kingbase => {
                format!("SELECT pg_advisory_unlock({}) AS result", Self::LOCK_KEY_PG)
            }
            _ => return Ok(()),
        };

        conn.execute(&sql).await?;
        self.acquired = false;
        Ok(())
    }
}

/// 迁移增强管理器
///
/// 封装迁移执行 + 校验和验证 + 并发锁 + dry-run，不修改 sz-orm-core 上游。
pub struct MigrationEnhanced {
    pool: Arc<Pool>,
    migrations: Vec<Migration>,
    db_type: DbType,
    table_name: String,
}

impl MigrationEnhanced {
    /// 创建迁移增强管理器
    pub fn new(pool: Arc<Pool>, migrations: Vec<Migration>, db_type: DbType) -> Self {
        Self {
            pool,
            migrations,
            db_type,
            table_name: "__migrations".to_string(),
        }
    }

    /// 使用自定义表名
    pub fn with_table_name(mut self, table_name: &str) -> Self {
        self.table_name = table_name.to_string();
        self
    }

    /// 执行迁移（spec 5.3.1 规则1：幂等跳过已应用版本）
    pub async fn migrate(&self) -> Result<MigrationReport, MigrationError> {
        let mut conn = self.pool.acquire().await?;
        let mut lock = MigrationLock::acquire(&mut conn, self.db_type).await?;
        let result = self.migrate_inner(&mut conn).await;
        lock.release(&mut conn).await?;
        result
    }

    /// 回滚到指定版本（spec 5.3.1 规则3：版本 > target 全部回滚）
    pub async fn rollback_to(&self, version: &str) -> Result<MigrationReport, MigrationError> {
        let mut conn = self.pool.acquire().await?;
        let mut lock = MigrationLock::acquire(&mut conn, self.db_type).await?;
        let result = self.rollback_to_inner(&mut conn, version).await;
        lock.release(&mut conn).await?;
        result
    }

    /// dry-run 模式（spec 5.3.1 规则6：展示待执行迁移不实际执行）
    pub async fn dry_run(&self) -> Result<MigrationReport, MigrationError> {
        let mut conn = self.pool.acquire().await?;
        self.ensure_migrations_table(&mut conn).await?;
        let applied = self.load_applied_checksums(&mut conn).await?;

        let mut report = MigrationReport::new();
        for m in &self.migrations {
            let checksum = ChecksumVerifier::compute(&m.sql_up);
            let record = MigrationRecord {
                version: m.version.clone(),
                name: m.name.clone(),
                checksum,
            };
            if applied.contains_key(&m.version) {
                report.skipped.push(record);
            } else {
                report.applied.push(record);
            }
        }
        Ok(report)
    }

    /// 校验已应用迁移的校验和（spec 5.3.1 规则2）
    pub async fn verify_checksums(&self) -> Result<(), MigrationError> {
        let mut conn = self.pool.acquire().await?;
        self.ensure_migrations_table(&mut conn).await?;
        let applied = self.load_applied_checksums(&mut conn).await?;

        for m in &self.migrations {
            if let Some(stored) = applied.get(&m.version) {
                let computed = ChecksumVerifier::compute(&m.sql_up);
                if stored != &computed {
                    return Err(MigrationError::ChecksumMismatch(m.version.clone()));
                }
            }
        }
        Ok(())
    }

    async fn migrate_inner(
        &self,
        conn: &mut PooledConnection,
    ) -> Result<MigrationReport, MigrationError> {
        self.ensure_migrations_table(conn).await?;
        let applied = self.load_applied_checksums(conn).await?;

        for m in &self.migrations {
            if let Some(stored) = applied.get(&m.version) {
                let computed = ChecksumVerifier::compute(&m.sql_up);
                if stored != &computed {
                    return Err(MigrationError::ChecksumMismatch(m.version.clone()));
                }
            }
        }

        let mut report = MigrationReport::new();
        let current_batch = self.get_max_batch(conn).await? + 1;

        for m in &self.migrations {
            let checksum = ChecksumVerifier::compute(&m.sql_up);
            let record = MigrationRecord {
                version: m.version.clone(),
                name: m.name.clone(),
                checksum: checksum.clone(),
            };

            if applied.contains_key(&m.version) {
                report.skipped.push(record);
                continue;
            }

            if !m.sql_up.is_empty() {
                if let Err(e) = conn.execute(&m.sql_up).await {
                    report.failed.push(MigrationFailure {
                        version: m.version.clone(),
                        error: format!("{:?}", e),
                    });
                    return Ok(report);
                }
            }

            self.record_migration(conn, &m.version, &m.name, current_batch, &checksum)
                .await?;
            report.applied.push(record);
        }

        Ok(report)
    }

    async fn rollback_to_inner(
        &self,
        conn: &mut PooledConnection,
        target_version: &str,
    ) -> Result<MigrationReport, MigrationError> {
        self.ensure_migrations_table(conn).await?;
        let applied = self.load_applied_checksums(conn).await?;

        let target_idx = self
            .migrations
            .iter()
            .position(|m| m.version == target_version);
        if target_idx.is_none() && !target_version.is_empty() {
            return Err(MigrationError::NotFound(target_version.to_string()));
        }

        let rollback_start = target_idx.map(|i| i + 1).unwrap_or(0);
        let mut report = MigrationReport::new();

        for m in self.migrations[rollback_start..].iter().rev() {
            if !applied.contains_key(&m.version) {
                continue;
            }

            if !m.sql_down.is_empty() {
                if let Err(e) = conn.execute(&m.sql_down).await {
                    report.failed.push(MigrationFailure {
                        version: m.version.clone(),
                        error: format!("{:?}", e),
                    });
                    return Ok(report);
                }
            }

            self.delete_migration(conn, &m.version).await?;
            report.applied.push(MigrationRecord {
                version: m.version.clone(),
                name: m.name.clone(),
                checksum: String::new(),
            });
        }

        Ok(report)
    }

    async fn ensure_migrations_table(
        &self,
        conn: &mut PooledConnection,
    ) -> Result<(), MigrationError> {
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} (\
                version VARCHAR(255) NOT NULL PRIMARY KEY, \
                name VARCHAR(255), \
                batch INTEGER NOT NULL, \
                executed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                checksum VARCHAR(64)\
            )",
            self.table_name
        );
        conn.execute(&sql).await?;
        Ok(())
    }

    async fn load_applied_checksums(
        &self,
        conn: &mut PooledConnection,
    ) -> Result<HashMap<String, String>, MigrationError> {
        let sql = format!("SELECT version, checksum FROM {}", self.table_name);
        let rows = conn.query(&sql).await?;
        let mut result = HashMap::new();
        for row in rows {
            let version = row
                .get("version")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let checksum = row
                .get("checksum")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Null => Some(String::new()),
                    _ => None,
                })
                .unwrap_or_default();
            result.insert(version, checksum);
        }
        Ok(result)
    }

    async fn get_max_batch(&self, conn: &mut PooledConnection) -> Result<i32, MigrationError> {
        let sql = format!("SELECT MAX(batch) AS max_batch FROM {}", self.table_name);
        let rows = conn.query(&sql).await?;
        let max_batch = rows
            .first()
            .and_then(|row| row.get("max_batch"))
            .and_then(|v| match v {
                Value::I32(n) => Some(*n),
                Value::I64(n) => Some(*n as i32),
                Value::Null => Some(0),
                _ => None,
            })
            .unwrap_or(0);
        Ok(max_batch)
    }

    async fn record_migration(
        &self,
        conn: &mut PooledConnection,
        version: &str,
        name: &str,
        batch: i32,
        checksum: &str,
    ) -> Result<(), MigrationError> {
        let sql = format!(
            "INSERT INTO {} (version, name, batch, checksum) VALUES ('{}', '{}', {}, '{}')",
            self.table_name, version, name, batch, checksum
        );
        conn.execute(&sql).await?;
        Ok(())
    }

    async fn delete_migration(
        &self,
        conn: &mut PooledConnection,
        version: &str,
    ) -> Result<(), MigrationError> {
        let sql = format!(
            "DELETE FROM {} WHERE version = '{}'",
            self.table_name, version
        );
        conn.execute(&sql).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_verifier_deterministic() {
        let c1 = ChecksumVerifier::compute("CREATE TABLE users (id INT)");
        let c2 = ChecksumVerifier::compute("CREATE TABLE users (id INT)");
        assert_eq!(c1, c2);
        assert_eq!(c1.len(), 64, "SHA-256 hex should be 64 chars");
    }

    #[test]
    fn test_checksum_verifier_different_sql() {
        let c1 = ChecksumVerifier::compute("CREATE TABLE users (id INT)");
        let c2 = ChecksumVerifier::compute("CREATE TABLE posts (id INT)");
        assert_ne!(c1, c2, "different SQL should have different checksums");
    }

    #[test]
    fn test_migration_error_display() {
        let err = MigrationError::ChecksumMismatch("001".to_string());
        assert!(format!("{}", err).contains("001"));

        let err = MigrationError::LockConflict;
        assert!(format!("{}", err).contains("lock conflict"));

        let err = MigrationError::NotFound("999".to_string());
        assert!(format!("{}", err).contains("999"));
    }

    #[test]
    fn test_migration_report_new() {
        let report = MigrationReport::new();
        assert!(report.applied.is_empty());
        assert!(report.skipped.is_empty());
        assert!(report.failed.is_empty());
    }

    #[test]
    fn test_migration_record_fields() {
        let record = MigrationRecord {
            version: "001".to_string(),
            name: "create_users".to_string(),
            checksum: "abc123".to_string(),
        };
        assert_eq!(record.version, "001");
        assert_eq!(record.name, "create_users");
        assert_eq!(record.checksum, "abc123");
    }

    #[test]
    fn test_migration_failure_fields() {
        let failure = MigrationFailure {
            version: "002".to_string(),
            error: "syntax error".to_string(),
        };
        assert_eq!(failure.version, "002");
        assert_eq!(failure.error, "syntax error");
    }
}

//! SzRSQL 事务引擎：MVCC/WAL/Lock。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.4-9.5 节。

#![allow(dead_code)]

// Phase 7d.21：自动清理（AutoVacuum）
pub mod autovacuum;
pub mod consumer_offset;
pub mod lock;
pub mod mvcc;
pub mod schema_registry;
pub mod undo;
pub mod wal;
// Phase 7d.19：WAL 高级特性
pub mod wal_compression;
pub mod wal_fpi;
pub mod wal_summarizer;

// VacuumStats 重新导出，便于外部使用
pub use mvcc::VacuumStats;

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }
}

#[cfg(test)]
mod wal_fuzz;

#[cfg(test)]
mod mvcc_fuzz;

#[cfg(test)]
mod lock_fuzz;

#[cfg(test)]
mod isolation_fuzz;

#[cfg(test)]
mod jepsen_bank;

#[cfg(test)]
mod jepsen_register;

#[cfg(test)]
mod jepsen_set;

#[cfg(test)]
mod crash_recovery_fuzz;

#[cfg(test)]
mod vacuum;

//! SzRSQL SQLite 嵌入式适配 — L2 协议级兼容。
//!
//! 本 crate 实现 SQLite 数据库文件格式的读写适配，使 SzRSQL 能够
//! 直接读写 `.db` 文件，实现 L2 级兼容（文件格式级互操作）。
//!
//! # L2 兼容层级说明
//!
//! - **L1 协议级**：网络协议兼容（如 MySQL/PG wire protocol）
//! - **L2 文件级**：原生文件格式读写（本 crate 的目标）
//! - **L3 SQL 级**：SQL 方言兼容（通过 `convert_sql` 实现）
//!
//! # 模块组织
//!
//! - [`types`]：SQLite 类型系统与 SzRSQL `Value` 的映射
//! - [`format`]：SQLite 文件格式常量与头部编解码
//! - [`varint`]：SQLite varint（变长整数）编解码
//! - [`serial_type`]：SQLite Serial Type 系统编解码
//! - [`record`]：SQLite Record 格式编解码
//! - [`btree_page`]：SQLite B-tree 页面结构编解码
//! - [`adapter`]：适配器主入口（导入/导出/SQL 方言转换）
//!
//! # 设计原则
//!
//! - **零外部依赖**：不依赖 libsqlite3，纯 Rust 实现
//! - **真实可用**：头部编解码完全符合 SQLite 文件格式规范
//! - **错误透明**：使用 `thiserror` 提供结构化错误
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_sqlite_bridge::SqliteAdapter;
//! use std::path::Path;
//!
//! let adapter = SqliteAdapter::new();
//! // 将 SzRSQL 表导出为 SQLite 文件
//! adapter.export_to_sqlite(&[], Path::new("output.db")).unwrap();
//! // 从 SQLite 文件导入数据
//! let tables = adapter.import_from_sqlite(Path::new("input.db")).unwrap();
//! ```

pub mod adapter;
pub mod btree_page;
pub mod format;
pub mod record;
pub mod serial_type;
pub mod server;
pub mod types;
pub mod varint;

pub use adapter::{AdapterError, SqliteAdapter};
pub use format::{SqliteFormatError, SqliteHeader, HEADER_SIZE, MAGIC_HEADER, PAGE_SIZE_DEFAULT};
pub use server::{SqliteConfig, SqliteServer, SqliteServerError};
pub use types::SqliteType;

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

    #[test]
    fn version_matches_cargo_manifest() {
        // 严格校验：version() 必须与 CARGO_PKG_VERSION 一致
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_is_valid_semver() {
        // version() 应符合 semver 格式 X.Y.Z（可含预发布段，如 1.0.0-rc.1）
        let v = version();
        // 去掉预发布段后应得到 X.Y.Z
        let main = v.split('-').next().unwrap_or(v);
        let parts: Vec<&str> = main.split('.').collect();
        assert!(
            parts.len() >= 3,
            "version '{v}' is not semver (expected X.Y.Z, got main='{main}')"
        );
        for part in &parts[..3] {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "version part '{part}' is not numeric (in '{v}')"
            );
        }
    }
}

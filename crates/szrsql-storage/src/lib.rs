//! SzRSQL 存储引擎：Page/BufferPool/B-Tree/LSM。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.2-9.3 节。

#![allow(dead_code)]

pub mod btree;
pub mod buffer;
pub mod columnar;
pub mod cursor;
pub mod external_format;
pub mod format_version;
pub mod freelist;
pub mod heap;
pub mod kv_adapter;
pub mod page;
pub mod read_ahead;
pub mod remote_fs;
pub mod spill;
pub mod tiering;
pub mod toast;
pub mod tuple;
pub mod upgrade;

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
mod page_fuzz;

#[cfg(test)]
mod buffer_stress;

#[cfg(test)]
mod buffer_fuzz;

#[cfg(test)]
mod btree_fuzz;

#[cfg(test)]
mod btree_kani;

#[cfg(test)]
mod btree_sonnetdb_defects;

#[cfg(test)]
mod bulk_load;

#[cfg(test)]
mod btree_bench;

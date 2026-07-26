//! SzRSQL 查询优化器：CBE/RBO/Morsel-Driven。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.8 节。

#![allow(dead_code)]

pub mod cost;
pub mod explain;
pub mod join_order;
pub mod ml_cost;
pub mod plan_cache;
pub mod result_cache;
pub mod rule;
pub mod statistics;

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

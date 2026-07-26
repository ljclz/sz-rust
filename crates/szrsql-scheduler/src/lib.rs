//! SzRSQL 定时任务调度器。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.15 节。

#![allow(dead_code)]

pub mod scheduler;

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

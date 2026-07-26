//! SzRSQL 复制：物理/逻辑/PITR。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.16 节。

#![allow(dead_code)]

pub mod backup;
pub mod dr;
pub mod rolling;
pub mod stream;

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

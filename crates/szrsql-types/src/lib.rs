//! SzRSQL 类型系统：Value/Schema/ColumnDef。
//!
//! 对应 `SzRSQL技术实现方案.md` 9.1 节。

#![allow(dead_code)]

pub mod schema;
pub mod value;

#[cfg(test)]
mod fuzz;

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
        // 严格校验：version() 必须与 CARGO_PKG_VERSION 一致，
        // 防止返回任意非空字符串（如 "xyzzy"）的变异体存活
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

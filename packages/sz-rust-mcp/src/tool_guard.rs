// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! MCP 工具参数安全校验
//!
//! 所有会落盘或起子进程的工具参数必须经过本模块校验：
//! - 名称类参数限定字符集，防止注入额外 CLI flag（如 `--git=...`）
//! - 路径类参数限定为仓库内相对路径，防止路径穿越 / 任意脚本执行

use crate::tool::ToolError;

/// 校验名称类参数（crate 名、迁移名等）
///
/// 仅允许 ASCII 字母数字与 `_`、`-`，长度 1..=max，且不得以 `-` 开头
/// （防止被下游 CLI 当作 flag 解析，堵住 `cargo add --git=...` 类供应链注入）。
pub fn validate_name_component(value: &str, label: &str, max: usize) -> Result<(), ToolError> {
    if value.is_empty() || value.len() > max {
        return Err(ToolError::InvalidArgs(format!(
            "{label} 长度必须在 1..={max}: {value:?}"
        )));
    }
    if value.starts_with('-') {
        return Err(ToolError::InvalidArgs(format!(
            "{label} 不允许以 '-' 开头（防 flag 注入）: {value:?}"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ToolError::InvalidArgs(format!(
            "{label} 含非法字符（仅允许 ASCII 字母数字与 _ -）: {value:?}"
        )));
    }
    Ok(())
}

/// 校验相对路径参数
///
/// 拒绝绝对路径、反斜杠、NUL 以及 `..`/`.` 组件，把可写/可执行范围
/// 钉在当前工作目录之内；`must_end_with` 用于限定扩展名（如 ".js"、".sql"）。
pub fn validate_rel_path(
    value: &str,
    label: &str,
    must_end_with: Option<&str>,
) -> Result<(), ToolError> {
    if value.is_empty() || value.len() > 512 {
        return Err(ToolError::InvalidArgs(format!(
            "{label} 长度必须在 1..=512"
        )));
    }
    if value.contains('\\') || value.contains('\0') {
        return Err(ToolError::InvalidArgs(format!(
            "{label} 含非法字符（禁止反斜杠与 NUL）: {value:?}"
        )));
    }
    let path = std::path::Path::new(value);
    if path.is_absolute() {
        return Err(ToolError::InvalidArgs(format!(
            "{label} 必须是相对路径（仓库内）: {value:?}"
        )));
    }
    for comp in path.components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(ToolError::InvalidArgs(format!(
                "{label} 含非法路径组件（拒绝 .. / . / 绝对路径）: {value:?}"
            )));
        }
    }
    if let Some(ext) = must_end_with {
        if !value.ends_with(ext) {
            return Err(ToolError::InvalidArgs(format!(
                "{label} 必须以 {ext} 结尾: {value:?}"
            )));
        }
    }
    Ok(())
}

/// 压平单行文本（替换换行），防止注入多行注释逃逸
pub fn flatten_single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ").replace("--", "— ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_component_rejects_flag_injection() {
        for evil in [
            "--git=https://evil.example/x",
            "-registry=evil",
            "--registry",
            "foo bar",
            "foo/bar",
            "foo\\bar",
            "插件",
            "a\0b",
        ] {
            assert!(
                validate_name_component(evil, "plugin_name", 64).is_err(),
                "应拒绝 {evil:?}"
            );
        }
    }

    #[test]
    fn name_component_accepts_crate_names() {
        for ok in ["sz-rust-addons-loader", "serde", "tokio_util", "a1-b2_c3"] {
            assert!(
                validate_name_component(ok, "plugin_name", 64).is_ok(),
                "应接受 {ok:?}"
            );
        }
        assert!(validate_name_component("-serde", "x", 64).is_err());
        assert!(validate_name_component(&"x".repeat(65), "x", 64).is_err());
    }

    #[test]
    fn rel_path_rejects_traversal_and_absolute() {
        for evil in [
            "../evil.sql",
            "a/../../evil.sql",
            "..\\evil.sql",
            "/etc/passwd.sql",
            "C:\\x\\y.sql",
            "./x.sql",
            "a/../b.sql",
            "x.txt",
        ] {
            let must: Option<&str> = Some(".sql");
            assert!(
                validate_rel_path(evil, "path", must).is_err(),
                "应拒绝 {evil:?}"
            );
        }
    }

    #[test]
    fn rel_path_accepts_normal_paths() {
        assert!(validate_rel_path("scripts/e2e_deploy.js", "p", Some(".js")).is_ok());
        assert!(validate_rel_path("migrations", "p", None).is_ok());
        assert!(validate_rel_path("migrations/create_user.sql", "p", Some(".sql")).is_ok());
    }

    #[test]
    fn flatten_single_line_strips_comment_breaks() {
        assert_eq!(flatten_single_line("a\nb"), "a b");
        assert_eq!(
            flatten_single_line("a\r\n-- DROP TABLE"),
            "a  —  DROP TABLE"
        );
    }
}

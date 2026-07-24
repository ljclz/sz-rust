//! Env 模块 — 对齐 PHP `think\facade\Env`
//!
//! Phase P3-17 交付物。本模块实现环境变量管理，对齐 PHP `think\facade\Env` 的核心 API。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Env::get($name, $default = null)` | [`Env::get`] / [`Env::get_with_default`] | 获取环境变量 |
//! | `Env::set($name, $value)` | [`Env::set`] | 设置环境变量（仅写入内部存储） |
//! | `Env::has($name)` | [`Env::has`] | 检查环境变量是否存在 |
//! | `Env::load($file)` | [`Env::load_from_file`] | 从 `.env` 文件加载 |
//!
//! ### PHP 行为对齐
//!
//! - **优先级**：PHP `Env::get()` 优先返回真实环境变量（`$_SERVER` / `getenv()`），
//!   其次返回 `.env` 文件加载的值。Rust 同样优先 `std::env::var()`，其次查内部存储。
//! - **点分隔访问**：PHP `.env` 文件支持 `[section]` 段，通过 `section.key` 访问。
//!   Rust 通过 [`Env::load_from_file`] 解析 INI 风格 section，存储为 `section.key` 形式。
//! - **不污染进程环境**：PHP `Env::set()` 仅修改内部数组，不调用 `putenv()`。
//!   Rust `set()` 同样仅写入内部 `HashMap`，避免 `std::env::set_var` 的线程安全问题。
//!
//! ## .env 文件格式
//!
//! 支持 INI 风格的 section 嵌套：
//!
//! ```ini
//! APP_DEBUG = true
//! APP_KEY = base64:xxxxxx
//!
//! [database]
//! hostname = localhost
//! port = 3306
//! ```
//!
//! 访问方式：
//! - `APP_DEBUG` → 顶层键
//! - `database.hostname` → section 内键
//!
//! ## 架构说明
//!
//! - **无外部依赖**：不依赖 `dotenv` / `dotenvy` crate，自行实现 INI 解析
//! - **线程安全**：通过 `Arc<RwLock<HashMap>>` 提供并发读、互斥写
//! - **不修改进程环境变量**：所有 `set()` 仅写入内部存储

use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// 错误类型
// ============================================================================

/// Env 错误
#[derive(Debug, Error)]
pub enum EnvError {
    /// `.env` 文件读取失败
    #[error(".env 文件读取失败: {path} — {source}")]
    FileRead {
        /// 文件路径
        path: String,
        /// 底层 IO 错误
        #[source]
        source: std::io::Error,
    },
    /// `.env` 文件解析失败
    #[error(".env 文件解析失败: {path} — 行 {line}: {message}")]
    Parse {
        /// 文件路径
        path: String,
        /// 出错的行号（从 1 开始）
        line: usize,
        /// 错误描述
        message: String,
    },
}

// ============================================================================
// Env 主体
// ============================================================================

/// 环境变量管理器 — 对齐 PHP `think\facade\Env`
///
/// 通过 `.env` 文件加载配置，同时支持读取真实进程环境变量。
///
/// # 优先级
///
/// `get()` 查找顺序：
/// 1. 真实进程环境变量 `std::env::var(name)`
/// 2. 内部存储（`.env` 文件加载或 `set()` 写入的值）
///
/// # 线程安全
///
/// 内部存储通过 `Arc<RwLock<HashMap>>` 保护，支持并发读、互斥写。
/// 不调用 `std::env::set_var`，避免 Rust 2024 edition 的线程安全警告。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\facade\Env
/// Env::load('.env');          // 加载 .env 文件
/// Env::set('APP_KEY', 'xxx'); // 设置内部变量
/// Env::get('APP_KEY');        // 获取（优先真实环境变量）
/// Env::has('APP_KEY');        // 检查存在
/// ```
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// 内部存储（.env 文件加载 + set() 写入）
    data: Arc<RwLock<HashMap<String, String>>>,
}

impl Env {
    /// 创建空的 Env 实例
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `.env` 文件加载配置
    ///
    /// 支持 INI 风格的 `[section]` 段，section 内的键会以 `section.key` 形式存储。
    ///
    /// # 参数
    ///
    /// - `path`: `.env` 文件路径
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`EnvError`]。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Env::load('.env');
    /// ```
    ///
    /// # 错误
    ///
    /// - [`EnvError::FileRead`][]: 文件读取失败
    /// - [`EnvError::Parse`][]: 文件解析失败（格式错误）
    pub fn load_from_file(&self, path: impl AsRef<Path>) -> Result<(), EnvError> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref).map_err(|e| EnvError::FileRead {
            path: path_ref.display().to_string(),
            source: e,
        })?;

        self.parse_ini_content(&content, &path_ref.display().to_string())
    }

    /// 解析 INI 风格内容并写入内部存储
    ///
    /// # 格式规则
    ///
    /// - `key = value` → 顶层键值对
    /// - `[section]` → 后续键值对存储为 `section.key`
    /// - `#` 或 `;` 开头的行 → 注释，忽略
    /// - 空行 → 忽略
    /// - 引号包裹的值会去除引号（`"value"` → `value`）
    fn parse_ini_content(
        &self,
        content: &str,
        path: &str,
    ) -> Result<(), EnvError> {
        let mut data = self.data.write();
        let mut current_section: String = String::new();

        for (line_idx, raw_line) in content.lines().enumerate() {
            let line_no = line_idx + 1;
            let line = raw_line.trim();

            // 空行跳过
            if line.is_empty() {
                continue;
            }

            // 注释行跳过（# 或 ; 开头）
            if line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            // section 头：[section]
            if line.starts_with('[') {
                if let Some(end) = line.find(']') {
                    current_section = line[1..end].trim().to_string();
                } else {
                    return Err(EnvError::Parse {
                        path: path.to_string(),
                        line: line_no,
                        message: "section 头缺少闭合的 ']'".to_string(),
                    });
                }
                continue;
            }

            // 键值对：key = value
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim().to_string();
                let mut value = line[eq_pos + 1..].trim().to_string();

                if key.is_empty() {
                    return Err(EnvError::Parse {
                        path: path.to_string(),
                        line: line_no,
                        message: "键为空".to_string(),
                    });
                }

                // 去除引号包裹
                if value.len() >= 2 {
                    let first = value.chars().next().unwrap();
                    let last = value.chars().last().unwrap();
                    if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
                        value = value[1..value.len() - 1].to_string();
                    }
                }

                // 拼接完整键名（section.key 或顶层 key）
                let full_key = if current_section.is_empty() {
                    key
                } else {
                    format!("{}.{}", current_section, key)
                };

                data.insert(full_key, value);
            } else {
                return Err(EnvError::Parse {
                    path: path.to_string(),
                    line: line_no,
                    message: "缺少 '=' 分隔符".to_string(),
                });
            }
        }

        Ok(())
    }

    /// 获取环境变量值
    ///
    /// # 优先级
    ///
    /// 1. 真实进程环境变量 `std::env::var(name)`
    /// 2. 内部存储（`.env` 文件加载或 `set()` 写入的值）
    ///
    /// # 参数
    ///
    /// - `name`: 环境变量名（支持点分隔，如 `database.hostname`）
    ///
    /// # 返回
    ///
    /// 存在返回 `Some(value)`，不存在返回 `None`。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Env::get('APP_KEY');  // 无默认值，不存在返回 null
    /// ```
    pub fn get(&self, name: &str) -> Option<String> {
        // 优先真实进程环境变量
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return Some(value);
            }
        }

        // 其次内部存储
        let data = self.data.read();
        data.get(name).cloned()
    }

    /// 获取环境变量值，不存在时返回默认值
    ///
    /// # 参数
    ///
    /// - `name`: 环境变量名
    /// - `default`: 默认值
    ///
    /// # 返回
    ///
    /// 存在返回实际值，不存在返回 `default`。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Env::get('APP_DEBUG', false);  // 不存在时返回 false
    /// ```
    pub fn get_with_default(&self, name: &str, default: &str) -> String {
        self.get(name).unwrap_or_else(|| default.to_string())
    }

    /// 检查环境变量是否存在
    ///
    /// # 优先级
    ///
    /// 同 [`Env::get`]：真实进程环境变量优先于内部存储。
    ///
    /// # 参数
    ///
    /// - `name`: 环境变量名
    ///
    /// # 返回
    ///
    /// 存在返回 `true`，否则返回 `false`。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Env::has('APP_KEY');
    /// ```
    pub fn has(&self, name: &str) -> bool {
        // 优先真实进程环境变量
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return true;
            }
        }

        // 其次内部存储
        let data = self.data.read();
        data.contains_key(name)
    }

    /// 设置环境变量（仅写入内部存储）
    ///
    /// # 注意
    ///
    /// 本方法**不调用** `std::env::set_var`，仅修改内部 `HashMap`。
    /// 这样做的原因：
    /// 1. 避免 Rust 2024 edition 中 `set_var` 的线程安全警告
    /// 2. 对齐 PHP `think\facade\Env::set()` 的行为（仅修改内部数组，不调用 `putenv()`）
    ///
    /// # 参数
    ///
    /// - `name`: 环境变量名
    /// - `value`: 环境变量值
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Env::set('APP_KEY', 'base64:xxxxxx');
    /// ```
    pub fn set(&self, name: &str, value: &str) {
        let mut data = self.data.write();
        data.insert(name.to_string(), value.to_string());
    }

    /// 删除内部存储中的环境变量
    ///
    /// # 注意
    ///
    /// 本方法仅删除内部存储中的值，**不影响**真实进程环境变量。
    ///
    /// # 参数
    ///
    /// - `name`: 环境变量名
    ///
    /// # 返回
    ///
    /// 如果内部存储中存在该键并已删除，返回 `true`；否则返回 `false`。
    pub fn remove(&self, name: &str) -> bool {
        let mut data = self.data.write();
        data.remove(name).is_some()
    }

    /// 获取内部存储的所有键值对（快照）
    ///
    /// # 注意
    ///
    /// 返回的是内部存储的副本，**不包含**真实进程环境变量。
    /// 主要用于调试和测试。
    ///
    /// # 返回
    ///
    /// 所有内部存储键值对的 `HashMap`。
    pub fn all(&self) -> HashMap<String, String> {
        let data = self.data.read();
        data.clone()
    }

    /// 清空内部存储
    ///
    /// # 注意
    ///
    /// 仅清空内部存储，**不影响**真实进程环境变量。
    pub fn clear(&self) {
        let mut data = self.data.write();
        data.clear();
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 测试空 Env 实例
    #[test]
    fn test_new_env_is_empty() {
        let env = Env::new();
        assert!(env.all().is_empty());
        assert!(!env.has("NON_EXISTENT_KEY"));
        assert_eq!(env.get("NON_EXISTENT_KEY"), None);
    }

    /// 测试 set/get/remove 基本流程
    #[test]
    fn test_set_get_remove() {
        let env = Env::new();

        env.set("APP_KEY", "base64:xxxxxx");
        assert!(env.has("APP_KEY"));
        assert_eq!(env.get("APP_KEY"), Some("base64:xxxxxx".to_string()));

        assert!(env.remove("APP_KEY"));
        assert!(!env.has("APP_KEY"));
        assert_eq!(env.get("APP_KEY"), None);
    }

    /// 测试 get_with_default
    #[test]
    fn test_get_with_default() {
        let env = Env::new();

        // 不存在时返回默认值
        assert_eq!(env.get_with_default("MISSING", "fallback"), "fallback");

        // 存在时返回实际值
        env.set("EXISTING", "actual");
        assert_eq!(env.get_with_default("EXISTING", "fallback"), "actual");
    }

    /// 测试从 INI 格式字符串加载（含 section）
    #[test]
    fn test_load_from_ini_content_with_section() {
        let env = Env::new();
        let content = r#"
# 顶层配置
APP_DEBUG = true
APP_KEY = "base64:secret"

[database]
hostname = localhost
port = 3306

[redis]
host = "127.0.0.1"
"#;
        env.parse_ini_content(content, "<test>").unwrap();

        // 验证顶层键
        assert_eq!(env.get("APP_DEBUG"), Some("true".to_string()));
        assert_eq!(env.get("APP_KEY"), Some("base64:secret".to_string()));

        // 验证 section 内键
        assert_eq!(env.get("database.hostname"), Some("localhost".to_string()));
        assert_eq!(env.get("database.port"), Some("3306".to_string()));
        assert_eq!(env.get("redis.host"), Some("127.0.0.1".to_string()));
    }

    /// 测试引号去除（双引号和单引号）
    #[test]
    fn test_quote_stripping() {
        let env = Env::new();
        let content = r#"
DOUBLE = "value with spaces"
SINGLE = 'another value'
NO_QUOTE = plain
EMPTY = ""
"#;
        env.parse_ini_content(content, "<test>").unwrap();

        assert_eq!(env.get("DOUBLE"), Some("value with spaces".to_string()));
        assert_eq!(env.get("SINGLE"), Some("another value".to_string()));
        assert_eq!(env.get("NO_QUOTE"), Some("plain".to_string()));
        assert_eq!(env.get("EMPTY"), Some("".to_string()));
    }

    /// 测试注释行跳过（# 和 ;）
    #[test]
    fn test_comment_lines_skipped() {
        let env = Env::new();
        let content = r#"
# 这是注释
APP_KEY = value1
; 这也是注释
APP_DEBUG = value2
"#;
        env.parse_ini_content(content, "<test>").unwrap();

        assert_eq!(env.get("APP_KEY"), Some("value1".to_string()));
        assert_eq!(env.get("APP_DEBUG"), Some("value2".to_string()));
    }

    /// 测试从真实文件加载
    #[test]
    fn test_load_from_file() {
        // 创建临时 .env 文件
        let temp_dir = std::env::temp_dir().join("sz_rust_env_test");
        let _ = std::fs::create_dir_all(&temp_dir);
        let env_file = temp_dir.join(".env");

        let mut file = std::fs::File::create(&env_file).unwrap();
        writeln!(file, "TEST_KEY = test_value").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "[section]").unwrap();
        writeln!(file, "inner = inner_value").unwrap();
        drop(file);

        let env = Env::new();
        env.load_from_file(&env_file).unwrap();

        assert_eq!(env.get("TEST_KEY"), Some("test_value".to_string()));
        assert_eq!(env.get("section.inner"), Some("inner_value".to_string()));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// 测试文件不存在时返回错误
    #[test]
    fn test_load_nonexistent_file_errors() {
        let env = Env::new();
        let result = env.load_from_file("/nonexistent/path/.env");
        assert!(result.is_err());
        match result {
            Err(EnvError::FileRead { .. }) => {}
            _ => panic!("期望 FileRead 错误"),
        }
    }

    /// 测试解析错误：section 头未闭合
    #[test]
    fn test_parse_unclosed_section_errors() {
        let env = Env::new();
        let content = "[unclosed_section\nkey = value";
        let result = env.parse_ini_content(content, "<test>");
        assert!(result.is_err());
        match result {
            Err(EnvError::Parse { line, .. }) => {
                assert_eq!(line, 1);
            }
            _ => panic!("期望 Parse 错误"),
        }
    }

    /// 测试解析错误：缺少等号
    #[test]
    fn test_parse_missing_equals_errors() {
        let env = Env::new();
        let content = "this_is_not_a_key_value_pair";
        let result = env.parse_ini_content(content, "<test>");
        assert!(result.is_err());
        match result {
            Err(EnvError::Parse { line, .. }) => {
                assert_eq!(line, 1);
            }
            _ => panic!("期望 Parse 错误"),
        }
    }

    /// 测试解析错误：键为空
    #[test]
    fn test_parse_empty_key_errors() {
        let env = Env::new();
        let content = " = value";
        let result = env.parse_ini_content(content, "<test>");
        assert!(result.is_err());
        match result {
            Err(EnvError::Parse { line, .. }) => {
                assert_eq!(line, 1);
            }
            _ => panic!("期望 Parse 错误"),
        }
    }

    /// 测试真实进程环境变量优先于内部存储
    ///
    /// 验证：当 `std::env::var(name)` 返回非空值时，`get()` 返回进程环境变量值，
    /// 而非内部存储的值。
    #[test]
    fn test_process_env_takes_priority() {
        let env = Env::new();

        // 内部存储设置一个值
        env.set("SZ_RUST_TEST_ENV_PRIORITY", "internal_value");

        // 同时设置进程环境变量
        std::env::set_var("SZ_RUST_TEST_ENV_PRIORITY", "process_value");

        // get() 应返回进程环境变量值
        assert_eq!(
            env.get("SZ_RUST_TEST_ENV_PRIORITY"),
            Some("process_value".to_string())
        );

        std::env::remove_var("SZ_RUST_TEST_ENV_PRIORITY");
    }

    /// 测试进程环境变量为空字符串时回退到内部存储
    ///
    /// 验证：当 `std::env::var(name)` 返回空字符串时，`get()` 回退到内部存储。
    #[test]
    fn test_empty_process_env_falls_back_to_internal() {
        let env = Env::new();

        // 内部存储设置一个值
        env.set("SZ_RUST_TEST_EMPTY_FALLBACK", "internal_value");

        // 设置进程环境变量为空字符串
        std::env::set_var("SZ_RUST_TEST_EMPTY_FALLBACK", "");

        // get() 应回退到内部存储
        assert_eq!(
            env.get("SZ_RUST_TEST_EMPTY_FALLBACK"),
            Some("internal_value".to_string())
        );

        std::env::remove_var("SZ_RUST_TEST_EMPTY_FALLBACK");
    }

    /// 测试 clear 清空内部存储
    #[test]
    fn test_clear() {
        let env = Env::new();
        env.set("KEY1", "value1");
        env.set("KEY2", "value2");
        assert_eq!(env.all().len(), 2);

        env.clear();
        assert!(env.all().is_empty());
    }

    /// 测试 all() 返回内部存储快照
    #[test]
    fn test_all_returns_snapshot() {
        let env = Env::new();
        env.set("KEY1", "value1");
        env.set("KEY2", "value2");

        let snapshot = env.all();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot.get("KEY1"), Some(&"value1".to_string()));
        assert_eq!(snapshot.get("KEY2"), Some(&"value2".to_string()));

        // 修改内部存储不影响快照
        env.set("KEY3", "value3");
        assert_eq!(snapshot.len(), 2);
    }

    /// 测试 remove 不存在的键返回 false
    #[test]
    fn test_remove_nonexistent_returns_false() {
        let env = Env::new();
        assert!(!env.remove("NON_EXISTENT"));
    }

    /// 测试跨 section 重复加载（覆盖语义）
    ///
    /// 验证：同一键名在不同 section 下是独立的（`section1.key` vs `section2.key`），
    /// 但同 section 内的同名键会被覆盖。
    #[test]
    fn test_section_isolation() {
        let env = Env::new();
        let content = r#"
[section1]
key = value1

[section2]
key = value2
"#;
        env.parse_ini_content(content, "<test>").unwrap();

        assert_eq!(env.get("section1.key"), Some("value1".to_string()));
        assert_eq!(env.get("section2.key"), Some("value2".to_string()));
    }

    /// 测试多次 load_from_file 累加而非覆盖
    ///
    /// 验证：连续调用 `load_from_file` 会累加键值对，而非清空后重新加载。
    /// 这对齐 PHP `think\facade\Env::load()` 的行为。
    #[test]
    fn test_multiple_load_accumulates() {
        let env = Env::new();
        let content1 = "KEY1 = value1";
        let content2 = "KEY2 = value2";

        env.parse_ini_content(content1, "<test1>").unwrap();
        env.parse_ini_content(content2, "<test2>").unwrap();

        assert_eq!(env.get("KEY1"), Some("value1".to_string()));
        assert_eq!(env.get("KEY2"), Some("value2".to_string()));
    }
}

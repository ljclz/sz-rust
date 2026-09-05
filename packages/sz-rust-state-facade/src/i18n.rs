// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! i18n 国际化模块 — 对齐 PHP `think\facade\Lang`
//!
//! 本模块实现多语言支持，对齐 PHP `think\facade\Lang` 的核心 API。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Lang::set($name, $value, $lang = null)` | [`I18n::set`] | 设置语言项 |
//! | `Lang::get($name, $vars = [], $lang = null)` | [`I18n::get`] | 获取语言项（支持变量插值） |
//! | `Lang::has($name, $lang = null)` | [`I18n::has`] | 检查语言项是否存在 |
//! | `Lang::load($file, $lang = null)` | [`I18n::load_from_file`] | 从 JSON 文件加载语言包 |
//! | `Lang::range($range = null)` | [`I18n::current_lang`] / [`I18n::set_current_lang`] | 获取/设置当前语言 |
//! | `Lang::defaultLangSet($lang)` | [`I18n::set_default_lang`] | 设置默认语言 |
//!
//! ### PHP 行为对齐
//!
//! - **变量插值**：PHP 支持两种格式 `:name` 和 `{name}`。Rust 同样支持这两种格式。
//! - **语言回退**：PHP 当当前语言无对应项时回退到默认语言。Rust 通过 [`I18n::get`] 实现相同回退逻辑。
//! - **多语言范围**：PHP 通过 `range()` 切换当前语言。Rust 通过 [`I18n::set_current_lang`] 实现。
//!
//! ## 语言包文件格式
//!
//! 使用 JSON 格式（对齐 PHP 5.0+ 支持的 JSON 语言包）：
//!
//! ```json
//! {
//!     "hello": "Hello, :name!",
//!     "goodbye": "Goodbye, {name}!"
//! }
//! ```
//!
//! ## 架构说明
//!
//! - **无外部依赖**：不依赖 `fluent` / `rust-i18n` crate，自行实现简单 KV 存储 + 变量插值
//! - **线程安全**：通过 `Arc<RwLock<>>` 提供并发读、互斥写
//! - **JSON 解析**：复用 `serde_json`（已是项目依赖）

use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::fs;

// ============================================================================
// 错误类型
// ============================================================================

/// i18n 错误
#[derive(Debug, Error)]
pub enum I18nError {
    /// 语言包文件读取失败
    #[error("语言包文件读取失败: {path} — {source}")]
    FileRead {
        /// 文件路径
        path: String,
        /// 底层 IO 错误
        #[source]
        source: std::io::Error,
    },
    /// 语言包文件解析失败
    #[error("语言包文件解析失败: {path} — {source}")]
    Parse {
        /// 文件路径
        path: String,
        /// 底层 JSON 解析错误
        #[source]
        source: serde_json::Error,
    },
}

// ============================================================================
// I18n 主体
// ============================================================================

/// 国际化管理器 — 对齐 PHP `think\facade\Lang`
///
/// 管理多语言文案，支持变量插值和语言回退。
///
/// # 线程安全
///
/// 内部通过 `Arc<RwLock<>>` 保护，支持并发读、互斥写。
///
/// # PHP 对齐
///
/// ```php
/// // PHP think\facade\Lang
/// Lang::set('hello', 'Hello, :name!', 'en-us');
/// Lang::get('hello', ['name' => 'John']);              // 使用当前语言
/// Lang::get('hello', ['name' => 'John'], 'en-us');     // 指定语言
/// Lang::has('hello');
/// Lang::range('en-us');                                 // 切换当前语言
/// ```
#[derive(Debug, Clone)]
pub struct I18n {
    /// 多语言数据：lang_code → (key → value)
    data: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    /// 当前语言代码
    current_lang: Arc<RwLock<String>>,
    /// 默认语言代码（回退用）
    default_lang: Arc<RwLock<String>>,
}

impl Default for I18n {
    /// 默认实现：当前语言和默认语言均为 `zh-cn`
    fn default() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            current_lang: Arc::new(RwLock::new("zh-cn".to_string())),
            default_lang: Arc::new(RwLock::new("zh-cn".to_string())),
        }
    }
}

impl I18n {
    /// 创建新的 i18n 实例
    ///
    /// 默认语言和当前语言均为 `zh-cn`。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建指定默认语言的 i18n 实例
    ///
    /// # 参数
    ///
    /// - `default_lang`: 默认语言代码（如 `zh-cn`、`en-us`）
    pub fn with_default_lang(default_lang: &str) -> Self {
        let i18n = Self::default();
        *i18n.default_lang.write() = default_lang.to_string();
        *i18n.current_lang.write() = default_lang.to_string();
        i18n
    }

    /// 设置语言项
    ///
    /// # 参数
    ///
    /// - `lang`: 语言代码（如 `zh-cn`、`en-us`）
    /// - `key`: 语言项键名
    /// - `value`: 语言项值（支持 `:var` 和 `{var}` 插值占位符）
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Lang::set('hello', 'Hello, :name!', 'en-us');
    /// ```
    pub fn set(&self, lang: &str, key: &str, value: &str) {
        let mut data = self.data.write();
        data.entry(lang.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }

    /// 获取语言项（支持变量插值）
    ///
    /// # 语言回退
    ///
    /// 1. 优先查 `lang` 参数指定的语言
    /// 2. 若未指定 `lang`，查当前语言
    /// 3. 若当前语言无此项，回退到默认语言
    ///
    /// # 变量插值
    ///
    /// 支持 PHP 两种占位符格式：
    /// - `:name` → 用 `vars["name"]` 替换
    /// - `{name}` → 用 `vars["name"]` 替换
    ///
    /// # 参数
    ///
    /// - `key`: 语言项键名
    /// - `vars`: 插值变量（键名不含 `:` 或 `{}`）
    /// - `lang`: 语言代码，`None` 时使用当前语言
    ///
    /// # 返回
    ///
    /// 存在返回插值后的字符串，不存在返回 `None`。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Lang::get('hello', ['name' => 'John'], 'en-us');
    /// ```
    pub fn get(
        &self,
        key: &str,
        vars: &HashMap<String, String>,
        lang: Option<&str>,
    ) -> Option<String> {
        let data = self.data.read();

        // 确定查找语言顺序
        let current_lang = self.current_lang.read().clone();
        let default_lang = self.default_lang.read().clone();

        let lookup_langs: Vec<&str> = match lang {
            Some(l) => vec![l, &default_lang],
            None => vec![&current_lang, &default_lang],
        };

        // 按优先级查找
        for lookup_lang in lookup_langs {
            if let Some(lang_data) = data.get(lookup_lang) {
                if let Some(raw_value) = lang_data.get(key) {
                    return Some(self.interpolate(raw_value, vars));
                }
            }
        }

        None
    }

    /// 获取语言项（简化版，无变量插值）
    ///
    /// # 参数
    ///
    /// - `key`: 语言项键名
    /// - `lang`: 语言代码，`None` 时使用当前语言
    ///
    /// # 返回
    ///
    /// 存在返回原始字符串，不存在返回 `None`。
    pub fn get_simple(&self, key: &str, lang: Option<&str>) -> Option<String> {
        let empty_vars = HashMap::new();
        self.get(key, &empty_vars, lang)
    }

    /// 检查语言项是否存在
    ///
    /// # 语言回退
    ///
    /// 同 [`I18n::get`]：优先指定语言，其次当前语言，最后默认语言。
    ///
    /// # 参数
    ///
    /// - `key`: 语言项键名
    /// - `lang`: 语言代码，`None` 时使用当前语言
    ///
    /// # 返回
    ///
    /// 存在返回 `true`，否则返回 `false`。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Lang::has('hello');
    /// ```
    pub fn has(&self, key: &str, lang: Option<&str>) -> bool {
        let data = self.data.read();

        let current_lang = self.current_lang.read().clone();
        let default_lang = self.default_lang.read().clone();

        let lookup_langs: Vec<&str> = match lang {
            Some(l) => vec![l, &default_lang],
            None => vec![&current_lang, &default_lang],
        };

        for lookup_lang in lookup_langs {
            if let Some(lang_data) = data.get(lookup_lang) {
                if lang_data.contains_key(key) {
                    return true;
                }
            }
        }

        false
    }

    /// 从 JSON 文件加载语言包
    ///
    /// # 文件格式
    ///
    /// ```json
    /// {
    ///     "hello": "Hello, :name!",
    ///     "goodbye": "Goodbye, {name}!"
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `path`: JSON 文件路径
    /// - `lang`: 语言代码
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`I18nError`]。
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Lang::load('lang/en-us.json', 'en-us');
    /// ```
    pub async fn load_from_file(
        &self,
        path: impl AsRef<Path>,
        lang: &str,
    ) -> Result<(), I18nError> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref)
            .await
            .map_err(|e| I18nError::FileRead {
                path: path_ref.display().to_string(),
                source: e,
            })?;

        self.load_from_json_str(&content, lang, &path_ref.display().to_string())
    }

    /// 从 JSON 字符串加载语言包
    ///
    /// # 参数
    ///
    /// - `json_str`: JSON 格式字符串
    /// - `lang`: 语言代码
    /// - `path_for_error`: 用于错误信息的路径描述
    ///
    /// # 返回
    ///
    /// 成功返回 `Ok(())`，失败返回 [`I18nError`]。
    pub fn load_from_json_str(
        &self,
        json_str: &str,
        lang: &str,
        path_for_error: &str,
    ) -> Result<(), I18nError> {
        let parsed: HashMap<String, Value> =
            serde_json::from_str(json_str).map_err(|e| I18nError::Parse {
                path: path_for_error.to_string(),
                source: e,
            })?;

        let mut data = self.data.write();
        let lang_data = data.entry(lang.to_string()).or_default();

        for (key, value) in parsed {
            // 仅接受字符串值，非字符串值跳过
            if let Some(s) = value.as_str() {
                lang_data.insert(key, s.to_string());
            }
        }

        Ok(())
    }

    /// 获取当前语言代码
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// $current = Lang::range();
    /// ```
    pub fn current_lang(&self) -> String {
        self.current_lang.read().clone()
    }

    /// 设置当前语言代码
    ///
    /// # 参数
    ///
    /// - `lang`: 语言代码（如 `zh-cn`、`en-us`）
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Lang::range('en-us');
    /// ```
    pub fn set_current_lang(&self, lang: &str) {
        *self.current_lang.write() = lang.to_string();
    }

    /// 设置默认语言代码（回退语言）
    ///
    /// # 参数
    ///
    /// - `lang`: 语言代码
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// Lang::defaultLangSet('zh-cn');
    /// ```
    pub fn set_default_lang(&self, lang: &str) {
        *self.default_lang.write() = lang.to_string();
    }

    /// 获取默认语言代码
    pub fn default_lang(&self) -> String {
        self.default_lang.read().clone()
    }

    /// 变量插值
    ///
    /// 支持 PHP 两种占位符格式：
    /// - `:name` → 用 `vars["name"]` 替换
    /// - `{name}` → 用 `vars["name"]` 替换
    ///
    /// # 参数
    ///
    /// - `template`: 含占位符的模板字符串
    /// - `vars`: 插值变量（键名不含 `:` 或 `{}`）
    ///
    /// # 返回
    ///
    /// 插值后的字符串。未找到的变量保留原占位符。
    fn interpolate(&self, template: &str, vars: &HashMap<String, String>) -> String {
        let mut result = template.to_string();

        for (key, value) in vars {
            // 替换 :name 格式
            let colon_placeholder = format!(":{}", key);
            result = result.replace(&colon_placeholder, value);

            // 替换 {name} 格式
            let brace_placeholder = format!("{{{}}}", key);
            result = result.replace(&brace_placeholder, value);
        }

        result
    }

    /// 获取指定语言的所有键值对（快照）
    ///
    /// # 参数
    ///
    /// - `lang`: 语言代码
    ///
    /// # 返回
    ///
    /// 指定语言的所有键值对。语言不存在时返回空 HashMap。
    pub fn all_for_lang(&self, lang: &str) -> HashMap<String, String> {
        let data = self.data.read();
        data.get(lang).cloned().unwrap_or_default()
    }

    /// 获取已加载的所有语言代码列表
    ///
    /// # 返回
    ///
    /// 已加载的语言代码列表（无序）。
    pub fn available_langs(&self) -> Vec<String> {
        let data = self.data.read();
        data.keys().cloned().collect()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 测试空 I18n 实例
    #[test]
    fn test_new_i18n_default_lang() {
        let i18n = I18n::new();
        assert_eq!(i18n.current_lang(), "zh-cn");
        assert_eq!(i18n.default_lang(), "zh-cn");
    }

    /// 测试 with_default_lang 构造
    #[test]
    fn test_with_default_lang() {
        let i18n = I18n::with_default_lang("en-us");
        assert_eq!(i18n.current_lang(), "en-us");
        assert_eq!(i18n.default_lang(), "en-us");
    }

    /// 测试 set/get 基本流程
    #[test]
    fn test_set_get_basic() {
        let i18n = I18n::new();
        i18n.set("zh-cn", "hello", "你好");
        i18n.set("en-us", "hello", "Hello");

        assert_eq!(
            i18n.get_simple("hello", Some("zh-cn")),
            Some("你好".to_string())
        );
        assert_eq!(
            i18n.get_simple("hello", Some("en-us")),
            Some("Hello".to_string())
        );
    }

    /// 测试 get 使用当前语言
    #[test]
    fn test_get_uses_current_lang() {
        let i18n = I18n::with_default_lang("zh-cn");
        i18n.set("zh-cn", "hello", "你好");
        i18n.set("en-us", "hello", "Hello");

        // 默认当前语言 zh-cn
        assert_eq!(i18n.get_simple("hello", None), Some("你好".to_string()));

        // 切换到 en-us
        i18n.set_current_lang("en-us");
        assert_eq!(i18n.get_simple("hello", None), Some("Hello".to_string()));
    }

    /// 测试变量插值（:name 格式）
    #[test]
    fn test_interpolate_colon_format() {
        let i18n = I18n::new();
        i18n.set("en-us", "greeting", "Hello, :name! Welcome to :place.");

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "John".to_string());
        vars.insert("place".to_string(), "Rust".to_string());

        let result = i18n.get("greeting", &vars, Some("en-us")).unwrap();
        assert_eq!(result, "Hello, John! Welcome to Rust.");
    }

    /// 测试变量插值（{name} 格式）
    #[test]
    fn test_interpolate_brace_format() {
        let i18n = I18n::new();
        i18n.set("en-us", "greeting", "Hello, {name}! Welcome to {place}.");

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "John".to_string());
        vars.insert("place".to_string(), "Rust".to_string());

        let result = i18n.get("greeting", &vars, Some("en-us")).unwrap();
        assert_eq!(result, "Hello, John! Welcome to Rust.");
    }

    /// 测试混合占位符格式
    #[test]
    fn test_interpolate_mixed_format() {
        let i18n = I18n::new();
        i18n.set("en-us", "msg", ":name has {count} items in :place.");

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());
        vars.insert("count".to_string(), "5".to_string());
        vars.insert("place".to_string(), "basket".to_string());

        let result = i18n.get("msg", &vars, Some("en-us")).unwrap();
        assert_eq!(result, "Alice has 5 items in basket.");
    }

    /// 测试未找到的变量保留原占位符
    #[test]
    fn test_interpolate_missing_var_kept() {
        let i18n = I18n::new();
        i18n.set("en-us", "msg", "Hello, :name! Count: {count}");

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "John".to_string());
        // 不提供 count 变量

        let result = i18n.get("msg", &vars, Some("en-us")).unwrap();
        assert_eq!(result, "Hello, John! Count: {count}");
    }

    /// 测试语言回退到默认语言
    #[test]
    fn test_fallback_to_default_lang() {
        let i18n = I18n::with_default_lang("zh-cn");
        i18n.set("zh-cn", "only_in_zh", "只有中文");
        // 不设置 en-us 的对应项

        // 当前语言 en-us（无此项），回退到 zh-cn
        i18n.set_current_lang("en-us");
        assert_eq!(
            i18n.get_simple("only_in_zh", None),
            Some("只有中文".to_string())
        );
    }

    /// 测试 has 检查存在
    #[test]
    fn test_has() {
        let i18n = I18n::new();
        i18n.set("zh-cn", "existing", "存在");

        assert!(i18n.has("existing", Some("zh-cn")));
        assert!(!i18n.has("nonexistent", Some("zh-cn")));
    }

    /// 测试 has 的语言回退
    #[test]
    fn test_has_fallback() {
        let i18n = I18n::with_default_lang("zh-cn");
        i18n.set("zh-cn", "fallback_key", "回退");

        i18n.set_current_lang("en-us");
        assert!(i18n.has("fallback_key", None));
    }

    /// 测试从 JSON 字符串加载
    #[test]
    fn test_load_from_json_str() {
        let i18n = I18n::new();
        let json = r#"{"hello": "Hello, :name!", "bye": "Goodbye"}"#;
        i18n.load_from_json_str(json, "en-us", "<test>").unwrap();

        assert_eq!(
            i18n.get_simple("hello", Some("en-us")),
            Some("Hello, :name!".to_string())
        );
        assert_eq!(
            i18n.get_simple("bye", Some("en-us")),
            Some("Goodbye".to_string())
        );
    }

    /// 测试从 JSON 文件加载
    #[tokio::test]
    async fn test_load_from_file() {
        let temp_dir = std::env::temp_dir().join("sz_rust_i18n_test");
        let _ = std::fs::create_dir_all(&temp_dir);
        let lang_file = temp_dir.join("en-us.json");

        let mut file = std::fs::File::create(&lang_file).unwrap();
        writeln!(file, r#"{{"hello": "Hello!", "bye": "Goodbye"}}"#).unwrap();
        drop(file);

        let i18n = I18n::new();
        i18n.load_from_file(&lang_file, "en-us").await.unwrap();

        assert_eq!(
            i18n.get_simple("hello", Some("en-us")),
            Some("Hello!".to_string())
        );
        assert_eq!(
            i18n.get_simple("bye", Some("en-us")),
            Some("Goodbye".to_string())
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    /// 测试文件不存在时返回错误
    #[tokio::test]
    async fn test_load_nonexistent_file_errors() {
        let i18n = I18n::new();
        let result = i18n
            .load_from_file("/nonexistent/path/lang.json", "en-us")
            .await;
        assert!(result.is_err());
        match result {
            Err(I18nError::FileRead { .. }) => {}
            _ => panic!("期望 FileRead 错误"),
        }
    }

    /// 测试无效 JSON 返回解析错误
    #[test]
    fn test_load_invalid_json_errors() {
        let i18n = I18n::new();
        let result = i18n.load_from_json_str("{invalid json}", "en-us", "<test>");
        assert!(result.is_err());
        match result {
            Err(I18nError::Parse { .. }) => {}
            _ => panic!("期望 Parse 错误"),
        }
    }

    /// 测试 JSON 中的非字符串值被跳过
    #[test]
    fn test_load_json_non_string_values_skipped() {
        let i18n = I18n::new();
        let json = r#"{"valid": "string", "number": 123, "bool": true, "null_val": null}"#;
        i18n.load_from_json_str(json, "en-us", "<test>").unwrap();

        assert_eq!(
            i18n.get_simple("valid", Some("en-us")),
            Some("string".to_string())
        );
        assert_eq!(i18n.get_simple("number", Some("en-us")), None);
        assert_eq!(i18n.get_simple("bool", Some("en-us")), None);
        assert_eq!(i18n.get_simple("null_val", Some("en-us")), None);
    }

    /// 测试 set_current_lang / current_lang
    #[test]
    fn test_set_and_get_current_lang() {
        let i18n = I18n::new();
        assert_eq!(i18n.current_lang(), "zh-cn");

        i18n.set_current_lang("en-us");
        assert_eq!(i18n.current_lang(), "en-us");

        i18n.set_current_lang("ja-jp");
        assert_eq!(i18n.current_lang(), "ja-jp");
    }

    /// 测试 set_default_lang / default_lang
    #[test]
    fn test_set_and_get_default_lang() {
        let i18n = I18n::new();
        assert_eq!(i18n.default_lang(), "zh-cn");

        i18n.set_default_lang("en-us");
        assert_eq!(i18n.default_lang(), "en-us");
    }

    /// 测试 all_for_lang 返回指定语言的键值对快照
    #[test]
    fn test_all_for_lang() {
        let i18n = I18n::new();
        i18n.set("zh-cn", "key1", "值1");
        i18n.set("zh-cn", "key2", "值2");
        i18n.set("en-us", "key1", "value1");

        let zh_data = i18n.all_for_lang("zh-cn");
        assert_eq!(zh_data.len(), 2);
        assert_eq!(zh_data.get("key1"), Some(&"值1".to_string()));
        assert_eq!(zh_data.get("key2"), Some(&"值2".to_string()));

        let en_data = i18n.all_for_lang("en-us");
        assert_eq!(en_data.len(), 1);
        assert_eq!(en_data.get("key1"), Some(&"value1".to_string()));

        // 不存在的语言返回空
        let ja_data = i18n.all_for_lang("ja-jp");
        assert!(ja_data.is_empty());
    }

    /// 测试 available_langs 返回已加载语言列表
    #[test]
    fn test_available_langs() {
        let i18n = I18n::new();
        i18n.set("zh-cn", "key", "值");
        i18n.set("en-us", "key", "value");
        i18n.set("ja-jp", "key", "値");

        let langs = i18n.available_langs();
        assert_eq!(langs.len(), 3);
        assert!(langs.contains(&"zh-cn".to_string()));
        assert!(langs.contains(&"en-us".to_string()));
        assert!(langs.contains(&"ja-jp".to_string()));
    }

    /// 测试多次 load_from_json_str 累加而非覆盖
    #[test]
    fn test_multiple_load_accumulates() {
        let i18n = I18n::new();
        i18n.load_from_json_str(r#"{"key1": "value1"}"#, "en-us", "<test1>")
            .unwrap();
        i18n.load_from_json_str(r#"{"key2": "value2"}"#, "en-us", "<test2>")
            .unwrap();

        assert_eq!(
            i18n.get_simple("key1", Some("en-us")),
            Some("value1".to_string())
        );
        assert_eq!(
            i18n.get_simple("key2", Some("en-us")),
            Some("value2".to_string())
        );
    }

    /// 测试 get 不存在的键返回 None
    #[test]
    fn test_get_nonexistent_returns_none() {
        let i18n = I18n::new();
        assert_eq!(i18n.get_simple("nonexistent", Some("zh-cn")), None);
        assert_eq!(i18n.get_simple("nonexistent", None), None);
    }

    /// 测试空 vars 的插值
    #[test]
    fn test_interpolate_empty_vars() {
        let i18n = I18n::new();
        i18n.set("en-us", "msg", "Hello, :name!");

        let empty_vars = HashMap::new();
        let result = i18n.get("msg", &empty_vars, Some("en-us")).unwrap();
        // 未提供变量，占位符保留
        assert_eq!(result, "Hello, :name!");
    }
}

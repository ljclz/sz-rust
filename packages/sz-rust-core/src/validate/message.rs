//! 错误消息国际化 — 对齐 PHP `think\Lang` 和 `think\Validate::parseErrorMsg`
//!
//! Phase 5.4 核心交付物。本模块实现错误消息的国际化翻译机制，对齐 PHP
//! `think\Validate::parseErrorMsg`（第 1596-1633 行）的 Lang 翻译行为。
//!
//! ## PHP 对齐
//!
//! ### 核心接口映射
//!
//! | PHP 方法 | Rust 接口 | 说明 |
//! |---------|-----------|------|
//! | `Lang::has($name)` | [`Lang::has`] | 判断语言定义是否存在（不区分大小写） |
//! | `Lang::get($name)` | [`Lang::get`] | 获取语言定义（找不到返回 name 本身） |
//! | `Validate::setLang($lang)` | `Validate::set_lang` | 注入 Lang 实例 |
//! | `parseErrorMsg` 第 1598-1602 行 | [`translate_msg`] | 消息翻译（`{%var}` + `has` 检查） |
//!
//! ### PHP `parseErrorMsg` 翻译逻辑（R5-7）
//!
//! 对齐 PHP `Validate.php` 第 1598-1602 行：
//!
//! ```php
//! if (0 === strpos($msg, '{%')) {
//!     $msg = $this->lang->get(substr($msg, 2, -1));
//! } elseif ($this->lang->has($msg)) {
//!     $msg = $this->lang->get($msg);
//! }
//! ```
//!
//! 优先级：
//! 1. 如果 msg 以 `{%` 开头且以 `}` 结尾，提取内部 key 调用 `lang->get`
//! 2. 否则如果 `lang->has(msg)` 为真，调用 `lang->get(msg)`
//! 3. 否则返回 msg 原值
//!
//! ### PHP `Lang::has` / `Lang::get` 行为
//!
//! 对齐 PHP `Lang.php`：
//!
//! - **不区分大小写**：`strtolower($name)`（第 231/234 行）
//! - **找不到时返回 name 本身**：`?? $name`（第 263 行）
//! - **支持语言分组**：`allow_group` 配置（本实现不涉及，简化版）
//!
//! ## 架构说明
//!
//! 本模块提供 [`Lang`] trait 作为接口契约，[`SimpleLang`] 作为默认实现。
//! [`Validate`]`::set_lang` 接受 `Arc<dyn Lang>` 注入实例，`Option::None`
//! 时跳过翻译（对齐 PHP 未注入 Lang 时的行为）。
//!
//! [`Validate`]: crate::validate::Validate

use std::sync::Arc;

use indexmap::IndexMap;

// ============================================================================
// Lang trait — 对齐 PHP `think\Lang`
// ============================================================================

/// 多语言接口 — 对齐 PHP `think\Lang`
///
/// 对齐 PHP `Lang.php` 第 19-289 行的核心方法。
///
/// ## PHP 对齐
///
/// - `has(string $name): bool`（第 225-235 行）：不区分大小写
/// - `get(string $name = null, array $vars = []): mixed`（第 245-264 行）：
///   - 不区分大小写
///   - 找不到时返回 name 本身
///
/// ## Send + Sync 约束
///
/// 要求 `Send + Sync` 以便作为 `Arc<dyn Lang>` 在 `Validate` 中存储
/// 并跨线程共享。
pub trait Lang: Send + Sync {
    /// 判断是否存在语言定义（对齐 PHP `Lang::has`）
    ///
    /// 对齐 PHP `Lang.php` 第 225-235 行
    ///
    /// ## PHP 行为
    ///
    /// - 键不区分大小写（`strtolower($name)`）
    /// - 不支持分组（本简化版）
    fn has(&self, name: &str) -> bool;

    /// 获取语言定义（对齐 PHP `Lang::get`）
    ///
    /// 对齐 PHP `Lang.php` 第 245-264 行
    ///
    /// ## PHP 行为
    ///
    /// - 键不区分大小写
    /// - 找不到时返回 name 本身（对齐第 263 行 `?? $name`）
    fn get(&self, name: &str) -> String;
}

// ============================================================================
// SimpleLang — 基础实现
// ============================================================================

/// 简单多语言实现 — 基础键值对存储
///
/// 对齐 PHP `think\Lang` 的最小可用实现，不依赖文件加载、cookie、header
/// 检测等。适用于单元测试和简单场景。
///
/// ## PHP 对齐
///
/// - 键不区分大小写（对齐 PHP `strtolower($name)`）
/// - 找不到时返回 name 本身（对齐 PHP `?? $name`）
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::validate::message::{Lang, SimpleLang};
///
/// let lang = SimpleLang::new()
///     .set("not conform to the rules", "不符合规则")
///     .set("require", "必须填写");
/// assert!(lang.has("require"));
/// assert_eq!(lang.get("require"), "必须填写");
/// assert_eq!(lang.get("NOT FOUND"), "NOT FOUND"); // 找不到返回原值
/// ```
#[derive(Debug, Clone, Default)]
pub struct SimpleLang {
    /// 语言包（键已 lowercase，对齐 PHP `strtolower`）
    pack: IndexMap<String, String>,
}

impl SimpleLang {
    /// 创建空的语言包
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加翻译条目（对齐 PHP `$this->lang[$range][strtolower($name)] = $value`）
    pub fn set(mut self, name: &str, value: &str) -> Self {
        self.pack.insert(name.to_lowercase(), value.to_string());
        self
    }

    /// 批量添加翻译条目（对齐 PHP `Lang::load` 合并行为）
    pub fn extend(mut self, entries: IndexMap<String, String>) -> Self {
        for (k, v) in entries {
            self.pack.insert(k.to_lowercase(), v);
        }
        self
    }
}

impl Lang for SimpleLang {
    fn has(&self, name: &str) -> bool {
        self.pack.contains_key(&name.to_lowercase())
    }

    fn get(&self, name: &str) -> String {
        self.pack
            .get(&name.to_lowercase())
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
}

// ============================================================================
// 翻译函数 — 对齐 PHP `parseErrorMsg` 第 1598-1602 行
// ============================================================================

/// 翻译消息 — 对齐 PHP `Validate::parseErrorMsg` 第 1598-1602 行
///
/// ## PHP 行为（R5-7）
///
/// ```php
/// if (0 === strpos($msg, '{%')) {
///     $msg = $this->lang->get(substr($msg, 2, -1));
/// } elseif ($this->lang->has($msg)) {
///     $msg = $this->lang->get($msg);
/// }
/// ```
///
/// 优先级：
/// 1. `{%key}` 语法：提取 key 调用 `lang->get(key)`
/// 2. `lang->has(msg)`：调用 `lang->get(msg)`
/// 3. 否则返回 msg 原值
///
/// ## 参数
///
/// - `msg`：原始消息
/// - `lang`：可选的多语言实例（`None` 时跳过翻译）
///
/// ## 返回
///
/// 翻译后的消息（如果适用），否则返回原消息
pub fn translate_msg(msg: &str, lang: Option<&Arc<dyn Lang>>) -> String {
    let Some(lang) = lang else {
        return msg.to_string();
    };

    // 对齐 PHP 第 1598-1599 行：{%var} 语法
    // substr($msg, 2, -1) — 去掉开头 "{%" 和结尾 "}"
    if let Some(stripped) = msg.strip_prefix("{%").and_then(|s| s.strip_suffix('}')) {
        if !stripped.is_empty() {
            return lang.get(stripped);
        }
    }

    // 对齐 PHP 第 1600-1601 行：lang->has 检查
    if lang.has(msg) {
        return lang.get(msg);
    }

    msg.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // 组 1：SimpleLang 基础测试
    // ========================================================================

    #[test]
    fn test_simple_lang_new_empty() {
        let lang = SimpleLang::new();
        assert!(!lang.has("any"));
        assert_eq!(lang.get("any"), "any");
    }

    #[test]
    fn test_simple_lang_set_and_get() {
        let lang = SimpleLang::new().set("require", "必须填写");
        assert!(lang.has("require"));
        assert_eq!(lang.get("require"), "必须填写");
    }

    #[test]
    fn test_simple_lang_case_insensitive() {
        // 对齐 PHP strtolower 行为
        let lang = SimpleLang::new().set("Require", "必须填写");
        assert!(lang.has("require"));
        assert!(lang.has("REQUIRE"));
        assert!(lang.has("Require"));
        assert_eq!(lang.get("REQUIRE"), "必须填写");
    }

    #[test]
    fn test_simple_lang_not_found_returns_name() {
        // 对齐 PHP `?? $name` 行为
        let lang = SimpleLang::new().set("require", "必须填写");
        assert_eq!(lang.get("not_found"), "not_found");
    }

    #[test]
    fn test_simple_lang_chained_set() {
        let lang = SimpleLang::new()
            .set("require", "必须填写")
            .set("email", "邮箱格式错误");
        assert_eq!(lang.get("require"), "必须填写");
        assert_eq!(lang.get("email"), "邮箱格式错误");
    }

    #[test]
    fn test_simple_lang_extend() {
        let mut entries = IndexMap::new();
        entries.insert("require".to_string(), "必须".to_string());
        entries.insert("email".to_string(), "邮箱错误".to_string());
        let lang = SimpleLang::new().extend(entries);
        assert_eq!(lang.get("require"), "必须");
        assert_eq!(lang.get("email"), "邮箱错误");
    }

    #[test]
    fn test_simple_lang_override() {
        // 后设置的覆盖先设置的（对齐 PHP `+` 运算符行为）
        let lang = SimpleLang::new()
            .set("require", "旧值")
            .set("require", "新值");
        assert_eq!(lang.get("require"), "新值");
    }

    // ========================================================================
    // 组 2：translate_msg 测试（R5-7）
    // ========================================================================

    #[test]
    fn test_translate_msg_none_lang_returns_original() {
        // 无 Lang 实例时返回原消息
        let result = translate_msg("hello world", None);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_translate_msg_percent_var_syntax() {
        // 对齐 PHP `{%var}` 语法
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new().set("hello", "你好"));
        let result = translate_msg("{%hello}", Some(&lang));
        assert_eq!(result, "你好");
    }

    #[test]
    fn test_translate_msg_percent_var_not_found_returns_key() {
        // 对齐 PHP `Lang::get` 找不到返回 name 行为
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new());
        let result = translate_msg("{%not_found}", Some(&lang));
        assert_eq!(result, "not_found");
    }

    #[test]
    fn test_translate_msg_lang_has_check() {
        // 对齐 PHP `lang->has($msg)` 检查
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new().set("require", "必须填写"));
        let result = translate_msg("require", Some(&lang));
        assert_eq!(result, "必须填写");
    }

    #[test]
    fn test_translate_msg_no_match_returns_original() {
        // 既不是 {%var} 也不在 lang 中
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new().set("require", "必须填写"));
        let result = translate_msg("custom message", Some(&lang));
        assert_eq!(result, "custom message");
    }

    #[test]
    fn test_translate_msg_percent_var_priority_over_has() {
        // {%var} 优先于 has 检查
        let lang: Arc<dyn Lang> = Arc::new(
            SimpleLang::new()
                .set("require", "直接翻译")
                .set("custom_key", "key翻译"),
        );
        // {%custom_key} 应该返回 "key翻译"
        let result = translate_msg("{%custom_key}", Some(&lang));
        assert_eq!(result, "key翻译");
    }

    #[test]
    fn test_translate_msg_empty_percent_var() {
        // {%} 空键情况 — 不应该走 {%var} 分支
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new());
        let result = translate_msg("{}", Some(&lang));
        // 不以 {% 开头，走 has 检查 → 不存在 → 返回原值
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_translate_msg_only_prefix() {
        // "{%" 不完整，不应该走 {%var} 分支
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new());
        let result = translate_msg("{%", Some(&lang));
        // strip_suffix('}') 失败，走 has 检查 → 不存在 → 返回原值
        assert_eq!(result, "{%");
    }

    #[test]
    fn test_translate_msg_percent_var_with_special_chars() {
        // {%var} 中包含特殊字符
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new().set("foo.bar", "点号键"));
        let result = translate_msg("{%foo.bar}", Some(&lang));
        assert_eq!(result, "点号键");
    }

    // ========================================================================
    // 组 3：PHP 行为对齐测试
    // ========================================================================

    #[test]
    fn test_php_behavior_lang_case_insensitive() {
        // 对齐 PHP Lang::has/get 的 strtolower 行为
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new().set("Hello", "你好"));
        assert!(lang.has("hello"));
        assert!(lang.has("HELLO"));
        assert_eq!(lang.get("hello"), "你好");
        assert_eq!(lang.get("HELLO"), "你好");
    }

    #[test]
    fn test_php_behavior_lang_get_returns_name_when_not_found() {
        // 对齐 PHP Lang::get 第 263 行 `?? $name`
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new());
        assert_eq!(lang.get("nonexistent"), "nonexistent");
        assert_eq!(lang.get("Some Key"), "Some Key");
    }

    #[test]
    fn test_php_behavior_translate_percent_var_extracts_key() {
        // 对齐 PHP substr($msg, 2, -1) 提取 key
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new().set("my_key", "翻译值"));
        let result = translate_msg("{%my_key}", Some(&lang));
        assert_eq!(result, "翻译值");
    }

    #[test]
    fn test_php_behavior_translate_has_check_falls_back_to_original() {
        // 对齐 PHP elseif 分支：has 为 false 时返回原值
        let lang: Arc<dyn Lang> = Arc::new(SimpleLang::new());
        let result = translate_msg("plain message", Some(&lang));
        assert_eq!(result, "plain message");
    }

    #[test]
    fn test_php_behavior_translate_priority_percent_over_has() {
        // 对齐 PHP if 优先于 elseif
        // 当 msg 是 {%var} 形式时，即使 lang->has(msg) 为真，也走 {%var} 分支
        let lang: Arc<dyn Lang> = Arc::new(
            SimpleLang::new()
                .set("{%key}", "整体键值") // 这种情况理论上不会出现
                .set("key", "提取键值"),
        );
        let result = translate_msg("{%key}", Some(&lang));
        // {%key} 应该提取 "key" 然后翻译，而不是把 "{%key}" 作为整体键
        assert_eq!(result, "提取键值");
    }
}

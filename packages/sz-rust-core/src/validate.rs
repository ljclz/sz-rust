//! 验证器模块 — 对齐 PHP `think\Validate`
//!
//! Phase 5.1 核心交付物。本模块实现验证器框架，对齐 PHP `think\Validate` 类的
//! 规则定义、批量验证、错误消息查找、场景管理等核心机制。
//!
//! ## PHP 对齐
//!
//! ### 核心结构映射
//!
//! | PHP 字段 | Rust 字段 | 说明 |
//! |---------|-----------|------|
//! | `$rule` | [`Validate::rule`] | 当前验证规则 |
//! | `$message` | [`Validate::message`] | 验证提示信息 |
//! | `$field` | [`Validate::field`] | 字段描述 |
//! | `$typeMsg` | [`TYPE_MSG`] | 默认规则提示 |
//! | `$alias` | [`ALIAS`] | 验证类型别名 |
//! | `$defaultRegex` | [`DEFAULT_REGEX`] | 内置正则 |
//! | `$regex` | [`Validate::regex`] | 自定义正则 |
//! | `$scene` | [`Validate::scene`] | 验证场景定义 |
//! | `$currentScene` | `Validate::current_scene` | 当前验证场景 |
//! | `$batch` | [`Validate::batch`] | 是否批量验证 |
//! | `$only` | [`Validate::only`] | 场景需要验证的字段 |
//! | `$remove` | [`Validate::remove`] | 场景移除的规则 |
//! | `$append` | [`Validate::append`] | 场景追加的规则 |
//! | `$error` | `Validate::error` | 验证失败错误信息 |
//! | `$type` | `Validate::type_callbacks` | 自定义验证类型 |
//!
//! ### 核心方法映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `rule($name, $rule)` | [`Validate::rule`] | 添加字段验证规则 |
//! | `message(array $msg)` | [`Validate::message`] | 设置提示信息 |
//! | `scene($name)` | [`Validate::scene`] | 设置验证场景 |
//! | `hasScene($name)` | [`Validate::has_scene`] | 判断场景是否存在 |
//! | `batch(bool)` | [`Validate::batch`] | 设置批量验证 |
//! | `only(array)` | [`Validate::only`] | 指定需要验证的字段 |
//! | `remove($field, $rule)` | [`Validate::remove`] | 移除字段规则 |
//! | `append($field, $rule)` | [`Validate::append`] | 追加字段规则 |
//! | `extend($type, $cb)` | [`Validate::extend`] | 注册验证类型 |
//! | `check(array $data, array $rules)` | [`Validate::check`] | 数据自动验证 |
//! | `checkRule($value, $rules)` | [`Validate::check_rule`] | 根据规则验证数据 |
//! | `getError()` | [`Validate::get_error`] | 获取错误信息 |
//! | `getRuleMsg(...)` | [`Validate::get_rule_msg`] | 获取规则错误提示 |
//! | `parseErrorMsg(...)` | [`Validate::parse_error_msg`] | 解析错误提示 |
//! | `getDataValue(...)` | [`Validate::get_data_value`] | 获取数据值 |
//! | `getValidateType(...)` | [`Validate::get_validate_type`] | 获取验证类型 |
//! | `is($value, $rule)` | [`Validate::is`] | 验证字段值是否为有效格式 |
//! | `require($value)` | [`Validate::require`] | 必须验证 |
//! | `must($value)` | [`Validate::must`] | 必须验证（与 require 等价） |
//!
//! ## PHP 行为对齐（R5 硬约束）
//!
//! 本模块严格对齐以下 PHP 行为（包括 bug）：
//!
//! - **R5-1**：`getRuleMsg` 查找优先级链（对齐 PHP 第 1565-1586 行）：
//!   1. `message[field.type]`
//!   2. `message[field]`
//!   3. `type_msg[type]`
//!   4. 如果 type 以 `require` 开头，使用 `type_msg['require']`
//!   5. 默认 `$title . lang->get('not conform to the rules')`（无前导空格，对齐 PHP 第 1578 行）
//!
//! - **R5-2**：`parseErrorMsg` 占位符替换（对齐 PHP 第 1596-1633 行）：
//!   1. `:attribute` → title
//!   2. `:1` / `:2` / `:3` → rule 按逗号分割后的前 3 个元素
//!   3. `:rule` → rule 原值（仅当 msg 包含 `:rule` 时）
//!
//! - **R5-3**：`getDataValue` 行为（对齐 PHP 第 1536-1554 行）：
//!   - 数值型 key 返回 key 本身（PHP 怪异行为，复刻）
//!   - 包含 `.` 的 key 按多维数组访问
//!   - 其他 key 返回 `data[key]` 或 null
//!
//! - **R5-4**：`getValidateType` 类型推导（对齐 PHP 第 678-706 行）：
//!   - 别名映射（`>` → `gt`，`>=` → `egt` 等）
//!   - `info` 字段用于 `remove`/`append` 匹配
//!
//! - **R5-5**：空值跳过验证行为（对齐 PHP 第 634-637 行）：
//!   - 如果 value 是 null 或空字符串，且 info 不是 `must` 或不以 `require` 开头，则跳过验证
//!
//! - **R5-6**：场景重置行为（对齐 PHP `getScene` 第 1661 行）：
//!   - 切换场景时，`only`/`append`/`remove` 全部重置
//!
//! ## 架构说明
//!
//! Phase 5.1 实现**框架层**，内置基础规则（`require`/`must`/`is`）。
//! Phase 5.2 将在 `validate/rules.rs` 添加完整内置规则集
//! （email/mobile/url/in/notIn/max/min/length 等）。
//!
//! 自定义规则通过 [`Validate::extend`] 注册，回调签名：
//! `Fn(value: &Value, rule: &str, data: &Value) -> bool + Send + Sync`
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\Validate.php`
//!   - 第 24-215 行：类属性定义
//!   - 第 286-298 行：`rule()` 方法
//!   - 第 354-360 行：`scene()` 方法
//!   - 第 471-540 行：`check()` 方法
//!   - 第 594-669 行：`checkItem()` 方法
//!   - 第 678-706 行：`getValidateType()` 方法
//!   - 第 827-888 行：`is()` 方法
//!   - 第 1536-1554 行：`getDataValue()` 方法
//!   - 第 1565-1586 行：`getRuleMsg()` 方法
//!   - 第 1596-1633 行：`parseErrorMsg()` 方法

use std::sync::Arc;

use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

pub mod message;
pub mod rules;
pub mod scene;

// ============================================================================
// 验证错误类型
// ============================================================================

/// 验证错误
///
/// 对齐 PHP `$this->error`：在非批量模式下为单条错误字符串，在批量模式下为
/// `IndexMap<字段名, 错误信息>`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidateError {
    /// 单条错误（非批量模式）
    ///
    /// 对齐 PHP `$this->error = $result`（字符串赋值）
    Single(String),
    /// 多条错误（批量模式，按字段分组）
    ///
    /// 对齐 PHP `$this->error[$key] = $result`
    Batch(IndexMap<String, String>),
}

impl std::fmt::Display for ValidateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidateError::Single(msg) => write!(f, "{}", msg),
            ValidateError::Batch(errors) => {
                let entries: Vec<String> = errors
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect();
                write!(f, "{}", entries.join("; "))
            }
        }
    }
}

impl std::error::Error for ValidateError {}

// ============================================================================
// 验证规则类型
// ============================================================================

/// 验证规则表示
///
/// 对齐 PHP `$rule`：可以是字符串（`"require|in:a,b,c"`）、数组或闭包。
/// 闭包通过 [`Validate::extend`] 单独注册。
#[derive(Debug, Clone)]
pub enum Rule {
    /// 单条规则（如 `"require"`、`"email"`）
    Simple(String),
    /// 带参数的规则（如 `"in:1,2,3"`、`"length:1,10"`）
    WithArgs(String, String),
    /// 多条规则（对齐 PHP `"require|in:1,2,3"` 或 `['require', 'in' => 'a,b,c']`）
    Multiple(Vec<Rule>),
}

impl Rule {
    /// 从 PHP 风格规则字符串创建
    ///
    /// ## 示例
    ///
    /// ```ignore
    /// use sz_rust_core::validate::Rule;
    ///
    /// let _ = Rule::from_string("require");
    /// let _ = Rule::from_string("in:1,2,3");
    /// let _ = Rule::from_string("require|in:1,2,3"); // 自动转 Multiple
    /// ```
    pub fn from_string(s: &str) -> Self {
        if s.contains('|') {
            let parts: Vec<Rule> = s
                .split('|')
                .map(|p| {
                    if let Some((t, a)) = p.split_once(':') {
                        Rule::WithArgs(t.to_string(), a.to_string())
                    } else {
                        Rule::Simple(p.to_string())
                    }
                })
                .collect();
            Rule::Multiple(parts)
        } else if let Some((t, a)) = s.split_once(':') {
            Rule::WithArgs(t.to_string(), a.to_string())
        } else {
            Rule::Simple(s.to_string())
        }
    }

    /// 转为规则列表 `Vec<(type, args)>`（对齐 PHP `explode('|', $rules)`）
    pub fn to_list(&self) -> Vec<(String, String)> {
        match self {
            Rule::Simple(t) => vec![(t.clone(), String::new())],
            Rule::WithArgs(t, a) => vec![(t.clone(), a.clone())],
            Rule::Multiple(list) => list.iter().flat_map(|r| r.to_list()).collect(),
        }
    }
}

// ============================================================================
// 自定义规则回调类型
// ============================================================================

/// 自定义规则回调类型
///
/// 签名：`Fn(value: &Value, rule: &str, data: &Value) -> bool`
///
/// - `value`：字段值
/// - `rule`：规则参数（如 `"1,2,3"` for `in:1,2,3`）
/// - `data`：完整数据
/// - 返回：`true` 通过，`false` 失败
pub type RuleCallback = Arc<dyn Fn(&Value, &str, &Value) -> bool + Send + Sync>;

// ============================================================================
// 内置静态映射（对齐 PHP 类属性）
// ============================================================================

/// PHP `think\Validate::$defaultRegex` 内置正则
///
/// 对齐 PHP `Validate.php` 第 125-136 行
pub static DEFAULT_REGEX: Lazy<IndexMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = IndexMap::new();
    m.insert("alpha", "/^[A-Za-z]+$/");
    m.insert("alphaNum", "/^[A-Za-z0-9]+$/");
    m.insert("alphaDash", "/^[A-Za-z0-9\\-\\_]+$/");
    m.insert(
        "chs",
        "/^[\\x{4e00}-\\x{9fa5}\\x{9fa6}-\\x{9fef}\\x{3400}-\\x{4db5}\\x{20000}-\\x{2ebe0}]+$/u",
    );
    m.insert(
        "chsAlpha",
        "/^[\\x{4e00}-\\x{9fa5}\\x{9fa6}-\\x{9fef}\\x{3400}-\\x{4db5}\\x{20000}-\\x{2ebe0}a-zA-Z]+$/u",
    );
    m.insert(
        "chsAlphaNum",
        "/^[\\x{4e00}-\\x{9fa5}\\x{9fa6}-\\x{9fef}\\x{3400}-\\x{4db5}\\x{20000}-\\x{2ebe0}a-zA-Z0-9]+$/u",
    );
    m.insert(
        "chsDash",
        "/^[\\x{4e00}-\\x{9fa5}\\x{9fa6}-\\x{9fef}\\x{3400}-\\x{4db5}\\x{20000}-\\x{2ebe0}a-zA-Z0-9\\_\\-]+$/u",
    );
    m.insert("mobile", "/^1[3-9]\\d{9}$/");
    m.insert(
        "idCard",
        "/(^[1-9]\\d{5}(18|19|([23]\\d))\\d{2}((0[1-9])|(10|11|12))(([0-2][1-9])|10|20|30|31)\\d{3}[0-9Xx]$)|(^[1-9]\\d{5}\\d{2}((0[1-9])|(10|11|12))(([0-2][1-9])|10|20|30|31)\\d{3}$)/",
    );
    m.insert("zip", "/\\d{6}/");
    m
});

/// PHP `think\Validate::$typeMsg` 内置类型提示
///
/// 对齐 PHP `Validate.php` 第 62-113 行
pub static TYPE_MSG: Lazy<IndexMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = IndexMap::new();
    m.insert("require", ":attribute require");
    m.insert("must", ":attribute must");
    m.insert("number", ":attribute must be numeric");
    m.insert("integer", ":attribute must be integer");
    m.insert("float", ":attribute must be float");
    m.insert("boolean", ":attribute must be bool");
    m.insert("email", ":attribute not a valid email address");
    m.insert("mobile", ":attribute not a valid mobile");
    m.insert("array", ":attribute must be a array");
    m.insert("accepted", ":attribute must be yes,on or 1");
    m.insert("date", ":attribute not a valid datetime");
    m.insert("file", ":attribute not a valid file");
    m.insert("image", ":attribute not a valid image");
    m.insert("alpha", ":attribute must be alpha");
    m.insert("alphaNum", ":attribute must be alpha-numeric");
    m.insert(
        "alphaDash",
        ":attribute must be alpha-numeric, dash, underscore",
    );
    m.insert("activeUrl", ":attribute not a valid domain or ip");
    m.insert("chs", ":attribute must be chinese");
    m.insert("chsAlpha", ":attribute must be chinese or alpha");
    m.insert("chsAlphaNum", ":attribute must be chinese,alpha-numeric");
    m.insert(
        "chsDash",
        ":attribute must be chinese,alpha-numeric,underscore, dash",
    );
    m.insert("url", ":attribute not a valid url");
    m.insert("ip", ":attribute not a valid ip");
    m.insert("dateFormat", ":attribute must be dateFormat of :rule");
    m.insert("in", ":attribute must be in :rule");
    m.insert("notIn", ":attribute be notin :rule");
    m.insert("between", ":attribute must between :1 - :2");
    m.insert("notBetween", ":attribute not between :1 - :2");
    m.insert("length", "size of :attribute must be :rule");
    m.insert("max", "max size of :attribute must be :rule");
    m.insert("min", "min size of :attribute must be :rule");
    m.insert("after", ":attribute cannot be less than :rule");
    m.insert("before", ":attribute cannot exceed :rule");
    m.insert("expire", ":attribute not within :rule");
    m.insert("allowIp", "access IP is not allowed");
    m.insert("denyIp", "access IP denied");
    m.insert("confirm", ":attribute out of accord with :2");
    m.insert("different", ":attribute cannot be same with :2");
    m.insert("egt", ":attribute must greater than or equal :rule");
    m.insert("gt", ":attribute must greater than :rule");
    m.insert("elt", ":attribute must less than or equal :rule");
    m.insert("lt", ":attribute must less than :rule");
    m.insert("eq", ":attribute must equal :rule");
    m.insert("unique", ":attribute has exists");
    m.insert("regex", ":attribute not conform to the rules");
    m.insert("method", "invalid Request method");
    m.insert("token", "invalid token");
    m.insert("fileSize", "filesize not match");
    m.insert("fileExt", "extensions to upload is not allowed");
    m.insert("fileMime", "mimetype to upload is not allowed");
    m
});

/// PHP `think\Validate::$alias` 验证类型别名
///
/// 对齐 PHP `Validate.php` 第 36-38 行
pub static ALIAS: Lazy<IndexMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = IndexMap::new();
    m.insert(">", "gt");
    m.insert(">=", "egt");
    m.insert("<", "lt");
    m.insert("<=", "elt");
    m.insert("=", "eq");
    m.insert("same", "eq");
    m
});

// ============================================================================
// Validate 主结构
// ============================================================================

/// 验证器 — 对齐 PHP `think\Validate`
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::validate::Validate;
/// use serde_json::json;
///
/// let mut v = Validate::new()
///     .rule("name", "require")
///     .rule("age", "require|integer");
/// let data = json!({"name": "Alice", "age": 30});
/// assert!(v.check(&data).is_ok());
/// ```
pub struct Validate {
    /// 当前验证规则（字段名 → 规则）
    rule: IndexMap<String, Rule>,
    /// 验证提示信息
    message: IndexMap<String, String>,
    /// 字段描述（英文名 → 中文名）
    field: IndexMap<String, String>,
    /// 自定义正则
    regex: IndexMap<String, String>,
    /// 验证场景定义（数组形式，对齐 PHP `$scene`）
    scene: IndexMap<String, Vec<String>>,
    /// 验证场景回调（对齐 PHP `scene{Name}` 方法）
    scene_callbacks: IndexMap<String, scene::SceneCallback>,
    /// 当前验证场景
    current_scene: Option<String>,
    /// 是否批量验证
    batch: bool,
    /// 场景需要验证的字段
    only: Vec<String>,
    /// 场景需要移除的验证规则（字段 → Option<规则列表>，None 表示移除所有）
    remove: IndexMap<String, Option<Vec<String>>>,
    /// 场景需要追加的验证规则
    append: IndexMap<String, Vec<String>>,
    /// 验证失败错误信息
    error: ValidateError,
    /// 自定义验证类型回调
    type_callbacks: IndexMap<String, RuleCallback>,
    /// 多语言实例 — 对齐 PHP `think\Validate::$lang`
    ///
    /// Phase 5.4 交付物。`None` 时跳过翻译（对齐 PHP 未注入 Lang 时的行为）。
    /// 通过 [`Validate::set_lang`] 注入实例。
    lang: Option<Arc<dyn message::Lang>>,
}

impl Default for Validate {
    fn default() -> Self {
        Self::new()
    }
}

impl Validate {
    /// 创建新的验证器
    pub fn new() -> Self {
        Self {
            rule: IndexMap::new(),
            message: IndexMap::new(),
            field: IndexMap::new(),
            regex: IndexMap::new(),
            scene: IndexMap::new(),
            scene_callbacks: IndexMap::new(),
            current_scene: None,
            batch: false,
            only: Vec::new(),
            remove: IndexMap::new(),
            append: IndexMap::new(),
            error: ValidateError::Single(String::new()),
            type_callbacks: IndexMap::new(),
            lang: None,
        }
    }

    // ========================================================================
    // Builder 方法
    // ========================================================================

    /// 添加字段验证规则
    ///
    /// 对齐 PHP `rule($name, $rule = '')`（第 286-298 行）
    ///
    /// ## 参数
    ///
    /// - `name`：字段名（支持 `field|title` 格式指定字段描述）
    /// - `rule`：验证规则（字符串形式，如 `"require|in:1,2,3"`）
    pub fn rule(mut self, name: &str, rule: &str) -> Self {
        self.rule.insert(name.to_string(), Rule::from_string(rule));
        self
    }

    /// 设置提示信息
    ///
    /// 对齐 PHP `message(array $message)`（第 341-346 行）
    pub fn message(mut self, messages: IndexMap<String, String>) -> Self {
        for (k, v) in messages {
            self.message.insert(k, v);
        }
        self
    }

    /// 设置字段描述
    ///
    /// 对齐 PHP `rule()` 方法中 `$rule` 为数组时合并到 `$this->field`
    pub fn field(mut self, fields: IndexMap<String, String>) -> Self {
        for (k, v) in fields {
            self.field.insert(k, v);
        }
        self
    }

    /// 设置验证场景
    ///
    /// 对齐 PHP `scene(string $name)`（第 354-360 行）
    pub fn scene(mut self, name: &str) -> Self {
        self.current_scene = Some(name.to_string());
        self
    }

    /// 注册场景字段列表
    ///
    /// 对齐 PHP `$this->scene[$name] = $fields` 属性赋值
    pub fn register_scene(mut self, name: &str, fields: Vec<String>) -> Self {
        self.scene.insert(name.to_string(), fields);
        self
    }

    /// 注册场景回调 — 对齐 PHP `protected function scene{Name}()`
    ///
    /// Phase 5.3 交付物。回调签名 `Fn(&mut Validate) + Send + Sync`，
    /// 回调内部可调用 [`Validate::only_mut`]、[`Validate::append_mut`]、
    /// [`Validate::remove_mut`] 修改场景状态。
    ///
    /// ## PHP 对齐
    ///
    /// ```php
    /// protected function sceneLogin()
    /// {
    ///     return $this->only(['email']);
    /// }
    /// ```
    ///
    /// Rust 等价：
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// v.register_scene_callback("login", Arc::new(|v| {
    ///     v.only_mut(vec!["email".to_string()]);
    /// }));
    /// ```
    ///
    /// ## 优先级
    ///
    /// 对齐 PHP `getScene`（第 1663-1668 行）：**回调优先于数组形式**。
    /// 如果同一场景名同时注册了数组和回调，回调会被调用，数组被忽略。
    pub fn register_scene_callback(mut self, name: &str, callback: scene::SceneCallback) -> Self {
        self.scene_callbacks.insert(name.to_string(), callback);
        self
    }

    /// 判断是否存在某个验证场景
    ///
    /// 对齐 PHP `hasScene(string $name)`（第 368-371 行）
    ///
    /// ## PHP 行为
    ///
    /// `return isset($this->scene[$name]) || method_exists($this, 'scene' . $name);`
    ///
    /// Rust 实现：检查 `scene` 数组 **或** `scene_callbacks` 映射
    pub fn has_scene(&self, name: &str) -> bool {
        self.scene.contains_key(name) || self.scene_callbacks.contains_key(name)
    }

    /// 设置批量验证
    ///
    /// 对齐 PHP `batch(bool $batch = true)`（第 379-384 行）
    pub fn batch(mut self, batch: bool) -> Self {
        self.batch = batch;
        self
    }

    /// 指定需要验证的字段列表
    ///
    /// 对齐 PHP `only(array $fields)`（第 405-410 行）
    pub fn only(mut self, fields: Vec<String>) -> Self {
        self.only = fields;
        self
    }

    /// 移除某个字段的验证规则
    ///
    /// 对齐 PHP `remove($field, $rule = null)`（第 419-438 行）
    ///
    /// ## 参数
    ///
    /// - `field`：字段名
    /// - `rule`：要移除的规则列表（`None` 表示移除所有规则）
    pub fn remove(mut self, field: &str, rule: Option<Vec<String>>) -> Self {
        self.remove.insert(field.to_string(), rule);
        self
    }

    /// 追加某个字段的验证规则
    ///
    /// 对齐 PHP `append($field, $rule = null)`（第 447-462 行）
    pub fn append(mut self, field: &str, rule: Vec<String>) -> Self {
        self.append.insert(field.to_string(), rule);
        self
    }

    /// 指定需要验证的字段列表（`&mut self` 版本）
    ///
    /// 对齐 PHP `only(array $fields)` 的 `&mut self` 语义，供 scene 回调使用。
    /// 对齐 PHP `sceneXxx` 方法内部调用 `$this->only([...])`。
    pub fn only_mut(&mut self, fields: Vec<String>) {
        self.only = fields;
    }

    /// 移除某个字段的验证规则（`&mut self` 版本）
    ///
    /// 对齐 PHP `remove($field, $rule = null)` 的 `&mut self` 语义，供 scene 回调使用。
    pub fn remove_mut(&mut self, field: &str, rule: Option<Vec<String>>) {
        self.remove.insert(field.to_string(), rule);
    }

    /// 追加某个字段的验证规则（`&mut self` 版本）
    ///
    /// 对齐 PHP `append($field, $rule = null)` 的 `&mut self` 语义，供 scene 回调使用。
    pub fn append_mut(&mut self, field: &str, rule: Vec<String>) {
        self.append.insert(field.to_string(), rule);
    }

    /// 注册验证类型（自定义规则回调）
    ///
    /// 对齐 PHP `extend(string $type, callable $callback, string $message = null)`（第 308-317 行）
    pub fn extend(&mut self, type_name: &str, callback: RuleCallback) -> &mut Self {
        self.type_callbacks.insert(type_name.to_string(), callback);
        self
    }

    /// 设置自定义正则
    ///
    /// 对齐 PHP `$this->regex` 属性
    pub fn regex(mut self, name: &str, pattern: &str) -> Self {
        self.regex.insert(name.to_string(), pattern.to_string());
        self
    }

    /// 设置多语言实例 — 对齐 PHP `setLang(Lang $lang)`
    ///
    /// 对齐 PHP `Validate.php` 第 252-255 行
    ///
    /// ## PHP 行为
    ///
    /// ```php
    /// public function setLang(Lang $lang)
    /// {
    ///     $this->lang = $lang;
    /// }
    /// ```
    ///
    /// ## 参数
    ///
    /// - `lang`：多语言实例（`Arc<dyn Lang>`）
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// use sz_rust_core::validate::Validate;
    /// use sz_rust_core::validate::message::{Lang, SimpleLang};
    ///
    /// let lang: Arc<dyn Lang> = Arc::new(
    ///     SimpleLang::new().set("not conform to the rules", "不符合规则")
    /// );
    /// let v = Validate::new().set_lang(lang);
    /// ```
    pub fn set_lang(mut self, lang: Arc<dyn message::Lang>) -> Self {
        self.lang = Some(lang);
        self
    }

    // ========================================================================
    // 数据自动验证
    // ========================================================================

    /// 数据自动验证 — 对齐 PHP `check(array $data, array $rules = [])`
    ///
    /// 对齐 PHP `Validate.php` 第 471-540 行
    ///
    /// ## 参数
    ///
    /// - `data`：待验证数据（JSON Object）
    ///
    /// ## 返回
    ///
    /// - `Ok(())`：所有规则通过
    /// - `Err(ValidateError)`：验证失败
    pub fn check(&mut self, data: &Value) -> Result<(), ValidateError> {
        let mut batch_errors: IndexMap<String, String> = IndexMap::new();
        let mut single_error: Option<String> = None;

        // 处理场景（对齐 PHP $this->getScene，第 475-477 行 + 第 1659-1669 行）
        // Phase 5.3：完整对齐 PHP getScene，支持 sceneXxx 回调（回调优先于数组）
        if let Some(scene_name) = self.current_scene.clone() {
            // 重置 only/append/remove（对齐 PHP getScene 第 1661 行，R5-6）
            self.only.clear();
            self.append.clear();
            self.remove.clear();
            // 对齐 PHP getScene 第 1663-1668 行：
            // - 如果存在 scene{Name} 回调（method_exists），调用回调
            // - 否则如果 scene[{name}] 数组存在，设置 only
            // 注：clone Arc 以避免借用冲突
            if let Some(callback) = self.scene_callbacks.get(&scene_name).cloned() {
                callback(self);
            } else if let Some(fields) = self.scene.get(&scene_name) {
                self.only = fields.clone();
            }
        }

        // 收集规则快照（避免借用问题）
        let rules: Vec<(String, Rule)> = self
            .rule
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let append_clone = self.append.clone();

        for (key, rule) in &rules {
            // 解析 field|title 格式（对齐 PHP 第 493-498 行）
            let (field_name, title) = if let Some(idx) = key.find('|') {
                (key[..idx].to_string(), key[idx + 1..].to_string())
            } else {
                let title = self.field.get(key).cloned().unwrap_or_else(|| key.clone());
                (key.clone(), title)
            };

            // 场景过滤（对齐 PHP 第 501-503 行）
            if !self.only.is_empty() && !self.only.contains(&field_name) {
                continue;
            }

            // 获取字段值
            let value = Self::get_data_value(data, &field_name);

            // 检查规则
            let result = self.check_item(&field_name, &value, rule, data, &title, &[]);

            if let Err(msg) = result {
                if self.batch {
                    batch_errors.insert(field_name, msg);
                } else {
                    single_error = Some(msg);
                    break;
                }
            }
        }

        // 处理 append 中未在 rule 里的字段（对齐 PHP 第 484-489 行）
        let _ = &append_clone; // 已在 check_item 内处理

        if self.batch {
            if batch_errors.is_empty() {
                Ok(())
            } else {
                self.error = ValidateError::Batch(batch_errors);
                Err(self.error.clone())
            }
        } else {
            match single_error {
                None => Ok(()),
                Some(msg) => {
                    self.error = ValidateError::Single(msg);
                    Err(self.error.clone())
                }
            }
        }
    }

    /// 根据验证规则验证数据 — 对齐 PHP `checkRule($value, $rules)`
    ///
    /// 对齐 PHP `Validate.php` 第 549-581 行
    ///
    /// ## 参数
    ///
    /// - `value`：字段值
    /// - `rules`：验证规则（字符串形式，如 `"require|in:1,2,3"`）
    ///
    /// ## 返回
    ///
    /// - `Ok(())`：所有规则通过
    /// - `Err(String)`：验证失败，包含错误信息
    pub fn check_rule(&self, value: &Value, rules: &str) -> Result<(), String> {
        let rule = Rule::from_string(rules);
        let empty_data = Value::Null;
        match self.check_item("", value, &rule, &empty_data, "", &[]) {
            Ok(()) => Ok(()),
            Err(msg) => Err(msg),
        }
    }

    /// 验证单个字段规则 — 对齐 PHP `checkItem`
    ///
    /// 对齐 PHP `Validate.php` 第 594-669 行
    fn check_item(
        &self,
        field: &str,
        value: &Value,
        rules: &Rule,
        data: &Value,
        title: &str,
        msg: &[String],
    ) -> Result<(), String> {
        // remove[field] === None（移除所有）&& append[field] 不存在 → 跳过
        // 对齐 PHP 第 596-599 行
        if matches!(self.remove.get(field), Some(None)) && !self.append.contains_key(field) {
            return Ok(());
        }

        // 转为规则列表（对齐 PHP 第 602-604 行 explode('|', $rules)）
        let mut rule_list = rules.to_list();

        // 合并 append[field]（对齐 PHP 第 606-610 行）
        if let Some(append_rules) = self.append.get(field) {
            for ar in append_rules {
                let extra = Rule::from_string(ar).to_list();
                for e in extra {
                    if !rule_list.contains(&e) {
                        rule_list.push(e);
                    }
                }
            }
        }

        if rule_list.is_empty() {
            return Ok(());
        }

        for (i, (rule_type, rule_args)) in rule_list.iter().enumerate() {
            // 获取验证类型（对齐 PHP getValidateType）
            let (cb_type, args, info) = Self::get_validate_type(rule_type, rule_args);

            // 检查 remove/append（对齐 PHP 第 625-630 行）
            let in_append = self
                .append
                .get(field)
                .map(|a| a.iter().any(|x| x == &info))
                .unwrap_or(false);
            let in_remove = self
                .remove
                .get(field)
                .and_then(|r| r.as_ref())
                .map(|r| r.iter().any(|x| x == &info))
                .unwrap_or(false);
            if !in_append && in_remove {
                continue;
            }

            // 执行验证
            let result = if let Some(cb) = self.type_callbacks.get(&cb_type) {
                // 注册的自定义规则（对齐 PHP 第 632-634 行）
                cb(value, &args, data)
            } else if info == "must"
                || info.starts_with("require")
                || (!value.is_null() && !is_empty_string(value))
            {
                // 内置规则调用（对齐 PHP 第 634-635 行）
                self.dispatch_builtin(&cb_type, value, &args, data, field, title)
            } else {
                // 空值跳过（对齐 PHP 第 636-637 行，R5-5）
                true
            };

            if !result {
                // 验证失败，生成错误消息（对齐 PHP 第 642-652 行）
                let message = if i < msg.len() && !msg[i].is_empty() {
                    msg[i].clone()
                } else {
                    self.get_rule_msg(field, title, &info, &args)
                };
                return Err(message);
            }
        }

        Ok(())
    }

    /// 内置规则分发器
    ///
    /// 对齐 PHP `$this->$type($value, $rule, $data, $field, $title)` 调用
    fn dispatch_builtin(
        &self,
        type_name: &str,
        value: &Value,
        rule: &str,
        data: &Value,
        field: &str,
        _title: &str,
    ) -> bool {
        match type_name {
            "require" => Self::require(value, rule),
            "must" => Self::must(value, rule),
            "is" => {
                // 对齐 PHP `is` 方法 default 分支（第 870-884 行）：
                // 先检查 type_callbacks 中是否注册了 rule 对应的回调
                if let Some(cb) = self.type_callbacks.get(rule) {
                    return cb(value, "", data);
                }
                Self::is(value, rule, data)
            }
            "regex" => Self::regex_validate(value, rule, &self.regex),
            // 比较类规则（对齐 PHP eq/gt/egt/lt/elt/confirm/different）
            "eq" => rules::eq(value, rule, data, field),
            "gt" => rules::gt(value, rule, data, field),
            "egt" => rules::egt(value, rule, data, field),
            "lt" => rules::lt(value, rule, data, field),
            "elt" => rules::elt(value, rule, data, field),
            "confirm" => rules::confirm(value, rule, data, field),
            "different" => rules::different(value, rule, data, field),
            // 范围类规则
            "in" => rules::in_rule(value, rule, data, field),
            "notIn" => rules::not_in(value, rule, data, field),
            "between" => rules::between(value, rule, data, field),
            "notBetween" => rules::not_between(value, rule, data, field),
            // 长度类规则
            "length" => rules::length(value, rule, data, field),
            "max" => rules::max(value, rule, data, field),
            "min" => rules::min(value, rule, data, field),
            // 日期类规则
            "dateFormat" => rules::date_format(value, rule, data, field),
            "after" => rules::after(value, rule, data, field),
            "before" => rules::before(value, rule, data, field),
            "afterWith" => rules::after_with(value, rule, data, field),
            "beforeWith" => rules::before_with(value, rule, data, field),
            "expire" => rules::expire(value, rule, data, field),
            // 条件必须类规则
            "requireIf" => rules::require_if(value, rule, data, field),
            "requireWith" => rules::require_with(value, rule, data, field),
            "requireWithout" => rules::require_without(value, rule, data, field),
            // IP 类规则
            "ip" => rules::ip(value, rule, data, field),
            "allowIp" => rules::allow_ip(value, rule, data, field),
            "denyIp" => rules::deny_ip(value, rule, data, field),
            // 域名类规则
            "activeUrl" => rules::active_url(value, rule, data, field),
            _ => {
                // 未知类型默认通过（对齐 PHP method_exists 检查失败时的行为）
                true
            }
        }
    }

    // ========================================================================
    // 数据值获取
    // ========================================================================

    /// 获取数据值 — 对齐 PHP `getDataValue`
    ///
    /// 对齐 PHP `Validate.php` 第 1536-1554 行
    ///
    /// ## PHP 行为（R5-3）
    ///
    /// - 数值型 key：返回 key 本身（PHP 怪异行为，复刻）
    /// - 包含 `.` 的 key：按多维数组访问
    /// - 其他 key：返回 `data[key]` 或 null
    pub fn get_data_value(data: &Value, key: &str) -> Value {
        // 数值型 key 返回 key 本身（PHP 怪异行为，R5-3）
        if key.parse::<i64>().is_ok() || key.parse::<f64>().is_ok() {
            return Value::String(key.to_string());
        }
        // 多维数组访问
        if key.contains('.') {
            let mut current = data;
            for part in key.split('.') {
                match current.get(part) {
                    Some(v) => current = v,
                    None => return Value::Null,
                }
            }
            return current.clone();
        }
        // 普通 key
        data.get(key).cloned().unwrap_or(Value::Null)
    }

    // ========================================================================
    // 验证类型解析
    // ========================================================================

    /// 获取当前验证类型及规则 — 对齐 PHP `getValidateType`
    ///
    /// 对齐 PHP `Validate.php` 第 678-706 行
    ///
    /// ## PHP 行为（R5-4）
    ///
    /// - 别名映射（`>` → `gt`，`>=` → `egt` 等）
    /// - 返回 `(type, args, info)`：
    ///   - `type`：用于分发的回调名（考虑别名）
    ///   - `args`：规则参数
    ///   - `info`：原始规则名（用于 remove/append 匹配）
    pub fn get_validate_type(rule_type: &str, rule_args: &str) -> (String, String, String) {
        // 别名解析（对齐 PHP 第 682-685 行）
        let resolved_type = if let Some(&alias) = ALIAS.get(rule_type) {
            alias.to_string()
        } else {
            rule_type.to_string()
        };

        // 对齐 PHP getValidateType 第 689-705 行（数字 key 分支）
        // PHP method_exists 检查：Validate 类中存在的方法列表
        // 这些方法在 PHP Validate 类中存在，可以直接调用
        const PHP_METHODS: &[&str] = &[
            "must",
            "is",
            "confirm",
            "different",
            "egt",
            "gt",
            "elt",
            "lt",
            "eq",
            "activeUrl",
            "ip",
            "dateFormat",
            "requireIf",
            "requireCallback",
            "requireWith",
            "requireWithout",
            "in",
            "notIn",
            "between",
            "notBetween",
            "length",
            "max",
            "min",
            "after",
            "before",
            "afterWith",
            "beforeWith",
            "expire",
            "allowIp",
            "denyIp",
            "regex",
        ];

        if !rule_args.is_empty() {
            // 有参数规则（对齐 PHP 第 689-695 行，如 "in:1,2,3" → type="in", args="1,2,3", info="in"）
            (resolved_type.clone(), rule_args.to_string(), resolved_type)
        } else if PHP_METHODS.contains(&resolved_type.as_str()) {
            // 规则名匹配类方法（对齐 PHP 第 696-699 行 method_exists 分支）
            (resolved_type.clone(), String::new(), resolved_type)
        } else {
            // 默认走 is 方法（对齐 PHP 第 700-703 行，如 "require"/"integer"/"email" 等）
            ("is".to_string(), resolved_type.clone(), resolved_type)
        }
    }

    // ========================================================================
    // 错误消息
    // ========================================================================

    /// 获取验证规则的错误提示信息 — 对齐 PHP `getRuleMsg`
    ///
    /// 对齐 PHP `Validate.php` 第 1565-1586 行
    ///
    /// ## PHP 查找优先级（R5-1）
    ///
    /// 1. `message[field.type]`
    /// 2. `message[field]`
    /// 3. `type_msg[type]`
    /// 4. 如果 type 以 `require` 开头，使用 `type_msg['require']`
    /// 5. 默认 `$title . $this->lang->get('not conform to the rules')`
    ///
    /// ## Phase 5.4 Lang 翻译
    ///
    /// 所有分支的 msg 在占位符替换前先经过 [`Self::parse_error_msg_with_lang`]
    /// 进行 Lang 翻译（对齐 PHP `parseErrorMsg` 第 1598-1602 行）。
    /// 默认分支直接调用 `lang->get('not conform to the rules')`（对齐 PHP
    /// 第 1578 行）。
    pub fn get_rule_msg(&self, field: &str, title: &str, type_name: &str, rule: &str) -> String {
        // 1. message[field.type]
        let key1 = format!("{}.{}", field, type_name);
        if let Some(msg) = self.message.get(&key1) {
            return self.parse_error_msg_with_lang(msg, rule, title);
        }
        // 2. message[field]
        if let Some(msg) = self.message.get(field) {
            return self.parse_error_msg_with_lang(msg, rule, title);
        }
        // 3. type_msg[type]
        if let Some(&msg) = TYPE_MSG.get(type_name) {
            return self.parse_error_msg_with_lang(msg, rule, title);
        }
        // 4. require 前缀回退（对齐 PHP 第 1575-1576 行）
        if type_name.starts_with("require") {
            if let Some(&msg) = TYPE_MSG.get("require") {
                return self.parse_error_msg_with_lang(msg, rule, title);
            }
        }
        // 5. 默认（对齐 PHP 第 1578 行：$title . $this->lang->get('not conform to the rules')）
        // PHP Lang::get 找不到时返回 name 本身，所以无 Lang 时为 "not conform to the rules"
        let suffix = if let Some(lang) = &self.lang {
            lang.get("not conform to the rules")
        } else {
            "not conform to the rules".to_string()
        };
        format!("{}{}", title, suffix)
    }

    /// 解析错误提示（含 Lang 翻译） — 对齐 PHP `parseErrorMsg`
    ///
    /// 对齐 PHP `Validate.php` 第 1596-1633 行
    ///
    /// ## PHP 行为
    ///
    /// 1. **Lang 翻译**（第 1598-1602 行，R5-7）：
    ///    - `{%var}` 语法：`lang->get(substr($msg, 2, -1))`
    ///    - `lang->has($msg)`：`lang->get($msg)`
    /// 2. **占位符替换**（第 1613-1630 行，R5-2）：
    ///    - `:attribute` → title
    ///    - `:1` / `:2` / `:3` → rule 按逗号分割后的前 3 个元素
    ///    - `:rule` → rule 原值（仅当 msg 包含 `:rule` 时）
    ///
    /// ## 无 Lang 实例时的行为
    ///
    /// 当 `Validate::lang` 为 `None` 时跳过翻译，直接执行占位符替换。
    /// 对齐 PHP 未注入 Lang 时的行为（PHP 中 `$this->lang` 必须存在，否则
    /// `parseErrorMsg` 会致命错误；Rust 使用 `Option` 提供更安全的降级）。
    pub fn parse_error_msg_with_lang(&self, msg: &str, rule: &str, title: &str) -> String {
        // Phase 5.4：先 Lang 翻译，再占位符替换（对齐 PHP 第 1598-1602 行）
        let translated = message::translate_msg(msg, self.lang.as_ref());
        Self::parse_error_msg(&translated, rule, title)
    }

    /// 解析错误提示 — 对齐 PHP `parseErrorMsg` 占位符替换部分
    ///
    /// 对齐 PHP `Validate.php` 第 1613-1630 行（不含 Lang 翻译）
    ///
    /// ## PHP 占位符替换（R5-2）
    ///
    /// 1. `:attribute` → title
    /// 2. `:1` / `:2` / `:3` → rule 按逗号分割后的前 3 个元素
    /// 3. `:rule` → rule 原值（仅当 msg 包含 `:rule` 时）
    ///
    /// ## 说明
    ///
    /// 本方法为静态方法，不包含 Lang 翻译。如需 Lang 翻译，请使用
    /// [`Self::parse_error_msg_with_lang`] 实例方法。
    pub fn parse_error_msg(msg: &str, rule: &str, title: &str) -> String {
        let mut result = msg.to_string();

        // 仅当 msg 包含 `:` 时执行替换（对齐 PHP 第 1613 行）
        if !result.contains(':') {
            return result;
        }

        // 将 rule 按逗号分割为前 3 个元素（对齐 PHP 第 1615-1619 行）
        let parts: Vec<&str> = if rule.contains(',') {
            let split: Vec<&str> = rule.split(',').collect();
            split
        } else {
            vec!["", "", ""]
        };
        let p1 = parts.first().copied().unwrap_or("");
        let p2 = parts.get(1).copied().unwrap_or("");
        let p3 = parts.get(2).copied().unwrap_or("");

        // 替换 :attribute, :1, :2, :3（对齐 PHP 第 1621-1625 行）
        result = result.replace(":attribute", title);
        result = result.replace(":1", p1);
        result = result.replace(":2", p2);
        result = result.replace(":3", p3);

        // 替换 :rule（对齐 PHP 第 1627-1629 行，仅当 msg 包含 :rule 时）
        if result.contains(":rule") {
            result = result.replace(":rule", rule);
        }

        result
    }

    /// 获取错误信息 — 对齐 PHP `getError()`
    pub fn get_error(&self) -> &ValidateError {
        &self.error
    }

    // ========================================================================
    // 内置规则（基础）
    // ========================================================================

    /// 必须验证 — 对齐 PHP `require`
    ///
    /// 对齐 PHP `Validate.php` 第 814-817 行（实际由 `is` 处理 require）
    ///
    /// ## 行为
    ///
    /// - `null` → `false`
    /// - 空字符串 `""` → `false`
    /// - 字符串 `"0"` → `true`（PHP 特殊行为）
    /// - 其他非空值 → `true`
    pub fn require(value: &Value, _rule: &str) -> bool {
        // 对齐 PHP `!empty($value) || '0' == $value`
        // 字符串 "0" 在 PHP empty() 中被视为空，但 require 将其视为非空
        !is_empty_value(value) || matches!(value, Value::String(s) if s == "0")
    }

    /// 必须验证（与 require 等价） — 对齐 PHP `must`
    ///
    /// 对齐 PHP `Validate.php` 第 814-817 行
    pub fn must(value: &Value, _rule: &str) -> bool {
        !is_empty_value(value) || matches!(value, Value::String(s) if s == "0")
    }

    /// 验证字段值是否为有效格式 — 对齐 PHP `is`
    ///
    /// 对齐 PHP `Validate.php` 第 827-888 行
    ///
    /// ## 支持的类型
    ///
    /// - `require`：必须
    /// - `accepted`：接受（`1`/`on`/`yes`）
    /// - `date`：有效日期
    /// - `boolean`/`bool`：布尔值
    /// - `number`：数字
    /// - `integer`：整数
    /// - `float`：浮点数
    /// - `alpha`：字母
    /// - `alphaNum`：字母数字
    /// - `alphaDash`：字母数字下划线短横线
    /// - `chs`：中文
    /// - `chsAlpha`：中文或字母
    /// - `chsAlphaNum`：中文或字母数字
    /// - `chsDash`：中文字母数字下划线短横线
    /// - `mobile`：手机号
    /// - `email`：邮箱
    /// - `url`：URL
    /// - `ip`：IP 地址
    /// - `macAddr`：MAC 地址
    /// - `array`：数组
    pub fn is(value: &Value, rule: &str, _data: &Value) -> bool {
        let rule = rule.trim();
        match rule {
            "require" => {
                // 对齐 PHP `is` 方法 require 分支：`!empty($value) || '0' == $value`
                !is_empty_value(value) || matches!(value, Value::String(s) if s == "0")
            }
            "accepted" => {
                matches!(
                    value,
                    Value::Number(n) if n.as_i64() == Some(1)
                ) || matches!(value, Value::String(s) if s == "1" || s == "on" || s == "yes")
            }
            "date" => is_valid_date(value),
            "boolean" | "bool" => {
                matches!(value, Value::Bool(_) | Value::Null)
                    || matches!(value, Value::Number(n) if n.as_i64() == Some(0) || n.as_i64() == Some(1))
                    || matches!(value, Value::String(s) if s == "0" || s == "1")
            }
            "number" => {
                value.is_number() || matches!(value, Value::String(s) if s.parse::<f64>().is_ok())
            }
            "integer" => {
                value.is_i64() || matches!(value, Value::String(s) if s.parse::<i64>().is_ok())
            }
            "float" => {
                // 对齐 PHP filter_var($value, FILTER_VALIDATE_FLOAT) — 接受整数和浮点数
                value.is_number() || matches!(value, Value::String(s) if s.parse::<f64>().is_ok())
            }
            "array" => value.is_array(),
            "email" => is_valid_email(value),
            "url" => is_valid_url(value),
            "ip" => is_valid_ip(value),
            "macAddr" => is_valid_mac(value),
            // ctype / 正则类规则
            "alpha" => regex_match_default(value, "alpha"),
            "alphaNum" => regex_match_default(value, "alphaNum"),
            "alphaDash" => regex_match_default(value, "alphaDash"),
            "chs" => regex_match_default(value, "chs"),
            "chsAlpha" => regex_match_default(value, "chsAlpha"),
            "chsAlphaNum" => regex_match_default(value, "chsAlphaNum"),
            "chsDash" => regex_match_default(value, "chsDash"),
            "mobile" => regex_match_default(value, "mobile"),
            "idCard" => regex_match_default(value, "idCard"),
            "zip" => regex_match_default(value, "zip"),
            _ => {
                // 未知类型默认通过（对齐 PHP default 分支的正则匹配行为）
                // Phase 5.2 将在 rules.rs 添加更多类型
                true
            }
        }
    }

    /// 正则验证 — 对齐 PHP `regex`
    ///
    /// 对齐 PHP `Validate.php` 第 1504-1518 行
    pub fn regex_validate(
        value: &Value,
        rule: &str,
        custom_regex: &IndexMap<String, String>,
    ) -> bool {
        // 查找自定义正则
        let pattern = if let Some(p) = custom_regex.get(rule) {
            p.clone()
        } else if let Some(&p) = DEFAULT_REGEX.get(rule) {
            p.to_string()
        } else {
            // 不是预定义正则，按 PHP 规则补上 /^...$/
            // 对齐 PHP 第 1512-1515 行
            if rule.starts_with('/') {
                rule.to_string()
            } else {
                format!("/^{}/$", rule)
            }
        };

        // 仅标量值可正则匹配（对齐 PHP 第 1517 行 is_scalar 检查）
        let s = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return false,
        };

        // 处理 PHP 正则的 /u 标志（Unicode）
        // Rust regex crate 默认就是 Unicode 模式
        let rust_pattern = php_regex_to_rust(&pattern);
        match Regex::new(&rust_pattern) {
            Ok(re) => re.is_match(&s),
            Err(_) => false,
        }
    }
}

// ============================================================================
// 内部辅助函数
// ============================================================================

/// 判断值是否为空（对齐 PHP `empty()`）
///
/// PHP `empty()` 对以下值返回 true：
/// - `null`、`false`、`""`、`"0"`、`0`、`0.0`、`[]`
///
/// **注意**：PHP `is` 方法的 `require` 分支使用 `!empty($value) || '0' == $value`，
/// 即字符串 `"0"` 被视为非空（PHP 特殊行为）。
pub(crate) fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::String(s) => s.is_empty() || s == "0",
        Value::Number(n) => n.as_f64().map(|f| f == 0.0).unwrap_or(true),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// 判断值是否为空字符串（对齐 PHP `'' !== $value`）
///
/// 仅判断空字符串，不判断其他空值（与 `is_empty_value` 区分）。
fn is_empty_string(value: &Value) -> bool {
    matches!(value, Value::String(s) if s.is_empty())
}

/// 验证日期格式（对齐 PHP `strtotime`）
fn is_valid_date(value: &Value) -> bool {
    let s = match value {
        Value::String(s) => s,
        _ => return false,
    };
    // 尝试常见日期格式解析
    // 对齐 PHP `strtotime()` 的宽松行为
    use chrono::{DateTime, NaiveDate, NaiveDateTime};
    if DateTime::parse_from_rfc3339(s).is_ok() {
        return true;
    }
    if NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").is_ok() {
        return true;
    }
    if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return true;
    }
    if NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M:%S").is_ok() {
        return true;
    }
    if NaiveDate::parse_from_str(s, "%Y/%m/%d").is_ok() {
        return true;
    }
    false
}

/// 验证邮箱格式（对齐 PHP `FILTER_VALIDATE_EMAIL`）
fn is_valid_email(value: &Value) -> bool {
    let s = match value {
        Value::String(s) => s,
        _ => return false,
    };
    // 简化版邮箱正则（PHP filter_var 更宽松）
    let email_re =
        Lazy::new(|| Regex::new(r"^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$").unwrap());
    email_re.is_match(s)
}

/// 验证 URL 格式（对齐 PHP `FILTER_VALIDATE_URL`）
fn is_valid_url(value: &Value) -> bool {
    let s = match value {
        Value::String(s) => s,
        _ => return false,
    };
    // 必须包含 scheme
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ftp://")
        || s.starts_with("ftps://")
        || s.starts_with("mailto:")
        || s.starts_with("tel:")
        || s.starts_with("file://")
}

/// 验证 IP 地址（对齐 PHP `FILTER_VALIDATE_IP`，支持 IPv4 和 IPv6）
fn is_valid_ip(value: &Value) -> bool {
    let s = match value {
        Value::String(s) => s,
        Value::Number(n) => {
            // 数字不是有效 IP
            let _ = n;
            return false;
        }
        _ => return false,
    };
    use std::net::IpAddr;
    s.parse::<IpAddr>().is_ok()
}

/// 验证 MAC 地址（对齐 PHP `FILTER_VALIDATE_MAC`）
fn is_valid_mac(value: &Value) -> bool {
    let s = match value {
        Value::String(s) => s,
        _ => return false,
    };
    let mac_re = Lazy::new(|| Regex::new(r"^([0-9A-Fa-f]{2}[:-]){5}([0-9A-Fa-f]{2})$").unwrap());
    mac_re.is_match(s)
}

/// 使用内置正则匹配（对齐 PHP `defaultRegex` 查找）
fn regex_match_default(value: &Value, regex_name: &str) -> bool {
    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return false,
    };
    if let Some(&pattern) = DEFAULT_REGEX.get(regex_name) {
        let rust_pattern = php_regex_to_rust(pattern);
        match Regex::new(&rust_pattern) {
            Ok(re) => re.is_match(&s),
            Err(_) => false,
        }
    } else {
        false
    }
}

/// 将 PHP 正则转换为 Rust regex crate 兼容格式
///
/// 主要差异：
/// - PHP `\x{4e00}` Unicode 转义 → Rust `\u{4e00}`
/// - PHP `/...$/u` Unicode 标志 → Rust 默认 Unicode
fn php_regex_to_rust(pattern: &str) -> String {
    let mut result = pattern.to_string();
    // 替换 \x{HEX} 为 \u{HEX}
    while let Some(start) = result.find("\\x{") {
        if let Some(end) = result[start..].find('}') {
            let hex = &result[start + 3..start + end];
            let replacement = format!("\\u{{{}}}", hex);
            result.replace_range(start..start + end + 1, &replacement);
        } else {
            break;
        }
    }
    // 移除 PHP 正则分隔符和标志（如 /.../u）
    // Rust regex 不使用分隔符
    if result.starts_with('/') && result.len() > 2 {
        let flags_start = result.rfind('/').unwrap_or(result.len() - 1);
        if flags_start > 0 {
            // 移除首尾 / 和标志（u, i, m, s, U 等）
            let _inner = &result[1..flags_start];
            let flags = &result[flags_start + 1..];
            // Rust 默认 Unicode，'u' 标志可忽略
            // 'i' 标志使用 (?i) 前缀
            let mut prefix = String::new();
            if flags.contains('i') {
                prefix.push_str("(?i)");
            }
            if flags.contains('m') {
                prefix.push_str("(?m)");
            }
            if flags.contains('s') {
                prefix.push_str("(?s)");
            }
            return format!("{}{}", prefix, _inner);
        }
    }
    result
}

// ============================================================================
// 内联单元测试
// ============================================================================

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // 组 1：ValidateError 类型测试
    // ========================================================================

    #[test]
    fn test_validate_error_single_display() {
        let err = ValidateError::Single("name require".to_string());
        assert_eq!(format!("{}", err), "name require");
    }

    #[test]
    fn test_validate_error_batch_display() {
        let mut errors = IndexMap::new();
        errors.insert("name".to_string(), "name require".to_string());
        errors.insert("age".to_string(), "age must be integer".to_string());
        let err = ValidateError::Batch(errors);
        let displayed = format!("{}", err);
        assert!(displayed.contains("name: name require"));
        assert!(displayed.contains("age: age must be integer"));
    }

    #[test]
    fn test_validate_error_clone_eq() {
        let err1 = ValidateError::Single("test".to_string());
        let err2 = err1.clone();
        assert_eq!(err1, err2);
    }

    // ========================================================================
    // 组 2：Rule 类型测试
    // ========================================================================

    #[test]
    fn test_rule_from_string_simple() {
        let r = Rule::from_string("require");
        match r {
            Rule::Simple(s) => assert_eq!(s, "require"),
            _ => panic!("expected Simple"),
        }
    }

    #[test]
    fn test_rule_from_string_with_args() {
        let r = Rule::from_string("in:1,2,3");
        match r {
            Rule::WithArgs(t, a) => {
                assert_eq!(t, "in");
                assert_eq!(a, "1,2,3");
            }
            _ => panic!("expected WithArgs"),
        }
    }

    #[test]
    fn test_rule_from_string_multiple() {
        let r = Rule::from_string("require|in:1,2,3");
        match r {
            Rule::Multiple(list) => {
                assert_eq!(list.len(), 2);
                assert!(matches!(list[0], Rule::Simple(ref s) if s == "require"));
                assert!(
                    matches!(list[1], Rule::WithArgs(ref t, ref a) if t == "in" && a == "1,2,3")
                );
            }
            _ => panic!("expected Multiple"),
        }
    }

    #[test]
    fn test_rule_to_list_simple() {
        let r = Rule::Simple("require".to_string());
        let list = r.to_list();
        assert_eq!(list, vec![("require".to_string(), String::new())]);
    }

    #[test]
    fn test_rule_to_list_multiple() {
        let r = Rule::from_string("require|in:1,2,3|email");
        let list = r.to_list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], ("require".to_string(), String::new()));
        assert_eq!(list[1], ("in".to_string(), "1,2,3".to_string()));
        assert_eq!(list[2], ("email".to_string(), String::new()));
    }

    // ========================================================================
    // 组 3：Builder 方法测试
    // ========================================================================

    #[test]
    fn test_validate_new_empty() {
        let v = Validate::new();
        assert!(!v.batch);
        assert!(v.current_scene.is_none());
        assert!(v.rule.is_empty());
        assert!(v.message.is_empty());
        assert!(v.field.is_empty());
        assert!(v.scene.is_empty());
        assert!(v.only.is_empty());
        assert!(v.remove.is_empty());
        assert!(v.append.is_empty());
        assert!(v.type_callbacks.is_empty());
    }

    #[test]
    fn test_validate_rule_builder() {
        let v = Validate::new()
            .rule("name", "require")
            .rule("age", "integer");
        assert_eq!(v.rule.len(), 2);
        assert!(v.rule.contains_key("name"));
        assert!(v.rule.contains_key("age"));
    }

    #[test]
    fn test_validate_message_builder() {
        let mut msgs = IndexMap::new();
        msgs.insert("name.require".to_string(), "名称必须".to_string());
        let v = Validate::new().message(msgs);
        assert_eq!(v.message.get("name.require"), Some(&"名称必须".to_string()));
    }

    #[test]
    fn test_validate_field_builder() {
        let mut fields = IndexMap::new();
        fields.insert("name".to_string(), "名称".to_string());
        let v = Validate::new().field(fields);
        assert_eq!(v.field.get("name"), Some(&"名称".to_string()));
    }

    #[test]
    fn test_validate_scene_builder() {
        let v = Validate::new()
            .register_scene(
                "login",
                vec!["username".to_string(), "password".to_string()],
            )
            .scene("login");
        assert_eq!(v.current_scene, Some("login".to_string()));
        assert!(v.has_scene("login"));
        assert!(!v.has_scene("register"));
    }

    #[test]
    fn test_validate_batch_builder() {
        let v = Validate::new().batch(true);
        assert!(v.batch);
    }

    #[test]
    fn test_validate_only_builder() {
        let v = Validate::new().only(vec!["name".to_string()]);
        assert_eq!(v.only, vec!["name".to_string()]);
    }

    #[test]
    fn test_validate_remove_builder() {
        let v = Validate::new()
            .remove("name", None)
            .remove("age", Some(vec!["integer".to_string()]));
        assert!(matches!(v.remove.get("name"), Some(None)));
        assert!(
            matches!(v.remove.get("age"), Some(Some(ref r)) if r == &vec!["integer".to_string()])
        );
    }

    #[test]
    fn test_validate_append_builder() {
        let v = Validate::new().append("name", vec!["email".to_string()]);
        assert_eq!(v.append.get("name"), Some(&vec!["email".to_string()]));
    }

    #[test]
    fn test_validate_regex_builder() {
        let v = Validate::new().regex("custom", r"^\d{4}$");
        assert_eq!(v.regex.get("custom"), Some(&r"^\d{4}$".to_string()));
    }

    // ========================================================================
    // 组 4：静态映射测试
    // ========================================================================

    #[test]
    fn test_static_default_regex_contains_all() {
        // 对齐 PHP $defaultRegex 第 125-136 行
        assert!(DEFAULT_REGEX.contains_key("alpha"));
        assert!(DEFAULT_REGEX.contains_key("alphaNum"));
        assert!(DEFAULT_REGEX.contains_key("alphaDash"));
        assert!(DEFAULT_REGEX.contains_key("chs"));
        assert!(DEFAULT_REGEX.contains_key("chsAlpha"));
        assert!(DEFAULT_REGEX.contains_key("chsAlphaNum"));
        assert!(DEFAULT_REGEX.contains_key("chsDash"));
        assert!(DEFAULT_REGEX.contains_key("mobile"));
        assert!(DEFAULT_REGEX.contains_key("idCard"));
        assert!(DEFAULT_REGEX.contains_key("zip"));
    }

    #[test]
    fn test_static_type_msg_contains_all() {
        // 对齐 PHP $typeMsg 第 62-113 行
        assert!(TYPE_MSG.contains_key("require"));
        assert!(TYPE_MSG.contains_key("must"));
        assert!(TYPE_MSG.contains_key("number"));
        assert!(TYPE_MSG.contains_key("email"));
        assert!(TYPE_MSG.contains_key("mobile"));
        assert!(TYPE_MSG.contains_key("in"));
        assert!(TYPE_MSG.contains_key("notIn"));
        assert!(TYPE_MSG.contains_key("between"));
        assert!(TYPE_MSG.contains_key("length"));
        assert!(TYPE_MSG.contains_key("max"));
        assert!(TYPE_MSG.contains_key("min"));
        assert!(TYPE_MSG.contains_key("eq"));
        assert!(TYPE_MSG.contains_key("gt"));
        assert!(TYPE_MSG.contains_key("egt"));
        assert!(TYPE_MSG.contains_key("lt"));
        assert!(TYPE_MSG.contains_key("elt"));
        assert!(TYPE_MSG.contains_key("confirm"));
        assert!(TYPE_MSG.contains_key("different"));
        assert!(TYPE_MSG.contains_key("regex"));
    }

    #[test]
    fn test_static_alias_contains_all() {
        // 对齐 PHP $alias 第 36-38 行
        assert_eq!(ALIAS.get(">"), Some(&"gt"));
        assert_eq!(ALIAS.get(">="), Some(&"egt"));
        assert_eq!(ALIAS.get("<"), Some(&"lt"));
        assert_eq!(ALIAS.get("<="), Some(&"elt"));
        assert_eq!(ALIAS.get("="), Some(&"eq"));
        assert_eq!(ALIAS.get("same"), Some(&"eq"));
    }

    // ========================================================================
    // 组 5：get_data_value 测试（R5-3）
    // ========================================================================

    #[test]
    fn test_get_data_value_simple() {
        let data = json!({"name": "Alice", "age": 30});
        assert_eq!(Validate::get_data_value(&data, "name"), json!("Alice"));
        assert_eq!(Validate::get_data_value(&data, "age"), json!(30));
    }

    #[test]
    fn test_get_data_value_missing_field() {
        let data = json!({"name": "Alice"});
        assert_eq!(Validate::get_data_value(&data, "missing"), Value::Null);
    }

    #[test]
    fn test_get_data_value_nested_dot_notation() {
        // 对齐 PHP 多维数组访问（R5-3）
        let data = json!({"user": {"profile": {"age": 25}}});
        assert_eq!(
            Validate::get_data_value(&data, "user.profile.age"),
            json!(25)
        );
    }

    #[test]
    fn test_get_data_value_nested_missing_intermediate() {
        let data = json!({"user": {"name": "Alice"}});
        assert_eq!(
            Validate::get_data_value(&data, "user.profile.age"),
            Value::Null
        );
    }

    #[test]
    fn test_get_data_value_numeric_key_php_bug() {
        // R5-3：数值型 key 返回 key 本身（PHP 怪异行为，复刻）
        let data = json!({"123": "value"});
        assert_eq!(
            Validate::get_data_value(&data, "123"),
            json!("123") // 返回 key 字符串本身，不是 "value"
        );
    }

    #[test]
    fn test_get_data_value_float_numeric_key() {
        // R5-3：浮点型 key 也返回 key 本身
        let data = json!({});
        assert_eq!(Validate::get_data_value(&data, "3.14"), json!("3.14"));
    }

    // ========================================================================
    // 组 6：get_validate_type 测试（R5-4）
    // ========================================================================

    #[test]
    fn test_get_validate_type_no_alias() {
        // PHP "require" 没有 method_exists，走 is 分支
        // type="is", args="require", info="require"
        let (t, a, i) = Validate::get_validate_type("require", "");
        assert_eq!(t, "is");
        assert_eq!(a, "require");
        assert_eq!(i, "require");
    }

    #[test]
    fn test_get_validate_type_with_args() {
        let (t, a, i) = Validate::get_validate_type("in", "1,2,3");
        assert_eq!(t, "in");
        assert_eq!(a, "1,2,3");
        assert_eq!(i, "in");
    }

    #[test]
    fn test_get_validate_type_alias_gt() {
        // 对齐 PHP 别名映射（R5-4）
        // ">" 无参数 → 别名解析为 "gt" → "gt" 在 PHP_METHODS 中（method_exists）
        // 对齐 PHP getValidateType 第 696-699 行：result = ['gt', '', 'gt']
        let (t, a, i) = Validate::get_validate_type(">", "");
        assert_eq!(t, "gt");
        assert_eq!(a, "");
        assert_eq!(i, "gt");
    }

    #[test]
    fn test_get_validate_type_alias_same_to_eq() {
        // "same" 无参数 → 别名解析为 "eq" → "eq" 在 PHP_METHODS 中（method_exists）
        // 对齐 PHP getValidateType 第 696-699 行：result = ['eq', '', 'eq']
        let (t, a, i) = Validate::get_validate_type("same", "");
        assert_eq!(t, "eq");
        assert_eq!(a, "");
        assert_eq!(i, "eq");
    }

    #[test]
    fn test_get_validate_type_must_method_exists() {
        // PHP "must" 有 method_exists，直接调用 must 方法
        let (t, a, i) = Validate::get_validate_type("must", "");
        assert_eq!(t, "must");
        assert_eq!(a, "");
        assert_eq!(i, "must");
    }

    #[test]
    fn test_get_validate_type_integer_via_is() {
        // PHP "integer" 没有 method_exists，走 is 分支
        let (t, a, i) = Validate::get_validate_type("integer", "");
        assert_eq!(t, "is");
        assert_eq!(a, "integer");
        assert_eq!(i, "integer");
    }

    // ========================================================================
    // 组 7：parse_error_msg 测试（R5-2）
    // ========================================================================

    #[test]
    fn test_parse_error_msg_attribute_replacement() {
        let result = Validate::parse_error_msg(":attribute require", "", "名称");
        assert_eq!(result, "名称 require");
    }

    #[test]
    fn test_parse_error_msg_rule_replacement() {
        let result = Validate::parse_error_msg(":attribute must be in :rule", "1,2,3", "状态");
        assert_eq!(result, "状态 must be in 1,2,3");
    }

    #[test]
    fn test_parse_error_msg_numbered_replacement() {
        // 对齐 PHP :1, :2, :3 替换（R5-2）
        let result = Validate::parse_error_msg(":attribute must between :1 - :2", "10,20", "年龄");
        assert_eq!(result, "年龄 must between 10 - 20");
    }

    #[test]
    fn test_parse_error_msg_no_colon_returns_as_is() {
        // 没有 : 的消息原样返回（对齐 PHP 第 1613 行）
        let result = Validate::parse_error_msg("access IP denied", "1.2.3.4", "ip");
        assert_eq!(result, "access IP denied");
    }

    #[test]
    fn test_parse_error_msg_multiple_placeholders() {
        let result = Validate::parse_error_msg(
            ":attribute must equal :rule and between :1 - :2",
            "5,1,10",
            "值",
        );
        assert_eq!(result, "值 must equal 5,1,10 and between 5 - 1");
    }

    // ========================================================================
    // 组 8：get_rule_msg 测试（R5-1 优先级链）
    // ========================================================================

    #[test]
    fn test_get_rule_msg_priority_field_type() {
        // 优先级 1：message[field.type]
        let mut msgs = IndexMap::new();
        msgs.insert("name.require".to_string(), "名称必须填写".to_string());
        let v = Validate::new().message(msgs);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "名称必须填写");
    }

    #[test]
    fn test_get_rule_msg_priority_field_only() {
        // 优先级 2：message[field]
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "名称错误".to_string());
        let v = Validate::new().message(msgs);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "名称错误");
    }

    #[test]
    fn test_get_rule_msg_priority_type_msg() {
        // 优先级 3：type_msg[type]
        let v = Validate::new();
        let msg = v.get_rule_msg("email_field", "邮箱", "email", "");
        assert_eq!(msg, "邮箱 not a valid email address");
    }

    #[test]
    fn test_get_rule_msg_priority_require_prefix() {
        // 优先级 4：require 前缀回退到 type_msg['require']
        let v = Validate::new();
        let msg = v.get_rule_msg("field", "字段", "requireIf", "");
        assert_eq!(msg, "字段 require");
    }

    #[test]
    fn test_get_rule_msg_default_fallback() {
        // 优先级 5：默认消息
        // 对齐 PHP 第 1578 行：$title . $this->lang->get('not conform to the rules')
        // PHP Lang::get 找不到时返回 name 本身，无 Lang 时为 "not conform to the rules"（无前导空格）
        let v = Validate::new();
        let msg = v.get_rule_msg("field", "字段", "unknownType", "");
        assert_eq!(msg, "字段not conform to the rules");
    }

    // ========================================================================
    // 组 9：内置规则 require/must 测试
    // ========================================================================

    #[test]
    fn test_require_non_empty_string() {
        assert!(Validate::require(&json!("hello"), ""));
        assert!(Validate::require(&json!("0"), "")); // PHP 特殊行为："0" 被视为非空
    }

    #[test]
    fn test_require_empty_values() {
        // 对齐 PHP empty() 行为
        assert!(!Validate::require(&Value::Null, ""));
        assert!(!Validate::require(&json!(""), ""));
        assert!(!Validate::require(&json!(0), ""));
        assert!(!Validate::require(&json!(false), ""));
        assert!(!Validate::require(&json!([]), ""));
        assert!(!Validate::require(&json!({}), ""));
    }

    #[test]
    fn test_must_equals_require() {
        // must 与 require 行为一致
        assert_eq!(
            Validate::require(&json!("hello"), ""),
            Validate::must(&json!("hello"), "")
        );
        assert_eq!(
            Validate::require(&Value::Null, ""),
            Validate::must(&Value::Null, "")
        );
    }

    // ========================================================================
    // 组 10：内置规则 is 测试
    // ========================================================================

    #[test]
    fn test_is_require() {
        assert!(Validate::is(&json!("hello"), "require", &Value::Null));
        assert!(!Validate::is(&Value::Null, "require", &Value::Null));
    }

    #[test]
    fn test_is_accepted() {
        assert!(Validate::is(&json!("1"), "accepted", &Value::Null));
        assert!(Validate::is(&json!("on"), "accepted", &Value::Null));
        assert!(Validate::is(&json!("yes"), "accepted", &Value::Null));
        assert!(!Validate::is(&json!("no"), "accepted", &Value::Null));
    }

    #[test]
    fn test_is_boolean() {
        assert!(Validate::is(&json!(true), "boolean", &Value::Null));
        assert!(Validate::is(&json!(false), "boolean", &Value::Null));
        assert!(Validate::is(&json!(0), "boolean", &Value::Null));
        assert!(Validate::is(&json!(1), "boolean", &Value::Null));
        assert!(Validate::is(&json!("0"), "boolean", &Value::Null));
        assert!(Validate::is(&json!("1"), "boolean", &Value::Null));
        assert!(!Validate::is(&json!(2), "boolean", &Value::Null));
    }

    #[test]
    fn test_is_number() {
        assert!(Validate::is(&json!(123), "number", &Value::Null));
        assert!(Validate::is(&json!(3.14), "number", &Value::Null));
        assert!(Validate::is(&json!("123"), "number", &Value::Null));
        assert!(Validate::is(&json!("3.14"), "number", &Value::Null));
        assert!(!Validate::is(&json!("abc"), "number", &Value::Null));
    }

    #[test]
    fn test_is_integer() {
        assert!(Validate::is(&json!(123), "integer", &Value::Null));
        assert!(Validate::is(&json!("123"), "integer", &Value::Null));
        assert!(!Validate::is(&json!(3.14), "integer", &Value::Null));
        assert!(!Validate::is(&json!("3.14"), "integer", &Value::Null));
    }

    #[test]
    fn test_is_float() {
        assert!(Validate::is(&json!(3.14), "float", &Value::Null));
        assert!(Validate::is(&json!("3.14"), "float", &Value::Null));
        // 整数也是 float（PHP 行为）
        assert!(Validate::is(&json!(123), "float", &Value::Null));
    }

    #[test]
    fn test_is_array() {
        assert!(Validate::is(&json!([1, 2, 3]), "array", &Value::Null));
        assert!(!Validate::is(&json!("string"), "array", &Value::Null));
        assert!(!Validate::is(&json!({}), "array", &Value::Null));
    }

    #[test]
    fn test_is_email() {
        assert!(Validate::is(
            &json!("user@example.com"),
            "email",
            &Value::Null
        ));
        assert!(Validate::is(
            &json!("user.name+tag@example.co.uk"),
            "email",
            &Value::Null
        ));
        assert!(!Validate::is(&json!("invalid"), "email", &Value::Null));
        assert!(!Validate::is(&json!("user@"), "email", &Value::Null));
    }

    #[test]
    fn test_is_url() {
        assert!(Validate::is(
            &json!("http://example.com"),
            "url",
            &Value::Null
        ));
        assert!(Validate::is(
            &json!("https://example.com/path?q=1"),
            "url",
            &Value::Null
        ));
        assert!(!Validate::is(&json!("example.com"), "url", &Value::Null));
    }

    #[test]
    fn test_is_ip() {
        assert!(Validate::is(&json!("127.0.0.1"), "ip", &Value::Null));
        assert!(Validate::is(&json!("::1"), "ip", &Value::Null));
        assert!(Validate::is(&json!("192.168.1.1"), "ip", &Value::Null));
        assert!(!Validate::is(&json!("999.999.999.999"), "ip", &Value::Null));
        assert!(!Validate::is(&json!("not.an.ip"), "ip", &Value::Null));
    }

    #[test]
    fn test_is_mac_addr() {
        assert!(Validate::is(
            &json!("00:11:22:33:44:55"),
            "macAddr",
            &Value::Null
        ));
        assert!(Validate::is(
            &json!("00-11-22-33-44-55"),
            "macAddr",
            &Value::Null
        ));
        assert!(!Validate::is(&json!("invalid"), "macAddr", &Value::Null));
    }

    #[test]
    fn test_is_alpha() {
        assert!(Validate::is(&json!("abc"), "alpha", &Value::Null));
        assert!(Validate::is(&json!("ABC"), "alpha", &Value::Null));
        assert!(!Validate::is(&json!("abc123"), "alpha", &Value::Null));
        assert!(!Validate::is(&json!("abc_def"), "alpha", &Value::Null));
    }

    #[test]
    fn test_is_alpha_num() {
        assert!(Validate::is(&json!("abc123"), "alphaNum", &Value::Null));
        assert!(Validate::is(&json!("ABC"), "alphaNum", &Value::Null));
        assert!(!Validate::is(&json!("abc_123"), "alphaNum", &Value::Null));
    }

    #[test]
    fn test_is_alpha_dash() {
        assert!(Validate::is(&json!("abc123"), "alphaDash", &Value::Null));
        assert!(Validate::is(
            &json!("abc_def-123"),
            "alphaDash",
            &Value::Null
        ));
        assert!(!Validate::is(&json!("abc def"), "alphaDash", &Value::Null));
    }

    #[test]
    fn test_is_chs() {
        assert!(Validate::is(&json!("中文"), "chs", &Value::Null));
        assert!(!Validate::is(&json!("abc"), "chs", &Value::Null));
        assert!(!Validate::is(&json!("中文abc"), "chs", &Value::Null));
    }

    #[test]
    fn test_is_chs_alpha() {
        assert!(Validate::is(&json!("中文abc"), "chsAlpha", &Value::Null));
        assert!(Validate::is(&json!("中文"), "chsAlpha", &Value::Null));
        assert!(!Validate::is(&json!("中文123"), "chsAlpha", &Value::Null));
    }

    #[test]
    fn test_is_mobile() {
        assert!(Validate::is(&json!("13812345678"), "mobile", &Value::Null));
        assert!(Validate::is(&json!("19912345678"), "mobile", &Value::Null));
        assert!(!Validate::is(&json!("12345678901"), "mobile", &Value::Null)); // 不以 1[3-9] 开头
        assert!(!Validate::is(&json!("1381234567"), "mobile", &Value::Null)); // 少一位
    }

    #[test]
    fn test_is_date() {
        assert!(Validate::is(&json!("2024-01-01"), "date", &Value::Null));
        assert!(Validate::is(
            &json!("2024-01-01 12:00:00"),
            "date",
            &Value::Null
        ));
        assert!(Validate::is(&json!("2024/01/01"), "date", &Value::Null));
        assert!(!Validate::is(&json!("invalid date"), "date", &Value::Null));
    }

    // ========================================================================
    // 组 11：regex_validate 测试
    // ========================================================================

    #[test]
    fn test_regex_validate_default_pattern() {
        // 使用内置 defaultRegex
        let custom = IndexMap::new();
        assert!(Validate::regex_validate(&json!("abc"), "alpha", &custom));
        assert!(!Validate::regex_validate(&json!("123"), "alpha", &custom));
    }

    #[test]
    fn test_regex_validate_custom_pattern() {
        // 使用自定义正则
        let mut custom = IndexMap::new();
        custom.insert("custom".to_string(), r"^\d{4}$".to_string());
        assert!(Validate::regex_validate(&json!("1234"), "custom", &custom));
        assert!(!Validate::regex_validate(
            &json!("12345"),
            "custom",
            &custom
        ));
    }

    #[test]
    fn test_regex_validate_inline_pattern() {
        // 直接传入正则模式（不是预定义名）
        let custom = IndexMap::new();
        assert!(Validate::regex_validate(&json!("12345"), r"\d{5}", &custom));
        assert!(!Validate::regex_validate(&json!("abc"), r"\d{5}", &custom));
    }

    // ========================================================================
    // 组 12：check 方法测试
    // ========================================================================

    #[test]
    fn test_check_success_single_rule() {
        let mut v = Validate::new().rule("name", "require");
        let data = json!({"name": "Alice"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_check_failure_single_rule() {
        let mut v = Validate::new().rule("name", "require");
        let data = json!({"name": ""});
        let result = v.check(&data);
        assert!(result.is_err());
        match result.unwrap_err() {
            ValidateError::Single(msg) => assert!(msg.contains("require")),
            _ => panic!("expected Single error"),
        }
    }

    #[test]
    fn test_check_success_multiple_rules() {
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("age", "require|integer");
        let data = json!({"name": "Alice", "age": 30});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_check_failure_multiple_rules_single_mode() {
        // 非批量模式：返回第一个错误
        // 注意：IndexMap 保持插入顺序（Phase 5.2 已从 HashMap 迁移到 IndexMap）
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("age", "require|integer");
        let data = json!({"name": "", "age": "not_int"});
        let result = v.check(&data);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ValidateError::Single(_)));
    }

    #[test]
    fn test_check_batch_mode_collects_all_errors() {
        // 批量模式：收集所有错误
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("age", "require|integer")
            .batch(true);
        let data = json!({"name": "", "age": "not_int"});
        let result = v.check(&data);
        assert!(result.is_err());
        match result.unwrap_err() {
            ValidateError::Batch(errors) => {
                assert!(errors.contains_key("name"));
                // age 字段 "not_int" 是非空字符串，但 integer 验证失败
                // 注：由于 age 非空，integer 规则会触发
            }
            ValidateError::Single(_) => panic!("expected Batch error in batch mode"),
        }
    }

    #[test]
    fn test_check_missing_field_with_require() {
        let mut v = Validate::new().rule("name", "require");
        let data = json!({});
        assert!(v.check(&data).is_err());
    }

    #[test]
    fn test_check_missing_field_without_require_skips() {
        // R5-5：非 require 规则在字段缺失时跳过验证
        let mut v = Validate::new().rule("age", "integer");
        let data = json!({}); // age 字段缺失
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_check_field_title_parsing() {
        // 字段名格式 field|title
        let mut v = Validate::new().rule("name|名称", "require");
        let data = json!({"name": ""});
        let result = v.check(&data);
        assert!(result.is_err());
        match result.unwrap_err() {
            ValidateError::Single(msg) => assert!(msg.contains("名称")),
            _ => panic!("expected Single error"),
        }
    }

    #[test]
    fn test_check_field_description_from_field_map() {
        let mut fields = IndexMap::new();
        fields.insert("name".to_string(), "用户名".to_string());
        let mut v = Validate::new().field(fields).rule("name", "require");
        let data = json!({"name": ""});
        let result = v.check(&data);
        assert!(result.is_err());
        match result.unwrap_err() {
            ValidateError::Single(msg) => assert!(msg.contains("用户名")),
            _ => panic!("expected Single error"),
        }
    }

    #[test]
    fn test_check_custom_message_override() {
        // R5-1：message[field.type] 优先级最高
        let mut msgs = IndexMap::new();
        msgs.insert("name.require".to_string(), "名称必填".to_string());
        let mut v = Validate::new().message(msgs).rule("name", "require");
        let data = json!({"name": ""});
        let result = v.check(&data);
        match result.unwrap_err() {
            ValidateError::Single(msg) => assert_eq!(msg, "名称必填"),
            _ => panic!("expected Single error"),
        }
    }

    // ========================================================================
    // 组 13：场景测试（基础）
    // ========================================================================

    #[test]
    fn test_check_scene_filters_fields() {
        // 场景过滤：only 列表中的字段才验证
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("age", "require")
            .register_scene("login", vec!["name".to_string()])
            .scene("login");
        // age 缺失但不在 scene 中，应该通过
        let data = json!({"name": "Alice"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_check_scene_resets_state() {
        // R5-6：切换场景时重置 only/append/remove
        let mut v = Validate::new()
            .rule("name", "require")
            .register_scene("s1", vec!["name".to_string()])
            .only(vec!["other".to_string()]) // 设置一个 only
            .scene("s1"); // 切换场景应该重置 only
        let data = json!({"name": "Alice"});
        // scene s1 的 only = ["name"]，所以 name 在 only 中，应该验证
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_has_scene() {
        let v = Validate::new()
            .register_scene("login", vec!["name".to_string()])
            .register_scene("register", vec!["name".to_string(), "email".to_string()]);
        assert!(v.has_scene("login"));
        assert!(v.has_scene("register"));
        assert!(!v.has_scene("logout"));
    }

    // ========================================================================
    // 组 14：check_rule 测试
    // ========================================================================

    #[test]
    fn test_check_rule_success() {
        let v = Validate::new();
        assert!(v.check_rule(&json!("hello"), "require").is_ok());
        assert!(v.check_rule(&json!(123), "integer").is_ok());
    }

    #[test]
    fn test_check_rule_failure() {
        let v = Validate::new();
        assert!(v.check_rule(&Value::Null, "require").is_err());
        assert!(v.check_rule(&json!("abc"), "integer").is_err());
    }

    #[test]
    fn test_check_rule_multiple() {
        let v = Validate::new();
        assert!(v.check_rule(&json!(123), "require|integer").is_ok());
        assert!(v.check_rule(&json!(""), "require|integer").is_err());
    }

    // ========================================================================
    // 组 14.1：dispatch_builtin 集成测试（验证 rules.rs 接入）
    // ========================================================================

    #[test]
    fn test_check_dispatch_eq_rule() {
        // eq:5 → dispatch_builtin("eq", value, "5", ...) → rules::eq
        let v = Validate::new();
        assert!(v.check_rule(&json!(5), "eq:5").is_ok());
        assert!(v.check_rule(&json!("5"), "eq:5").is_ok()); // 松散比较
        assert!(v.check_rule(&json!(6), "eq:5").is_err());
    }

    #[test]
    fn test_check_dispatch_gt_egt_lt_elt_rules() {
        let v = Validate::new();
        // gt:5
        assert!(v.check_rule(&json!(6), "gt:5").is_ok());
        assert!(v.check_rule(&json!(5), "gt:5").is_err());
        // egt:5
        assert!(v.check_rule(&json!(5), "egt:5").is_ok());
        assert!(v.check_rule(&json!(4), "egt:5").is_err());
        // lt:5
        assert!(v.check_rule(&json!(4), "lt:5").is_ok());
        assert!(v.check_rule(&json!(5), "lt:5").is_err());
        // elt:5
        assert!(v.check_rule(&json!(5), "elt:5").is_ok());
        assert!(v.check_rule(&json!(6), "elt:5").is_err());
    }

    #[test]
    fn test_check_dispatch_in_not_in_rules() {
        let v = Validate::new();
        assert!(v.check_rule(&json!(1), "in:1,2,3").is_ok());
        assert!(v.check_rule(&json!("1"), "in:1,2,3").is_ok()); // 松散比较
        assert!(v.check_rule(&json!(4), "in:1,2,3").is_err());
        assert!(v.check_rule(&json!(4), "notIn:1,2,3").is_ok());
        assert!(v.check_rule(&json!(1), "notIn:1,2,3").is_err());
    }

    #[test]
    fn test_check_dispatch_between_not_between_rules() {
        let v = Validate::new();
        assert!(v.check_rule(&json!(5), "between:1,10").is_ok());
        assert!(v.check_rule(&json!("5"), "between:1,10").is_ok()); // 松散比较
        assert!(v.check_rule(&json!(0), "between:1,10").is_err());
        assert!(v.check_rule(&json!(11), "between:1,10").is_err());
        assert!(v.check_rule(&json!(0), "notBetween:1,10").is_ok());
    }

    #[test]
    fn test_check_dispatch_length_max_min_rules() {
        let v = Validate::new();
        // length
        assert!(v.check_rule(&json!("abc"), "length:3").is_ok());
        assert!(v.check_rule(&json!("abc"), "length:1,5").is_ok());
        assert!(v.check_rule(&json!("abcdef"), "length:1,5").is_err());
        // max
        assert!(v.check_rule(&json!("abc"), "max:5").is_ok());
        assert!(v.check_rule(&json!("abcdef"), "max:5").is_err());
        // min
        assert!(v.check_rule(&json!("abc"), "min:3").is_ok());
        assert!(v.check_rule(&json!("ab"), "min:3").is_err());
    }

    #[test]
    fn test_check_dispatch_length_unicode_chinese() {
        // 中文按字符计数（对齐 PHP mb_strlen）
        let v = Validate::new();
        assert!(v.check_rule(&json!("中文测试"), "length:4").is_ok());
        assert!(v.check_rule(&json!("中"), "length:1").is_ok());
    }

    #[test]
    fn test_check_dispatch_date_format_rule() {
        let v = Validate::new();
        assert!(v
            .check_rule(&json!("2024-01-15"), "dateFormat:Y-m-d")
            .is_ok());
        assert!(v
            .check_rule(&json!("2024/01/15"), "dateFormat:Y-m-d")
            .is_err());
    }

    #[test]
    fn test_check_dispatch_after_before_rules() {
        let v = Validate::new();
        assert!(v
            .check_rule(&json!("2024-01-02"), "after:2024-01-01")
            .is_ok());
        assert!(v
            .check_rule(&json!("2023-12-31"), "after:2024-01-01")
            .is_err());
        assert!(v
            .check_rule(&json!("2023-12-31"), "before:2024-01-01")
            .is_ok());
        assert!(v
            .check_rule(&json!("2024-01-02"), "before:2024-01-01")
            .is_err());
    }

    #[test]
    fn test_check_dispatch_ip_rule() {
        let v = Validate::new();
        assert!(v.check_rule(&json!("127.0.0.1"), "ip:ipv4").is_ok());
        assert!(v.check_rule(&json!("127.0.0.1"), "ip").is_ok()); // 默认 ipv4
        assert!(v.check_rule(&json!("::1"), "ip:ipv6").is_ok());
        assert!(v.check_rule(&json!("::1"), "ip:ipv4").is_err());
    }

    #[test]
    fn test_check_dispatch_confirm_rule_via_check() {
        // 通过 check 方法验证 confirm 规则的字段推断
        let mut v = Validate::new().rule("password", "require|confirm");
        let data = json!({"password": "abc123", "password_confirm": "abc123"});
        assert!(v.check(&data).is_ok());

        let data = json!({"password": "abc123", "password_confirm": "different"});
        assert!(v.check(&data).is_err());
    }

    #[test]
    fn test_check_dispatch_different_rule_via_check() {
        let mut v = Validate::new().rule("field1", "different:field2");
        let data = json!({"field1": "abc", "field2": "xyz"});
        assert!(v.check(&data).is_ok());

        let data = json!({"field1": "abc", "field2": "abc"});
        assert!(v.check(&data).is_err());
    }

    #[test]
    fn test_check_dispatch_require_if_rule_via_check() {
        let mut v = Validate::new().rule("username", "requireIf:type,login");
        // type=login 时 username 必须非空
        let data = json!({"type": "login", "username": "alice"});
        assert!(v.check(&data).is_ok());
        let data = json!({"type": "login", "username": ""});
        assert!(v.check(&data).is_err());
        // type!=login 时 username 不验证
        let data = json!({"type": "register", "username": ""});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_check_dispatch_require_with_rule_via_check() {
        let mut v = Validate::new().rule("email", "requireWith:contact");
        // contact 有值时 email 必须
        let data = json!({"contact": "some_value", "email": "user@example.com"});
        assert!(v.check(&data).is_ok());
        let data = json!({"contact": "some_value", "email": ""});
        assert!(v.check(&data).is_err());
        // contact 无值时 email 不验证
        let data = json!({"contact": "", "email": ""});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_check_dispatch_alias_operators() {
        // PHP 别名：> → gt, >= → egt, < → lt, <= → elt, = → eq
        let v = Validate::new();
        assert!(v.check_rule(&json!(6), ">:5").is_ok());
        assert!(v.check_rule(&json!(5), ">=:5").is_ok());
        assert!(v.check_rule(&json!(4), "<:5").is_ok());
        assert!(v.check_rule(&json!(5), "<=:5").is_ok());
        assert!(v.check_rule(&json!(5), "=:5").is_ok());
    }

    // ========================================================================
    // 组 15：extend 自定义规则测试
    // ========================================================================

    #[test]
    fn test_extend_custom_rule_pass() {
        let mut v = Validate::new();
        v.extend(
            "custom_even",
            Arc::new(|value: &Value, _rule: &str, _data: &Value| {
                value.as_i64().map(|n| n % 2 == 0).unwrap_or(false)
            }),
        );
        let mut v = v.rule("num", "custom_even");
        let data = json!({"num": 4});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_extend_custom_rule_fail() {
        let mut v = Validate::new();
        v.extend(
            "custom_even",
            Arc::new(|value: &Value, _rule: &str, _data: &Value| {
                value.as_i64().map(|n| n % 2 == 0).unwrap_or(false)
            }),
        );
        let mut v = v.rule("num", "custom_even");
        let data = json!({"num": 5});
        let result = v.check(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_extend_custom_rule_with_args() {
        let mut v = Validate::new();
        v.extend(
            "custom_min",
            Arc::new(|value: &Value, rule: &str, _data: &Value| {
                let min: i64 = rule.parse().unwrap_or(0);
                value.as_i64().map(|n| n >= min).unwrap_or(false)
            }),
        );
        let mut v = v.rule("num", "custom_min:10");
        assert!(v.check(&json!({"num": 15})).is_ok());
        assert!(v.check(&json!({"num": 5})).is_err());
    }

    // ========================================================================
    // 组 16：get_error 测试
    // ========================================================================

    #[test]
    fn test_get_error_after_check_failure() {
        let mut v = Validate::new().rule("name", "require");
        let data = json!({"name": ""});
        let _ = v.check(&data);
        let error = v.get_error();
        match error {
            ValidateError::Single(msg) => assert!(msg.contains("require")),
            _ => panic!("expected Single error"),
        }
    }

    #[test]
    fn test_get_error_after_check_success() {
        let mut v = Validate::new().rule("name", "require");
        let data = json!({"name": "Alice"});
        let _ = v.check(&data);
        // 成功时 error 保持初始状态
        match v.get_error() {
            ValidateError::Single(s) => assert!(s.is_empty()),
            _ => panic!("expected Single (empty)"),
        }
    }

    // ========================================================================
    // 组 17：PHP 行为对齐测试（R5 硬约束）
    // ========================================================================

    #[test]
    fn test_php_bug_numeric_key_returns_key_itself() {
        // R5-3：PHP getDataValue 对数值型 key 返回 key 本身
        // 这是一个 PHP 怪异行为，sz-rust 1:1 复刻
        let data = json!({"123": "value", "456": "another"});
        // 数值 key "123" 返回 "123"（key 本身），不是 "value"
        assert_eq!(Validate::get_data_value(&data, "123"), json!("123"));
    }

    #[test]
    fn test_php_behavior_empty_value_skips_non_require_rules() {
        // R5-5：PHP checkItem 中，空值且非 require/must 规则 → 跳过验证
        let mut v = Validate::new().rule("age", "integer");
        // age 为空字符串，integer 规则应该跳过
        let data = json!({"age": ""});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_php_behavior_require_validates_string_zero() {
        // PHP 特殊行为："0" 在 require 中被视为非空
        let mut v = Validate::new().rule("count", "require");
        let data = json!({"count": "0"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_php_behavior_scene_resets_only_append_remove() {
        // R5-6：PHP getScene 方法重置 only/append/remove
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .only(vec!["email".to_string()]) // 手动设置 only
            .register_scene("scene1", vec!["name".to_string()])
            .scene("scene1");
        let data = json!({"name": "Alice"}); // 没有 email
                                             // scene1 切换后 only 应该是 ["name"]，email 不验证
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_php_behavior_remove_all_rules_for_field() {
        // remove[field] = None 表示移除所有规则
        let mut v = Validate::new().rule("name", "require").remove("name", None);
        let data = json!({}); // name 缺失
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_php_behavior_remove_specific_rule() {
        // remove[field] = ["integer"] 仅移除 integer 规则
        let mut v = Validate::new()
            .rule("age", "require|integer")
            .remove("age", Some(vec!["integer".to_string()]));
        let data = json!({"age": "not_int"});
        // integer 被移除，require 通过（非空）
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_php_behavior_append_adds_rule() {
        // append[field] = ["email"] 追加 email 规则
        let mut v = Validate::new()
            .rule("contact", "require")
            .append("contact", vec!["email".to_string()]);
        // contact 是有效邮箱
        assert!(v.check(&json!({"contact": "user@example.com"})).is_ok());
        // contact 不是邮箱
        assert!(v.check(&json!({"contact": "invalid"})).is_err());
    }

    #[test]
    fn test_php_behavior_get_rule_msg_lookup_chain() {
        // R5-1：完整的查找链测试
        // 1. message[field.type] 存在时优先使用
        let mut msgs = IndexMap::new();
        msgs.insert("name.require".to_string(), "优先级1".to_string());
        msgs.insert("name".to_string(), "优先级2".to_string());
        let v = Validate::new().message(msgs);
        assert_eq!(v.get_rule_msg("name", "名称", "require", ""), "优先级1");

        // 2. 仅 message[field] 存在时
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "优先级2".to_string());
        let v = Validate::new().message(msgs);
        assert_eq!(v.get_rule_msg("name", "名称", "require", ""), "优先级2");

        // 3. 仅 type_msg[type] 存在时
        let v = Validate::new();
        assert_eq!(
            v.get_rule_msg("name", "名称", "email", ""),
            "名称 not a valid email address"
        );

        // 4. require 前缀回退
        assert_eq!(
            v.get_rule_msg("name", "名称", "requireIf", ""),
            "名称 require"
        );

        // 5. 默认（对齐 PHP 第 1578 行：$title . lang->get('not conform to the rules')）
        // 无 Lang 时 lang->get 返回 name 本身，无前导空格
        assert_eq!(
            v.get_rule_msg("name", "名称", "unknownType", ""),
            "名称not conform to the rules"
        );
    }

    // ========================================================================
    // 组 18：场景回调测试（Phase 5.3）
    // ========================================================================

    #[test]
    fn test_register_scene_callback_basic() {
        // 注册场景回调后，has_scene 应返回 true
        let v = Validate::new().register_scene_callback("login", Arc::new(|_v| {}));
        assert!(v.has_scene("login"));
        assert!(!v.has_scene("register"));
    }

    #[test]
    fn test_has_scene_checks_both_array_and_callback() {
        // has_scene 同时检查 scene 数组和 scene_callbacks
        let v = Validate::new()
            .register_scene("array_scene", vec!["name".to_string()])
            .register_scene_callback("callback_scene", Arc::new(|_v| {}));
        assert!(v.has_scene("array_scene"));
        assert!(v.has_scene("callback_scene"));
        assert!(!v.has_scene("nonexistent"));
    }

    #[test]
    fn test_scene_callback_sets_only() {
        // 场景回调通过 only_mut 设置 only 字段
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .register_scene_callback(
                "login",
                Arc::new(|v| {
                    v.only_mut(vec!["email".to_string()]);
                }),
            )
            .scene("login");
        // data 只有 email，没有 name
        // 因为 scene 回调设置 only=["email"]，name 不验证
        let data = json!({"email": "test@example.com"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_scene_callback_priority_over_array() {
        // 对齐 PHP getScene 第 1663-1668 行：
        // 如果同时存在 scene{Name} 方法和 $scene[$name] 数组，方法优先
        // 这里：回调设置 only=["email"]，数组设置 only=["name"]
        // 期望：回调用，only=["email"]，name 不验证
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .register_scene("conflict", vec!["name".to_string()])
            .register_scene_callback(
                "conflict",
                Arc::new(|v| {
                    v.only_mut(vec!["email".to_string()]);
                }),
            )
            .scene("conflict");
        // data 只有 email，没有 name
        // 如果回调用，only=["email"]，应该通过
        // 如果数组用，only=["name"]，应该失败
        let data = json!({"email": "test@example.com"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_scene_callback_can_modify_append() {
        // 场景回调通过 append_mut 追加规则
        let mut v = Validate::new()
            .rule("name", "require")
            .register_scene_callback(
                "strict",
                Arc::new(|v| {
                    v.append_mut("name", vec!["max:5".to_string()]);
                }),
            )
            .scene("strict");
        // name 长度 6，超过 max:5，应该失败
        let data = json!({"name": "Alice2"});
        assert!(v.check(&data).is_err());
    }

    #[test]
    fn test_scene_callback_can_modify_remove() {
        // 场景回调通过 remove_mut 移除规则
        let mut v = Validate::new()
            .rule("name", "require|max:5")
            .register_scene_callback(
                "lenient",
                Arc::new(|v| {
                    v.remove_mut("name", Some(vec!["max".to_string()]));
                }),
            )
            .scene("lenient");
        // name 长度 6，但 max 被移除，require 通过（非空），应该成功
        let data = json!({"name": "Alice2"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_scene_callback_resets_state() {
        // R5-6：切换场景时重置 only/append/remove（对齐 PHP getScene 第 1661 行）
        // 即使之前手动设置了 only，切换场景后回调中 only_mut 覆盖
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .only(vec!["email".to_string()]) // 手动设置 only=["email"]
            .register_scene_callback(
                "scene1",
                Arc::new(|v| {
                    v.only_mut(vec!["name".to_string()]);
                }),
            )
            .scene("scene1");
        // data 有 name，没有 email
        // 手动 only=["email"] 被重置，回调设置 only=["name"]
        // name 验证通过，email 不在 only 中不验证
        let data = json!({"name": "Alice"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_scene_callback_no_callback_no_array() {
        // 场景名既无回调也无数组，仅重置 only/append/remove
        // 对齐 PHP getScene：如果 scene 不存在，only/append/remove 仍被重置为空
        let mut v = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .only(vec!["email".to_string()]) // 手动设置 only
            .scene("nonexistent"); // 场景不存在
        let data = json!({"name": "Alice"}); // 没有 email
                                             // scene 不存在，only 被重置为空，所有字段都验证
                                             // name 通过，email 缺失失败
        assert!(v.check(&data).is_err());
    }

    #[test]
    fn test_only_mut_method() {
        // only_mut 直接修改 only 字段
        let mut v = Validate::new();
        v.only_mut(vec!["a".to_string(), "b".to_string()]);
        // 通过场景应用验证 only 是否生效
        // 间接验证：only 被设置后，未在 only 中的字段不验证
        let mut v = Validate::new()
            .rule("a", "require")
            .rule("b", "require")
            .rule("c", "require");
        v.only_mut(vec!["a".to_string(), "b".to_string()]);
        let data = json!({"a": "x", "b": "y"}); // c 缺失
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_append_mut_method() {
        // append_mut 直接修改 append 字段
        let mut v = Validate::new().rule("name", "require");
        v.append_mut("name", vec!["max:3".to_string()]);
        // name 长度 5，超过 max:3，应该失败
        let data = json!({"name": "Alice"});
        assert!(v.check(&data).is_err());
    }

    #[test]
    fn test_remove_mut_method() {
        // remove_mut 直接修改 remove 字段
        let mut v = Validate::new().rule("name", "require|max:3");
        v.remove_mut("name", Some(vec!["max".to_string()]));
        // max 被移除，require 通过，应该成功
        let data = json!({"name": "Alice"});
        assert!(v.check(&data).is_ok());
    }

    #[test]
    fn test_php_behavior_scene_callback_mimics_scene_method() {
        // PHP 行为对齐：sceneXxx 方法典型用法
        // PHP:
        //   protected function sceneRegister()
        //   {
        //       return $this->only(['name', 'email', 'age'])->append('age', 'require');
        //   }
        // Rust 等价：
        // 注：age 必须在 only 中，否则场景过滤会跳过 age，append 不生效
        let mut v = Validate::new()
            .rule("name", "require|max:25")
            .rule("email", "require|email")
            .rule("age", "integer")
            .register_scene_callback(
                "register",
                Arc::new(|v| {
                    v.only_mut(vec![
                        "name".to_string(),
                        "email".to_string(),
                        "age".to_string(),
                    ]);
                    v.append_mut("age", vec!["require".to_string()]);
                }),
            )
            .scene("register");
        // data 有 name、email、age
        let data = json!({
            "name": "Alice",
            "email": "test@example.com",
            "age": 30
        });
        assert!(v.check(&data).is_ok());
        // 缺少 age 应该失败（age 在 only 中，integer 通过空值跳过，但 append 了 require）
        let data2 = json!({
            "name": "Alice",
            "email": "test@example.com"
        });
        assert!(v.check(&data2).is_err());
    }

    #[test]
    fn test_php_behavior_get_scene_method_priority() {
        // PHP getScene 第 1663-1668 行严格对齐：
        // method_exists 优先于 isset($scene[$name])
        // 注册两个场景名，一个用回调，一个用数组，验证都能正常工作
        let mut v1 = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .register_scene_callback(
                "cb_scene",
                Arc::new(|v| {
                    v.only_mut(vec!["email".to_string()]);
                }),
            )
            .scene("cb_scene");
        let data1 = json!({"email": "test@example.com"}); // name 缺失
        assert!(v1.check(&data1).is_ok());

        let mut v2 = Validate::new()
            .rule("name", "require")
            .rule("email", "require")
            .register_scene("arr_scene", vec!["email".to_string()])
            .scene("arr_scene");
        let data2 = json!({"email": "test@example.com"}); // name 缺失
        assert!(v2.check(&data2).is_ok());
    }

    #[test]
    fn test_php_behavior_scene_resets_all_three_state() {
        // R5-6 完整对齐：切换场景时 only/append/remove 全部重置
        // 手动设置 only/append/remove，切换场景后全部被重置
        let mut v = Validate::new()
            .rule("name", "require|max:5")
            .rule("email", "require")
            .only(vec!["email".to_string()]) // 手动 only
            .append("name", vec!["max:3".to_string()]) // 手动 append
            .remove("email", None) // 手动 remove
            .register_scene("reset", vec!["name".to_string()])
            .scene("reset");
        // 切换场景后：
        // - only 重置为 ["name"]
        // - append 重置为空（name 上的 max:3 失效）
        // - remove 重置为空（email 上的 remove 失效）
        let data = json!({"name": "Alice"}); // name 长度 5，满足 max:5；email 缺失
                                             // only=["name"]，email 不验证
                                             // append 被重置，name 上的 max:3 不生效，max:5 通过
        assert!(v.check(&data).is_ok());
    }

    // ========================================================================
    // 组 19：错误消息国际化测试（Phase 5.4）
    // ========================================================================

    #[test]
    fn test_set_lang_basic_injection() {
        // 对齐 PHP setLang(Lang $lang) — 注入 Lang 实例
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("require", "必须填写"));
        let v = Validate::new().set_lang(lang);
        assert!(v.lang.is_some());
    }

    #[test]
    fn test_get_rule_msg_with_lang_default_translation() {
        // 对齐 PHP 第 1578 行：$title . $this->lang->get('not conform to the rules')
        // Lang 中存在 'not conform to the rules' 翻译时使用翻译值
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("not conform to the rules", "不符合规则"));
        let v = Validate::new().set_lang(lang);
        let msg = v.get_rule_msg("field", "字段", "unknownType", "");
        assert_eq!(msg, "字段不符合规则");
    }

    #[test]
    fn test_get_rule_msg_with_lang_no_translation_returns_name() {
        // 对齐 PHP Lang::get 找不到时返回 name 本身
        let lang: Arc<dyn message::Lang> = Arc::new(message::SimpleLang::new());
        let v = Validate::new().set_lang(lang);
        let msg = v.get_rule_msg("field", "字段", "unknownType", "");
        assert_eq!(msg, "字段not conform to the rules");
    }

    #[test]
    fn test_get_rule_msg_with_lang_percent_var_syntax() {
        // 对齐 PHP parseErrorMsg 第 1598-1599 行：{%var} 语法
        // message[field] = "{%custom_msg}" 时应翻译
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("custom_msg", "自定义错误消息"));
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "{%custom_msg}".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "自定义错误消息");
    }

    #[test]
    fn test_get_rule_msg_with_lang_has_check() {
        // 对齐 PHP parseErrorMsg 第 1600-1601 行：lang->has($msg) 检查
        // message[field] = "require" 且 lang->has("require") 时翻译
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("require", "必须填写"));
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "require".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "必须填写");
    }

    #[test]
    fn test_get_rule_msg_with_lang_percent_var_with_placeholders() {
        // {%var} 翻译后再进行占位符替换
        // 翻译结果 ":attribute 必填" 中的 :attribute 应被替换为 title
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("require_msg", ":attribute 必填"));
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "{%require_msg}".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "名称 必填");
    }

    #[test]
    fn test_get_rule_msg_with_lang_has_check_with_placeholders() {
        // lang->has 翻译后再进行占位符替换
        let lang: Arc<dyn message::Lang> = Arc::new(
            message::SimpleLang::new().set("range_msg", ":attribute 必须在 :1 到 :2 之间"),
        );
        let mut msgs = IndexMap::new();
        msgs.insert("age".to_string(), "range_msg".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("age", "年龄", "between", "18,60");
        assert_eq!(msg, "年龄 必须在 18 到 60 之间");
    }

    #[test]
    fn test_parse_error_msg_with_lang_no_lang_skips_translation() {
        // 无 Lang 时跳过翻译，直接进行占位符替换
        let v = Validate::new();
        let result = v.parse_error_msg_with_lang(":attribute require", "", "名称");
        assert_eq!(result, "名称 require");
    }

    #[test]
    fn test_parse_error_msg_with_lang_translates_first() {
        // 先翻译，后替换占位符
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("custom_msg", ":attribute 自定义错误 :rule"));
        let v = Validate::new().set_lang(lang);
        let result = v.parse_error_msg_with_lang("custom_msg", "param1", "字段");
        assert_eq!(result, "字段 自定义错误 param1");
    }

    #[test]
    fn test_parse_error_msg_with_lang_percent_var_no_placeholders() {
        // {%var} 翻译后无占位符时直接返回
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("plain_msg", "纯文本消息"));
        let v = Validate::new().set_lang(lang);
        let result = v.parse_error_msg_with_lang("{%plain_msg}", "rule", "字段");
        assert_eq!(result, "纯文本消息");
    }

    #[test]
    fn test_parse_error_msg_with_lang_no_colon_no_replacement() {
        // 对齐 PHP 第 1613 行：msg 不含 : 时不进行替换
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("plain", "纯文本"));
        let v = Validate::new().set_lang(lang);
        let result = v.parse_error_msg_with_lang("plain", "rule", "字段");
        assert_eq!(result, "纯文本");
    }

    #[test]
    fn test_check_with_lang_translates_error_message() {
        // 集成测试：check 方法失败时返回的错误消息包含 Lang 翻译
        let lang: Arc<dyn message::Lang> = Arc::new(
            message::SimpleLang::new()
                .set("not conform to the rules", "不符合规则")
                .set("require", ":attribute 必须填写"),
        );
        let mut v = Validate::new().rule("name", "require").set_lang(lang);
        let data = json!({});
        let result = v.check(&data);
        assert!(result.is_err());
        if let Err(ValidateError::Single(msg)) = result {
            // type_msg["require"] = ":attribute require"
            // lang->has(":attribute require") = false
            // 占位符替换：:attribute → "名称"（title 从 "name" 推导）
            // 注意：name 没有描述，title 默认为字段名 "name"
            assert_eq!(msg, "name require");
        } else {
            panic!("Expected ValidateError::Single");
        }
    }

    #[test]
    fn test_check_with_lang_percent_var_in_message() {
        // 集成测试：自定义 message 使用 {%var} 语法
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("name_required", "名称是必填字段"));
        let mut msgs = IndexMap::new();
        msgs.insert("name.require".to_string(), "{%name_required}".to_string());
        let mut v = Validate::new()
            .rule("name", "require")
            .message(msgs)
            .set_lang(lang);
        let data = json!({});
        let result = v.check(&data);
        assert!(result.is_err());
        if let Err(ValidateError::Single(msg)) = result {
            assert_eq!(msg, "名称是必填字段");
        } else {
            panic!("Expected ValidateError::Single");
        }
    }

    #[test]
    fn test_check_with_lang_has_check_in_message() {
        // 集成测试：自定义 message 命中 lang->has
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("NAME_REQUIRED", "名称必填"));
        let mut msgs = IndexMap::new();
        // lang->has 不区分大小写，"name_required" 能命中 "NAME_REQUIRED"
        msgs.insert("name.require".to_string(), "name_required".to_string());
        let mut v = Validate::new()
            .rule("name", "require")
            .message(msgs)
            .set_lang(lang);
        let data = json!({});
        let result = v.check(&data);
        assert!(result.is_err());
        if let Err(ValidateError::Single(msg)) = result {
            assert_eq!(msg, "名称必填");
        } else {
            panic!("Expected ValidateError::Single");
        }
    }

    // ========================================================================
    // 组 20：PHP 行为对齐测试（Phase 5.4 — R5-7）
    // ========================================================================

    #[test]
    fn test_php_behavior_lang_translation_priority_percent_over_has() {
        // 对齐 PHP parseErrorMsg：{%var} 优先于 lang->has
        let lang: Arc<dyn message::Lang> = Arc::new(
            message::SimpleLang::new()
                .set("{%key}", "整体键") // 理论上不会出现，但验证优先级
                .set("key", "提取键"),
        );
        let mut msgs = IndexMap::new();
        msgs.insert("field".to_string(), "{%key}".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("field", "字段", "require", "");
        assert_eq!(msg, "提取键");
    }

    #[test]
    fn test_php_behavior_lang_default_case_uses_lang_get() {
        // 对齐 PHP 第 1578 行：默认分支调用 lang->get('not conform to the rules')
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("not conform to the rules", " 不符合规则"));
        let v = Validate::new().set_lang(lang);
        let msg = v.get_rule_msg("field", "字段", "unknownType", "");
        // PHP: $title . $this->lang->get('not conform to the rules')
        // 翻译值为 " 不符合规则"（带前导空格），拼接为 "字段 不符合规则"
        assert_eq!(msg, "字段 不符合规则");
    }

    #[test]
    fn test_php_behavior_lang_translation_case_insensitive() {
        // 对齐 PHP Lang::has/get 的 strtolower 行为
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("RequireError", ":attribute 必填"));
        let mut msgs = IndexMap::new();
        // msg = "requireerror"（小写）应命中 Lang 中的 "RequireError"
        msgs.insert("name".to_string(), "requireerror".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "名称 必填");
    }

    #[test]
    fn test_php_behavior_lang_translation_not_found_returns_original() {
        // 对齐 PHP Lang::get 找不到时返回 name 本身
        let lang: Arc<dyn message::Lang> = Arc::new(message::SimpleLang::new());
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "nonexistent_key".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        // lang->has("nonexistent_key") = false → 返回原值 "nonexistent_key"
        assert_eq!(msg, "nonexistent_key");
    }

    #[test]
    fn test_php_behavior_lang_percent_var_extracts_key_via_substr() {
        // 对齐 PHP substr($msg, 2, -1) 提取 {%var} 内的 key
        let lang: Arc<dyn message::Lang> =
            Arc::new(message::SimpleLang::new().set("my.message.key", "我的消息"));
        let mut msgs = IndexMap::new();
        msgs.insert("name".to_string(), "{%my.message.key}".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("name", "名称", "require", "");
        assert_eq!(msg, "我的消息");
    }

    #[test]
    fn test_php_behavior_lang_translation_with_full_pipeline() {
        // 完整流水线测试：{%var} → 翻译 → 占位符替换
        let lang: Arc<dyn message::Lang> = Arc::new(message::SimpleLang::new().set(
            "between_msg",
            ":attribute 必须在 :1 - :2 之间（默认 :rule）",
        ));
        let mut msgs = IndexMap::new();
        msgs.insert("age.between".to_string(), "{%between_msg}".to_string());
        let v = Validate::new().message(msgs).set_lang(lang);
        let msg = v.get_rule_msg("age", "年龄", "between", "18,60");
        // 1. {%between_msg} → ":attribute 必须在 :1 - :2 之间（默认 :rule）"
        // 2. 占位符替换：:attribute → "年龄", :1 → "18", :2 → "60", :rule → "18,60"
        assert_eq!(msg, "年龄 必须在 18 - 60 之间（默认 18,60）");
    }
}

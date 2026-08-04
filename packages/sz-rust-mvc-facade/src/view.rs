//! 视图渲染器 — 对齐 PHP `think\View`
//!
//! ## PHP 对齐说明
//! 对齐 PHP `think\View`（外观模式）+ `think\contract\TemplateHandlerInterface`（驱动接口）+
//! `think\Template`（模板引擎核心）。PHP 使用 `ob_start` + `include`/`eval` 机制，
//! Rust 改为直接返回 `String`，避免 I/O 副作用。
//!
//! ## 核心类型
//! - [`View`]：视图入口（对齐 PHP `think\View`）
//! - [`TemplateEngine`] trait：模板引擎接口（对齐 PHP `think\contract\TemplateHandlerInterface`）
//! - [`SimpleTemplateEngine`]：默认模板引擎（对齐 PHP `think\Template` 的基本标签）
//! - [`ViewConfig`]：视图配置（对齐 PHP `config/view.php`）
//!
//! ## 支持的标签
//! - `{$var}` — 变量插值（对齐 PHP `parseVar`）
//! - `{$var.attr}` — 嵌套属性（对齐 PHP `.` 语法，默认 array 模式）
//! - `{$var|filter}` — 过滤器（对齐 PHP `parseVarFunction`）
//! - `{$var|default=x}` — 默认值
//! - `{$var?='x'}` / `{$var?:'x'}` / `{$var??'x'}` — 三元表达式
//! - `{:func(args)}` — 函数调用（对齐 PHP `{:fun()}`）
//! - `{//comment}` / `{/*comment*/}` — 注释（对齐 PHP `parseTag` 注释分支）
//! - `{literal}...{/literal}` — 原文保留（对齐 PHP `parseLiteral`）
//! - `{if}/{elseif}/{else}` — 条件判断（对齐 PHP Cx `tagIf`）
//! - `{foreach}` — foreach 循环（对齐 PHP Cx `tagForeach`）
//! - `{volist}` — volist 循环（对齐 PHP Cx `tagVolist`）
//! - `{switch}/{case}/{default}` — switch 分支（对齐 PHP Cx `tagSwitch`）
//! - `{for}` — for 循环（对齐 PHP Cx `tagFor`）
//! - 配置模式布局（`layout_on=true`，对齐 PHP `compiler()`）
//! - `{layout name="..." replace="..."}` — 标签模式布局（对齐 PHP `parseLayout()`）
//! - `{__NOLAYOUT__}` — 单独禁用布局
//! - `{extend name="..."}` — 模板继承（对齐 PHP `parseExtend()`）
//! - `{block name="..."}...{/block}` — block 定义（对齐 PHP `parseBlock()`）
//! - `{__BLOCK__}` / `{__block__}` — block 合并标记（对齐 PHP `str_replace`）
//!
//! ## PHP 源码参考
//! - `think\framework\src\think\View.php`（195 行）
//! - `think\framework\src\think\contract\TemplateHandlerInterface.php`
//! - `think\framework\src\think\view\driver\Php.php`
//! - `think-template\src\Template.php`（~1800 行）
//! - `think-view\src\Think.php`
//! - `config\view.php`

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use regex::Regex;
use serde_json::Value;

// Cx 标签库控制流标签（对齐 PHP `think\template\taglib\Cx`）
pub mod template;

// 模板布局（对齐 PHP `Template::compiler()` + `parseLayout()`）
pub mod layout;

// 模板继承（对齐 PHP `Template::parseExtend()` + `parseBlock()`）
pub mod inheritance;

// ============================================================================
// 错误类型
// ============================================================================

/// 视图错误
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    /// 模板文件未找到（对齐 PHP `TemplateNotFoundException`）
    #[error("模板文件未找到: {0}")]
    TemplateNotFound(String),

    /// 模板语法错误
    #[error("模板语法错误: {0}")]
    SyntaxError(String),

    /// 模板渲染错误
    #[error("模板渲染错误: {0}")]
    RenderError(String),

    /// IO 错误
    #[error("IO 错误: {0}")]
    IoError(#[from] std::io::Error),
}

// ============================================================================
// 类型别名
// ============================================================================

/// 模板变量数据（对齐 PHP `$data` 数组）
pub type ViewData = HashMap<String, Value>;

/// 内容过滤器（对齐 PHP `View::filter`，单值回调）
pub type ContentFilter = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// 模板函数（对齐 PHP `{:func()}` 中的函数）
pub type TemplateFn = Arc<dyn Fn(&[Value]) -> Result<Value, ViewError> + Send + Sync>;

// ============================================================================
// 视图配置
// ============================================================================

/// 视图配置（对齐 PHP `config/view.php` + `think\Template` 配置）
#[derive(Debug, Clone)]
pub struct ViewConfig {
    /// 视图路径（对齐 PHP `view_path`）
    pub view_path: PathBuf,

    /// 视图后缀（对齐 PHP `view_suffix`，默认 'html'）
    pub view_suffix: String,

    /// 视图分隔符（对齐 PHP `view_depr`，默认 '/'）
    pub view_depr: String,

    /// 模板标签开始（对齐 PHP `tpl_begin`，默认 '{'）
    pub tpl_begin: String,

    /// 模板标签结束（对齐 PHP `tpl_end`，默认 '}'）
    pub tpl_end: String,

    /// 标签库开始（对齐 PHP `taglib_begin`，默认 '{'）
    pub taglib_begin: String,

    /// 标签库结束（对齐 PHP `taglib_end`，默认 '}'）
    pub taglib_end: String,

    /// 默认过滤器（对齐 PHP `default_filter`，默认 'htmlentities'）
    pub default_filter: String,

    /// 布局开关（对齐 PHP `layout_on`，默认 false）
    pub layout_on: bool,

    /// 布局名称（对齐 PHP `layout_name`，默认 'layout'）
    pub layout_name: String,

    /// 布局替换项（对齐 PHP `layout_item`，默认 '{__CONTENT__}'）
    pub layout_item: String,

    /// 变量识别方式（对齐 PHP `tpl_var_identify`，默认 'array'）
    pub tpl_var_identify: String,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            view_path: PathBuf::from("view"),
            view_suffix: "html".to_string(),
            view_depr: "/".to_string(),
            tpl_begin: "{".to_string(),
            tpl_end: "}".to_string(),
            taglib_begin: "{".to_string(),
            taglib_end: "}".to_string(),
            default_filter: "htmlentities".to_string(),
            layout_on: false,
            layout_name: "layout".to_string(),
            layout_item: "{__CONTENT__}".to_string(),
            tpl_var_identify: "array".to_string(),
        }
    }
}

// ============================================================================
// 模板引擎接口
// ============================================================================

/// 模板引擎接口（对齐 PHP `think\contract\TemplateHandlerInterface`）
///
/// PHP 接口方法：
/// - `exists(string $template): bool`
/// - `fetch(string $template, array $data = []): void`（PHP 用 echo，Rust 返回 String）
/// - `display(string $content, array $data = []): void`（PHP 用 echo，Rust 返回 String）
/// - `config(array $config): void`
/// - `getConfig(string $name): mixed`
pub trait TemplateEngine: Send + Sync {
    /// 模板是否存在（对齐 PHP `exists`）
    fn exists(&self, template: &str) -> bool;

    /// 渲染模板文件（对齐 PHP `fetch`）
    fn fetch(&self, template: &str, data: &ViewData) -> Result<String, ViewError>;

    /// 渲染字符串内容（对齐 PHP `display`）
    fn display(&self, content: &str, data: &ViewData) -> Result<String, ViewError>;

    /// 设置配置（对齐 PHP `config`）
    fn set_config(&mut self, config: ViewConfig);

    /// 读取配置（对齐 PHP `getConfig`）
    fn get_config(&self, name: &str) -> Option<Value>;

    /// 返回 `&dyn Any` 以支持 downcast（Rust 特有，无 PHP 对应）
    fn as_any(&self) -> &dyn std::any::Any;
}

// ============================================================================
// 简易模板引擎
// ============================================================================

/// 简易模板引擎（对齐 PHP `think\Template` 的基本标签解析）
///
/// ## 支持的标签
///
/// | 标签 | 示例 | 说明 |
/// |------|------|------|
/// | `{$var}` | `{$name}` | 变量插值 |
/// | `{$var.attr}` | `{$user.name}` | 嵌套属性（对齐 PHP `.` 语法，array 模式） |
/// | `{$var\|filter}` | `{$name\|upper}` | 过滤器 |
/// | `{$var\|default=x}` | `{$name\|default='N/A'}` | 默认值 |
/// | `{$var?='x'}` | | 三元：真则输出 |
/// | `{$var?:'x'}` | | 三元：假则输出 x |
/// | `{$var??'x'}` | | null 合并 |
/// | `{:func(args)}` | `{:date('Y')}` | 函数调用 |
/// | `{//comment}` | | 单行注释 |
/// | `{/*comment*/}` | | 块注释 |
/// | `{literal}...{/literal}` | | 原文保留 |
pub struct SimpleTemplateEngine {
    config: RwLock<ViewConfig>,
    functions: RwLock<HashMap<String, TemplateFn>>,
}

impl SimpleTemplateEngine {
    /// 创建新引擎
    pub fn new(config: ViewConfig) -> Self {
        let mut functions = HashMap::new();
        register_builtin_functions(&mut functions);
        Self {
            config: RwLock::new(config),
            functions: RwLock::new(functions),
        }
    }

    /// 注册自定义函数（对齐 PHP `Template::extend` 扩展机制）
    pub fn register_function(&self, name: &str, func: TemplateFn) {
        self.functions.write().insert(name.to_string(), func);
    }

    /// 解析模板路径（对齐 PHP `parseTemplateFile`）
    ///
    /// PHP 规则：
    /// 1. 含 `@` → 跨应用调用 `app@template`
    /// 2. 首字符 `/` → 绝对路径
    /// 3. 否则 → `view_path/template.view_suffix`
    pub fn parse_template_path(&self, template: &str) -> PathBuf {
        let config = self.config.read();
        let view_path = &config.view_path;
        let suffix = &config.view_suffix;

        if template.is_empty() {
            return view_path.join(format!("index.{}", suffix));
        }

        // 绝对路径
        if let Some(stripped) = template.strip_prefix('/') {
            let mut path = PathBuf::from(stripped);
            if path.extension().is_none() {
                path = path.with_extension(suffix);
            }
            return path;
        }

        // 跨应用调用（对齐 PHP `app@template`）
        if let Some(at_pos) = template.find('@') {
            let app = &template[..at_pos];
            let tpl = &template[at_pos + 1..];
            let mut path = PathBuf::from(app);
            path.push("view");
            path.push(tpl);
            if path.extension().is_none() {
                path = path.with_extension(suffix);
            }
            return path;
        }

        // 相对路径（默认）
        let mut path = view_path.join(template);
        if path.extension().is_none() {
            path = path.with_extension(suffix);
        }
        path
    }

    /// 渲染内容字符串（核心解析逻辑）
    ///
    /// 对齐 PHP `Template::parse()` 的解析顺序：
    /// 1. parseLiteral（暂存 literal 内容）
    /// 2. parseTagLib（控制流标签：if/foreach/volist/switch/for）
    /// 3. parseTag（变量/函数/注释）
    /// 4. 还原 literal
    fn render_content(&self, content: &str, data: &ViewData) -> Result<String, ViewError> {
        // 1. 暂存 {literal}...{/literal} 内容
        let (content, literals) = self.extract_literals(content);

        // 2. 解析控制流标签（对齐 PHP `TagLib::parseTag`）
        let config = self.config.read().clone();
        let content = template::render_control_flow(&content, data, &config, |c, d| {
            self.render_content(c, d)
        })?;

        // 3. 解析标签（变量/函数/注释）
        let content = self.parse_tags(&content, data)?;

        // 4. 还原 literal
        let content = self.restore_literals(&content, &literals);

        Ok(content)
    }

    /// 暂存 {literal}...{/literal} 内容（对齐 PHP `parseLiteral`）
    fn extract_literals(&self, content: &str) -> (String, Vec<String>) {
        let config = self.config.read();
        let begin = &config.tpl_begin;
        let end = &config.tpl_end;
        let literal_open = format!("{}literal{}", begin, end);
        let literal_close = format!("{}/literal{}", begin, end);

        let mut result = String::with_capacity(content.len());
        let mut literals = Vec::new();
        let mut remaining = content;

        loop {
            if let Some(open_pos) = remaining.find(&literal_open) {
                result.push_str(&remaining[..open_pos]);
                let after_open = &remaining[open_pos + literal_open.len()..];
                if let Some(close_pos) = after_open.find(&literal_close) {
                    let literal_content = &after_open[..close_pos];
                    let placeholder = format!("<!--###LITERAL{}###-->", literals.len());
                    literals.push(literal_content.to_string());
                    result.push_str(&placeholder);
                    remaining = &after_open[close_pos + literal_close.len()..];
                } else {
                    // 未闭合的 literal，原样输出
                    result.push_str(&remaining[open_pos..]);
                    break;
                }
            } else {
                result.push_str(remaining);
                break;
            }
        }

        (result, literals)
    }

    /// 还原 literal 内容
    fn restore_literals(&self, content: &str, literals: &[String]) -> String {
        let mut result = content.to_string();
        for (i, literal) in literals.iter().enumerate() {
            let placeholder = format!("<!--###LITERAL{}###-->", i);
            result = result.replace(&placeholder, literal);
        }
        result
    }

    /// 解析标签（对齐 PHP `parseTag`）
    ///
    /// PHP 按首字符分支：
    /// - `$` → 变量 `{$var}`
    /// - `:` → 函数输出 `{:fun()}`
    /// - `~` → 函数执行 `{~fun()}`
    /// - `+` / `-` → 表达式
    /// - `/` → 注释 `{//...}` `{/*...*/}`
    fn parse_tags(&self, content: &str, data: &ViewData) -> Result<String, ViewError> {
        let config = self.config.read();
        let begin = regex::escape(&config.tpl_begin);
        let end = regex::escape(&config.tpl_end);

        // 匹配 {tag_content}（非贪婪，允许跨行）
        // begin/end 已经过 regex::escape，直接拼接即可
        let pattern = format!("{}(.*?){}", begin, end);
        let re = Regex::new(&pattern).map_err(|e| ViewError::SyntaxError(e.to_string()))?;

        let mut result = String::with_capacity(content.len());
        let mut last_end = 0;

        for caps in re.captures_iter(content) {
            let full_match = caps.get(0).expect("正则捕获组 0 必定存在");
            let tag_content = caps.get(1).expect("正则捕获组 1 必定存在").as_str();

            result.push_str(&content[last_end..full_match.start()]);

            let rendered = self.render_tag(tag_content, data)?;
            result.push_str(&rendered);

            last_end = full_match.end();
        }
        result.push_str(&content[last_end..]);

        Ok(result)
    }

    /// 渲染单个标签（对齐 PHP `parseTag` 首字符分支）
    fn render_tag(&self, tag: &str, data: &ViewData) -> Result<String, ViewError> {
        let tag = tag.trim();

        if tag.is_empty() {
            return Ok(String::new());
        }

        // 按首字符分支（对齐 PHP parseTag）
        let first_char = tag.chars().next().expect("已检查 tag 非空");

        match first_char {
            '$' => self.render_var_tag(tag, data),
            ':' => self.render_func_tag(tag, data, false),
            '~' => self.render_func_tag(tag, data, true),
            '/' => {
                // 注释 {//...} 或 {/*...*/}
                Ok(String::new())
            }
            _ => {
                // 未识别标签，原样输出（对齐 PHP parseTag "其他" 分支）
                let config = self.config.read();
                Ok(format!("{}{}{}", config.tpl_begin, tag, config.tpl_end))
            }
        }
    }

    /// 渲染变量标签 `{$var}`（对齐 PHP `parseVar` + `parseVarFunction`）
    ///
    /// PHP 语法：
    /// - `{$var}` → 输出变量值
    /// - `{$var.attr}` → 嵌套属性（array 模式：`$var['attr']`）
    /// - `{$var|filter}` → 过滤器
    /// - `{$var|default=x}` → 默认值
    /// - `{$var?='x'}` → 真则输出
    /// - `{$var?:'x'}` → 假则输出 x
    /// - `{$var??'x'}` → null 合并
    fn render_var_tag(&self, tag: &str, data: &ViewData) -> Result<String, ViewError> {
        // 去掉前导 $
        let expr = &tag[1..];

        // 分离变量名和过滤器/三元表达式
        // PHP 用 `|` 分隔过滤器，`?` 用于三元
        let (var_expr, filters, ternary) = self.split_var_expr(expr);

        // 解析变量值
        let value = self.resolve_var(&var_expr, data);

        // 应用三元表达式（对齐 PHP parseVar 的 ? 处理）
        let value = if let Some(ternary_expr) = &ternary {
            self.apply_ternary(&value, ternary_expr)?
        } else {
            value
        };

        // 应用过滤器（对齐 PHP parseVarFunction）
        let value = self.apply_filters(value, &filters)?;

        // 转为字符串输出（对齐 PHP echo）
        Ok(value_to_string(&value))
    }

    /// 分离变量表达式、过滤器、三元表达式
    ///
    /// PHP 规则：
    /// - `|` 分隔过滤器（如 `name|upper|default='N/A'`）
    /// - `?` 用于三元（如 `var?='x'`、`var?:'x'`、`var??'x'`）
    fn split_var_expr(&self, expr: &str) -> (String, Vec<String>, Option<String>) {
        // 检查三元表达式（?? / ?: / ?= / ?）
        // PHP 中 `??` 优先于 `?:` 和 `?=`
        if let Some(pos) = expr.find("??") {
            let var = expr[..pos].trim().to_string();
            let ternary = expr[pos..].trim().to_string();
            return (var, Vec::new(), Some(ternary));
        }

        // 分离 `|` 过滤器
        // 注意：`?` 可能在过滤器参数中，所以先找 `?`（但不在引号内）
        // 简化处理：先找 `|` 分割，再在每个部分中找 `?`
        let parts: Vec<&str> = expr.split('|').collect();
        let var_expr = parts[0].trim().to_string();

        // 在 var_expr 中检查三元
        if let Some(pos) = var_expr.find('?') {
            let var = var_expr[..pos].trim().to_string();
            let ternary = var_expr[pos..].trim().to_string();
            let filters: Vec<String> = parts[1..]
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            return (var, filters, Some(ternary));
        }

        let filters: Vec<String> = parts[1..]
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        (var_expr, filters, None)
    }

    /// 解析变量值（对齐 PHP `parseVar` 的 `.` 语法）
    ///
    /// PHP `tpl_var_identify` 配置：
    /// - `array`（默认）：`$a.b.c` → `$a['b']['c']`
    /// - `obj`：`$a.b.c` → `$a->b->c`
    /// - `''`（自动）：`(is_array($a)?$a['b']:$a->b)`
    fn resolve_var(&self, expr: &str, data: &ViewData) -> Value {
        resolve_var_expr(expr, data)
    }

    /// 应用三元表达式（对齐 PHP `parseVar` 的 `?` 处理）
    ///
    /// PHP 语法：
    /// - `??'x'` → `isset($var) ? $var : 'x'`（null 合并）
    /// - `?:'x'` → `!empty($var) ? $var : 'x'`
    /// - `?='x'` → `if ($var) echo 'x'`
    /// - `? 'a' : 'b'` → `!empty($var) ? 'a' : 'b'`
    fn apply_ternary(&self, value: &Value, ternary: &str) -> Result<Value, ViewError> {
        // `??` null 合并
        if let Some(default) = ternary.strip_prefix("??") {
            if value.is_null() {
                return Ok(parse_literal(default.trim()));
            }
            return Ok(value.clone());
        }

        // `?:` 假则输出
        if let Some(default) = ternary.strip_prefix("?:") {
            if is_truthy(value) {
                return Ok(value.clone());
            }
            return Ok(parse_literal(default.trim()));
        }

        // `?=` 真则输出
        if let Some(output) = ternary.strip_prefix("?=") {
            if is_truthy(value) {
                return Ok(parse_literal(output.trim()));
            }
            return Ok(Value::Null);
        }

        // `? a : b` 标准三元
        if let Some(rest) = ternary.strip_prefix('?') {
            if let Some(colon_pos) = rest.find(':') {
                let true_val = rest[..colon_pos].trim();
                let false_val = rest[colon_pos + 1..].trim();
                if is_truthy(value) {
                    return Ok(parse_literal(true_val));
                }
                return Ok(parse_literal(false_val));
            }
            // `? 'x'`（无冒号，真则输出）
            if is_truthy(value) {
                return Ok(parse_literal(rest.trim()));
            }
            return Ok(Value::Null);
        }

        Ok(value.clone())
    }

    /// 应用过滤器（对齐 PHP `parseVarFunction`）
    ///
    /// PHP 内置过滤器：
    /// - `htmlentities`（默认，自动追加）
    /// - `raw`（跳过过滤）
    /// - `upper` / `lower`
    /// - `default=x`
    /// - `date=格式`
    fn apply_filters(&self, mut value: Value, filters: &[String]) -> Result<Value, ViewError> {
        let config = self.config.read();
        let default_filter = &config.default_filter;

        // PHP 默认追加 htmlentities（除非显式 |raw）
        let has_raw = filters.iter().any(|f| f.starts_with("raw"));
        if !has_raw && !default_filter.is_empty() && default_filter != "raw" {
            value = apply_builtin_filter(value, default_filter, None)?;
        }

        // 应用用户指定的过滤器
        for filter in filters {
            if filter.starts_with("raw") {
                continue;
            }

            // 分离过滤器名和参数（`filter=arg` 或 `filter(arg)`）
            let (filter_name, filter_arg) = if let Some(eq_pos) = filter.find('=') {
                (&filter[..eq_pos], Some(filter[eq_pos + 1..].to_string()))
            } else if let Some(paren_pos) = filter.find('(') {
                (
                    &filter[..paren_pos],
                    Some(filter[paren_pos + 1..].trim_end_matches(')').to_string()),
                )
            } else {
                (filter.as_str(), None)
            };

            value = apply_builtin_filter(value, filter_name.trim(), filter_arg)?;
        }

        Ok(value)
    }

    /// 渲染函数标签 `{:func()}` 或 `{~func()}`（对齐 PHP `parseTag` 的 `:` 和 `~` 分支）
    ///
    /// PHP 语法：
    /// - `{:func(args)}` → `echo func(args)`
    /// - `{~func(args)}` → `func(args)`（执行，不输出）
    fn render_func_tag(
        &self,
        tag: &str,
        _data: &ViewData,
        suppress_output: bool,
    ) -> Result<String, ViewError> {
        // 去掉前导 `:` 或 `~`
        let expr = &tag[1..];

        // 解析函数名和参数
        let (func_name, args) = parse_func_call(expr)?;

        // 查找函数
        let functions = self.functions.read();
        let func = functions
            .get(&func_name)
            .ok_or_else(|| ViewError::RenderError(format!("未注册的模板函数: {}", func_name)))?;

        // 调用函数
        let result = func(&args)?;

        // `~` 前缀：执行但不输出（对齐 PHP `{~fun()}`）
        if suppress_output {
            return Ok(String::new());
        }

        Ok(value_to_string(&result))
    }
}

impl TemplateEngine for SimpleTemplateEngine {
    fn exists(&self, template: &str) -> bool {
        let path = self.parse_template_path(template);
        path.is_file()
    }

    fn fetch(&self, template: &str, data: &ViewData) -> Result<String, ViewError> {
        let path = self.parse_template_path(template);

        if !path.is_file() {
            return Err(ViewError::TemplateNotFound(format!(
                "{} (解析路径: {})",
                template,
                path.display()
            )));
        }

        let content = std::fs::read_to_string(&path)?;
        let config = self.config.read().clone();
        // PHP `parse()` 顺序：parseExtend → parseLayout
        // 应用继承（对齐 PHP `Template::parseExtend()`）
        let content = inheritance::apply_inheritance(&content, &config)?;
        // 应用布局（对齐 PHP `Template::compiler()` 在 `parse()` 之前应用布局）
        let content = layout::apply_layout(&content, &config)?;
        self.render_content(&content, data)
    }

    fn display(&self, content: &str, data: &ViewData) -> Result<String, ViewError> {
        let config = self.config.read().clone();
        // PHP `parse()` 顺序：parseExtend → parseLayout
        // 应用继承（对齐 PHP `Template::parseExtend()`）
        let content = inheritance::apply_inheritance(content, &config)?;
        // 应用布局（对齐 PHP `Template::display()` 也通过 `compiler()` 应用布局）
        let content = layout::apply_layout(&content, &config)?;
        self.render_content(&content, data)
    }

    fn set_config(&mut self, config: ViewConfig) {
        *self.config.write() = config;
    }

    fn get_config(&self, name: &str) -> Option<Value> {
        let config = self.config.read();
        match name {
            "view_path" => Some(Value::String(config.view_path.to_string_lossy().into())),
            "view_suffix" => Some(Value::String(config.view_suffix.clone())),
            "view_depr" => Some(Value::String(config.view_depr.clone())),
            "tpl_begin" => Some(Value::String(config.tpl_begin.clone())),
            "tpl_end" => Some(Value::String(config.tpl_end.clone())),
            "taglib_begin" => Some(Value::String(config.taglib_begin.clone())),
            "taglib_end" => Some(Value::String(config.taglib_end.clone())),
            "default_filter" => Some(Value::String(config.default_filter.clone())),
            "layout_on" => Some(Value::Bool(config.layout_on)),
            "layout_name" => Some(Value::String(config.layout_name.clone())),
            "layout_item" => Some(Value::String(config.layout_item.clone())),
            "tpl_var_identify" => Some(Value::String(config.tpl_var_identify.clone())),
            _ => None,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ============================================================================
// 视图入口
// ============================================================================

/// 视图入口（对齐 PHP `think\View`）
///
/// PHP `think\View` 继承 `Manager`（多驱动管理），通过 `__call` 转发到默认驱动。
/// Rust 实现简化为直接持有引擎实例。
///
/// ## PHP 对齐方法
///
/// | PHP 方法 | Rust 方法 | 说明 |
/// |----------|-----------|------|
/// | `assign($name, $value)` | [`View::assign`] | 赋值模板变量 |
/// | `filter(callable)` | [`View::set_filter`] | 设置内容过滤器 |
/// | `fetch($template, $vars)` | [`View::fetch`] | 渲染模板文件 |
/// | `display($content, $vars)` | [`View::display`] | 渲染字符串内容 |
/// | `engine($type)` | [`View::engine`] / [`View::set_engine`] | 获取/切换引擎 |
/// | `exists($template)` | [`View::exists`] | 模板是否存在 |
/// | `__get($name)` | [`View::get_var`] | 读取变量 |
/// | `__isset($name)` | [`View::has_var`] | 变量是否存在 |
pub struct View {
    /// 模板变量池（对齐 PHP `$data`）
    data: RwLock<ViewData>,

    /// 内容过滤器（对齐 PHP `$filter`，单值回调）
    filter: RwLock<Option<ContentFilter>>,

    /// 模板引擎（对齐 PHP `$drivers['default']`）
    engine: RwLock<Box<dyn TemplateEngine>>,
}

impl View {
    /// 创建新视图（对齐 PHP `new View()`）
    pub fn new(engine: Box<dyn TemplateEngine>) -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
            filter: RwLock::new(None),
            engine: RwLock::new(engine),
        }
    }

    /// 创建使用 SimpleTemplateEngine 的默认视图
    pub fn with_default_engine() -> Self {
        Self::new(Box::new(SimpleTemplateEngine::new(ViewConfig::default())))
    }

    /// 创建使用指定配置的默认视图
    pub fn with_config(config: ViewConfig) -> Self {
        Self::new(Box::new(SimpleTemplateEngine::new(config)))
    }

    /// 赋值模板变量（对齐 PHP `View::assign`）
    ///
    /// PHP: `$view->assign('name', 'value')` 或 `$view->assign(['k1' => 'v1'])`
    pub fn assign(&self, name: &str, value: Value) -> &Self {
        self.data.write().insert(name.to_string(), value);
        self
    }

    /// 批量赋值模板变量
    pub fn assign_many(&self, vars: ViewData) -> &Self {
        self.data.write().extend(vars);
        self
    }

    /// 设置内容过滤器（对齐 PHP `View::filter`）
    ///
    /// PHP: `$view->filter(function($content) { return strtoupper($content); })`
    pub fn set_filter(&self, filter: ContentFilter) -> &Self {
        *self.filter.write() = Some(filter);
        self
    }

    /// 清除过滤器
    pub fn clear_filter(&self) -> &Self {
        *self.filter.write() = None;
        self
    }

    /// 渲染模板文件（对齐 PHP `View::fetch`）
    ///
    /// PHP: `$content = $view->fetch('index', ['name' => 'value'])`
    ///
    /// 合并规则：`$vars` 优先于 `$this->data`（对齐 PHP `array_merge`）
    pub fn fetch(&self, template: &str, vars: Option<ViewData>) -> Result<String, ViewError> {
        let mut data = self.data.read().clone();
        if let Some(vars) = vars {
            data.extend(vars);
        }

        let content = self.engine.read().fetch(template, &data)?;
        self.apply_filter(content)
    }

    /// 渲染字符串内容（对齐 PHP `View::display`）
    ///
    /// PHP: `$content = $view->display('Hello {$name}!', ['name' => 'World'])`
    pub fn display(&self, content: &str, vars: Option<ViewData>) -> Result<String, ViewError> {
        let mut data = self.data.read().clone();
        if let Some(vars) = vars {
            data.extend(vars);
        }

        let rendered = self.engine.read().display(content, &data)?;
        self.apply_filter(rendered)
    }

    /// 模板是否存在（对齐 PHP `View::exists`）
    pub fn exists(&self, template: &str) -> bool {
        self.engine.read().exists(template)
    }

    /// 获取变量（对齐 PHP `View::__get`）
    pub fn get_var(&self, name: &str) -> Option<Value> {
        self.data.read().get(name).cloned()
    }

    /// 检查变量是否存在（对齐 PHP `View::__isset`）
    pub fn has_var(&self, name: &str) -> bool {
        self.data.read().contains_key(name)
    }

    /// 清除所有变量
    pub fn clear_vars(&self) -> &Self {
        self.data.write().clear();
        self
    }

    /// 获取引擎（对齐 PHP `View::engine`）
    pub fn engine(&self) -> parking_lot::RwLockReadGuard<'_, Box<dyn TemplateEngine>> {
        self.engine.read()
    }

    /// 替换引擎
    pub fn set_engine(&self, engine: Box<dyn TemplateEngine>) -> &Self {
        *self.engine.write() = engine;
        self
    }

    /// 应用过滤器（对齐 PHP `getContent` 中的 filter 调用）
    fn apply_filter(&self, content: String) -> Result<String, ViewError> {
        if let Some(filter) = self.filter.read().as_ref() {
            Ok(filter(&content))
        } else {
            Ok(content)
        }
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 解析变量表达式（对齐 PHP `parseVar` 的 `.` 语法）
///
/// 从 `resolve_var` 方法提取为自由函数，供 `template` 子模块复用。
pub(super) fn resolve_var_expr(expr: &str, data: &ViewData) -> Value {
    let parts: Vec<&str> = expr.split('.').collect();
    let mut current = data.get(parts[0]).cloned().unwrap_or(Value::Null);

    for part in &parts[1..] {
        current = match &current {
            Value::Object(map) => map.get(*part).cloned().unwrap_or(Value::Null),
            Value::Array(arr) => {
                // 数组按整数索引或字符串 key 查找
                if let Ok(idx) = part.parse::<usize>() {
                    arr.get(idx).cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
            _ => Value::Null,
        };
    }

    current
}

/// HTML 转义（对齐 PHP `htmlentities`）
fn htmlentities(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#039;"),
            _ => result.push(c),
        }
    }
    result
}

/// 判断值是否为真（对齐 PHP 真值判断）
///
/// PHP 真值规则：
/// - `false`、`0`、`0.0`、`""`、`"0"`、`[]`、`null` → false
/// - 其他 → true
pub(super) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty() && s != "0",
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Value 转字符串（对齐 PHP `echo`）
pub(super) fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "1" } else { "" }.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                if f == f.trunc() {
                    format!("{}", f as i64)
                } else {
                    format!("{}", f)
                }
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.clone(),
        Value::Array(a) => serde_json::to_string(a).unwrap_or_default(),
        Value::Object(o) => serde_json::to_string(o).unwrap_or_default(),
    }
}

/// 解析字面量（对齐 PHP 模板中的字符串/数字字面量）
pub(super) fn parse_literal(s: &str) -> Value {
    let s = s.trim();

    // 去除引号
    if (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
        || (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
    {
        return Value::String(s[1..s.len() - 1].to_string());
    }

    // 数字
    if let Ok(i) = s.parse::<i64>() {
        return Value::Number(i.into());
    }
    if let Ok(f) = s.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }

    // 布尔
    match s {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" => return Value::Null,
        _ => {}
    }

    // 默认作为字符串
    Value::String(s.to_string())
}

/// 解析函数调用（对齐 PHP `{:func(args)}`）
///
/// 返回 (函数名, 参数列表)
fn parse_func_call(expr: &str) -> Result<(String, Vec<Value>), ViewError> {
    let expr = expr.trim();

    if let Some(paren_pos) = expr.find('(') {
        let func_name = expr[..paren_pos].trim().to_string();
        let args_str = expr[paren_pos + 1..].trim_end_matches(')');

        let mut args = Vec::new();
        if !args_str.trim().is_empty() {
            for arg in split_args(args_str) {
                args.push(parse_literal(arg.trim()));
            }
        }

        Ok((func_name, args))
    } else {
        // 无参数调用 `{:func}`
        Ok((expr.to_string(), Vec::new()))
    }
}

/// 分割函数参数（处理引号内的逗号）
fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    for c in s.chars() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(c);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(c);
            }
            ',' if !in_single_quote && !in_double_quote => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }

    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }

    args
}

/// 应用内置过滤器（对齐 PHP `parseVarFunction` 内置函数）
fn apply_builtin_filter(
    value: Value,
    filter_name: &str,
    arg: Option<String>,
) -> Result<Value, ViewError> {
    match filter_name {
        "raw" => Ok(value),
        "htmlentities" | "htmlspecialchars" => {
            Ok(Value::String(htmlentities(&value_to_string(&value))))
        }
        "upper" | "strtoupper" => Ok(Value::String(value_to_string(&value).to_uppercase())),
        "lower" | "strtolower" => Ok(Value::String(value_to_string(&value).to_lowercase())),
        "default" => {
            if is_truthy(&value) {
                Ok(value)
            } else {
                let default_val = arg.unwrap_or_default();
                Ok(parse_literal(&default_val))
            }
        }
        "first" => {
            if let Value::Array(arr) = &value {
                Ok(arr.first().cloned().unwrap_or(Value::Null))
            } else {
                Ok(Value::Null)
            }
        }
        "last" => {
            if let Value::Array(arr) = &value {
                Ok(arr.last().cloned().unwrap_or(Value::Null))
            } else {
                Ok(Value::Null)
            }
        }
        _ => Err(ViewError::RenderError(format!(
            "未知的模板过滤器: {}",
            filter_name
        ))),
    }
}

/// 注册内置函数（对齐 PHP `Template` 注册的 `$Think` 扩展）
fn register_builtin_functions(functions: &mut HashMap<String, TemplateFn>) {
    // date 函数（对齐 PHP `date('Y-m-d')`）
    functions.insert(
        "date".to_string(),
        Arc::new(|args: &[Value]| -> Result<Value, ViewError> {
            let format = args
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("Y-m-d H:i:s");
            let now = chrono::Local::now();
            let php_format = php_date_to_chrono(format);
            Ok(Value::String(now.format(&php_format).to_string()))
        }),
    );

    // strtoupper 函数
    functions.insert(
        "strtoupper".to_string(),
        Arc::new(|args: &[Value]| -> Result<Value, ViewError> {
            let s = args.first().map(value_to_string).unwrap_or_default();
            Ok(Value::String(s.to_uppercase()))
        }),
    );

    // strtolower 函数
    functions.insert(
        "strtolower".to_string(),
        Arc::new(|args: &[Value]| -> Result<Value, ViewError> {
            let s = args.first().map(value_to_string).unwrap_or_default();
            Ok(Value::String(s.to_lowercase()))
        }),
    );
}

/// PHP date 格式转 chrono 格式
fn php_date_to_chrono(php_format: &str) -> String {
    let mut result = String::with_capacity(php_format.len() * 2);
    let chars = php_format.chars();
    for c in chars {
        match c {
            'Y' => result.push_str("%Y"),
            'y' => result.push_str("%y"),
            'm' => result.push_str("%m"),
            'n' => result.push_str("%-m"),
            'd' => result.push_str("%d"),
            'j' => result.push_str("%-d"),
            'H' => result.push_str("%H"),
            'G' => result.push_str("%-H"),
            'i' => result.push_str("%M"),
            's' => result.push_str("%S"),
            'D' => result.push_str("%a"),
            'l' => result.push_str("%A"),
            'M' => result.push_str("%b"),
            'F' => result.push_str("%B"),
            'a' => result.push_str("%p"),
            'A' => result.push_str("%p"),
            'U' => result.push_str("%s"),
            _ => {
                result.push(c);
            }
        }
    }
    result
}

// ============================================================================
// 模板渲染兜底场景（对齐 DefaultResponseType::Html）
//
// 项目主策略为前后端分离（JSON 默认返回），但部分场景需要渲染 HTML 模板：
// - PDF 导出：渲染 HTML 模板作为 PDF 输入（对齐 PHP pdf-pdftk 表单填充场景）
// - Excel 导出：渲染 HTML 表格作为 Excel 输入（对齐 PHP PhpSpreadsheet 场景）
// - 邮件内容：渲染 HTML 模板作为邮件正文
// - 报表页面：渲染 HTML 报表
//
// 本模块提供 View 渲染到 axum Response 的桥接方法，复用
// `respond_html` 函数，确保 Content-Type 统一为 text/html; charset=utf-8。
//
// ## PHP 源码参考
//
// PHP `Dispatch::autoResponse()` 第 96 行（vendor/topthink/framework/src/think/route/Dispatch.php）：
// ```php
// $type     = $this->request->isJson() ? 'json' : 'html';
// $response = Response::create($data, $type);
// ```
// 当 `$type = 'html'` 时，PHP 创建 HTML Response。
// 本模块对应 Rust 的 HTML Response 创建路径。
//
// ## PHP 项目实际使用情况
//
// 鲜视达 PHP 项目（e:\vue\test\鲜视达\server）实际不使用 ThinkPHP 模板渲染做 PDF/Excel 导出：
// - PDF 导出：使用 mikehaertl/php-pdftk 填充 PDF 表单（addons/finance/model/Payment.php:1357-1514）
//   + HTTP 调用 Java 服务（app/job/controller/Pdf.php，http_java_post 到 127.0.0.1:8086）
// - Excel 导出：使用 PhpSpreadsheet 直接操作 Spreadsheet 对象（app/common/service/order/ExportService.php）
//   + fputcsv CSV 流式输出（app/common.php:924-956）
// 但框架层保留了 HTML 兜底分支（Dispatch::autoResponse 第 96 行 isJson() ? 'json' : 'html'），
// 本模块对齐此设计，为 Rust 实现提供模板渲染兜底能力。
// ============================================================================

use axum::response::Response;

/// 模板渲染兜底场景 helper（对齐 `DefaultResponseType::Html`）
///
/// 项目主策略为前后端分离（JSON 默认返回），但 PDF/Excel 导出、邮件内容、
/// 报表页面等场景需要渲染 HTML 模板。本结构体封装了 View 渲染到 HTML Response
/// 的桥接逻辑，提供便捷的链式调用。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_core::view::{ViewFallback, ViewConfig};
/// use serde_json::json;
///
/// let fallback = ViewFallback::with_default_engine();
/// fallback.assign("name", json!("World"));
///
/// // 渲染字符串内容为 HTML Response
/// let response = fallback.render_display("Hello {$name}!", None).unwrap();
///
/// // 渲染为字符串（用于 PDF/Excel 生成器输入）
/// let html = fallback.display_to_string("Hello {$name}!", None).unwrap();
/// ```
pub struct ViewFallback {
    /// 内部 View 实例
    view: View,
}

impl ViewFallback {
    /// 创建新的模板渲染兜底 helper
    pub fn new(view: View) -> Self {
        Self { view }
    }

    /// 从默认配置创建模板渲染兜底 helper
    pub fn with_default_engine() -> Self {
        Self::new(View::with_default_engine())
    }

    /// 从指定配置创建模板渲染兜底 helper
    pub fn with_config(config: ViewConfig) -> Self {
        Self::new(View::with_config(config))
    }

    /// 渲染模板文件为 HTML Response（兜底场景）
    ///
    /// 对齐 PHP `$this->fetch('template')` + `Response::create($content, 'html')`。
    /// 项目主策略为 JSON，但 PDF/Excel 导出等场景需要 HTML 渲染。
    ///
    /// # 参数
    ///
    /// - `template`：模板名（对齐 PHP `fetch($template)`）
    /// - `vars`：模板变量（可选，对齐 PHP `fetch($template, $vars)`）
    ///
    /// # 返回
    ///
    /// `Ok(Response)`：HTTP 200，Content-Type: text/html; charset=utf-8
    /// `Err(ViewError)`：模板未找到 / 渲染失败
    pub fn render_template(
        &self,
        template: &str,
        vars: Option<ViewData>,
    ) -> Result<Response, ViewError> {
        let content = self.view.fetch(template, vars)?;
        Ok(sz_rust_http_facade::response::respond_html(content))
    }

    /// 渲染字符串内容为 HTML Response（兜底场景）
    ///
    /// 对齐 PHP `$this->display($content)` + `Response::create($content, 'html')`。
    ///
    /// # 参数
    ///
    /// - `content`：模板字符串内容
    /// - `vars`：模板变量（可选）
    ///
    /// # 返回
    ///
    /// `Ok(Response)`：HTTP 200，Content-Type: text/html; charset=utf-8
    /// `Err(ViewError)`：渲染失败
    pub fn render_display(
        &self,
        content: &str,
        vars: Option<ViewData>,
    ) -> Result<Response, ViewError> {
        let rendered = self.view.display(content, vars)?;
        Ok(sz_rust_http_facade::response::respond_html(rendered))
    }

    /// 渲染模板文件为字符串（用于 PDF/Excel 生成器输入）
    ///
    /// PDF/Excel 生成器通常需要 HTML 字符串作为输入，而非 HTTP Response。
    /// 本方法直接返回渲染后的 HTML 字符串。
    ///
    /// # 参数
    ///
    /// - `template`：模板名
    /// - `vars`：模板变量（可选）
    ///
    /// # 返回
    ///
    /// `Ok(String)`：渲染后的 HTML 字符串
    /// `Err(ViewError)`：模板未找到 / 渲染失败
    pub fn render_to_string(
        &self,
        template: &str,
        vars: Option<ViewData>,
    ) -> Result<String, ViewError> {
        self.view.fetch(template, vars)
    }

    /// 渲染字符串内容为字符串（用于 PDF/Excel 生成器输入）
    ///
    /// # 参数
    ///
    /// - `content`：模板字符串内容
    /// - `vars`：模板变量（可选）
    ///
    /// # 返回
    ///
    /// `Ok(String)`：渲染后的 HTML 字符串
    /// `Err(ViewError)`：渲染失败
    pub fn display_to_string(
        &self,
        content: &str,
        vars: Option<ViewData>,
    ) -> Result<String, ViewError> {
        self.view.display(content, vars)
    }

    /// 获取内部 View 引用（用于直接操作 View）
    pub fn view(&self) -> &View {
        &self.view
    }

    /// 赋值模板变量（对齐 PHP `View::assign`）
    pub fn assign(&self, name: &str, value: Value) -> &Self {
        self.view.assign(name, value);
        self
    }

    /// 批量赋值模板变量
    pub fn assign_many(&self, vars: ViewData) -> &Self {
        self.view.assign_many(vars);
        self
    }

    /// 清除所有变量
    pub fn clear_vars(&self) -> &Self {
        self.view.clear_vars();
        self
    }
}

/// 渲染模板文件为 HTML Response（兜底场景，自由函数版本）
///
/// 对齐 PHP `$this->fetch('template')` + `Response::create($content, 'html')`。
/// 便捷函数，无需创建 `ViewFallback` 实例。
///
/// # 参数
///
/// - `view`：视图实例
/// - `template`：模板名
/// - `vars`：模板变量（可选）
///
/// # 返回
///
/// `Ok(Response)`：HTTP 200，Content-Type: text/html; charset=utf-8
/// `Err(ViewError)`：模板未找到 / 渲染失败
pub fn render_template_response(
    view: &View,
    template: &str,
    vars: Option<ViewData>,
) -> Result<Response, ViewError> {
    let content = view.fetch(template, vars)?;
    Ok(sz_rust_http_facade::response::respond_html(content))
}

/// 渲染字符串内容为 HTML Response（兜底场景，自由函数版本）
///
/// 对齐 PHP `$this->display($content)` + `Response::create($content, 'html')`。
///
/// # 参数
///
/// - `view`：视图实例
/// - `content`：模板字符串内容
/// - `vars`：模板变量（可选）
///
/// # 返回
///
/// `Ok(Response)`：HTTP 200，Content-Type: text/html; charset=utf-8
/// `Err(ViewError)`：渲染失败
pub fn render_display_response(
    view: &View,
    content: &str,
    vars: Option<ViewData>,
) -> Result<Response, ViewError> {
    let rendered = view.display(content, vars)?;
    Ok(sz_rust_http_facade::response::respond_html(rendered))
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    // =========================================================================
    // 辅助函数
    // =========================================================================

    /// 创建测试用 View（使用临时目录作为 view_path）
    fn make_view() -> View {
        View::with_default_engine()
    }

    /// 创建测试用 View（使用指定 view_path）
    fn make_view_with_path(path: &Path) -> View {
        let config = ViewConfig {
            view_path: path.to_path_buf(),
            ..Default::default()
        };
        View::with_config(config)
    }

    /// 创建临时目录
    fn make_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sz_rust_view_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 写入临时模板文件
    fn write_template(dir: &Path, name: &str, content: &str) {
        let path = dir.join(format!("{}.html", name));
        std::fs::write(&path, content).unwrap();
    }

    /// 清理临时目录
    fn cleanup_dir(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // =========================================================================
    // 组 1：View::assign 基本赋值（对齐 PHP testAssignData）
    // =========================================================================

    #[test]
    fn test_assign_single_var() {
        // 对齐 PHP: $view->assign('foo', 'bar')
        let view = make_view();
        view.assign("foo", json!("bar"));
        assert_eq!(view.get_var("foo"), Some(json!("bar")));
    }

    #[test]
    fn test_assign_multiple_vars() {
        let view = make_view();
        view.assign("foo", json!("bar"))
            .assign("baz", json!("boom"));
        assert_eq!(view.get_var("foo"), Some(json!("bar")));
        assert_eq!(view.get_var("baz"), Some(json!("boom")));
    }

    #[test]
    fn test_assign_overwrite() {
        let view = make_view();
        view.assign("foo", json!("bar"));
        view.assign("foo", json!("new"));
        assert_eq!(view.get_var("foo"), Some(json!("new")));
    }

    #[test]
    fn test_has_var() {
        let view = make_view();
        assert!(!view.has_var("foo"));
        view.assign("foo", json!("bar"));
        assert!(view.has_var("foo"));
    }

    #[test]
    fn test_clear_vars() {
        let view = make_view();
        view.assign("foo", json!("bar"));
        view.clear_vars();
        assert!(!view.has_var("foo"));
    }

    #[test]
    fn test_assign_many() {
        let view = make_view();
        let mut vars = ViewData::new();
        vars.insert("a".to_string(), json!(1));
        vars.insert("b".to_string(), json!(2));
        view.assign_many(vars);
        assert_eq!(view.get_var("a"), Some(json!(1)));
        assert_eq!(view.get_var("b"), Some(json!(2)));
    }

    // =========================================================================
    // 组 2：View::display 基本渲染（对齐 PHP testRender）
    // =========================================================================

    #[test]
    fn test_display_string_var() {
        // 对齐 PHP: $view->display('Hello {$name}!', ['name' => 'World'])
        let view = make_view();
        let result = view
            .display(
                "Hello {$name}!",
                Some(ViewData::from([("name".to_string(), json!("World"))])),
            )
            .unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_display_with_assign() {
        // 对齐 PHP: $view->assign('name', 'World'); $view->display('Hello {$name}!')
        let view = make_view();
        view.assign("name", json!("World"));
        let result = view.display("Hello {$name}!", None).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_display_vars_override_assign() {
        // 对齐 PHP: vars 优先于 $this->data（array_merge 后者覆盖）
        let view = make_view();
        view.assign("name", json!("Default"));
        let result = view
            .display(
                "Hello {$name}!",
                Some(ViewData::from([("name".to_string(), json!("Override"))])),
            )
            .unwrap();
        assert_eq!(result, "Hello Override!");
    }

    #[test]
    fn test_display_no_vars() {
        let view = make_view();
        let result = view.display("Hello World!", None).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_display_missing_var() {
        // 未定义变量输出空字符串（对齐 PHP echo null）
        let view = make_view();
        let result = view.display("Hello {$name}!", None).unwrap();
        assert_eq!(result, "Hello !");
    }

    #[test]
    fn test_display_multiple_vars() {
        let view = make_view();
        let result = view
            .display(
                "{$greeting}, {$name}!",
                Some(ViewData::from([
                    ("greeting".to_string(), json!("Hello")),
                    ("name".to_string(), json!("World")),
                ])),
            )
            .unwrap();
        assert_eq!(result, "Hello, World!");
    }

    // =========================================================================
    // 组 3：变量嵌套属性（对齐 PHP `.` 语法）
    // =========================================================================

    #[test]
    fn test_display_nested_object() {
        // 对齐 PHP: {$user.name} → $user['name']（array 模式）
        let view = make_view();
        let result = view
            .display(
                "Name: {$user.name}",
                Some(ViewData::from([(
                    "user".to_string(),
                    json!({"name": "Alice", "age": 30}),
                )])),
            )
            .unwrap();
        assert_eq!(result, "Name: Alice");
    }

    #[test]
    fn test_display_deep_nested() {
        let view = make_view();
        let result = view
            .display(
                "{$a.b.c}",
                Some(ViewData::from([(
                    "a".to_string(),
                    json!({"b": {"c": "deep"}}),
                )])),
            )
            .unwrap();
        assert_eq!(result, "deep");
    }

    #[test]
    fn test_display_array_index() {
        // 对齐 PHP: {$arr.0} → $arr[0]
        let view = make_view();
        let result = view
            .display(
                "{$arr.0}",
                Some(ViewData::from([(
                    "arr".to_string(),
                    json!(["first", "second"]),
                )])),
            )
            .unwrap();
        assert_eq!(result, "first");
    }

    #[test]
    fn test_display_nested_missing() {
        let view = make_view();
        let result = view
            .display(
                "{$user.name}",
                Some(ViewData::from([("user".to_string(), json!({}))])),
            )
            .unwrap();
        assert_eq!(result, "");
    }

    // =========================================================================
    // 组 4：过滤器（对齐 PHP parseVarFunction）
    // =========================================================================

    #[test]
    fn test_filter_upper() {
        // 对齐 PHP: {$name|upper}
        let view = make_view();
        let result = view
            .display(
                "{$name|upper}",
                Some(ViewData::from([("name".to_string(), json!("hello"))])),
            )
            .unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_filter_lower() {
        let view = make_view();
        let result = view
            .display(
                "{$name|lower}",
                Some(ViewData::from([("name".to_string(), json!("HELLO"))])),
            )
            .unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_filter_default_with_value() {
        // 对齐 PHP: {$name|default='N/A'} — 有值时返回原值
        let view = make_view();
        let result = view
            .display(
                "{$name|default='N/A'}",
                Some(ViewData::from([("name".to_string(), json!("Alice"))])),
            )
            .unwrap();
        assert_eq!(result, "Alice");
    }

    #[test]
    fn test_filter_default_without_value() {
        // 对齐 PHP: {$name|default='N/A'} — 无值时返回默认值
        let view = make_view();
        let result = view.display("{$name|default='N/A'}", None).unwrap();
        assert_eq!(result, "N/A");
    }

    #[test]
    fn test_filter_raw() {
        // 对齐 PHP: {$name|raw} — 跳过默认 htmlentities
        let view = make_view();
        let result = view
            .display(
                "{$name|raw}",
                Some(ViewData::from([("name".to_string(), json!("<b>bold</b>"))])),
            )
            .unwrap();
        assert_eq!(result, "<b>bold</b>");
    }

    #[test]
    fn test_filter_default_htmlentities() {
        // 对齐 PHP: 默认追加 htmlentities（default_filter='htmlentities'）
        let view = make_view();
        let result = view
            .display(
                "{$name}",
                Some(ViewData::from([("name".to_string(), json!("<b>bold</b>"))])),
            )
            .unwrap();
        assert_eq!(result, "&lt;b&gt;bold&lt;/b&gt;");
    }

    #[test]
    fn test_filter_chained() {
        // 对齐 PHP: {$name|upper|lower} — 链式过滤器
        let view = make_view();
        let result = view
            .display(
                "{$name|upper|lower}",
                Some(ViewData::from([("name".to_string(), json!("Hello"))])),
            )
            .unwrap();
        assert_eq!(result, "hello");
    }

    // =========================================================================
    // 组 5：三元表达式（对齐 PHP parseVar `?` 处理）
    // =========================================================================

    #[test]
    fn test_ternary_null_coalescing() {
        // 对齐 PHP: {$name??'default'} — null 合并
        let view = make_view();
        let result = view.display("{$name??'default'}", None).unwrap();
        assert_eq!(result, "default");
    }

    #[test]
    fn test_ternary_null_coalescing_with_value() {
        let view = make_view();
        let result = view
            .display(
                "{$name??'default'}",
                Some(ViewData::from([("name".to_string(), json!("Alice"))])),
            )
            .unwrap();
        assert_eq!(result, "Alice");
    }

    #[test]
    fn test_ternary_falsy_default() {
        // 对齐 PHP: {$name?:'default'} — 假则输出 default
        let view = make_view();
        let result = view.display("{$name?:'default'}", None).unwrap();
        assert_eq!(result, "default");
    }

    #[test]
    fn test_ternary_truthy_output() {
        // 对齐 PHP: {$name?='yes'} — 真则输出 yes
        let view = make_view();
        let result = view
            .display(
                "{$name?='yes'}",
                Some(ViewData::from([("name".to_string(), json!("Alice"))])),
            )
            .unwrap();
        assert_eq!(result, "yes");
    }

    // =========================================================================
    // 组 6：函数调用（对齐 PHP {:func()} 和 {~func()}）
    // =========================================================================

    #[test]
    fn test_func_date() {
        // 对齐 PHP: {:date('Y')} — 函数调用
        let view = make_view();
        let result = view.display("{:date('Y')}", None).unwrap();
        let year: u32 = result.parse().unwrap();
        assert!((2000..=2100).contains(&year));
    }

    #[test]
    fn test_func_strtoupper() {
        let view = make_view();
        let result = view.display("{:strtoupper('hello')}", None).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn test_func_no_args() {
        // 无参数函数调用
        let view = make_view();
        let result = view.display("{:date()}", None).unwrap();
        // date() 默认返回当前日期时间
        assert!(!result.is_empty());
    }

    #[test]
    fn test_func_suppress_output() {
        // 对齐 PHP: {~func()} — 执行但不输出
        let view = make_view();
        let result = view.display("{~strtoupper('hello')}", None).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_func_unknown() {
        let view = make_view();
        let result = view.display("{:unknown_func()}", None);
        assert!(result.is_err());
    }

    // =========================================================================
    // 组 7：注释（对齐 PHP parseTag `/` 分支）
    // =========================================================================

    #[test]
    fn test_single_line_comment() {
        // 对齐 PHP: {//这是注释}
        let view = make_view();
        let result = view.display("Hello{//这是注释}World", None).unwrap();
        assert_eq!(result, "HelloWorld");
    }

    #[test]
    fn test_block_comment() {
        // 对齐 PHP: {/*块注释*/}
        let view = make_view();
        let result = view.display("Hello{/*块注释*/}World", None).unwrap();
        assert_eq!(result, "HelloWorld");
    }

    // =========================================================================
    // 组 8：literal 原文保留（对齐 PHP parseLiteral）
    // =========================================================================

    #[test]
    fn test_literal_preserves_tags() {
        // 对齐 PHP: {literal}{$var}{/literal} — literal 内不解析
        let view = make_view();
        let result = view
            .display(
                "{literal}{$name}{/literal}",
                Some(ViewData::from([("name".to_string(), json!("World"))])),
            )
            .unwrap();
        assert_eq!(result, "{$name}");
    }

    #[test]
    fn test_literal_mixed() {
        let view = make_view();
        let result = view
            .display(
                "Hello {$name}! {literal}{$raw}{/literal} Bye",
                Some(ViewData::from([("name".to_string(), json!("World"))])),
            )
            .unwrap();
        assert_eq!(result, "Hello World! {$raw} Bye");
    }

    #[test]
    fn test_literal_multiple() {
        let view = make_view();
        let result = view
            .display(
                "{literal}A{/literal} {$name} {literal}B{/literal}",
                Some(ViewData::from([("name".to_string(), json!("X"))])),
            )
            .unwrap();
        assert_eq!(result, "A X B");
    }

    // =========================================================================
    // 组 9：View::fetch 文件渲染（对齐 PHP fetch）
    // =========================================================================

    #[test]
    fn test_fetch_template_file() {
        let dir = make_temp_dir();
        write_template(&dir, "index", "<h1>{$title}</h1>");
        let view = make_view_with_path(&dir);
        let result = view
            .fetch(
                "index",
                Some(ViewData::from([("title".to_string(), json!("Hello"))])),
            )
            .unwrap();
        assert_eq!(result, "<h1>Hello</h1>");
        cleanup_dir(&dir);
    }

    #[test]
    fn test_fetch_not_found() {
        let dir = make_temp_dir();
        let view = make_view_with_path(&dir);
        let result = view.fetch("nonexistent", None);
        assert!(matches!(result, Err(ViewError::TemplateNotFound(_))));
        cleanup_dir(&dir);
    }

    #[test]
    fn test_exists() {
        let dir = make_temp_dir();
        write_template(&dir, "index", "content");
        let view = make_view_with_path(&dir);
        assert!(view.exists("index"));
        assert!(!view.exists("nonexistent"));
        cleanup_dir(&dir);
    }

    #[test]
    fn test_fetch_with_assign() {
        let dir = make_temp_dir();
        write_template(&dir, "index", "Name: {$name}");
        let view = make_view_with_path(&dir);
        view.assign("name", json!("Alice"));
        let result = view.fetch("index", None).unwrap();
        assert_eq!(result, "Name: Alice");
        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 10：内容过滤器（对齐 PHP View::filter）
    // =========================================================================

    #[test]
    fn test_content_filter() {
        // 对齐 PHP: $view->filter(function($c) { return strtoupper($c); })
        let view = make_view();
        view.set_filter(Arc::new(|content: &str| content.to_uppercase()));
        let result = view.display("hello world", None).unwrap();
        assert_eq!(result, "HELLO WORLD");
    }

    #[test]
    fn test_clear_filter() {
        let view = make_view();
        view.set_filter(Arc::new(|content: &str| content.to_uppercase()));
        view.clear_filter();
        let result = view.display("hello world", None).unwrap();
        assert_eq!(result, "hello world");
    }

    // =========================================================================
    // 组 11：数值/布尔/数组变量（对齐 PHP echo 类型转换）
    // =========================================================================

    #[test]
    fn test_integer_var() {
        let view = make_view();
        let result = view
            .display(
                "Count: {$count}",
                Some(ViewData::from([("count".to_string(), json!(42))])),
            )
            .unwrap();
        assert_eq!(result, "Count: 42");
    }

    #[test]
    fn test_boolean_true() {
        // PHP: echo true → "1"
        let view = make_view();
        let result = view
            .display(
                "Flag: {$flag}",
                Some(ViewData::from([("flag".to_string(), json!(true))])),
            )
            .unwrap();
        assert_eq!(result, "Flag: 1");
    }

    #[test]
    fn test_boolean_false() {
        // PHP: echo false → ""
        let view = make_view();
        let result = view
            .display(
                "Flag: {$flag}",
                Some(ViewData::from([("flag".to_string(), json!(false))])),
            )
            .unwrap();
        assert_eq!(result, "Flag: ");
    }

    #[test]
    fn test_float_var() {
        let view = make_view();
        let result = view
            .display(
                "Float: {$f}",
                Some(ViewData::from([("f".to_string(), json!(2.5))])),
            )
            .unwrap();
        assert_eq!(result, "Float: 2.5");
    }

    #[test]
    fn test_float_integer_value() {
        // PHP: echo 3.0 → "3"
        let view = make_view();
        let result = view
            .display(
                "Num: {$num}",
                Some(ViewData::from([("num".to_string(), json!(3.0))])),
            )
            .unwrap();
        assert_eq!(result, "Num: 3");
    }

    // =========================================================================
    // 组 12：ViewConfig 配置（对齐 PHP config/view.php）
    // =========================================================================

    #[test]
    fn test_config_default() {
        let config = ViewConfig::default();
        assert_eq!(config.view_suffix, "html");
        assert_eq!(config.tpl_begin, "{");
        assert_eq!(config.tpl_end, "}");
        assert_eq!(config.default_filter, "htmlentities");
        assert_eq!(config.tpl_var_identify, "array");
        assert!(!config.layout_on);
    }

    #[test]
    fn test_config_get_config() {
        let engine = SimpleTemplateEngine::new(ViewConfig::default());
        assert_eq!(
            engine.get_config("view_suffix"),
            Some(Value::String("html".to_string()))
        );
        assert_eq!(
            engine.get_config("tpl_begin"),
            Some(Value::String("{".to_string()))
        );
        assert_eq!(engine.get_config("nonexistent"), None);
    }

    // =========================================================================
    // 组 13：自定义函数注册（对齐 PHP Template::extend）
    // =========================================================================

    #[test]
    fn test_register_custom_function() {
        let view = make_view();
        // 注册自定义函数
        if let Some(engine) = view
            .engine()
            .as_any()
            .downcast_ref::<SimpleTemplateEngine>()
        {
            engine.register_function(
                "greet",
                Arc::new(|args: &[Value]| {
                    let name = args.first().and_then(|v| v.as_str()).unwrap_or("World");
                    Ok(Value::String(format!("Hello, {}!", name)))
                }),
            );
        }
        let result = view.display("{:greet('Alice')}", None).unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    // =========================================================================
    // 组 14：模板路径解析（对齐 PHP parseTemplateFile）
    // =========================================================================

    #[test]
    fn test_parse_template_path_relative() {
        let engine = SimpleTemplateEngine::new(ViewConfig::default());
        let path = engine.parse_template_path("index");
        assert_eq!(path, PathBuf::from("view/index.html"));
    }

    #[test]
    fn test_parse_template_path_with_extension() {
        let engine = SimpleTemplateEngine::new(ViewConfig::default());
        let path = engine.parse_template_path("index.html");
        assert_eq!(path, PathBuf::from("view/index.html"));
    }

    #[test]
    fn test_parse_template_path_absolute() {
        let engine = SimpleTemplateEngine::new(ViewConfig::default());
        let path = engine.parse_template_path("/absolute/path");
        assert_eq!(path, PathBuf::from("absolute/path.html"));
    }

    #[test]
    fn test_parse_template_path_cross_app() {
        // 对齐 PHP: app@template
        let engine = SimpleTemplateEngine::new(ViewConfig::default());
        let path = engine.parse_template_path("admin@dashboard");
        assert_eq!(path, PathBuf::from("admin/view/dashboard.html"));
    }

    #[test]
    fn test_parse_template_path_empty() {
        let engine = SimpleTemplateEngine::new(ViewConfig::default());
        let path = engine.parse_template_path("");
        assert_eq!(path, PathBuf::from("view/index.html"));
    }

    // =========================================================================
    // 组 15：辅助函数测试
    // =========================================================================

    #[test]
    fn test_htmlentities_basic() {
        assert_eq!(htmlentities("<b>"), "&lt;b&gt;");
        assert_eq!(htmlentities("\"quote\""), "&quot;quote&quot;");
        assert_eq!(htmlentities("'apos'"), "&#039;apos&#039;");
        assert_eq!(htmlentities("&amp;"), "&amp;amp;");
    }

    #[test]
    fn test_is_truthy() {
        assert!(!is_truthy(&Value::Null));
        assert!(!is_truthy(&Value::Bool(false)));
        assert!(is_truthy(&Value::Bool(true)));
        assert!(!is_truthy(&json!(0)));
        assert!(is_truthy(&json!(1)));
        assert!(!is_truthy(&json!("")));
        assert!(!is_truthy(&json!("0")));
        assert!(is_truthy(&json!("hello")));
        assert!(!is_truthy(&json!([])));
        assert!(is_truthy(&json!([1, 2])));
        assert!(!is_truthy(&json!({})));
        assert!(is_truthy(&json!({"a": 1})));
    }

    #[test]
    fn test_value_to_string() {
        assert_eq!(value_to_string(&Value::Null), "");
        assert_eq!(value_to_string(&Value::Bool(true)), "1");
        assert_eq!(value_to_string(&Value::Bool(false)), "");
        assert_eq!(value_to_string(&json!(42)), "42");
        assert_eq!(value_to_string(&json!(2.5)), "2.5");
        assert_eq!(value_to_string(&json!(3.0)), "3");
        assert_eq!(value_to_string(&json!("hello")), "hello");
    }

    #[test]
    fn test_parse_literal() {
        assert_eq!(parse_literal("'string'"), Value::String("string".into()));
        assert_eq!(parse_literal("\"double\""), Value::String("double".into()));
        assert_eq!(parse_literal("42"), json!(42));
        assert_eq!(parse_literal("2.5"), json!(2.5));
        assert_eq!(parse_literal("true"), Value::Bool(true));
        assert_eq!(parse_literal("false"), Value::Bool(false));
        assert_eq!(parse_literal("null"), Value::Null);
    }

    #[test]
    fn test_split_args() {
        assert_eq!(split_args("a, b, c"), vec!["a", "b", "c"]);
        assert_eq!(split_args("'a,b', c"), vec!["'a,b'", "c"]);
        assert_eq!(split_args("\"a,b\", c"), vec!["\"a,b\"", "c"]);
        assert_eq!(split_args(""), Vec::<String>::new());
    }

    #[test]
    fn test_parse_func_call() {
        let (name, args) = parse_func_call("date('Y')").unwrap();
        assert_eq!(name, "date");
        assert_eq!(args, vec![Value::String("Y".into())]);

        let (name, args) = parse_func_call("now()").unwrap();
        assert_eq!(name, "now");
        assert!(args.is_empty());

        let (name, _args) = parse_func_call("noargs").unwrap();
        assert_eq!(name, "noargs");
    }

    // =========================================================================
    // 组 16：模板渲染兜底场景（ViewFallback + render_*_response）
    // =========================================================================

    /// 辅助函数：提取 Response 的 body 为 String（异步）
    async fn extract_body_string(resp: axum::response::Response) -> String {
        use http_body_util::BodyExt;
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn test_view_fallback_new() {
        // ViewFallback::new 创建测试
        let view = View::with_default_engine();
        let fallback = ViewFallback::new(view);
        assert!(!fallback.view().has_var("any"));
    }

    #[test]
    fn test_view_fallback_with_default_engine() {
        // ViewFallback::with_default_engine 创建测试
        let fallback = ViewFallback::with_default_engine();
        assert!(!fallback.view().has_var("any"));
    }

    #[test]
    fn test_view_fallback_with_config() {
        // ViewFallback::with_config 创建测试
        let config = ViewConfig {
            view_suffix: "tpl".to_string(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);
        let view = fallback.view();
        // 验证配置传递
        let engine = view.engine();
        assert_eq!(
            engine.get_config("view_suffix"),
            Some(Value::String("tpl".to_string()))
        );
    }

    #[test]
    fn test_view_fallback_assign() {
        // ViewFallback::assign 赋值测试
        let fallback = ViewFallback::with_default_engine();
        fallback.assign("name", json!("Alice"));
        assert_eq!(fallback.view().get_var("name"), Some(json!("Alice")));
    }

    #[test]
    fn test_view_fallback_assign_many() {
        // ViewFallback::assign_many 批量赋值测试
        let fallback = ViewFallback::with_default_engine();
        let mut vars = ViewData::new();
        vars.insert("a".to_string(), json!(1));
        vars.insert("b".to_string(), json!(2));
        fallback.assign_many(vars);
        assert_eq!(fallback.view().get_var("a"), Some(json!(1)));
        assert_eq!(fallback.view().get_var("b"), Some(json!(2)));
    }

    #[test]
    fn test_view_fallback_assign_chain() {
        // ViewFallback::assign 链式调用测试
        let fallback = ViewFallback::with_default_engine();
        fallback
            .assign("a", json!(1))
            .assign("b", json!(2))
            .assign("c", json!(3));
        assert_eq!(fallback.view().get_var("a"), Some(json!(1)));
        assert_eq!(fallback.view().get_var("b"), Some(json!(2)));
        assert_eq!(fallback.view().get_var("c"), Some(json!(3)));
    }

    #[test]
    fn test_view_fallback_clear_vars() {
        // ViewFallback::clear_vars 清除变量测试
        let fallback = ViewFallback::with_default_engine();
        fallback.assign("name", json!("Alice"));
        assert!(fallback.view().has_var("name"));
        fallback.clear_vars();
        assert!(!fallback.view().has_var("name"));
    }

    #[test]
    fn test_view_fallback_display_to_string() {
        // ViewFallback::display_to_string 渲染字符串为字符串测试
        // 对齐 PHP: $this->display('Hello {$name}!', ['name' => 'World'])
        let fallback = ViewFallback::with_default_engine();
        let result = fallback
            .display_to_string(
                "Hello {$name}!",
                Some(ViewData::from([("name".to_string(), json!("World"))])),
            )
            .unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_view_fallback_display_to_string_with_assign() {
        // ViewFallback::display_to_string 配合 assign 测试
        let fallback = ViewFallback::with_default_engine();
        fallback.assign("name", json!("Alice"));
        let result = fallback.display_to_string("Hello {$name}!", None).unwrap();
        assert_eq!(result, "Hello Alice!");
    }

    #[tokio::test]
    async fn test_view_fallback_render_display_response() {
        // ViewFallback::render_display 渲染字符串为 HTML Response 测试
        // 对齐 PHP: $this->display($content) + Response::create($content, 'html')
        let fallback = ViewFallback::with_default_engine();
        let resp = fallback
            .render_display(
                "<h1>Hello {$name}!</h1>",
                Some(ViewData::from([("name".to_string(), json!("World"))])),
            )
            .unwrap();

        // 验证 HTTP 状态码 200
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // 验证 Content-Type: text/html; charset=utf-8
        let content_type = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(content_type, "text/html; charset=utf-8");

        // 验证 body 内容
        let body = extract_body_string(resp).await;
        assert_eq!(body, "<h1>Hello World!</h1>");
    }

    #[tokio::test]
    async fn test_view_fallback_render_display_with_assign() {
        // ViewFallback::render_display 配合 assign 测试
        let fallback = ViewFallback::with_default_engine();
        fallback.assign("title", json!("Report"));
        let resp = fallback
            .render_display("<title>{$title}</title>", None)
            .unwrap();
        let body = extract_body_string(resp).await;
        assert_eq!(body, "<title>Report</title>");
    }

    #[tokio::test]
    async fn test_view_fallback_render_template_file() {
        // ViewFallback::render_template 渲染模板文件为 HTML Response 测试
        // 对齐 PHP: $this->fetch('template') + Response::create($content, 'html')
        let dir = make_temp_dir();
        write_template(&dir, "pdf_template", "<pdf>{$content}</pdf>");
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        let resp = fallback
            .render_template(
                "pdf_template",
                Some(ViewData::from([(
                    "content".to_string(),
                    json!("Hello PDF"),
                )])),
            )
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let content_type = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(content_type, "text/html; charset=utf-8");
        let body = extract_body_string(resp).await;
        assert_eq!(body, "<pdf>Hello PDF</pdf>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_view_fallback_render_template_not_found() {
        // ViewFallback::render_template 模板未找到错误测试
        // 对齐 PHP: TemplateNotFoundException
        let dir = make_temp_dir();
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        let result = fallback.render_template("nonexistent", None);
        assert!(result.is_err());
        match result {
            Err(ViewError::TemplateNotFound(_)) => {}
            Err(e) => panic!("Expected TemplateNotFound, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }

        cleanup_dir(&dir);
    }

    #[test]
    fn test_view_fallback_render_to_string() {
        // ViewFallback::render_to_string 渲染模板文件为字符串测试
        // 用于 PDF/Excel 生成器输入
        let dir = make_temp_dir();
        write_template(
            &dir,
            "excel_template",
            "<table><tr><td>{$value}</td></tr></table>",
        );
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        let html = fallback
            .render_to_string(
                "excel_template",
                Some(ViewData::from([("value".to_string(), json!(42))])),
            )
            .unwrap();
        assert_eq!(html, "<table><tr><td>42</td></tr></table>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_view_fallback_render_to_string_not_found() {
        // ViewFallback::render_to_string 模板未找到错误测试
        let dir = make_temp_dir();
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        let result = fallback.render_to_string("nonexistent", None);
        assert!(result.is_err());

        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn test_render_template_response_free_function() {
        // 自由函数 render_template_response 测试
        let dir = make_temp_dir();
        write_template(&dir, "report", "<report>{$title}</report>");
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let view = View::with_config(config);

        let resp = render_template_response(
            &view,
            "report",
            Some(ViewData::from([("title".to_string(), json!("Monthly"))])),
        )
        .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let content_type = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(content_type, "text/html; charset=utf-8");
        let body = extract_body_string(resp).await;
        assert_eq!(body, "<report>Monthly</report>");

        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn test_render_display_response_free_function() {
        // 自由函数 render_display_response 测试
        let view = View::with_default_engine();
        let resp = render_display_response(
            &view,
            "<p>{$msg}</p>",
            Some(ViewData::from([("msg".to_string(), json!("Hello"))])),
        )
        .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let content_type = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(content_type, "text/html; charset=utf-8");
        let body = extract_body_string(resp).await;
        assert_eq!(body, "<p>Hello</p>");
    }

    #[test]
    fn test_render_template_response_not_found() {
        // 自由函数 render_template_response 模板未找到错误测试
        let dir = make_temp_dir();
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let view = View::with_config(config);

        let result = render_template_response(&view, "nonexistent", None);
        assert!(result.is_err());

        cleanup_dir(&dir);
    }

    #[test]
    fn test_view_fallback_pdf_export_scenario() {
        // R5 PHP/Rust 行为对比测试：PDF 导出场景
        // 对齐 PHP pdf-pdftk 表单填充场景：渲染 HTML 模板作为 PDF 输入
        let dir = make_temp_dir();
        write_template(
            &dir,
            "payment_pdf",
            r#"<html><body><h1>付款单 {$payment_id}</h1><p>金额: {$amount}</p></body></html>"#,
        );
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        // 模拟 PDF 导出场景：渲染模板为字符串，传给 PDF 生成器
        let html = fallback
            .render_to_string(
                "payment_pdf",
                Some(ViewData::from([
                    ("payment_id".to_string(), json!("PAY-001")),
                    ("amount".to_string(), json!("¥1,234.56")),
                ])),
            )
            .unwrap();

        assert_eq!(
            html,
            r#"<html><body><h1>付款单 PAY-001</h1><p>金额: ¥1,234.56</p></body></html>"#
        );

        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn test_view_fallback_excel_export_scenario() {
        // R5 PHP/Rust 行为对比测试：Excel 导出场景
        // 对齐 PHP PhpSpreadsheet 场景：渲染 HTML 表格作为 Excel 输入
        let dir = make_temp_dir();
        write_template(
            &dir,
            "order_excel",
            r#"<table><tr><th>订单号</th><th>金额</th></tr><tr><td>{$order_no}</td><td>{$amount}</td></tr></table>"#,
        );
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        // 模拟 Excel 导出场景：渲染模板为 HTML Response
        let resp = fallback
            .render_template(
                "order_excel",
                Some(ViewData::from([
                    ("order_no".to_string(), json!("ORD-2026-001")),
                    ("amount".to_string(), json!(99.50)),
                ])),
            )
            .unwrap();

        let body = extract_body_string(resp).await;
        assert!(body.contains("<th>订单号</th>"));
        assert!(body.contains("<td>ORD-2026-001</td>"));
        assert!(body.contains("<td>99.5</td>"));

        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn test_view_fallback_email_scenario() {
        // R5 PHP/Rust 行为对比测试：邮件内容渲染场景
        let fallback = ViewFallback::with_default_engine();
        let resp = fallback
            .render_display(
                r#"<html><body><h2>Dear {$name}</h2><p>Your order #{$order_id} has been shipped.</p></body></html>"#,
                Some(ViewData::from([
                    ("name".to_string(), json!("Alice")),
                    ("order_id".to_string(), json!(12345)),
                ])),
            )
            .unwrap();

        let body = extract_body_string(resp).await;
        assert!(body.contains("Dear Alice"));
        assert!(body.contains("#12345"));
        assert!(body.contains("has been shipped"));
    }

    #[test]
    fn test_view_fallback_content_type_header() {
        // 验证 HTML Response 的 Content-Type 头正确设置
        // 对齐 PHP Response::create($content, 'html') 的 Content-Type: text/html
        let fallback = ViewFallback::with_default_engine();
        let resp = fallback.render_display("<p>test</p>", None).unwrap();

        let content_type = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        // Rust 版本增加 charset=utf-8（PHP 默认不设置 charset）
        assert!(content_type.starts_with("text/html"));
        assert!(content_type.contains("charset=utf-8"));
    }

    #[test]
    fn test_view_fallback_http_status() {
        // 验证 HTML Response 的 HTTP 状态码为 200
        // 对齐 PHP Response::create($content, 'html', 200)
        let fallback = ViewFallback::with_default_engine();
        let resp = fallback.render_display("<html></html>", None).unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[test]
    fn test_view_fallback_with_layout() {
        // ViewFallback + 布局集成测试
        // 对齐 PHP layout_on=true 场景下的模板渲染
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        write_template(&dir, "page", "<p>{$content}</p>");
        let config = ViewConfig {
            view_path: dir.clone(),
            layout_on: true,
            layout_name: "layout".to_string(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        let html = fallback
            .render_to_string(
                "page",
                Some(ViewData::from([("content".to_string(), json!("Hello"))])),
            )
            .unwrap();
        assert_eq!(html, "<html><body><p>Hello</p></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_view_fallback_with_inheritance() {
        // ViewFallback + 模板继承集成测试
        // 对齐 PHP {extend name="..."} 场景下的模板渲染
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name='content'}default{/block}</html>",
        );
        write_template(
            &dir,
            "child",
            "{extend name='base'}{block name='content'}{$msg}{/block}",
        );
        let config = ViewConfig {
            view_path: dir.clone(),
            ..Default::default()
        };
        let fallback = ViewFallback::with_config(config);

        let html = fallback
            .render_to_string(
                "child",
                Some(ViewData::from([(
                    "msg".to_string(),
                    json!("Hello Inheritance"),
                )])),
            )
            .unwrap();
        assert_eq!(html, "<html>Hello Inheritance</html>");

        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn test_view_fallback_complex_template() {
        // ViewFallback 复杂模板渲染测试（变量 + 过滤器 + 三元表达式）
        let fallback = ViewFallback::with_default_engine();
        let template = r#"<div class="user">
  <span>{$name|upper}</span>
  <span>{$email|default='N/A'}</span>
  <span>{$active?='启用':'禁用'}</span>
</div>"#;
        let resp = fallback
            .render_display(
                template,
                Some(ViewData::from([
                    ("name".to_string(), json!("alice")),
                    ("active".to_string(), json!(true)),
                ])),
            )
            .unwrap();
        let body = extract_body_string(resp).await;
        assert!(body.contains("ALICE"));
        assert!(body.contains("N/A"));
        assert!(body.contains("启用"));
    }
}

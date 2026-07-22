//! 模板布局 — 对齐 PHP `think\Template` 的布局（layout）机制
//!
//! Phase 7.3 核心交付物。实现对齐 PHP `Template::compiler()` 和 `Template::parseLayout()`
//! 的布局功能。
//!
//! ## PHP 对齐说明
//!
//! ### 配置模式（对齐 `compiler()` 中的 `layout_on` 检查）
//! - `layout_on=true` + `layout_name='layout'` + `layout_item='{__CONTENT__}'`
//! - 编译时自动读取布局模板，将 `{__CONTENT__}` 替换为当前模板内容
//! - `{__NOLAYOUT__}` 标签可单独禁用布局
//!
//! ### 标签模式（对齐 `parseLayout()`）
//! - `{layout name="layout" replace="{__CONTENT__}"}`
//! - `parseLayout()` 解析标签并替换
//! - 如果 `layout_on=true` 且 `layout_name` == 标签 `name`，则跳过（已由 `compiler()` 处理）
//!
//! ### PHP 解析顺序
//! 1. `compiler()` — 配置模式布局（在 `parse()` 之前）
//! 2. `parse()`:
//!    - `parseLiteral` — 暂存 literal
//!    - `parseExtend` — 继承
//!    - `parseLayout` — 标签模式布局
//!    - `parseInclude` — 包含
//!    - ...
//!
//! ### PHP `str_replace` 行为
//! - `str_replace($search, $replace, $subject)` — 单次遍历，非递归
//! - 如果 `$replace` 包含 `$search`，不会再次替换
//! - Rust `String::replace` 行为一致
//!
//! ## PHP 源码参考
//! - `think-template\src\Template.php` — `compiler()` (389-432), `parseLayout()` (529-551),
//!   `layout()` (316-336), `getRegex('layout')` (1306-1319), `parseAttr()` (816-833),
//!   `parseTemplateFile()` (1238-1259)

use std::path::PathBuf;

use regex::Regex;

use super::{ViewConfig, ViewError};

// ============================================================================
// 公共入口
// ============================================================================

/// 应用布局（对齐 PHP `compiler()` + `parseLayout()`）
///
/// PHP 流程：
/// 1. `compiler()` 检查 `layout_on`，如果开启则读取布局模板并替换 `{__CONTENT__}`
/// 2. `parseLayout()` 检查 `{layout}` 标签，如果存在且未被 `compiler()` 处理则应用
///
/// Rust 合并为单函数，按 PHP 顺序执行：
/// 1. 应用配置布局（`layout_on=true`），处理 `{__NOLAYOUT__}`
/// 2. 解析 `{layout}` 标签（对齐 `parseLayout()`）
///
/// # 参数
/// - `content` — 模板内容
/// - `config` — 视图配置
///
/// # 返回
/// 应用布局后的内容
///
/// # 错误
/// - 如果 `layout_on=true` 且布局文件不存在，返回 `ViewError::TemplateNotFound`
/// - 如果 `{layout}` 标签指定的布局文件不存在，返回 `ViewError::TemplateNotFound`
pub fn apply_layout(content: &str, config: &ViewConfig) -> Result<String, ViewError> {
    let mut result = content.to_string();

    // 1. 应用配置布局（对齐 PHP `compiler()` 中的 `layout_on` 检查）
    if config.layout_on {
        if result.contains("{__NOLAYOUT__}") {
            // 单独禁用布局（对齐 PHP `compiler()` 中的 `{__NOLAYOUT__}` 检查）
            result = result.replace("{__NOLAYOUT__}", "");
        } else {
            // 读取布局模板并替换
            let layout_file = resolve_template_path(&config.layout_name, config);
            if !layout_file.is_file() {
                return Err(ViewError::TemplateNotFound(format!(
                    "布局模板: {} (解析路径: {})",
                    config.layout_name,
                    layout_file.display()
                )));
            }
            let layout_content = std::fs::read_to_string(&layout_file)?;
            // 对齐 PHP: str_replace(layout_item, content, layout_content)
            // 搜索 layout_item，替换为 result，目标为 layout_content
            result = layout_content.replace(&config.layout_item, &result);
        }
    } else {
        // 对齐 PHP `compiler()` else 分支：移除 {__NOLAYOUT__}
        result = result.replace("{__NOLAYOUT__}", "");
    }

    // 2. 解析 {layout} 标签（对齐 PHP `parseLayout()`）
    result = parse_layout_tag(&result, config)?;

    Ok(result)
}

// ============================================================================
// 内部实现
// ============================================================================

/// 解析 `{layout name="..." replace="..."}` 标签（对齐 PHP `parseLayout()`）
///
/// PHP `parseLayout()` 逻辑：
/// 1. 用 `preg_match`（单匹配）查找 `{layout}` 标签
/// 2. 如果找到：
///    - 移除 `{layout}` 标签
///    - 解析属性（`name` 必填，`replace` 可选）
///    - 如果 `layout_on=false` 或 `layout_name != tag.name`：
///      - 读取布局模板文件
///      - 替换 `replace`（或默认 `layout_item`）为内容
/// 3. 如果未找到：移除 `{__NOLAYOUT__}`（对齐 PHP else 分支）
///
/// # PHP 正则限制
/// PHP `getRegex('layout')` 要求 `name=` 存在且值非空。
/// Rust `regex` 不支持 lookahead `(?!name=)`，采用两步法：
/// 1. 先匹配 `{layout ...}` 任意标签
/// 2. 再解析 `name` 属性，若缺失或为空则视为无匹配
fn parse_layout_tag(content: &str, config: &ViewConfig) -> Result<String, ViewError> {
    let begin = regex::escape(&config.taglib_begin);
    let end = regex::escape(&config.taglib_end);

    // 匹配 {layout ...} 标签（对齐 PHP getRegex('layout')，简化为不使用 lookahead）
    // 对齐 PHP: 标签内容允许任意非 end 字符（包括 begin 字符 {）
    // PHP regex: (?>(?:(?!end).)*) — 仅排除 end，不排除 begin
    // 例：{layout name="x" replace="{__BODY__}"} 会匹配到第一个 }（即 {__BODY__} 的 }）
    let pattern = format!(r"{}layout\b\s+([^}}]+){}", begin, end);
    let re = Regex::new(&pattern).map_err(|e| ViewError::SyntaxError(e.to_string()))?;

    if let Some(caps) = re.captures(content) {
        let full_match = caps.get(0).unwrap();
        let tag_name = parse_attr(full_match.as_str(), "name");

        // 如果 name 属性缺失或为空，视为无 {layout} 标签
        // （对齐 PHP regex 要求 name 值至少 1 字符：[\$\w\-\/\.\:@,\\]+）
        let tag_name = match tag_name {
            Some(name) if !name.is_empty() => name,
            _ => {
                // 无有效 {layout} 标签，移除 {__NOLAYOUT__}（对齐 PHP else 分支）
                return Ok(content.replace("{__NOLAYOUT__}", ""));
            }
        };

        // 移除 {layout} 标签（对齐 PHP str_replace($matches[0], '', $content)）
        let result = content.replace(full_match.as_str(), "");

        // 如果配置布局已开启且名称匹配，则跳过
        // 对齐 PHP: if (!$this->config['layout_on'] || $this->config['layout_name'] != $array['name'])
        if config.layout_on && config.layout_name == tag_name {
            return Ok(result);
        }

        // 解析 replace 属性
        // 对齐 PHP: isset($array['replace']) ? $array['replace'] : $this->config['layout_item']
        let replace = parse_attr(full_match.as_str(), "replace")
            .unwrap_or_else(|| config.layout_item.clone());

        // 读取布局模板文件（对齐 PHP parseTemplateFile）
        let layout_file = resolve_template_path(&tag_name, config);
        if !layout_file.is_file() {
            return Err(ViewError::TemplateNotFound(format!(
                "布局模板: {} (解析路径: {})",
                tag_name,
                layout_file.display()
            )));
        }
        let layout_content = std::fs::read_to_string(&layout_file)?;

        // 替换布局主体内容
        // 对齐 PHP: str_replace($replace, $content, file_get_contents($layoutFile))
        // 搜索 replace，替换为 result，目标为 layout_content
        Ok(layout_content.replace(&replace, &result))
    } else {
        // 无 {layout} 标签，移除 {__NOLAYOUT__}（对齐 PHP parseLayout() else 分支）
        Ok(content.replace("{__NOLAYOUT__}", ""))
    }
}

/// 解析标签属性（对齐 PHP `parseAttr`）
///
/// PHP 正则: `/\s+(?>(?P<name>[\w-]+)\s*)=(?>\s*)([\"\'])(?P<value>(?:(?!\2).)*)\2/is`
///
/// PHP `parseAttr` 返回所有属性的 map，此处简化为返回指定属性名的值。
///
/// # 限制
/// Rust `regex` 不支持 backreference `\1`，因此 `[^"']*` 同时排除两种引号。
/// PHP 仅排除匹配的引号（允许 value 中包含另一种引号）。
/// 在实际模板中，属性值几乎不包含引号，此限制可忽略。
fn parse_attr(tag: &str, attr_name: &str) -> Option<String> {
    // 匹配 attr_name="value" 或 attr_name='value'
    // 对齐 PHP parseAttr 的 \s+(name)\s*=\s*(["'])value\2
    let pattern = format!(r#"{}\s*=\s*["']([^"']*)["']"#, regex::escape(attr_name));
    let re = Regex::new(&pattern).ok()?;
    re.captures(tag).map(|c| c[1].to_string())
}

/// 解析模板文件路径（对齐 PHP `parseTemplateFile`）
///
/// PHP 规则：
/// 1. 如果无扩展名：
///    - 首字符 `/` → 去掉 `/`，替换 `/`/`:` 为 `view_depr`
///    - 否则 → 替换 `/`/`:` 为 `view_depr`
///    - 拼接 `view_path` + normalized + `.suffix`（字符串拼接，非 with_extension）
/// 2. 如果有扩展名 → 直接使用
fn resolve_template_path(name: &str, config: &ViewConfig) -> PathBuf {
    // 检查是否已有扩展名（对齐 PHP pathinfo PATHINFO_EXTENSION）
    if std::path::Path::new(name).extension().is_some() {
        return PathBuf::from(name);
    }

    // 去掉首字符 `/`（对齐 PHP substr($template, 1)）
    let name = name.strip_prefix('/').unwrap_or(name);

    // 替换分隔符（对齐 PHP str_replace(['/', ':'], view_depr, $template)）
    let normalized = name.replace(['/', ':'], &config.view_depr);

    // 拼接 view_path + normalized + .suffix（对齐 PHP 字符串拼接）
    // 不使用 PathBuf::with_extension，因为 with_extension 会替换最后一个扩展名
    // 例：view_depr='.' 时 normalized='admin.layout'，with_extension('html') 会得到 'admin.html'
    // PHP 行为：view_path + 'admin.layout' + '.html' = 'view_path/admin.layout.html'
    let suffix = config.view_suffix.trim_start_matches('.');
    let file_name = format!("{}.{}", normalized, suffix);
    config.view_path.join(file_name)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // =========================================================================
    // 辅助函数
    // =========================================================================

    /// 创建临时目录
    fn make_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sz_rust_layout_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 写入模板文件
    fn write_template(dir: &std::path::Path, name: &str, content: &str) {
        let path = dir.join(format!("{}.html", name));
        std::fs::write(&path, content).unwrap();
    }

    /// 清理临时目录
    fn cleanup_dir(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 创建默认配置（layout_on=false）
    fn make_config(view_path: PathBuf) -> ViewConfig {
        ViewConfig {
            view_path,
            ..Default::default()
        }
    }

    /// 创建 layout_on=true 配置
    fn make_config_layout_on(view_path: PathBuf) -> ViewConfig {
        ViewConfig {
            view_path,
            layout_on: true,
            ..Default::default()
        }
    }

    // =========================================================================
    // 组 1：配置模式布局（对齐 PHP compiler() layout_on=true）
    // =========================================================================

    #[test]
    fn test_config_layout_on_basic() {
        // 对齐 PHP: layout_on=true，内容替换 {__CONTENT__}
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_layout_on_with_nolayout() {
        // 对齐 PHP: layout_on=true + {__NOLAYOUT__} → 跳过布局
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("{__NOLAYOUT__}<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_layout_on_custom_name() {
        // 对齐 PHP: layout_name="custom"
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<div class=\"wrapper\">{__CONTENT__}</div>");
        let config = ViewConfig {
            view_path: dir.clone(),
            layout_on: true,
            layout_name: "custom".to_string(),
            ..Default::default()
        };

        let result = apply_layout("<p>Content</p>", &config).unwrap();
        assert_eq!(result, "<div class=\"wrapper\"><p>Content</p></div>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_layout_on_custom_item() {
        // 对齐 PHP: layout_item="{__BODY__}"
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__BODY__}</body></html>");
        let config = ViewConfig {
            view_path: dir.clone(),
            layout_on: true,
            layout_item: "{__BODY__}".to_string(),
            ..Default::default()
        };

        let result = apply_layout("<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_layout_on_file_not_found() {
        // 对齐 PHP: parseTemplateFile 抛出异常
        let dir = make_temp_dir();
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("<h1>Hello</h1>", &config);
        assert!(matches!(result, Err(ViewError::TemplateNotFound(_))));

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 2：配置关闭布局（对齐 PHP compiler() layout_on=false）
    // =========================================================================

    #[test]
    fn test_config_layout_off_basic() {
        // 对齐 PHP: layout_on=false → 不应用布局
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout("<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_layout_off_with_nolayout() {
        // 对齐 PHP: layout_on=false → 移除 {__NOLAYOUT__}
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout("{__NOLAYOUT__}<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 3：标签模式布局（对齐 PHP parseLayout()）
    // =========================================================================

    #[test]
    fn test_tag_layout_basic() {
        // 对齐 PHP: {layout name="custom" /}
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config(dir.clone());

        let result = apply_layout(r#"{layout name="custom" /}<h1>Hello</h1>"#, &config).unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_tag_layout_without_self_closing() {
        // 对齐 PHP: {layout name="custom"}（无 / 自闭合）
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config(dir.clone());

        let result = apply_layout(r#"{layout name="custom"}<h1>Hello</h1>"#, &config).unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_tag_layout_custom_replace() {
        // 对齐 PHP: {layout name="custom" replace="BODY"}
        // 注：replace 值不能含 }（PHP regex 以 } 作为标签结束符，会提前终止匹配）
        // PHP 行为：{layout name="x" replace="{__BODY__}"} 会匹配到第一个 }，
        // 导致 replace 值被截断为 {__BODY__（缺少 }），无法匹配布局中的 {__BODY__}
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>BODY</body></html>");
        let config = make_config(dir.clone());

        let result = apply_layout(
            r#"{layout name="custom" replace="BODY"}<h1>Hello</h1>"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_tag_layout_replace_with_brace_php_bug() {
        // R5 PHP/Rust 行为对比：复刻 PHP 源码 bug
        // PHP regex 以 } 作为标签结束符，replace 值含 } 会提前终止匹配
        // 输入：{layout name="custom" replace="{__BODY__}"}<h1>Hello</h1>
        // PHP 匹配：{layout name="custom" replace="{__BODY__}（到第一个 }）
        // PHP parseAttr: replace="{__BODY__ 缺少闭合引号 "，不匹配 → replace 属性解析失败
        // PHP 回退到 layout_item ({__CONTENT__})，但布局中无 {__CONTENT__}，不替换
        // 结果：布局原样输出，内容被丢弃
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__BODY__}</body></html>");
        let config = make_config(dir.clone());

        let result = apply_layout(
            r#"{layout name="custom" replace="{__BODY__}"}<h1>Hello</h1>"#,
            &config,
        )
        .unwrap();
        // 对齐 PHP: 标签匹配到第一个 }，replace 属性解析失败（无闭合引号）
        // 回退到 layout_item ({__CONTENT__})，布局中无此标记，原样输出
        assert_eq!(result, "<html><body>{__BODY__}</body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_tag_layout_file_not_found() {
        // 对齐 PHP: parseTemplateFile 抛出异常
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout(r#"{layout name="nonexistent" /}<h1>Hello</h1>"#, &config);
        assert!(matches!(result, Err(ViewError::TemplateNotFound(_))));

        cleanup_dir(&dir);
    }

    #[test]
    fn test_tag_layout_missing_name_attr() {
        // 对齐 PHP: regex 要求 name= 存在，否则不匹配
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config(dir.clone());

        // 无 name 属性 → 视为无 {layout} 标签
        let result =
            apply_layout(r#"{layout replace="{__CONTENT__}"}<h1>Hello</h1>"#, &config).unwrap();
        // {layout} 标签未被移除（因为不匹配），内容原样输出
        assert_eq!(result, r#"{layout replace="{__CONTENT__}"}<h1>Hello</h1>"#);

        cleanup_dir(&dir);
    }

    #[test]
    fn test_tag_layout_empty_name() {
        // 对齐 PHP: regex 要求 name 值至少 1 字符
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout(r#"{layout name="" /}<h1>Hello</h1>"#, &config).unwrap();
        // name 为空 → 视为无 {layout} 标签
        assert_eq!(result, r#"{layout name="" /}<h1>Hello</h1>"#);

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 4：配置 + 标签交互（对齐 PHP compiler() + parseLayout()）
    // =========================================================================

    #[test]
    fn test_config_on_tag_same_name_skip() {
        // 对齐 PHP: layout_on=true + {layout name="layout"} → 标签跳过
        // compiler() 已应用配置布局，parseLayout() 检查 layout_name==tag.name 则跳过
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout(r#"{layout name="layout" /}<h1>Hello</h1>"#, &config).unwrap();
        // 配置布局应用，{layout} 标签被移除，标签布局跳过
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_on_tag_different_name_double_layout() {
        // 对齐 PHP: layout_on=true + {layout name="other"} → 双重布局
        // compiler() 应用配置布局，parseLayout() 再应用标签布局
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        write_template(&dir, "other", "<wrapper>{__CONTENT__}</wrapper>");
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout(r#"{layout name="other" /}<h1>Hello</h1>"#, &config).unwrap();
        // 先应用配置布局：layout → <html><body>{layout name="other" /}<h1>Hello</h1></body></html>
        // 再应用标签布局：other → <wrapper><html><body><h1>Hello</h1></body></html></wrapper>
        assert_eq!(
            result,
            "<wrapper><html><body><h1>Hello</h1></body></html></wrapper>"
        );

        cleanup_dir(&dir);
    }

    #[test]
    fn test_config_off_tag_layout_applied() {
        // 对齐 PHP: layout_on=false + {layout name="custom"} → 标签布局应用
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config(dir.clone());

        let result = apply_layout(r#"{layout name="custom" /}<h1>Hello</h1>"#, &config).unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 5：{__NOLAYOUT__} 标签处理
    // =========================================================================

    #[test]
    fn test_nolayout_with_config_on() {
        // 对齐 PHP: layout_on=true + {__NOLAYOUT__} → 跳过配置布局
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("{__NOLAYOUT__}<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_nolayout_with_config_off() {
        // 对齐 PHP: layout_on=false + {__NOLAYOUT__} → 移除 {__NOLAYOUT__}
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout("{__NOLAYOUT__}<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_nolayout_in_layout_file() {
        // 对齐 PHP: 布局文件中的 {__NOLAYOUT__} 被 parseLayout() else 分支移除
        let dir = make_temp_dir();
        write_template(
            &dir,
            "layout",
            "{__NOLAYOUT__}<html><body>{__CONTENT__}</body></html>",
        );
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("<h1>Hello</h1>", &config).unwrap();
        // compiler() 应用布局后，{__NOLAYOUT__} 来自布局文件
        // parseLayout() else 分支移除 {__NOLAYOUT__}
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 6：str_replace 行为对齐
    // =========================================================================

    #[test]
    fn test_str_replace_non_recursive() {
        // 对齐 PHP str_replace 非递归行为
        // content 包含 {__CONTENT__}，但替换后不再被处理
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config_layout_on(dir.clone());

        // content 中的 {__CONTENT__} 不应被再次替换
        let result = apply_layout("{__CONTENT__}<h1>Hello</h1>", &config).unwrap();
        // layout_content.replace("{__CONTENT__}", "{__CONTENT__}<h1>Hello</h1>")
        // = "<html><body>{__CONTENT__}<h1>Hello</h1></body></html>"
        // {__CONTENT__} 在替换值中不被再次替换（非递归）
        assert_eq!(
            result,
            "<html><body>{__CONTENT__}<h1>Hello</h1></body></html>"
        );

        cleanup_dir(&dir);
    }

    #[test]
    fn test_multiple_content_placeholders() {
        // 对齐 PHP str_replace 替换所有出现
        let dir = make_temp_dir();
        write_template(
            &dir,
            "layout",
            "<header>{__CONTENT__}</header><main>{__CONTENT__}</main>",
        );
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("<h1>Hello</h1>", &config).unwrap();
        assert_eq!(
            result,
            "<header><h1>Hello</h1></header><main><h1>Hello</h1></main>"
        );

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 7：模板路径解析（对齐 PHP parseTemplateFile）
    // =========================================================================

    #[test]
    fn test_resolve_path_simple() {
        let config = make_config(PathBuf::from("/view"));
        let path = resolve_template_path("layout", &config);
        assert_eq!(path, PathBuf::from("/view/layout.html"));
    }

    #[test]
    fn test_resolve_path_with_slash_prefix() {
        // 对齐 PHP: 首字符 / → 去掉 /
        let config = make_config(PathBuf::from("/view"));
        let path = resolve_template_path("/layout", &config);
        assert_eq!(path, PathBuf::from("/view/layout.html"));
    }

    #[test]
    fn test_resolve_path_nested() {
        let config = make_config(PathBuf::from("/view"));
        let path = resolve_template_path("admin/layout", &config);
        assert_eq!(path, PathBuf::from("/view/admin/layout.html"));
    }

    #[test]
    fn test_resolve_path_with_colon() {
        // 对齐 PHP: str_replace(['/', ':'], view_depr, $template)
        let config = make_config(PathBuf::from("/view"));
        let path = resolve_template_path("admin:layout", &config);
        assert_eq!(path, PathBuf::from("/view/admin/layout.html"));
    }

    #[test]
    fn test_resolve_path_with_extension() {
        // 对齐 PHP: 有扩展名 → 直接使用
        let config = make_config(PathBuf::from("/view"));
        let path = resolve_template_path("layout.tpl", &config);
        assert_eq!(path, PathBuf::from("layout.tpl"));
    }

    #[test]
    fn test_resolve_path_custom_view_depr() {
        // 对齐 PHP: view_depr 自定义分隔符
        let config = ViewConfig {
            view_path: PathBuf::from("/view"),
            view_depr: ".".to_string(),
            ..Default::default()
        };
        let path = resolve_template_path("admin/layout", &config);
        assert_eq!(path, PathBuf::from("/view/admin.layout.html"));
    }

    // =========================================================================
    // 组 8：属性解析（对齐 PHP parseAttr）
    // =========================================================================

    #[test]
    fn test_parse_attr_double_quote() {
        let val = parse_attr(r#"name="custom""#, "name");
        assert_eq!(val, Some("custom".to_string()));
    }

    #[test]
    fn test_parse_attr_single_quote() {
        let val = parse_attr(r#"name='custom'"#, "name");
        assert_eq!(val, Some("custom".to_string()));
    }

    #[test]
    fn test_parse_attr_missing() {
        let val = parse_attr(r#"other="value""#, "name");
        assert_eq!(val, None);
    }

    #[test]
    fn test_parse_attr_with_spaces() {
        let val = parse_attr(r#"name  =  "custom""#, "name");
        assert_eq!(val, Some("custom".to_string()));
    }

    #[test]
    fn test_parse_attr_multiple_attrs() {
        let tag = r#"{layout name="custom" replace="{__BODY__}"}"#;
        assert_eq!(parse_attr(tag, "name"), Some("custom".to_string()));
        assert_eq!(parse_attr(tag, "replace"), Some("{__BODY__}".to_string()));
    }

    // =========================================================================
    // 组 9：边界情况
    // =========================================================================

    #[test]
    fn test_empty_content() {
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout("", &config).unwrap();
        assert_eq!(result, "");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_empty_content_with_layout_on() {
        let dir = make_temp_dir();
        write_template(&dir, "layout", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("", &config).unwrap();
        assert_eq!(result, "<html><body></body></html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_no_layout_tag_no_nolayout() {
        // 无 {layout} 标签，无 {__NOLAYOUT__} → 原样返回
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_layout("<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_layout_tag_not_first() {
        // {layout} 标签不在内容开头
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__CONTENT__}</body></html>");
        let config = make_config(dir.clone());

        let result = apply_layout(r#"Hello {layout name="custom" /}World"#, &config).unwrap();
        // {layout} 标签被移除，剩余内容 "Hello World" 被放入布局
        assert_eq!(result, "<html><body>Hello World</body></html>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 10：自定义 taglib_begin/end
    // =========================================================================

    #[test]
    fn test_custom_taglib_delimiters() {
        // 对齐 PHP: 自定义 taglib_begin/taglib_end
        let dir = make_temp_dir();
        write_template(&dir, "custom", "<html><body>{__CONTENT__}</body></html>");
        let config = ViewConfig {
            view_path: dir.clone(),
            taglib_begin: "<".to_string(),
            taglib_end: "/>".to_string(),
            ..Default::default()
        };

        let result = apply_layout(r#"<layout name="custom" /><h1>Hello</h1>"#, &config).unwrap();
        assert_eq!(result, "<html><body><h1>Hello</h1></body></html>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 11：集成场景（布局 + 变量插值）
    // =========================================================================

    #[test]
    fn test_layout_preserves_variable_tags() {
        // 布局应用在变量解析之前，{$var} 应保留
        let dir = make_temp_dir();
        write_template(
            &dir,
            "layout",
            "<html><body>{__CONTENT__}</body><title>{$title}</title></html>",
        );
        let config = make_config_layout_on(dir.clone());

        let result = apply_layout("<h1>{$name}</h1>", &config).unwrap();
        // 布局中的 {$title} 和内容中的 {$name} 都应保留
        assert_eq!(
            result,
            "<html><body><h1>{$name}</h1></body><title>{$title}</title></html>"
        );

        cleanup_dir(&dir);
    }
}

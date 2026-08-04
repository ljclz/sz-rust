//! 模板继承 — 对齐 PHP `think\Template` 的继承（extend）机制
//!
//! 实现对齐 PHP `Template::parseExtend()` 和 `parseBlock()`
//! 的模板继承功能。
//!
//! ## PHP 对齐说明
//!
//! ### 继承机制
//! - `{extend name="base"}` — 声明继承自 base 模板
//! - `{block name="xxx"}...{/block}` — 定义 block，子模板可覆盖
//! - `{__BLOCK__}` / `{__block__}` — 合并标记，表示与父模板合并（而不是覆盖）
//!
//! ### PHP 解析顺序（`Template::parse()`）
//! 1. `parseLiteral` — 暂存 literal
//! 2. `parseExtend` — 继承
//! 3. `parseLayout` — 标签模式布局
//! 4. `parseInclude` — 包含
//!
//! ### PHP `parseExtend` 流程
//! 1. 使用 `getRegex('extend')` 匹配 `{extend name="..."}` 标签
//! 2. 递归闭包 `$func` 向上查找最顶层模板（带循环检测 `$array`）
//! 3. 最顶层模板的 block 作为 `$baseBlocks`（`parseBlock($template, true)` sort=true 按位置排序）
//! 4. 回溯过程中，子模板的 block 收集到 `$blocks`（`parseBlock($template)` sort=false）
//! 5. 合并：遍历 `$baseBlocks`，用 `$blocks` 覆盖，处理 `{__BLOCK__}` 合并标记
//!
//! ### PHP `parseBlock` 流程
//! - 使用 `getRegex('block')` 匹配 `{block name="xxx"}` 和 `{/block}` 标签
//! - **栈式匹配**（`$right` 数组）：
//!   - 遇到 `{block name="xxx"}`：压栈
//!   - 遇到 `{/block}`：弹栈，记录该 block 的 begin/content/end/parent
//!   - `parent` = 弹栈后栈顶的 name（外层 block 的 name），空字符串表示顶级
//! - 如果 `sort=true`：按 block 结束符在模板中的位置排序（`array_multisort`）
//!
//! ### PHP bug 复刻
//! - 循环继承（A extends B, B extends A）：`$array` 检测终止递归，但最终结果
//!   是最后一次设置的 `$extend`（A 内容），且 `$baseBlocks` 为空（因为 A 有
//!   `{extend}` 走的是 if 分支不是 else 分支），所以不做任何 block 替换
//!
//! ## PHP 源码参考
//! - `think-template\src\Template.php` — `parseExtend()` (599-679), `parseBlock()` (723-765),
//!   `getRegex('extend')` (1310-1319), `getRegex('block')` (1285-1291),
//!   `parseTemplateName()` (1206-1230), `parseTemplateFile()` (1238-1259)

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use indexmap::IndexMap;
use regex::Regex;

use super::{ViewConfig, ViewError};

// ============================================================================
// 公共入口
// ============================================================================

/// 应用模板继承（对齐 PHP `Template::parseExtend()`）
///
/// PHP 流程：
/// 1. 递归查找 `{extend name="..."}` 标签，向上查找最顶层模板
/// 2. 最顶层模板的 block 作为 `base_blocks`（sort=true 按位置排序）
/// 3. 子模板的 block 收集到 `blocks`
/// 4. 合并：遍历 `base_blocks`，用 `blocks` 覆盖，处理 `{__BLOCK__}` 合并标记
///
/// # 参数
/// - `content` — 模板内容
/// - `config` — 视图配置
///
/// # 返回
/// 应用继承后的内容
///
/// # 错误
/// - 如果 `{extend name="..."}` 指定的模板文件不存在，返回 `ViewError::TemplateNotFound`
pub fn apply_inheritance(content: &str, config: &ViewConfig) -> Result<String, ViewError> {
    // 对齐 PHP parseExtend() 递归闭包 $func
    let mut state = ExtendState::default();

    find_extend_recursive(content, config, &mut state)?;

    if state.extend.is_empty() {
        // 无 {extend} 标签且无 {block} 标签，直接返回原内容
        // 对齐 PHP: if (!empty($extend)) 才进行处理
        return Ok(content.to_string());
    }

    // 合并 block
    let result = merge_blocks(&state.extend, &state.base_blocks, &mut state.blocks);
    Ok(result)
}

// ============================================================================
// 内部实现
// ============================================================================

/// 继承状态（对齐 PHP `parseExtend()` 闭包外部变量 `$extend`/`$blocks`/`$baseBlocks`/`$array`）
#[derive(Default)]
struct ExtendState {
    /// 最顶层模板内容（对齐 PHP `$extend`）
    extend: String,
    /// 子模板 block（对齐 PHP `$blocks`）
    blocks: IndexMap<String, BlockInfo>,
    /// 顶层模板 block（对齐 PHP `$baseBlocks`，sort=true 按位置排序）
    base_blocks: IndexMap<String, BlockInfo>,
    /// 循环检测（对齐 PHP `$array`）
    visited: HashSet<String>,
}

/// Block 信息（对齐 PHP `parseBlock()` 返回的数组元素）
#[derive(Debug, Clone)]
struct BlockInfo {
    /// Block 名称（对齐 PHP `$val['name']`，与 IndexMap key 冗余但保留用于调试与测试）
    #[allow(dead_code)]
    name: String,
    /// 开始标签（如 `{block name="xxx"}`）
    begin: String,
    /// Block 内容
    content: String,
    /// 结束标签（`{/block}`）
    end: String,
    /// 父 block 名称（空字符串表示顶级）
    parent: String,
}

/// 递归查找继承（对齐 PHP `parseExtend()` 递归闭包 `$func`）
///
/// PHP 逻辑：
/// - 如果当前模板包含 `{extend name="..."}`：
///   - 如果未访问过该 name：读取父模板，递归，回溯时收集当前模板的 block
///   - 如果已访问过：直接 return（循环检测）
/// - 如果当前模板不包含 `{extend}`：
///   - 设置 `base_blocks`（sort=true），如果 `extend` 为空则 `extend = 当前模板`
fn find_extend_recursive(
    template: &str,
    config: &ViewConfig,
    state: &mut ExtendState,
) -> Result<(), ViewError> {
    let extend_name = parse_extend_name(template, config);

    if let Some(name) = extend_name {
        if !state.visited.contains(&name) {
            // 第一次访问（对齐 PHP !isset($array[$matches['name']])）
            state.visited.insert(name.clone());

            // 读取继承模板（对齐 PHP $extend = $this->parseTemplateName($matches['name'])）
            let extend_file = resolve_template_path(&name, config);
            if !extend_file.is_file() {
                return Err(ViewError::TemplateNotFound(format!(
                    "继承模板: {} (解析路径: {})",
                    name,
                    extend_file.display()
                )));
            }
            let extend_content = std::fs::read_to_string(&extend_file)?;
            state.extend = extend_content.clone();

            // 递归检查继承（对齐 PHP $func($extend)）
            find_extend_recursive(&extend_content, config, state)?;

            // 回溯时收集当前模板的 block（对齐 PHP $blocks = array_merge($blocks, parseBlock($template))）
            let current_blocks = parse_blocks(template, false, config);
            for (k, v) in current_blocks {
                state.blocks.insert(k, v);
            }
        }
        // 已访问过，直接 return（对齐 PHP 循环检测，if (!isset) 为 false 时 return）
    } else {
        // 最顶层模板（对齐 PHP else 分支）
        state.base_blocks = parse_blocks(template, true, config);
        if state.extend.is_empty() {
            // 无 {extend} 但有 {block} 的情况（对齐 PHP if (empty($extend)) $extend = $template）
            state.extend = template.to_string();
        }
    }

    Ok(())
}

/// 解析 `{extend name="..."}` 标签（对齐 PHP `getRegex('extend')`）
///
/// PHP 正则要求 `name=` 属性存在且值非空。
/// Rust regex 不支持 lookahead `(?!name=)`，采用两步法：
/// 1. 先匹配 `{extend ...}` 任意标签
/// 2. 再解析 `name` 属性，若缺失或为空则返回 None
fn parse_extend_name(content: &str, config: &ViewConfig) -> Option<String> {
    let begin = regex::escape(&config.taglib_begin);
    let end = regex::escape(&config.taglib_end);
    let end_char = regex::escape(&config.taglib_end);

    // 匹配 {extend ...} 标签（对齐 PHP getRegex('extend')，简化为不使用 lookahead）
    // 对齐 PHP: 标签内容允许任意非 end 字符（PHP regex (?>(?:(?!end).)*) 仅排除 end）
    let pattern = format!(
        "{begin}extend\\b\\s+[^{end_char}]+{end}",
        begin = begin,
        end = end,
        end_char = end_char,
    );
    let re = Regex::new(&pattern).ok()?;

    let caps = re.captures(content)?;
    let full_match = caps.get(0)?;
    let name = parse_attr(full_match.as_str(), "name");
    match name {
        Some(n) if !n.is_empty() => Some(n),
        _ => None,
    }
}

/// 解析 block 标签（对齐 PHP `parseBlock()`）
///
/// PHP 逻辑：
/// - 使用 `getRegex('block')` 匹配 `{block name="xxx"}` 和 `{/block}` 标签
/// - **栈式匹配**（`$right` 数组）：
///   - 遇到 `{block name="xxx"}`：压栈
///   - 遇到 `{/block}`：弹栈，记录该 block 的 begin/content/end/parent
/// - 如果 `sort=true`：按 block 结束符在模板中的位置排序（`array_multisort`）
///
/// # 参数
/// - `content` — 模板内容
/// - `sort` — 是否按 block 结束位置排序（对齐 PHP `$sort` 参数）
/// - `config` — 视图配置
///
/// # 返回
/// Block 信息映射（key 是 block name，对齐 PHP 关联数组；同名 block 后面覆盖前面）
fn parse_blocks(content: &str, sort: bool, config: &ViewConfig) -> IndexMap<String, BlockInfo> {
    let begin = regex::escape(&config.taglib_begin);
    let end = regex::escape(&config.taglib_end);
    let end_char = regex::escape(&config.taglib_end);

    // 匹配 {block ...} 或 {/block}（对齐 PHP getRegex('block')）
    // 对齐 PHP: 标签内容允许任意非 end 字符
    let pattern = format!(
        "{begin}(?:block\\b\\s+[^{end_char}]+|/block){end}",
        begin = begin,
        end = end,
        end_char = end_char,
    );
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return IndexMap::new(),
    };

    // 栈：(name, offset, tag)
    let mut stack: Vec<(String, usize, String)> = Vec::new();
    let mut result: IndexMap<String, BlockInfo> = IndexMap::new();
    let mut keys: IndexMap<String, usize> = IndexMap::new();

    let block_open_prefix = format!("{}block", config.taglib_begin);
    let block_close_prefix = format!("{}/block", config.taglib_begin);

    for caps in re.captures_iter(content) {
        let full_match = caps.get(0).expect("正则捕获组 0 必定存在");
        let full_str = full_match.as_str();

        if full_str.starts_with(&block_close_prefix) {
            // 结束标签 {/block}（对齐 PHP empty($match['name'][0]) 分支）
            if let Some((name, offset, tag)) = stack.pop() {
                let start = offset + tag.len();
                let block_content = &content[start..full_match.start()];
                // parent = 弹栈后栈顶的 name（对齐 PHP count($right) ? end($right)['name'] : ''）
                let parent = stack.last().map(|(n, _, _)| n.clone()).unwrap_or_default();
                let end_pos = full_match.end();
                result.insert(
                    name.clone(),
                    BlockInfo {
                        name: name.clone(),
                        begin: tag,
                        content: block_content.to_string(),
                        end: full_str.to_string(),
                        parent,
                    },
                );
                keys.insert(name, end_pos);
            }
        } else if full_str.starts_with(&block_open_prefix) {
            // 开始标签 {block name="xxx"}（对齐 PHP else 分支压栈）
            let name = parse_attr(full_str, "name").unwrap_or_default();
            if !name.is_empty() {
                stack.push((name, full_match.start(), full_str.to_string()));
            }
        }
    }

    if sort {
        // 按 block 结束符在模板中的位置排序（对齐 PHP array_multisort($keys, $result)）
        let mut pairs: Vec<(String, BlockInfo, usize)> = keys
            .into_iter()
            .map(|(name, pos)| {
                let info = result
                    .get(&name)
                    .cloned()
                    .expect("keys 与 result 同源，name 必定存在");
                (name, info, pos)
            })
            .collect();
        pairs.sort_by_key(|(_, _, pos)| *pos);

        result.clear();
        for (name, info, _) in pairs {
            result.insert(name, info);
        }
    }

    result
}

/// 合并 block（对齐 PHP `parseExtend()` 中的合并逻辑）
///
/// PHP 逻辑（Template.php 634-678）：
/// 1. 遍历 `$baseBlocks`（顶层模板的 block，按位置排序）
/// 2. `$replace = $val['content']` — 顶层 block 的内容
/// 3. 如果该 block 有子 block（`$children[$name]`）：在 `$replace` 中替换子 block
/// 4. 如果 `$blocks[$name]` 存在（子模板覆盖了该 block）：
///    - 处理 `{__BLOCK__}` / `{__block__}` 合并标记
///    - 如果是嵌套 block（`$val['parent']` 非空）：在父 block 中替换，`continue`
/// 5. 如果 `$blocks[$name]` 不存在且 `$val['parent']` 非空：用原值
/// 6. 如果是顶级 block（`$val['parent']` 为空）：在 `$extend` 中替换
fn merge_blocks(
    extend: &str,
    base_blocks: &IndexMap<String, BlockInfo>,
    blocks: &mut IndexMap<String, BlockInfo>,
) -> String {
    let mut result = extend.to_string();
    let mut children: HashMap<String, Vec<String>> = HashMap::new();

    for (name, val) in base_blocks.iter() {
        let name = name.clone();
        let val = val.clone();
        let mut replace = val.content.clone();
        let mut skip_top_replace = false;

        // 如果该 block 有子 block（对齐 PHP !empty($children[$name])）
        if let Some(child_keys) = children.get(&name).cloned() {
            for key in &child_keys {
                if let (Some(child_base), Some(child_block)) =
                    (base_blocks.get(key), blocks.get(key))
                {
                    let search = format!(
                        "{}{}{}",
                        child_base.begin, child_base.content, child_base.end
                    );
                    let replacement = &child_block.content;
                    replace = replace.replace(&search, replacement);
                }
            }
        }

        if let Some(block) = blocks.get(&name).cloned() {
            // 子模板覆盖了该 block（对齐 PHP isset($blocks[$name])）
            // 处理 {__BLOCK__} / {__block__} 合并标记
            // 对齐 PHP: $replace = str_replace(['{__BLOCK__}', '{__block__}'], $replace, $blocks[$name]['content'])
            let merged = block
                .content
                .replace("{__BLOCK__}", &replace)
                .replace("{__block__}", &replace);

            if !val.parent.is_empty() {
                // 嵌套 block（对齐 PHP !empty($val['parent'])）
                let parent = val.parent.clone();
                // 在父 block 中替换子 block 标签
                // 对齐 PHP: $blocks[$parent]['content'] = str_replace($blocks[$name]['begin']...end, $replace, $blocks[$parent]['content'])
                let search = format!("{}{}{}", block.begin, block.content, block.end);
                if let Some(parent_block) = blocks.get_mut(&parent) {
                    parent_block.content = parent_block.content.replace(&search, &merged);
                }
                // 更新 $blocks[$name]['content'] = $replace
                if let Some(b) = blocks.get_mut(&name) {
                    b.content = merged.clone();
                }
                children.entry(parent).or_default().push(name);
                skip_top_replace = true;
            } else {
                // 顶级 block，用合并后的内容作为 $replace
                replace = merged;
            }
        } else if !val.parent.is_empty() {
            // 子标签没有被继承，用原值（对齐 PHP elseif !empty($val['parent'])）
            children
                .entry(val.parent.clone())
                .or_default()
                .push(name.clone());
            blocks.insert(name, val.clone());
            // skip_top_replace 保持 false，但 val.parent 非空，所以下面的 if 不会执行
        }

        if !skip_top_replace && val.parent.is_empty() {
            // 替换模板中的顶级block标签（对齐 PHP !$val['parent']）
            let search = format!("{}{}{}", val.begin, val.content, val.end);
            result = result.replace(&search, &replace);
        }
    }

    result
}

/// 解析标签属性（对齐 PHP `parseAttr`）
///
/// PHP 正则: `/\s+(?>(?P<name>[\w-]+)\s*)=(?>\s*)([\"\'])(?P<value>(?:(?!\2).)*)\2/is`
///
/// # 限制
/// Rust `regex` 不支持 backreference `\1`，因此 `[^"']*` 同时排除两种引号。
/// PHP 仅排除匹配的引号（允许 value 中包含另一种引号）。
/// 在实际模板中，属性值几乎不包含引号，此限制可忽略。
fn parse_attr(tag: &str, attr_name: &str) -> Option<String> {
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
    if std::path::Path::new(name).extension().is_some() {
        return PathBuf::from(name);
    }
    let name = name.strip_prefix('/').unwrap_or(name);
    let normalized = name.replace(['/', ':'], &config.view_depr);
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

    fn make_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sz_rust_inheritance_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_template(dir: &std::path::Path, name: &str, content: &str) {
        let path = dir.join(format!("{}.html", name));
        std::fs::write(&path, content).unwrap();
    }

    fn cleanup_dir(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn make_config(view_path: PathBuf) -> ViewConfig {
        ViewConfig {
            view_path,
            ..Default::default()
        }
    }

    // =========================================================================
    // 组 1：基本继承（对齐 PHP parseExtend 基本流程）
    // =========================================================================

    #[test]
    fn test_basic_inheritance() {
        // 对齐 PHP: 子模板 extends 基模板，覆盖 block
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}default{/block}</html>",
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="content"}hello{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>hello</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_inheritance_block_not_overridden() {
        // 对齐 PHP: 子模板未覆盖 block，使用基模板默认内容
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}default{/block}</html>",
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(r#"{extend name="base"}"#, &config).unwrap();
        assert_eq!(result, "<html>default</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_no_extend_no_block() {
        // 对齐 PHP: 无 {extend} 无 {block}，直接返回原内容
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_inheritance("<h1>Hello</h1>", &config).unwrap();
        assert_eq!(result, "<h1>Hello</h1>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_no_extend_but_has_block() {
        // 对齐 PHP: 无 {extend} 但有 {block}，$extend = $template
        // PHP parseExtend else 分支: $baseBlocks = parseBlock(template, true)
        // 然后 foreach $baseBlocks: $replace = $val['content']，str_replace(begin+content+end, replace, $extend)
        // 结果：block 标签被移除，仅保留内容
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"<html>{block name="content"}hello{/block}</html>"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, r#"<html>hello</html>"#);

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 2：{__BLOCK__} 合并标记
    // =========================================================================

    #[test]
    fn test_block_merge_with_block_uppercase() {
        // 对齐 PHP: {__BLOCK__} 表示与父模板合并（大写）
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}base{/block}</html>",
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="content"}{__BLOCK__} child{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>base child</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_block_merge_with_block_lowercase() {
        // 对齐 PHP: {__block__} 表示与父模板合并（小写）
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}base{/block}</html>",
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="content"}{__block__} child{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>base child</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_block_override_without_merge_marker() {
        // 对齐 PHP: 无 {__BLOCK__} 时直接覆盖
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}base{/block}</html>",
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="content"}child{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>child</html>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 3：多 block 场景
    // =========================================================================

    #[test]
    fn test_multiple_blocks() {
        // 对齐 PHP: 多个 block 同时覆盖
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html><head>{block name="title"}default title{/block}</head><body>{block name="content"}default content{/block}</body></html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="title"}My Title{/block}{block name="content"}My Content{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(
            result,
            r#"<html><head>My Title</head><body>My Content</body></html>"#
        );

        cleanup_dir(&dir);
    }

    #[test]
    fn test_multiple_blocks_partial_override() {
        // 对齐 PHP: 部分覆盖
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html><head>{block name="title"}default title{/block}</head><body>{block name="content"}default content{/block}</body></html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="title"}My Title{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(
            result,
            r#"<html><head>My Title</head><body>default content</body></html>"#
        );

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 4：嵌套 block
    // =========================================================================

    #[test]
    fn test_nested_block_override_inner() {
        // 对齐 PHP: 嵌套 block，子模板覆盖内层 block
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html>{block name="outer"}outer {block name="inner"}inner default{/block}{/block}</html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="inner"}inner override{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>outer inner override</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_nested_block_override_outer() {
        // 对齐 PHP: 嵌套 block，子模板覆盖外层 block（内层被完全替换）
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html>{block name="outer"}outer {block name="inner"}inner default{/block}{/block}</html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="outer"}completely replaced{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>completely replaced</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_nested_block_override_both() {
        // 对齐 PHP: 嵌套 block，子模板同时覆盖外层和内层
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html>{block name="outer"}outer {block name="inner"}inner default{/block}{/block}</html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="outer"}new outer {block name="inner"}new inner{/block}{/block}"#,
            &config,
        )
        .unwrap();
        // 外层被完全替换为 "new outer new inner"
        assert_eq!(result, "<html>new outer new inner</html>");

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 5：多层继承
    // =========================================================================

    #[test]
    fn test_multi_level_inheritance() {
        // 对齐 PHP: A extends B extends C，递归继承
        let dir = make_temp_dir();
        write_template(
            &dir,
            "grandparent",
            "<html>{block name=\"content\"}grandparent{/block}</html>",
        );
        write_template(
            &dir,
            "parent",
            r#"{extend name="grandparent"}{block name="content"}parent{/block}"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="parent"}{block name="content"}child{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>child</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_multi_level_inheritance_partial_override() {
        // 对齐 PHP: 多层继承，中间层覆盖部分 block
        let dir = make_temp_dir();
        write_template(
            &dir,
            "grandparent",
            r#"<html>{block name="title"}gp title{/block}{block name="content"}gp content{/block}</html>"#,
        );
        write_template(
            &dir,
            "parent",
            r#"{extend name="grandparent"}{block name="title"}p title{/block}"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="parent"}{block name="content"}child content{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, r#"<html>p titlechild content</html>"#);

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 6：循环继承（PHP bug 复刻）
    // =========================================================================

    #[test]
    fn test_circular_inheritance_php_bug() {
        // R5 PHP/Rust 行为对比：复刻 PHP 源码 bug
        // A extends B, B extends A
        // PHP 递归追踪: $func(content) → $func(a) → $func(b) → $func(a) 循环检测终止
        // $extend 最后被设为 b 内容（$func(b) 中 $extend = parseTemplateName('a') 不执行，
        // 但 $func(a) 中 $extend = parseTemplateName('b') = b 内容，是最后一次成功赋值）
        // $baseBlocks 为空（所有模板都有 {extend}，从未走 else 分支）
        // 所以不做任何 block 替换，直接返回 b 内容
        let dir = make_temp_dir();
        write_template(
            &dir,
            "a",
            r#"{extend name="b"}{block name="content"}a content{/block}"#,
        );
        write_template(
            &dir,
            "b",
            r#"{extend name="a"}{block name="content"}b content{/block}"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="a"}{block name="content"}start{/block}"#,
            &config,
        )
        .unwrap();
        // 对齐 PHP: $extend = b 内容（最后一次成功赋值），$baseBlocks 为空，直接返回 b 内容
        assert_eq!(
            result,
            r#"{extend name="a"}{block name="content"}b content{/block}"#
        );

        cleanup_dir(&dir);
    }

    #[test]
    fn test_self_inheritance_php_bug() {
        // R5 PHP/Rust 行为对比：A extends A
        let dir = make_temp_dir();
        write_template(
            &dir,
            "self_ref",
            r#"{extend name="self_ref"}{block name="content"}content{/block}"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="self_ref"}{block name="content"}start{/block}"#,
            &config,
        )
        .unwrap();
        // 对齐 PHP: 第一次访问 self_ref，设置 $extend = self_ref 内容
        // 递归调用 $func(self_ref 内容)，self_ref 已访问过，直接 return
        // $baseBlocks 为空，直接返回 self_ref 内容
        assert_eq!(
            result,
            r#"{extend name="self_ref"}{block name="content"}content{/block}"#
        );

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 7：错误处理
    // =========================================================================

    #[test]
    fn test_extend_template_not_found() {
        // 对齐 PHP: parseTemplateFile 找不到文件抛出异常
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="nonexistent"}{block name="content"}hello{/block}"#,
            &config,
        );
        assert!(matches!(result, Err(ViewError::TemplateNotFound(_))));

        cleanup_dir(&dir);
    }

    // =========================================================================
    // 组 8：parse_blocks 函数测试
    // =========================================================================

    #[test]
    fn test_parse_blocks_basic() {
        let config = ViewConfig::default();
        let blocks = parse_blocks(
            r#"<html>{block name="a"}content a{/block}</html>"#,
            false,
            &config,
        );
        assert_eq!(blocks.len(), 1);
        let block_a = blocks.get("a").unwrap();
        assert_eq!(block_a.name, "a");
        assert_eq!(block_a.content, "content a");
        assert_eq!(block_a.parent, "");
    }

    #[test]
    fn test_parse_blocks_nested() {
        let config = ViewConfig::default();
        let blocks = parse_blocks(
            r#"{block name="outer"}outer {block name="inner"}inner{/block}{/block}"#,
            false,
            &config,
        );
        assert_eq!(blocks.len(), 2);
        let outer = blocks.get("outer").unwrap();
        let inner = blocks.get("inner").unwrap();
        assert_eq!(outer.parent, "");
        assert_eq!(inner.parent, "outer");
        assert_eq!(outer.content, r#"outer {block name="inner"}inner{/block}"#);
        assert_eq!(inner.content, "inner");
    }

    #[test]
    fn test_parse_blocks_sort() {
        let config = ViewConfig::default();
        // 按 block 结束位置排序
        let blocks = parse_blocks(
            r#"{block name="z"}z{/block}{block name="a"}a{/block}"#,
            true,
            &config,
        );
        let keys: Vec<&String> = blocks.keys().collect();
        // z 先结束，所以排序后 z 在前
        assert_eq!(keys[0], "z");
        assert_eq!(keys[1], "a");
    }

    #[test]
    fn test_parse_blocks_no_blocks() {
        let config = ViewConfig::default();
        let blocks = parse_blocks("<html>no blocks</html>", false, &config);
        assert_eq!(blocks.len(), 0);
    }

    #[test]
    fn test_parse_blocks_same_name_overrides() {
        // 对齐 PHP: 同名 block 后面覆盖前面
        let config = ViewConfig::default();
        let blocks = parse_blocks(
            r#"{block name="a"}first{/block}{block name="a"}second{/block}"#,
            false,
            &config,
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks.get("a").unwrap().content, "second");
    }

    // =========================================================================
    // 组 9：parse_extend_name 函数测试
    // =========================================================================

    #[test]
    fn test_parse_extend_name_basic() {
        let config = ViewConfig::default();
        let name = parse_extend_name(r#"{extend name="base"}"#, &config);
        assert_eq!(name, Some("base".to_string()));
    }

    #[test]
    fn test_parse_extend_name_no_extend() {
        let config = ViewConfig::default();
        let name = parse_extend_name("<html>no extend</html>", &config);
        assert_eq!(name, None);
    }

    #[test]
    fn test_parse_extend_name_no_name_attr() {
        let config = ViewConfig::default();
        // {extend foo} 无 name 属性
        let name = parse_extend_name(r#"{extend foo}"#, &config);
        assert_eq!(name, None);
    }

    // =========================================================================
    // 组 10：路径解析
    // =========================================================================

    #[test]
    fn test_resolve_template_path_basic() {
        let config = ViewConfig {
            view_path: PathBuf::from("view"),
            ..Default::default()
        };
        let path = resolve_template_path("base", &config);
        assert_eq!(path, PathBuf::from("view/base.html"));
    }

    #[test]
    fn test_resolve_template_path_with_slash() {
        let config = ViewConfig {
            view_path: PathBuf::from("view"),
            ..Default::default()
        };
        let path = resolve_template_path("/base", &config);
        assert_eq!(path, PathBuf::from("view/base.html"));
    }

    #[test]
    fn test_resolve_template_path_with_extension() {
        let config = ViewConfig {
            view_path: PathBuf::from("view"),
            ..Default::default()
        };
        let path = resolve_template_path("base.tpl", &config);
        assert_eq!(path, PathBuf::from("base.tpl"));
    }

    #[test]
    fn test_resolve_template_path_custom_depr() {
        let config = ViewConfig {
            view_path: PathBuf::from("view"),
            view_depr: ".".to_string(),
            ..Default::default()
        };
        let path = resolve_template_path("admin/base", &config);
        assert_eq!(path, PathBuf::from("view/admin.base.html"));
    }

    // =========================================================================
    // 组 11：R5 PHP 行为对齐验证
    // =========================================================================

    #[test]
    fn test_r5_php_basic_inheritance_alignment() {
        // R5-1: PHP parseExtend 基本流程对齐
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}default{/block}</html>",
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="content"}override{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>override</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_r5_php_block_merge_alignment() {
        // R5-2: PHP {__BLOCK__} / {__block__} 合并标记对齐
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            "<html>{block name=\"content\"}base{/block}</html>",
        );
        let config = make_config(dir.clone());

        // 大写 {__BLOCK__}
        let result_upper = apply_inheritance(
            r#"{extend name="base"}{block name="content"}{__BLOCK__} extended{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result_upper, "<html>base extended</html>");

        // 小写 {__block__}
        let result_lower = apply_inheritance(
            r#"{extend name="base"}{block name="content"}{__block__} extended{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result_lower, "<html>base extended</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_r5_php_nested_block_alignment() {
        // R5-3: PHP 嵌套 block parent 字段对齐
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html>{block name="outer"}o-{block name="inner"}i{/block}{/block}</html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="inner"}I{/block}"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, "<html>o-I</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_r5_php_multi_level_inheritance_alignment() {
        // R5-4: PHP 多层继承递归对齐
        let dir = make_temp_dir();
        write_template(&dir, "c", "<html>{block name=\"x\"}C{/block}</html>");
        write_template(&dir, "b", r#"{extend name="c"}{block name="x"}B{/block}"#);
        let config = make_config(dir.clone());

        let result =
            apply_inheritance(r#"{extend name="b"}{block name="x"}A{/block}"#, &config).unwrap();
        assert_eq!(result, "<html>A</html>");

        cleanup_dir(&dir);
    }

    #[test]
    fn test_r5_php_circular_inheritance_bug_alignment() {
        // R5-5: PHP 循环继承 bug 复刻对齐
        // PHP 递归: $func(content)→$func(a)→$func(b)→$func(a) 循环终止
        // $extend 最后成功赋值 = b 内容（$func(a) 中 $extend = parseTemplateName('b')）
        // $baseBlocks 为空（从未走 else 分支），直接返回 b 内容
        let dir = make_temp_dir();
        write_template(&dir, "a", r#"{extend name="b"}A{/block}"#);
        write_template(&dir, "b", r#"{extend name="a"}B{/block}"#);
        let config = make_config(dir.clone());

        let result = apply_inheritance(r#"{extend name="a"}"#, &config).unwrap();
        // 对齐 PHP: $extend = b 内容（最后一次成功赋值），$baseBlocks 为空，直接返回 b 内容
        assert_eq!(result, r#"{extend name="a"}B{/block}"#);

        cleanup_dir(&dir);
    }

    #[test]
    fn test_r5_php_sort_alignment() {
        // R5-6: PHP parseBlock sort=true 按结束位置排序对齐
        let config = ViewConfig::default();
        let blocks = parse_blocks(
            r#"{block name="a"}a{/block}{block name="b"}b{/block}"#,
            true,
            &config,
        );
        let keys: Vec<&String> = blocks.keys().collect();
        // a 先结束，所以排序后 a 在前
        assert_eq!(keys[0], "a");
        assert_eq!(keys[1], "b");
    }

    #[test]
    fn test_r5_php_no_extend_has_block_alignment() {
        // R5-7: PHP 无 {extend} 但有 {block} 时 $extend = $template 对齐
        // PHP parseExtend else 分支: $baseBlocks = parseBlock(template, true), $extend = $template
        // 然后 foreach $baseBlocks: $replace = $val['content']（blocks 为空，不进入 isset 分支）
        // str_replace(begin+content+end, replace, $extend) → block 标签被移除，仅保留内容
        let dir = make_temp_dir();
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"<html>{block name="content"}hello{/block}</html>"#,
            &config,
        )
        .unwrap();
        assert_eq!(result, r#"<html>hello</html>"#);

        cleanup_dir(&dir);
    }

    #[test]
    fn test_r5_php_multiple_blocks_order_alignment() {
        // R5-8: PHP 多 block 覆盖顺序对齐（按 baseBlocks 位置排序）
        let dir = make_temp_dir();
        write_template(
            &dir,
            "base",
            r#"<html>{block name="header"}H{/block}{block name="footer"}F{/block}</html>"#,
        );
        let config = make_config(dir.clone());

        let result = apply_inheritance(
            r#"{extend name="base"}{block name="footer"}new footer{/block}{block name="header"}new header{/block}"#,
            &config,
        )
        .unwrap();
        // 对齐 PHP: 按 baseBlocks 位置排序替换，header 在前 footer 在后
        assert_eq!(result, r#"<html>new headernew footer</html>"#);

        cleanup_dir(&dir);
    }
}

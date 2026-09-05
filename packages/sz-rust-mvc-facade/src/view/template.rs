// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Cx 标签库控制流标签 — 对齐 PHP `think\template\taglib\Cx`
//!
//! 实现对齐 PHP Cx 标签库的控制流标签解析与渲染。
//!
//! ## 支持的标签
//!
//! | 标签 | PHP 对齐方法 | 说明 |
//! |------|-------------|------|
//! | `{if}/{elseif}/{else}` | `tagIf/tagElseif/tagElse` | 条件判断 |
//! | `{foreach}` | `tagForeach` | foreach 循环 |
//! | `{volist}` | `tagVolist` | volist 循环 |
//! | `{switch}/{case}/{default}` | `tagSwitch/tagCase/tagDefault` | switch 分支 |
//! | `{for}` | `tagFor` | for 循环 |
//!
//! ## PHP 对齐说明
//!
//! ### 条件简写（对齐 PHP `TagLib::parseCondition`）
//! 使用 `str_ireplace`（大小写不敏感）替换：
//! - ` eq ` → ` == `
//! - ` neq ` → ` != `
//! - ` gt ` → ` > `
//! - ` lt ` → ` < `
//! - ` egt ` → ` >= `
//! - ` elt ` → ` <= `
//! - ` heq ` → ` === `
//! - ` nheq ` → ` !== `
//!
//! 简写两侧必须有空格（对齐 PHP `$comparison` 数组的 key 格式）。
//!
//! ### autoBuildVar（对齐 PHP `TagLib::autoBuildVar`）
//! - `:` 开头 → 函数调用，去掉 `:` 前缀
//! - `$` 开头 → 变量引用，保留
//! - 字母/下划线开头 → 自动补 `$` 前缀（视为变量名）
//! - 其他 → 字面量
//!
//! ### volist 的 key 属性（对齐 PHP `tagVolist`）
//! `key` 属性是计数器变量名（默认 `i`，1-based），不是数组键变量名。
//! 数组键总是赋给字面变量 `$key`。
//!
//! ### case 的 break 陷阱（对齐 PHP `tagCase`）
//! `break="false"` 中字符串 `"false"` 是 truthy，实际会 break。
//! 要实现贯穿需用 `break="0"`（字符串 `"0"` 是 falsy）。
//!
//! ## PHP 源码参考
//! - `think-template\src\template\taglib\Cx.php`（715 行）
//! - `think-template\src\template\TagLib.php`（349 行）

use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;

#[cfg(test)]
use super::value_to_string;
use super::{is_truthy, parse_literal, resolve_var_expr, ViewConfig, ViewData, ViewError};

// ============================================================================
// 公共入口
// ============================================================================

/// 渲染控制流标签（对齐 PHP `TagLib::parseTag`）
///
/// 在 `render_content` 中调用，处理 `{if}`、`{foreach}`、`{volist}`、`{switch}`、`{for}` 块标签。
/// 对每个块标签的内部内容递归调用 `render_inner`（即 `render_content`），以处理嵌套控制流和变量标签。
///
/// # 参数
/// - `content` — 模板内容
/// - `data` — 模板变量
/// - `config` — 视图配置
/// - `render_inner` — 内部内容渲染器（递归调用 `render_content`）
pub fn render_control_flow<F>(
    content: &str,
    data: &ViewData,
    config: &ViewConfig,
    render_inner: F,
) -> Result<String, ViewError>
where
    F: Fn(&str, &ViewData) -> Result<String, ViewError>,
{
    let renderer = CxRenderer {
        config,
        render_inner,
    };
    renderer.render(content, data)
}

// ============================================================================
// CxRenderer — 控制流标签渲染器
// ============================================================================

/// Cx 标签渲染器
struct CxRenderer<'a, F>
where
    F: Fn(&str, &ViewData) -> Result<String, ViewError>,
{
    config: &'a ViewConfig,
    render_inner: F,
}

impl<'a, F> CxRenderer<'a, F>
where
    F: Fn(&str, &ViewData) -> Result<String, ViewError>,
{
    /// 渲染主循环：扫描控制流开标签，找到匹配闭标签，渲染块内容
    fn render(&self, content: &str, data: &ViewData) -> Result<String, ViewError> {
        let begin = &self.config.tpl_begin;
        let begin_esc = regex::escape(begin);

        // 匹配 {if / {foreach / {volist / {switch / {for（后跟空白或 `(`）
        let pattern = format!(r"{}\s*(if|foreach|volist|switch|for)(\s|\()", begin_esc);
        let re = Regex::new(&pattern).map_err(|e| ViewError::SyntaxError(e.to_string()))?;

        let mut result = String::with_capacity(content.len());
        let mut last_end = 0;

        for caps in re.captures_iter(content) {
            let m = caps
                .get(0)
                .ok_or_else(|| ViewError::SyntaxError("正则捕获组 0（整体匹配）缺失".into()))?;
            // 跳过已处理区域内的匹配（嵌套标签已由 render_inner 递归处理）
            if m.start() < last_end {
                continue;
            }
            let tag_type = caps
                .get(1)
                .ok_or_else(|| ViewError::SyntaxError("正则捕获组 1（标签类型）缺失".into()))?
                .as_str();
            let separator = caps
                .get(2)
                .ok_or_else(|| ViewError::SyntaxError("正则捕获组 2（分隔符）缺失".into()))?
                .as_str(); // 空格或 `(`

            // 找到开标签的闭合 `}`
            let after_name = m.start() + m.len() - separator.len();
            let end_str = &self.config.tpl_end;
            let tag_end_pos = match content[after_name..].find(end_str.as_str()) {
                Some(pos) => after_name + pos + end_str.len(),
                None => continue, // 无闭合 `}`，跳过
            };

            result.push_str(&content[last_end..m.start()]);

            // 提取开标签属性内容（{tag_name ... } 中的 `...`）
            let tag_content_start = m.start() + begin.len() + tag_type.len();
            // tag_content_start 指向 separator（空格或 `(`）
            let tag_inner = &content[tag_content_start..tag_end_pos - end_str.len()];

            // 找到匹配的闭标签
            match self.find_matching_close(content, tag_end_pos, tag_type) {
                Ok((block_content, block_end)) => {
                    let rendered = match tag_type {
                        "if" => self.render_if(tag_inner, &block_content, data)?,
                        "foreach" => self.render_foreach(tag_inner, &block_content, data)?,
                        "volist" => self.render_volist(tag_inner, &block_content, data)?,
                        "switch" => self.render_switch(tag_inner, &block_content, data)?,
                        "for" => self.render_for(tag_inner, &block_content, data)?,
                        _ => block_content.to_string(),
                    };
                    result.push_str(&rendered);
                    last_end = block_end;
                }
                Err(e) => {
                    // 未闭合标签：输出错误提示
                    return Err(e);
                }
            }
        }

        result.push_str(&content[last_end..]);
        Ok(result)
    }

    /// 找到匹配的闭标签（对齐 PHP `TagLib::parseTag` 栈式匹配）
    ///
    /// 从 `start` 位置开始，查找与 `tag_type` 匹配的闭标签 `{/tag_type}`。
    /// 跟踪嵌套深度，正确处理同名标签的嵌套。
    fn find_matching_close(
        &self,
        content: &str,
        start: usize,
        tag_type: &str,
    ) -> Result<(String, usize), ViewError> {
        let begin = &self.config.tpl_begin;
        let end = &self.config.tpl_end;

        let open_prefix = format!("{}{}", begin, tag_type);
        let close_tag = format!("{}{}{}{}", begin, "/", tag_type, end);

        let mut depth: i32 = 1;
        let mut pos = start;

        while pos < content.len() {
            let remaining = &content[pos..];

            // 查找下一个闭标签
            let next_close = remaining.find(&close_tag);

            // 查找下一个开标签（必须后跟空白或 `(` 或 `}`）
            let next_open = remaining.find(&open_prefix).and_then(|p| {
                let after = pos + p + open_prefix.len();
                if after < content.len() {
                    let next_char = content.as_bytes()[after];
                    // 开标签后必须是空白、`(` 或 `}`
                    if next_char == b' '
                        || next_char == b'\t'
                        || next_char == b'\n'
                        || next_char == b'\r'
                        || next_char == b'('
                        || content[after..].starts_with(end.as_str())
                    {
                        return Some(p);
                    }
                }
                None
            });

            match (next_open, next_close) {
                (_, None) => {
                    return Err(ViewError::SyntaxError(format!(
                        "未闭合的 {{{}}} 标签",
                        tag_type
                    )));
                }
                (Some(open_pos), Some(close_pos)) if open_pos < close_pos => {
                    depth += 1;
                    pos += open_pos + open_prefix.len();
                }
                (_, Some(close_pos)) => {
                    depth -= 1;
                    if depth == 0 {
                        let block_content = content[start..pos + close_pos].to_string();
                        let block_end = pos + close_pos + close_tag.len();
                        return Ok((block_content, block_end));
                    }
                    pos += close_pos + close_tag.len();
                }
            }
        }

        Err(ViewError::SyntaxError(format!(
            "未闭合的 {{{}}} 标签",
            tag_type
        )))
    }

    // ========================================================================
    // {if}/{elseif}/{else} — 对齐 PHP `tagIf/tagElseif/tagElse`
    // ========================================================================

    /// 渲染 {if}...{/if} 块
    ///
    /// PHP `tagIf` 编译为 `<?php if(...): ?>...<?php endif; ?>`。
    /// Rust 改为直接求值条件，渲染匹配分支的内容。
    ///
    /// 支持两种语法：
    /// - 属性形式：`{if condition="$a eq 1"}`
    /// - 表达式形式：`{if ($a == 1)}`
    fn render_if(
        &self,
        tag_inner: &str,
        block_content: &str,
        data: &ViewData,
    ) -> Result<String, ViewError> {
        let attrs = parse_attributes(tag_inner);
        let condition = if let Some(cond) = attrs.get("condition") {
            cond.clone()
        } else {
            // 表达式形式：tag_inner 本身是条件表达式
            // 去掉前导空白和前导 `(`
            tag_inner
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .trim()
                .to_string()
        };

        // 求值 if 条件
        if evaluate_condition(&condition, data)? {
            // 渲染 if 分支内容（在 `{elseif}` 或 `{else}` 之前的部分）
            let if_body = self.split_if_branches(block_content).0;
            return (self.render_inner)(&if_body, data);
        }

        // 查找 {elseif} 和 {else} 分支
        let branches = self.split_if_branches(block_content);
        let branches_ref = &branches.1;

        for (elseif_cond, elseif_body) in branches_ref {
            if evaluate_condition(elseif_cond, data)? {
                return (self.render_inner)(elseif_body, data);
            }
        }

        // {else} 分支
        if let Some(else_body) = &branches.2 {
            return (self.render_inner)(else_body, data);
        }

        Ok(String::new())
    }

    /// 将 {if} 块内容拆分为 (if_body, elseif_branches, else_body)
    ///
    /// `elseif_branches` 是 `Vec<(condition, body)>`
    /// `else_body` 是 `Option<String>`
    fn split_if_branches(&self, content: &str) -> (String, Vec<(String, String)>, Option<String>) {
        let begin = &self.config.tpl_begin;
        let end = &self.config.tpl_end;

        // 匹配 {elseif ... /} 或 {elseif ...}
        // 注意：format! 中字面 `{}` 必须用 `{{}}` 转义；去掉 r 前缀并将 `\s` 写成 `\\s`
        let elseif_pattern = format!(
            "{}\\s*elseif\\s+([^{{}}]*?)(?:\\s*/)?{}",
            regex::escape(begin),
            regex::escape(end)
        );
        // 匹配 {else /} 或 {else}
        let else_pattern = format!(
            r"{}\s*else\s*/?{}",
            regex::escape(begin),
            regex::escape(end)
        );

        let elseif_re = Regex::new(&elseif_pattern).unwrap_or_else(|e| panic!("elseif regex: {e}"));
        let else_re = Regex::new(&else_pattern).unwrap_or_else(|e| panic!("else regex: {e}"));

        // 找到第一个 {elseif} 或 {else} 的位置
        let first_elseif = elseif_re.find(content).map(|m| m.start());
        let first_else = else_re.find(content).map(|m| m.start());

        let first_split = match (first_elseif, first_else) {
            (Some(ei), Some(el)) => ei.min(el),
            (Some(ei), None) => ei,
            (None, Some(el)) => el,
            (None, None) => return (content.to_string(), Vec::new(), None),
        };

        let if_body = content[..first_split].to_string();
        let rest = &content[first_split..];

        let mut elseif_branches = Vec::new();
        let mut else_body = None;
        let mut remaining = rest;

        loop {
            // 尝试匹配 {elseif condition="..." /}
            if let Some(m) = elseif_re.find(remaining) {
                if m.start() == 0 {
                    let attrs_str = elseif_re
                        .captures(remaining)
                        .expect("elseif_re 已通过 find 匹配且 start==0，captures 必成功")
                        .get(1)
                        .expect("elseif 正则含捕获组 1，匹配成功时必存在")
                        .as_str();
                    let attrs = parse_attributes(attrs_str);
                    let cond = attrs
                        .get("condition")
                        .cloned()
                        .unwrap_or_else(|| attrs_str.trim().to_string());
                    let after = &remaining[m.end()..];

                    // 找到下一个 {elseif} 或 {else} 的位置
                    let next_split = self.find_next_branch(after);

                    let body = match next_split {
                        Some(pos) => {
                            let b = after[..pos].to_string();
                            remaining = &after[pos..];
                            b
                        }
                        None => {
                            let b = after.to_string();
                            remaining = "";
                            b
                        }
                    };
                    elseif_branches.push((cond, body));
                    if remaining.is_empty() {
                        break;
                    }
                    continue;
                }
            }

            // 尝试匹配 {else /}
            if let Some(m) = else_re.find(remaining) {
                if m.start() == 0 {
                    else_body = Some(remaining[m.end()..].to_string());
                    break;
                }
            }

            break;
        }

        (if_body, elseif_branches, else_body)
    }

    /// 在内容中查找下一个 {elseif} 或 {else} 标签的位置
    fn find_next_branch(&self, content: &str) -> Option<usize> {
        let begin = &self.config.tpl_begin;
        let end = &self.config.tpl_end;

        let elseif_pattern = format!(
            "{}\\s*elseif\\s+[^{{}}]*?{}",
            regex::escape(begin),
            regex::escape(end)
        );
        let else_pattern = format!(
            "{}\\s*else\\s*/?{}",
            regex::escape(begin),
            regex::escape(end)
        );

        let elseif_re = Regex::new(&elseif_pattern).ok()?;
        let else_re = Regex::new(&else_pattern).ok()?;

        let ei = elseif_re.find(content).map(|m| m.start());
        let el = else_re.find(content).map(|m| m.start());

        match (ei, el) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    // ========================================================================
    // {foreach} — 对齐 PHP `tagForeach`
    // ========================================================================

    /// 渲染 {foreach}...{/foreach} 块
    ///
    /// PHP `tagForeach` 支持两种形式：
    /// - 属性形式：`{foreach name="list" id="item" key="k" index="i"}`
    /// - 表达式形式：`{foreach $list as $k=>$v}`
    ///
    /// 属性说明：
    /// - `name` — 数组变量名（不含 `$`）
    /// - `id` / `item` — 元素变量名
    /// - `key` — 键变量名（默认 `key`）
    /// - `index` — 索引计数器变量名（默认 `i`，0-based）
    /// - `offset` — 起始偏移
    /// - `length` — 最大长度
    /// - `empty` — 空数据提示
    fn render_foreach(
        &self,
        tag_inner: &str,
        block_content: &str,
        data: &ViewData,
    ) -> Result<String, ViewError> {
        let attrs = parse_attributes(tag_inner);

        // 表达式形式：{foreach ($list as $k=>$v)}
        // PHP `parseAttr` 在无 key="value" 属性且标签支持 expression 时，将整个标签内容作为 expression
        if attrs.is_empty() && tag_inner.trim().starts_with('(') {
            return self.render_foreach_expression(tag_inner.trim(), block_content, data);
        }
        if let Some(expr) = attrs.get("expression") {
            return self.render_foreach_expression(expr, block_content, data);
        }

        let name = attrs.get("name").cloned().unwrap_or_default();
        let item_name = attrs
            .get("id")
            .or_else(|| attrs.get("item"))
            .cloned()
            .unwrap_or_else(|| "item".to_string());
        let key_name = attrs
            .get("key")
            .cloned()
            .unwrap_or_else(|| "key".to_string());
        let index_name = attrs.get("index").cloned();
        let empty_msg = attrs.get("empty").cloned().unwrap_or_default();
        let offset: usize = attrs
            .get("offset")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let length: Option<usize> = attrs.get("length").and_then(|v| v.parse().ok());

        // 解析数组变量
        let array_val = resolve_name(&name, data);

        let items = match &array_val {
            Value::Array(arr) => arr.clone(),
            Value::Object(map) => {
                // 对象按 key=>value 遍历
                map.iter()
                    .map(|(k, v)| serde_json::json!({"key": k, "value": v.clone()}))
                    .collect()
            }
            _ => Vec::new(),
        };

        // 应用 offset 和 length
        let start = offset.min(items.len());
        let end = match length {
            Some(len) => (start + len).min(items.len()),
            None => items.len(),
        };
        let slice = &items[start..end];

        if slice.is_empty() && !empty_msg.is_empty() {
            return Ok(empty_msg);
        }

        let mut result = String::new();
        for (idx, item) in slice.iter().enumerate() {
            let mut child_data = data.clone();

            // 设置元素变量
            if array_val.is_array() {
                child_data.insert(item_name.clone(), item.clone());
            } else {
                // 对象遍历：从 {key, value} 对中提取
                if let Some(v) = item.get("value") {
                    child_data.insert(item_name.clone(), v.clone());
                }
                if let Some(k) = item.get("key").and_then(|k| k.as_str()) {
                    child_data.insert(key_name.clone(), Value::String(k.to_string()));
                }
            }

            // 数组遍历：设置 key
            if array_val.is_array() {
                if let Value::Array(arr) = &array_val {
                    let actual_idx = start + idx;
                    if let Some(actual_item) = arr.get(actual_idx) {
                        child_data.insert(item_name.clone(), actual_item.clone());
                    }
                    child_data.insert(key_name.clone(), Value::Number((actual_idx).into()));
                }
            }

            // 设置索引计数器
            if let Some(ref index) = index_name {
                child_data.insert(index.clone(), Value::Number((idx).into()));
            }

            let rendered = (self.render_inner)(block_content, &child_data)?;
            result.push_str(&rendered);
        }

        Ok(result)
    }

    /// 渲染表达式形式 {foreach $list as $k=>$v}
    fn render_foreach_expression(
        &self,
        expr: &str,
        block_content: &str,
        data: &ViewData,
    ) -> Result<String, ViewError> {
        // 去掉括号
        let expr = expr
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim();

        // 解析 `$list as $k=>$v` 或 `$list as $v`
        let as_pos = expr
            .find(" as ")
            .ok_or_else(|| ViewError::SyntaxError("foreach 表达式缺少 'as'".to_string()))?;

        let name_part = expr[..as_pos].trim();
        let after_as = expr[as_pos + 4..].trim();

        // 去掉变量前缀 $
        let name = name_part.trim_start_matches('$');
        let array_val = resolve_var_expr(name, data);

        let (key_name, item_name) = if let Some(arrow_pos) = after_as.find("=>") {
            let k = after_as[..arrow_pos].trim().trim_start_matches('$');
            let v = after_as[arrow_pos + 2..].trim().trim_start_matches('$');
            (k.to_string(), v.to_string())
        } else {
            (
                "key".to_string(),
                after_as.trim_start_matches('$').to_string(),
            )
        };

        let mut result = String::new();

        match &array_val {
            Value::Array(arr) => {
                for (idx, item) in arr.iter().enumerate() {
                    let mut child_data = data.clone();
                    child_data.insert(item_name.clone(), item.clone());
                    child_data.insert(key_name.clone(), Value::Number((idx).into()));
                    let rendered = (self.render_inner)(block_content, &child_data)?;
                    result.push_str(&rendered);
                }
            }
            Value::Object(map) => {
                for (k, v) in map.iter() {
                    let mut child_data = data.clone();
                    child_data.insert(item_name.clone(), v.clone());
                    child_data.insert(key_name.clone(), Value::String(k.clone()));
                    let rendered = (self.render_inner)(block_content, &child_data)?;
                    result.push_str(&rendered);
                }
            }
            _ => {}
        }

        Ok(result)
    }

    // ========================================================================
    // {volist} — 对齐 PHP `tagVolist`
    // ========================================================================

    /// 渲染 {volist}...{/volist} 块
    ///
    /// PHP `tagVolist` 与 `tagForeach` 的关键差异：
    /// - `key` 属性是计数器变量名（默认 `i`，1-based），不是数组键变量名
    /// - 数组键总是赋给字面变量 `$key`
    /// - 不支持表达式形式
    ///
    /// 属性说明：
    /// - `name` — 数组变量名
    /// - `id` — 元素变量名
    /// - `key` — 计数器变量名（默认 `i`，1-based）
    /// - `offset` — 起始偏移
    /// - `length` — 最大长度
    /// - `mod` — 取模值（默认 2）
    /// - `empty` — 空数据提示
    fn render_volist(
        &self,
        tag_inner: &str,
        block_content: &str,
        data: &ViewData,
    ) -> Result<String, ViewError> {
        let attrs = parse_attributes(tag_inner);

        let name = attrs.get("name").cloned().unwrap_or_default();
        let id_name = attrs
            .get("id")
            .cloned()
            .unwrap_or_else(|| "item".to_string());
        let key_name = attrs.get("key").cloned().unwrap_or_else(|| "i".to_string());
        let empty_msg = attrs.get("empty").cloned().unwrap_or_default();
        let offset: usize = attrs
            .get("offset")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let length: Option<usize> = attrs.get("length").and_then(|v| v.parse().ok());

        // 解析数组变量
        let array_val = resolve_name(&name, data);

        // 收集 (key, value) 对
        let pairs: Vec<(Value, Value)> = match &array_val {
            Value::Array(arr) => arr
                .iter()
                .enumerate()
                .map(|(i, v)| (Value::Number(i.into()), v.clone()))
                .collect(),
            Value::Object(map) => map
                .iter()
                .map(|(k, v)| (Value::String(k.clone()), v.clone()))
                .collect(),
            _ => Vec::new(),
        };

        // 应用 offset 和 length
        let start = offset.min(pairs.len());
        let end = match length {
            Some(len) => (start + len).min(pairs.len()),
            None => pairs.len(),
        };

        if start >= end && !empty_msg.is_empty() {
            return Ok(empty_msg);
        }

        let mut result = String::new();
        for (view_idx, (key_val, item_val)) in pairs[start..end].iter().enumerate() {
            let mut child_data = data.clone();

            // PHP volist: $key = 数组原始键，$i (key属性) = 1-based 计数器
            child_data.insert("key".to_string(), key_val.clone());
            child_data.insert(key_name.clone(), Value::Number((view_idx + 1).into()));
            child_data.insert(id_name.clone(), item_val.clone());

            let rendered = (self.render_inner)(block_content, &child_data)?;
            result.push_str(&rendered);
        }

        Ok(result)
    }

    // ========================================================================
    // {switch}/{case}/{default} — 对齐 PHP `tagSwitch/tagCase/tagDefault`
    // ========================================================================

    /// 渲染 {switch}...{/switch} 块
    ///
    /// PHP `tagSwitch` 编译为 `<?php switch(...): ?>...<?php endswitch; ?>`。
    ///
    /// {case} 标签支持：
    /// - `value="1"` — 单值匹配
    /// - `value="1|2|3"` — 多值匹配（`|` 分隔）
    /// - `break="false"` — 陷阱：字符串 "false" 是 truthy，实际会 break
    /// - `break="0"` — 贯穿（字符串 "0" 是 falsy）
    fn render_switch(
        &self,
        tag_inner: &str,
        block_content: &str,
        data: &ViewData,
    ) -> Result<String, ViewError> {
        let attrs = parse_attributes(tag_inner);
        let name = attrs.get("name").cloned().unwrap_or_default();
        let switch_val = resolve_name(&name, data);

        // 解析 case 和 default 分支
        let cases = self.parse_switch_cases(block_content);

        let mut matched = false;
        let mut result = String::new();
        let mut fall_through = false;

        for case in &cases {
            if fall_through {
                // 贯穿：渲染当前 case 内容
                let rendered = (self.render_inner)(&case.content, data)?;
                result.push_str(&rendered);
                if case.should_break() {
                    break;
                }
                continue;
            }

            if matched {
                break;
            }

            match &case.kind {
                CaseKind::Default => {
                    matched = true;
                    let rendered = (self.render_inner)(&case.content, data)?;
                    result.push_str(&rendered);
                    if case.should_break() {
                        break;
                    }
                    fall_through = true;
                }
                CaseKind::Values(values) => {
                    for val in values {
                        let case_val = resolve_case_value(val, data);
                        if loose_equal(&switch_val, &case_val) {
                            matched = true;
                            let rendered = (self.render_inner)(&case.content, data)?;
                            result.push_str(&rendered);
                            if case.should_break() {
                                break;
                            }
                            fall_through = true;
                            break;
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// 解析 {switch} 块内容中的 {case} 和 {default} 分支
    fn parse_switch_cases(&self, content: &str) -> Vec<SwitchCase> {
        let begin = &self.config.tpl_begin;
        let end = &self.config.tpl_end;

        // 匹配 {case value="..." /} 或 {case value="..."}
        let case_pattern = format!(
            "{}\\s*case\\s+([^{{}}]*?)(?:\\s*/)?{}",
            regex::escape(begin),
            regex::escape(end)
        );
        // 匹配 {default /} 或 {default}
        let default_pattern = format!(
            "{}\\s*default\\s*/?{}",
            regex::escape(begin),
            regex::escape(end)
        );

        let case_re = Regex::new(&case_pattern).unwrap_or_else(|e| panic!("case regex: {e}"));
        let default_re =
            Regex::new(&default_pattern).unwrap_or_else(|e| panic!("default regex: {e}"));

        let mut cases = Vec::new();
        let mut pos = 0;

        while pos < content.len() {
            // 查找下一个 {case} 或 {default}
            let next_case = case_re
                .find(&content[pos..])
                .map(|m| (pos + m.start(), m.end() - m.start()));
            let next_default = default_re
                .find(&content[pos..])
                .map(|m| (pos + m.start(), m.end() - m.start()));

            let (tag_start, tag_len, is_default) = match (next_case, next_default) {
                (Some((cs, cl)), Some((ds, dl))) => {
                    if cs <= ds {
                        (cs, cl, false)
                    } else {
                        (ds, dl, true)
                    }
                }
                (Some((cs, cl)), None) => (cs, cl, false),
                (None, Some((ds, dl))) => (ds, dl, true),
                (None, None) => break,
            };

            // 提取标签属性
            let tag_str = &content[tag_start..tag_start + tag_len];

            if is_default {
                let body_start = tag_start + tag_len;
                // 查找下一个 {case} 或 {default} 或内容结束
                let (body_end, is_close_case) = self
                    .find_next_case_or_default(&content[body_start..])
                    .map(|(p, is_cc)| (body_start + p, is_cc))
                    .unwrap_or((content.len(), false));
                let body = content[body_start..body_end].to_string();
                cases.push(SwitchCase {
                    kind: CaseKind::Default,
                    content: body,
                    break_attr: None,
                });
                // 如果遇到 {/case}，跳过它
                let close_case_tag = format!("{}/case{}", begin, end);
                if is_close_case {
                    pos = body_end + close_case_tag.len();
                } else {
                    pos = body_end;
                }
            } else {
                let caps = case_re
                    .captures(tag_str)
                    .expect("else 分支仅处理 case 标签，case_re 必匹配");
                let attrs_str = caps
                    .get(1)
                    .expect("case 正则含捕获组 1，匹配成功时必存在")
                    .as_str();
                let attrs = parse_attributes(attrs_str);
                let value = attrs.get("value").cloned().unwrap_or_default();
                let break_attr = attrs.get("break").cloned();

                let body_start = tag_start + tag_len;
                let (body_end, is_close_case) = self
                    .find_next_case_or_default(&content[body_start..])
                    .map(|(p, is_cc)| (body_start + p, is_cc))
                    .unwrap_or((content.len(), false));
                let body = content[body_start..body_end].to_string();

                // 解析 value（支持 | 多值）
                let values: Vec<String> = value.split('|').map(|s| s.trim().to_string()).collect();

                cases.push(SwitchCase {
                    kind: CaseKind::Values(values),
                    content: body,
                    break_attr,
                });
                // 如果遇到 {/case}，跳过它
                let close_case_tag = format!("{}/case{}", begin, end);
                if is_close_case {
                    pos = body_end + close_case_tag.len();
                } else {
                    pos = body_end;
                }
            }
        }

        cases
    }

    /// 查找下一个 {case}、{default} 或 {/case} 标签位置
    ///
    /// 返回 `(位置, 是否是 {/case} 闭合标签)`
    fn find_next_case_or_default(&self, content: &str) -> Option<(usize, bool)> {
        let begin = &self.config.tpl_begin;
        let end = &self.config.tpl_end;

        let case_pattern = format!(
            "{}\\s*case\\s+[^{{}}]*?{}",
            regex::escape(begin),
            regex::escape(end)
        );
        let default_pattern = format!(
            "{}\\s*default\\s*/?{}",
            regex::escape(begin),
            regex::escape(end)
        );
        // {/case} 闭合标签
        let close_case_pattern = format!(
            "{}\\s*/case\\s*{}",
            regex::escape(begin),
            regex::escape(end)
        );

        let case_re = Regex::new(&case_pattern).ok()?;
        let default_re = Regex::new(&default_pattern).ok()?;
        let close_case_re = Regex::new(&close_case_pattern).ok()?;

        let cs = case_re.find(content).map(|m| m.start());
        let ds = default_re.find(content).map(|m| m.start());
        let ccs = close_case_re.find(content).map(|m| m.start());

        // 找到最近的位置
        let mut candidates: Vec<(usize, bool)> = Vec::new();
        if let Some(p) = cs {
            candidates.push((p, false));
        }
        if let Some(p) = ds {
            candidates.push((p, false));
        }
        if let Some(p) = ccs {
            candidates.push((p, true));
        }

        candidates.into_iter().min_by_key(|(p, _)| *p)
    }

    // ========================================================================
    // {for} — 对齐 PHP `tagFor`
    // ========================================================================

    /// 渲染 {for}...{/for} 块
    ///
    /// PHP `tagFor` 用 `rand()` 生成唯一临时变量名防止嵌套冲突。
    /// Rust 不需要临时变量（直接求值），但仍保留 `$name` 变量供循环体使用。
    ///
    /// 属性说明：
    /// - `start` — 起始值（默认 0）
    /// - `end` — 结束值（默认 0）
    /// - `comparison` — 比较类型（默认 `lt`，即 `<`）
    /// - `step` — 步长（默认 1）
    /// - `name` — 循环变量名（默认 `i`）
    fn render_for(
        &self,
        tag_inner: &str,
        block_content: &str,
        data: &ViewData,
    ) -> Result<String, ViewError> {
        let attrs = parse_attributes(tag_inner);

        let start: f64 = attrs
            .get("start")
            .and_then(|v| parse_operand_value(v, data).as_f64())
            .unwrap_or(0.0);
        let end: f64 = attrs
            .get("end")
            .and_then(|v| parse_operand_value(v, data).as_f64())
            .unwrap_or(0.0);
        let step: f64 = attrs
            .get("step")
            .and_then(|v| parse_operand_value(v, data).as_f64())
            .unwrap_or(1.0);
        let comparison = attrs
            .get("comparison")
            .cloned()
            .unwrap_or_else(|| "lt".to_string());
        let name = attrs
            .get("name")
            .cloned()
            .unwrap_or_else(|| "i".to_string());

        // 防止死循环：step 必须非零
        if step == 0.0 {
            return Err(ViewError::SyntaxError("{for} step 不能为 0".to_string()));
        }

        let mut result = String::new();
        let mut current = start;

        loop {
            // 检查循环条件
            let should_continue = match comparison.as_str() {
                "lt" | "<" => current < end,
                "elt" | "<=" => current <= end,
                "gt" | ">" => current > end,
                "egt" | ">=" => current >= end,
                "eq" | "==" => current == end,
                "neq" | "!=" => current != end,
                "heq" | "===" => current == end,
                "nheq" | "!==" => current != end,
                _ => current < end, // 默认 lt
            };

            if !should_continue {
                break;
            }

            // 设置循环变量
            let mut child_data = data.clone();
            child_data.insert(
                name.clone(),
                serde_json::Number::from_f64(current)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
            );

            let rendered = (self.render_inner)(block_content, &child_data)?;
            result.push_str(&rendered);

            current += step;

            // 安全限制：防止无限循环
            if result.len() > 10_000_000 {
                return Err(ViewError::RenderError(
                    "{for} 循环输出超过 10MB 限制".to_string(),
                ));
            }
        }

        Ok(result)
    }
}

// ============================================================================
// switch case 辅助类型
// ============================================================================

/// switch case 的种类
#[derive(Debug, Clone)]
enum CaseKind {
    /// {case value="1|2|3"} — 值列表
    Values(Vec<String>),
    /// {default /}
    Default,
}

/// 一个 switch case 分支
#[derive(Debug, Clone)]
struct SwitchCase {
    kind: CaseKind,
    content: String,
    break_attr: Option<String>,
}

impl SwitchCase {
    /// 是否应该 break（对齐 PHP `tagCase` 的 break 逻辑）
    ///
    /// PHP 逻辑：
    /// ```php
    /// $isBreak = isset($tag['break']) ? $tag['break'] : '';
    /// if ('' == $isBreak || $isBreak) { break; }
    /// ```
    /// - 无 break 属性 → break（默认）
    /// - break="" → break（空字符串等于 ''）
    /// - break="false" → break（"false" 是 truthy）
    /// - break="0" → 不 break（"0" 是 falsy）
    fn should_break(&self) -> bool {
        match &self.break_attr {
            None => true, // 默认 break
            Some(v) => is_truthy(&Value::String(v.clone())),
        }
    }
}

// ============================================================================
// 条件求值器 — 对齐 PHP `parseCondition` + `eval`
// ============================================================================

/// 求值条件表达式
///
/// PHP 通过 `parseCondition` 替换简写后，用 `eval` 执行。
/// Rust 改为递归下降求值器，支持 `||`、`&&`、`!`、比较运算符和括号。
pub fn evaluate_condition(condition: &str, data: &ViewData) -> Result<bool, ViewError> {
    let condition = replace_condition_shorthands(condition.trim());
    evaluate_or(&condition, data)
}

/// 求值 OR 表达式（`||` 分隔）
fn evaluate_or(expr: &str, data: &ViewData) -> Result<bool, ViewError> {
    for part in split_top_level(expr, "||") {
        if evaluate_and(part.trim(), data)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 求值 AND 表达式（`&&` 分隔）
fn evaluate_and(expr: &str, data: &ViewData) -> Result<bool, ViewError> {
    for part in split_top_level(expr, "&&") {
        if !evaluate_not(part.trim(), data)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 求值 NOT 表达式（`!` 前缀）
fn evaluate_not(expr: &str, data: &ViewData) -> Result<bool, ViewError> {
    let expr = expr.trim();
    if let Some(rest) = expr.strip_prefix('!') {
        return Ok(!evaluate_not(rest.trim(), data)?);
    }
    evaluate_comparison(expr, data)
}

/// 求值比较表达式
///
/// 支持运算符（按优先匹配长度排序）：`===`、`!==`、`==`、`!=`、`>=`、`<=`、`>`、`<`
fn evaluate_comparison(expr: &str, data: &ViewData) -> Result<bool, ViewError> {
    let expr = expr.trim();

    // 去掉外层括号
    if expr.starts_with('(') && expr.ends_with(')') && is_balanced_parens(&expr[1..expr.len() - 1])
    {
        return evaluate_or(&expr[1..expr.len() - 1], data);
    }

    // 查找比较运算符（在顶层，不在括号或引号内）
    if let Some((op, pos)) = find_comparison_operator(expr) {
        let left = expr[..pos].trim();
        let right = expr[pos + op.len()..].trim();
        let left_val = parse_operand_value(left, data);
        let right_val = parse_operand_value(right, data);
        return Ok(compare_values(&left_val, op, &right_val));
    }

    // 无运算符：检查真值
    let val = parse_operand_value(expr, data);
    Ok(is_truthy(&val))
}

/// 在顶层查找比较运算符（跳过括号和引号内的）
fn find_comparison_operator(expr: &str) -> Option<(&'static str, usize)> {
    let bytes = expr.as_bytes();
    let mut depth = 0;
    let mut in_single = false;
    let mut in_double = false;

    let operators: &[&'static str] = &["===", "!==", "==", "!=", ">=", "<=", ">", "<"];

    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' if !in_single && !in_double => {
                depth += 1;
            }
            b')' if !in_single && !in_double => {
                depth -= 1;
            }
            b'\'' if !in_double => {
                in_single = !in_single;
            }
            b'"' if !in_single => {
                in_double = !in_double;
            }
            _ if depth == 0 && !in_single && !in_double => {
                let rest = &expr[i..];
                for op in operators {
                    if rest.starts_with(op) {
                        return Some((op, i));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// 检查括号是否平衡
fn is_balanced_parens(expr: &str) -> bool {
    let mut depth = 0;
    for c in expr.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// 在顶层分隔字符串（跳过括号和引号内的分隔符）
fn split_top_level<'a>(expr: &'a str, sep: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut last = 0;

    let bytes = expr.as_bytes();
    let sep_bytes = sep.as_bytes();
    let sep_len = sep_bytes.len();

    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => depth -= 1,
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            _ if depth == 0
                && !in_single
                && !in_double
                && i + sep_len <= bytes.len()
                && &bytes[i..i + sep_len] == sep_bytes =>
            {
                parts.push(&expr[last..i]);
                last = i + sep_len;
                i += sep_len;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&expr[last..]);
    parts
}

// ============================================================================
// 值解析与比较
// ============================================================================

/// 解析操作数为 Value（对齐 PHP `autoBuildVar`）
///
/// PHP 规则：
/// - `:` 开头 → 函数调用（NOTE: 函数调用支持）
/// - `$` 开头 → 变量引用
/// - 字母/下划线开头 → 自动补 `$` 前缀（视为变量名）
/// - 数字 → 数字字面量
/// - 引号包裹 → 字符串字面量
/// - `true`/`false`/`null` → 对应字面量
fn parse_operand_value(expr: &str, data: &ViewData) -> Value {
    let expr = expr.trim();
    if expr.is_empty() {
        return Value::Null;
    }

    // 变量 ($var 或 $var.attr)
    if let Some(rest) = expr.strip_prefix('$') {
        return resolve_var_expr(rest, data);
    }

    // 函数调用 (:func())
    if expr.starts_with(':') {
        // NOTE: 支持条件中的函数调用
        return Value::Null;
    }

    // 引号字符串
    if (expr.starts_with('\'') && expr.ends_with('\'') && expr.len() >= 2)
        || (expr.starts_with('"') && expr.ends_with('"') && expr.len() >= 2)
    {
        return Value::String(expr[1..expr.len() - 1].to_string());
    }

    // 数字
    if let Ok(n) = expr.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(f) = expr.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }

    // true / false / null
    match expr {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" => return Value::Null,
        _ => {}
    }

    // autoBuildVar：字母/下划线开头 → 视为变量名
    // 注：expr 已在 1245 行确认非空，next() 必返回 Some；
    // 使用 match 而非 unwrap() 以保持防御性（避免未来重构破坏不变量时 panic）
    match expr.chars().next() {
        Some(first) if first.is_alphabetic() || first == '_' => {
            return resolve_var_expr(expr, data);
        }
        _ => {}
    }

    Value::Null
}

/// 比较两个 Value（对齐 PHP 比较语义）
fn compare_values(left: &Value, op: &str, right: &Value) -> bool {
    match op {
        "==" => loose_equal(left, right),
        "!=" => !loose_equal(left, right),
        "===" => strict_equal(left, right),
        "!==" => !strict_equal(left, right),
        ">" => value_to_f64(left) > value_to_f64(right),
        "<" => value_to_f64(left) < value_to_f64(right),
        ">=" => value_to_f64(left) >= value_to_f64(right),
        "<=" => value_to_f64(left) <= value_to_f64(right),
        _ => false,
    }
}

/// PHP 松散相等（`==`）
///
/// - 数字与数字：数值比较
/// - 字符串与字符串：字符串比较
/// - 数字与字符串：尝试将字符串转数字比较，否则按字符串比较
/// - 其他：先转 bool 再比较
fn loose_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Number(a), Value::String(b)) => {
            if let Ok(n) = b.parse::<f64>() {
                a.as_f64() == Some(n)
            } else {
                a.to_string() == *b
            }
        }
        (Value::String(a), Value::Number(b)) => {
            if let Ok(n) = a.parse::<f64>() {
                Some(n) == b.as_f64()
            } else {
                *a == b.to_string()
            }
        }
        (Value::Bool(a), Value::Number(b)) => *a == (b.as_f64().unwrap_or(0.0) != 0.0),
        (Value::Number(a), Value::Bool(b)) => (a.as_f64().unwrap_or(0.0) != 0.0) == *b,
        (Value::Bool(a), Value::String(b)) => *a == is_truthy(&Value::String(b.clone())),
        (Value::String(a), Value::Bool(b)) => is_truthy(&Value::String(a.clone())) == *b,
        _ => left == right,
    }
}

/// PHP 严格相等（`===`）— 类型和值都必须相同
fn strict_equal(left: &Value, right: &Value) -> bool {
    left == right
}

/// Value 转 f64（对齐 PHP 数值转换）
fn value_to_f64(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse::<f64>().unwrap_or(0.0),
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Null => 0.0,
        Value::Array(a) => a.len() as f64,
        Value::Object(o) => o.len() as f64,
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 解析标签属性（对齐 PHP `TagLib::parseXmlTag` 属性解析）
///
/// 支持双引号和单引号：`key="value"` 或 `key='value'`
fn parse_attributes(tag: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let re = Regex::new(r#"(\w+)=["']([^"']*)["']"#).unwrap_or_else(|e| panic!("attr regex: {e}"));
    for cap in re.captures_iter(tag) {
        attrs.insert(cap[1].to_string(), cap[2].to_string());
    }
    attrs
}

/// 替换条件简写（对齐 PHP `TagLib::parseCondition` 的 `str_ireplace`）
///
/// PHP `$comparison` 映射（大小写不敏感）：
/// - ` eq ` → ` == `
/// - ` neq ` → ` != `
/// - ` gt ` → ` > `
/// - ` lt ` → ` < `
/// - ` egt ` → ` >= `
/// - ` elt ` → ` <= `
/// - ` heq ` → ` === `
/// - ` nheq ` → ` !== `
fn replace_condition_shorthands(condition: &str) -> String {
    // PHP 使用 str_ireplace（大小写不敏感），Rust 用 to_lowercase 模拟
    let mut result = condition.to_string();

    // 按长度降序替换，避免短简写覆盖长简写（如 "nheq" 被 "heq" 部分匹配）
    let replacements: &[(&str, &str)] = &[
        (" nheq ", " !== "),
        (" heq ", " === "),
        (" neq ", " != "),
        (" egt ", " >= "),
        (" elt ", " <= "),
        (" eq ", " == "),
        (" gt ", " > "),
        (" lt ", " < "),
    ];

    for (from, to) in replacements {
        let mut search_pos = 0;
        loop {
            // 每次重新生成 lower，确保位置与 result 同步
            let lower = result.to_lowercase();
            match lower[search_pos..].find(from) {
                Some(pos) => {
                    let abs_pos = search_pos + pos;
                    result.replace_range(abs_pos..abs_pos + from.len(), to);
                    search_pos = abs_pos + to.len();
                }
                None => break,
            }
        }
    }

    result
}

/// 解析变量名（对齐 PHP `autoBuildVar`）
///
/// - `:` 开头 → 函数调用（NOTE: 暂不支持）
/// - `$` 开头 → 去掉 `$` 后解析变量
/// - 其他 → 直接作为变量名解析
fn resolve_name(name: &str, data: &ViewData) -> Value {
    let name = name.trim();
    if name.is_empty() {
        return Value::Null;
    }

    // 函数调用
    if name.starts_with(':') {
        // NOTE: 支持函数调用
        return Value::Null;
    }

    // 去掉 $ 前缀
    let var_name = name.strip_prefix('$').unwrap_or(name);
    resolve_var_expr(var_name, data)
}

/// 解析 case value（对齐 PHP `tagCase` 的 value 解析）
///
/// - `$` 开头 → 变量引用
/// - `:` 开头 → 函数调用（NOTE）
/// - 其他 → 字面量
fn resolve_case_value(value: &str, data: &ViewData) -> Value {
    let value = value.trim();
    if value.is_empty() {
        return Value::Null;
    }

    if let Some(rest) = value.strip_prefix('$') {
        return resolve_var_expr(rest, data);
    }

    if value.starts_with(':') {
        return Value::Null;
    }

    parse_literal(value)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// 创建默认配置
    fn make_config() -> ViewConfig {
        ViewConfig::default()
    }

    /// 创建测试数据
    fn make_data() -> ViewData {
        let mut data = HashMap::new();
        data.insert("a".to_string(), json!(1));
        data.insert("b".to_string(), json!(2));
        data.insert("name".to_string(), json!("Alice"));
        data.insert("list".to_string(), json!([1, 2, 3]));
        data.insert("empty_list".to_string(), json!([]));
        data.insert("obj".to_string(), json!({"x": 10, "y": 20}));
        data.insert("var".to_string(), json!(2));
        data.insert("flag".to_string(), json!(true));
        data
    }

    /// 渲染辅助函数：使用默认配置和递归内部渲染器
    ///
    /// render_recursive 先处理控制流标签（递归），再处理 {$var} 标签
    fn render(template: &str, data: &ViewData) -> String {
        let config = make_config();

        fn render_recursive(
            content: &str,
            d: &ViewData,
            config: &ViewConfig,
        ) -> Result<String, ViewError> {
            // 先处理控制流标签（递归）
            let content =
                render_control_flow(content, d, config, |c, d2| render_recursive(c, d2, config))?;
            // 再处理 {$var} 标签
            let begin = &config.tpl_begin;
            let end = &config.tpl_end;
            let pattern = format!("{}(.*?){}", regex::escape(begin), regex::escape(end));
            let re = Regex::new(&pattern).unwrap();
            let mut result = String::new();
            let mut last = 0;
            for caps in re.captures_iter(&content) {
                let m = caps.get(0).unwrap();
                let tag = caps.get(1).unwrap().as_str().trim();
                result.push_str(&content[last..m.start()]);
                if let Some(rest) = tag.strip_prefix('$') {
                    let val = resolve_var_expr(rest, d);
                    result.push_str(&value_to_string(&val));
                } else {
                    result.push_str(&content[m.start()..m.end()]);
                }
                last = m.end();
            }
            result.push_str(&content[last..]);
            Ok(result)
        }

        render_recursive(template, data, &config).unwrap()
    }

    // =========================================================================
    // 组 1：{if} 条件判断
    // =========================================================================

    #[test]
    fn test_if_true() {
        let data = make_data();
        let result = render("{if condition=\"$a eq 1\"}yes{/if}", &data);
        assert_eq!(result, "yes");
    }

    #[test]
    fn test_if_false() {
        let data = make_data();
        let result = render("{if condition=\"$a eq 2\"}yes{/if}", &data);
        assert_eq!(result, "");
    }

    #[test]
    fn test_if_else_true() {
        let data = make_data();
        let result = render("{if condition=\"$a eq 1\"}yes{else /}no{/if}", &data);
        assert_eq!(result, "yes");
    }

    #[test]
    fn test_if_else_false() {
        let data = make_data();
        let result = render("{if condition=\"$a eq 2\"}yes{else /}no{/if}", &data);
        assert_eq!(result, "no");
    }

    #[test]
    fn test_if_elseif_else() {
        let data = make_data();
        let result = render(
            "{if condition=\"$a eq 2\"}two{elseif condition=\"$a eq 1\" /}one{else /}other{/if}",
            &data,
        );
        assert_eq!(result, "one");
    }

    #[test]
    fn test_if_elseif_else_default() {
        let data = make_data();
        let result = render(
            "{if condition=\"$a eq 5\"}five{elseif condition=\"$a eq 6\" /}six{else /}other{/if}",
            &data,
        );
        assert_eq!(result, "other");
    }

    #[test]
    fn test_if_with_neq() {
        let data = make_data();
        let result = render("{if condition=\"$a neq 2\"}not_two{/if}", &data);
        assert_eq!(result, "not_two");
    }

    #[test]
    fn test_if_with_gt_lt() {
        let data = make_data();
        assert_eq!(render("{if condition=\"$b gt 1\"}big{/if}", &data), "big");
        assert_eq!(
            render("{if condition=\"$a lt 2\"}small{/if}", &data),
            "small"
        );
    }

    #[test]
    fn test_if_with_egt_elt() {
        let data = make_data();
        assert_eq!(render("{if condition=\"$a egt 1\"}ge1{/if}", &data), "ge1");
        assert_eq!(render("{if condition=\"$b elt 2\"}le2{/if}", &data), "le2");
    }

    #[test]
    fn test_if_with_heq_nheq() {
        let data = make_data();
        assert_eq!(
            render("{if condition=\"$a heq 1\"}identical{/if}", &data),
            "identical"
        );
        assert_eq!(
            render("{if condition=\"$a nheq 2\"}not_identical{/if}", &data),
            "not_identical"
        );
    }

    #[test]
    fn test_if_expression_form() {
        let data = make_data();
        let result = render("{if ($a == 1)}match{/if}", &data);
        assert_eq!(result, "match");
    }

    #[test]
    fn test_if_with_and() {
        let data = make_data();
        let result = render("{if condition=\"$a gt 0 && $b lt 5\"}in_range{/if}", &data);
        assert_eq!(result, "in_range");
    }

    #[test]
    fn test_if_with_or() {
        let data = make_data();
        let result = render("{if condition=\"$a eq 5 || $b eq 2\"}match{/if}", &data);
        assert_eq!(result, "match");
    }

    #[test]
    fn test_if_with_not() {
        let data = make_data();
        let result = render("{if condition=\"!$flag\"}not_flag{/if}", &data);
        assert_eq!(result, "");
    }

    #[test]
    fn test_if_with_parens() {
        let data = make_data();
        let result = render("{if condition=\"($a eq 1) && ($b eq 2)\"}both{/if}", &data);
        assert_eq!(result, "both");
    }

    #[test]
    fn test_if_case_insensitive_shorthand() {
        let data = make_data();
        // PHP str_ireplace 是大小写不敏感的
        let result = render("{if condition=\"$a EQ 1\"}yes{/if}", &data);
        assert_eq!(result, "yes");
    }

    // =========================================================================
    // 组 2：{foreach} 循环
    // =========================================================================

    #[test]
    fn test_foreach_basic() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\"}{$item},{/foreach}",
            &data,
        );
        assert_eq!(result, "1,2,3,");
    }

    #[test]
    fn test_foreach_with_key() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\" key=\"k\"}{$k}={$item},{/foreach}",
            &data,
        );
        assert_eq!(result, "0=1,1=2,2=3,");
    }

    #[test]
    fn test_foreach_with_index() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\" index=\"idx\"}{$idx}:{$item},{/foreach}",
            &data,
        );
        assert_eq!(result, "0:1,1:2,2:3,");
    }

    #[test]
    fn test_foreach_object() {
        let data = make_data();
        let result = render(
            "{foreach name=\"obj\" id=\"v\" key=\"k\"}{$k}={$v},{/foreach}",
            &data,
        );
        assert_eq!(result, "x=10,y=20,");
    }

    #[test]
    fn test_foreach_empty() {
        let data = make_data();
        let result = render(
            "{foreach name=\"empty_list\" id=\"item\" empty=\"no data\"}{$item}{/foreach}",
            &data,
        );
        assert_eq!(result, "no data");
    }

    #[test]
    fn test_foreach_expression_form() {
        let data = make_data();
        let result = render("{foreach ($list as $k=>$v)}{$k}={$v},{/foreach}", &data);
        assert_eq!(result, "0=1,1=2,2=3,");
    }

    #[test]
    fn test_foreach_expression_form_no_key() {
        let data = make_data();
        let result = render("{foreach ($list as $v)}{$v},{/foreach}", &data);
        assert_eq!(result, "1,2,3,");
    }

    #[test]
    fn test_foreach_with_offset() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\" offset=\"1\"}{$item},{/foreach}",
            &data,
        );
        assert_eq!(result, "2,3,");
    }

    #[test]
    fn test_foreach_with_length() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\" length=\"2\"}{$item},{/foreach}",
            &data,
        );
        assert_eq!(result, "1,2,");
    }

    // =========================================================================
    // 组 3：{volist} 循环
    // =========================================================================

    #[test]
    fn test_volist_basic() {
        let data = make_data();
        let result = render("{volist name=\"list\" id=\"item\"}{$item},{/volist}", &data);
        assert_eq!(result, "1,2,3,");
    }

    #[test]
    fn test_volist_key_is_counter() {
        // PHP volist: key 属性是计数器变量名（默认 i，1-based）
        let data = make_data();
        let result = render(
            "{volist name=\"list\" id=\"item\" key=\"i\"}{$i}:{$item},{/volist}",
            &data,
        );
        assert_eq!(result, "1:1,2:2,3:3,");
    }

    #[test]
    fn test_volist_key_default_is_i() {
        let data = make_data();
        let result = render(
            "{volist name=\"list\" id=\"item\" key=\"counter\"}{$counter}:{$item},{/volist}",
            &data,
        );
        assert_eq!(result, "1:1,2:2,3:3,");
    }

    #[test]
    fn test_volist_array_key_variable() {
        // PHP volist: 数组键总是赋给字面变量 $key
        let data = make_data();
        let result = render(
            "{volist name=\"list\" id=\"item\"}key={$key},item={$item};{/volist}",
            &data,
        );
        assert_eq!(result, "key=0,item=1;key=1,item=2;key=2,item=3;");
    }

    #[test]
    fn test_volist_empty() {
        let data = make_data();
        let result = render(
            "{volist name=\"empty_list\" id=\"item\" empty=\"no data\"}{$item}{/volist}",
            &data,
        );
        assert_eq!(result, "no data");
    }

    #[test]
    fn test_volist_with_offset() {
        let data = make_data();
        let result = render(
            "{volist name=\"list\" id=\"item\" offset=\"1\"}{$item},{/volist}",
            &data,
        );
        assert_eq!(result, "2,3,");
    }

    #[test]
    fn test_volist_with_length() {
        let data = make_data();
        let result = render(
            "{volist name=\"list\" id=\"item\" length=\"2\"}{$item},{/volist}",
            &data,
        );
        assert_eq!(result, "1,2,");
    }

    // =========================================================================
    // 组 4：{switch}/{case}/{default}
    // =========================================================================

    #[test]
    fn test_switch_case_match() {
        let data = make_data();
        let result = render(
            "{switch name=\"var\"}{case value=\"1\"}one{/case}{case value=\"2\"}two{/case}{default /}other{/switch}",
            &data,
        );
        assert_eq!(result, "two");
    }

    #[test]
    fn test_switch_default() {
        let data = make_data();
        let result = render(
            "{switch name=\"var\"}{case value=\"5\"}five{/case}{default /}other{/switch}",
            &data,
        );
        assert_eq!(result, "other");
    }

    #[test]
    fn test_switch_case_multi_value() {
        let data = make_data();
        let result = render(
            "{switch name=\"var\"}{case value=\"1|2|3\"}low{/case}{default /}high{/switch}",
            &data,
        );
        assert_eq!(result, "low");
    }

    #[test]
    fn test_switch_case_no_close_tag() {
        // 不使用 {/case} 闭合
        let data = make_data();
        let result = render(
            "{switch name=\"var\"}{case value=\"1\"}one{case value=\"2\"}two{default /}other{/switch}",
            &data,
        );
        assert_eq!(result, "two");
    }

    #[test]
    fn test_switch_case_break_false_still_breaks() {
        // PHP 陷阱：break="false" 中 "false" 是 truthy，实际会 break
        let data = make_data();
        let result = render(
            "{switch name=\"var\"}{case value=\"2\" break=\"false\"}two{/case}{case value=\"2\"}also_two{/case}{default /}other{/switch}",
            &data,
        );
        assert_eq!(result, "two");
    }

    #[test]
    fn test_switch_case_break_zero_fall_through() {
        // break="0" 中 "0" 是 falsy，实现贯穿
        let data = make_data();
        let result = render(
            "{switch name=\"var\"}{case value=\"2\" break=\"0\"}two_{/case}{case value=\"2\"}also_two{/case}{default /}other{/switch}",
            &data,
        );
        assert_eq!(result, "two_also_two");
    }

    // =========================================================================
    // 组 5：{for} 循环
    // =========================================================================

    #[test]
    fn test_for_basic() {
        let data = make_data();
        let result = render("{for start=\"0\" end=\"3\" name=\"i\"}{$i},{/for}", &data);
        assert_eq!(result, "0,1,2,");
    }

    #[test]
    fn test_for_with_step() {
        let data = make_data();
        let result = render(
            "{for start=\"0\" end=\"10\" step=\"3\" name=\"i\"}{$i},{/for}",
            &data,
        );
        assert_eq!(result, "0,3,6,9,");
    }

    #[test]
    fn test_for_default_name() {
        let data = make_data();
        let result = render("{for start=\"1\" end=\"3\"}{$i},{/for}", &data);
        assert_eq!(result, "1,2,");
    }

    #[test]
    fn test_for_comparison_elt() {
        let data = make_data();
        let result = render(
            "{for start=\"0\" end=\"3\" comparison=\"elt\" name=\"i\"}{$i},{/for}",
            &data,
        );
        assert_eq!(result, "0,1,2,3,");
    }

    #[test]
    fn test_for_comparison_gt() {
        let data = make_data();
        let result = render(
            "{for start=\"5\" end=\"0\" step=\"-1\" comparison=\"gt\" name=\"i\"}{$i},{/for}",
            &data,
        );
        assert_eq!(result, "5,4,3,2,1,");
    }

    // =========================================================================
    // 组 6：嵌套控制流
    // =========================================================================

    #[test]
    fn test_nested_if_in_foreach() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\"}{if condition=\"$item gt 1\"}big{else /}small{/if},{/foreach}",
            &data,
        );
        assert_eq!(result, "small,big,big,");
    }

    #[test]
    fn test_nested_foreach() {
        let mut data = make_data();
        data.insert("matrix".to_string(), json!([[1, 2], [3, 4]]));
        let result = render(
            "{foreach name=\"matrix\" id=\"row\"}{foreach name=\"row\" id=\"col\"}{$col} {/foreach}| {/foreach}",
            &data,
        );
        assert_eq!(result, "1 2 | 3 4 | ");
    }

    #[test]
    fn test_nested_if() {
        let data = make_data();
        let result = render(
            "{if condition=\"$a eq 1\"}{if condition=\"$b eq 2\"}both{/if}{/if}",
            &data,
        );
        assert_eq!(result, "both");
    }

    #[test]
    fn test_switch_in_foreach() {
        let data = make_data();
        let result = render(
            "{foreach name=\"list\" id=\"item\"}{switch name=\"item\"}{case value=\"1\"}one{/case}{case value=\"2\"}two{/case}{default /}other{/switch},{/foreach}",
            &data,
        );
        assert_eq!(result, "one,two,other,");
    }

    // =========================================================================
    // 组 7：条件求值器单元测试
    // =========================================================================

    #[test]
    fn test_evaluate_condition_eq() {
        let data = make_data();
        assert!(evaluate_condition("$a eq 1", &data).unwrap());
        assert!(!evaluate_condition("$a eq 2", &data).unwrap());
    }

    #[test]
    fn test_evaluate_condition_neq() {
        let data = make_data();
        assert!(evaluate_condition("$a neq 2", &data).unwrap());
    }

    #[test]
    fn test_evaluate_condition_and() {
        let data = make_data();
        assert!(evaluate_condition("$a eq 1 && $b eq 2", &data).unwrap());
        assert!(!evaluate_condition("$a eq 1 && $b eq 3", &data).unwrap());
    }

    #[test]
    fn test_evaluate_condition_or() {
        let data = make_data();
        assert!(evaluate_condition("$a eq 5 || $b eq 2", &data).unwrap());
        assert!(!evaluate_condition("$a eq 5 || $b eq 5", &data).unwrap());
    }

    #[test]
    fn test_evaluate_condition_not() {
        let data = make_data();
        assert!(!evaluate_condition("!$flag", &data).unwrap());
        assert!(!evaluate_condition("!$a", &data).unwrap()); // $a=1 is truthy
    }

    #[test]
    fn test_evaluate_condition_parens() {
        let data = make_data();
        assert!(evaluate_condition("($a eq 1) && ($b eq 2)", &data).unwrap());
    }

    #[test]
    fn test_evaluate_condition_truthy_var() {
        let data = make_data();
        assert!(evaluate_condition("$flag", &data).unwrap());
        assert!(evaluate_condition("$name", &data).unwrap());
    }

    #[test]
    fn test_evaluate_condition_strict_equal() {
        let data = make_data();
        assert!(evaluate_condition("$a heq 1", &data).unwrap());
        assert!(!evaluate_condition("$a heq '1'", &data).unwrap());
    }

    // =========================================================================
    // 组 8：辅助函数单元测试
    // =========================================================================

    #[test]
    fn test_replace_shorthands_basic() {
        assert_eq!(replace_condition_shorthands("$a eq 1"), "$a == 1");
        assert_eq!(replace_condition_shorthands("$a neq 1"), "$a != 1");
        assert_eq!(replace_condition_shorthands("$a gt 1"), "$a > 1");
        assert_eq!(replace_condition_shorthands("$a lt 1"), "$a < 1");
        assert_eq!(replace_condition_shorthands("$a egt 1"), "$a >= 1");
        assert_eq!(replace_condition_shorthands("$a elt 1"), "$a <= 1");
        assert_eq!(replace_condition_shorthands("$a heq 1"), "$a === 1");
        assert_eq!(replace_condition_shorthands("$a nheq 1"), "$a !== 1");
    }

    #[test]
    fn test_replace_shorthands_case_insensitive() {
        assert_eq!(replace_condition_shorthands("$a EQ 1"), "$a == 1");
        assert_eq!(replace_condition_shorthands("$a NEQ 1"), "$a != 1");
    }

    #[test]
    fn test_replace_shorthands_multiple() {
        assert_eq!(
            replace_condition_shorthands("$a eq 1 && $b neq 2"),
            "$a == 1 && $b != 2"
        );
    }

    #[test]
    fn test_parse_attributes() {
        let attrs = parse_attributes("name=\"list\" id=\"item\" key=\"k\"");
        assert_eq!(attrs.get("name"), Some(&"list".to_string()));
        assert_eq!(attrs.get("id"), Some(&"item".to_string()));
        assert_eq!(attrs.get("key"), Some(&"k".to_string()));
    }

    #[test]
    fn test_parse_attributes_single_quote() {
        let attrs = parse_attributes("name='list' id='item'");
        assert_eq!(attrs.get("name"), Some(&"list".to_string()));
        assert_eq!(attrs.get("id"), Some(&"item".to_string()));
    }

    #[test]
    fn test_parse_attributes_empty() {
        let attrs = parse_attributes("");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_loose_equal_numbers() {
        assert!(loose_equal(&json!(1), &json!(1)));
        assert!(!loose_equal(&json!(1), &json!(2)));
    }

    #[test]
    fn test_loose_equal_number_string() {
        assert!(loose_equal(&json!(1), &json!("1")));
        assert!(loose_equal(&json!("1"), &json!(1)));
    }

    #[test]
    fn test_loose_equal_strings() {
        assert!(loose_equal(&json!("hello"), &json!("hello")));
        assert!(!loose_equal(&json!("hello"), &json!("world")));
    }

    #[test]
    fn test_strict_equal() {
        assert!(strict_equal(&json!(1), &json!(1)));
        assert!(!strict_equal(&json!(1), &json!("1")));
        assert!(!strict_equal(&json!(1), &json!(true)));
    }

    #[test]
    fn test_split_top_level_and() {
        let parts = split_top_level("$a eq 1 && $b eq 2", "&&");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].trim(), "$a eq 1");
        assert_eq!(parts[1].trim(), "$b eq 2");
    }

    #[test]
    fn test_split_top_level_or() {
        let parts = split_top_level("$a eq 1 || $b eq 2", "||");
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_split_top_level_with_parens() {
        let parts = split_top_level("($a eq 1 || $b eq 2) && $c eq 3", "&&");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].trim(), "($a eq 1 || $b eq 2)");
        assert_eq!(parts[1].trim(), "$c eq 3");
    }

    #[test]
    fn test_find_comparison_operator() {
        assert_eq!(find_comparison_operator("$a == 1"), Some(("==", 3)));
        assert_eq!(find_comparison_operator("$a === 1"), Some(("===", 3)));
        assert_eq!(find_comparison_operator("$a >= 1"), Some((">=", 3)));
    }

    #[test]
    fn test_find_comparison_operator_in_parens() {
        // 运算符在括号内不应被找到
        assert!(find_comparison_operator("($a == 1)").is_none());
    }

    // =========================================================================
    // 组 9：SwitchCase should_break 逻辑
    // =========================================================================

    #[test]
    fn test_should_break_default() {
        let case = SwitchCase {
            kind: CaseKind::Default,
            content: String::new(),
            break_attr: None,
        };
        assert!(case.should_break()); // 默认 break
    }

    #[test]
    fn test_should_break_false_string() {
        // PHP 陷阱：break="false" 中 "false" 是 truthy
        let case = SwitchCase {
            kind: CaseKind::Values(vec!["1".to_string()]),
            content: String::new(),
            break_attr: Some("false".to_string()),
        };
        assert!(case.should_break()); // "false" 是 truthy，仍 break
    }

    #[test]
    fn test_should_break_zero_string() {
        // break="0" 中 "0" 是 falsy
        let case = SwitchCase {
            kind: CaseKind::Values(vec!["1".to_string()]),
            content: String::new(),
            break_attr: Some("0".to_string()),
        };
        assert!(!case.should_break()); // "0" 是 falsy，不 break（贯穿）
    }

    #[test]
    fn test_should_break_empty_string() {
        // break="" 等于 PHP ''，是 falsy
        let case = SwitchCase {
            kind: CaseKind::Values(vec!["1".to_string()]),
            content: String::new(),
            break_attr: Some("".to_string()),
        };
        // PHP: '' == '' → true，但 PHP 真值中空字符串是 false
        // PHP 代码: if ('' == $isBreak || $isBreak) → '' == '' 为 true → break
        assert!(!case.should_break()); // is_truthy("") = false
    }

    // =========================================================================
    // 组 10：边界情况
    // =========================================================================

    #[test]
    fn test_no_control_flow_tags() {
        let data = make_data();
        let result = render("hello world", &data);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_control_flow_with_static_text() {
        let data = make_data();
        let result = render("before {if condition=\"$a eq 1\"}mid{/if} after", &data);
        assert_eq!(result, "before mid after");
    }

    #[test]
    fn test_multiple_control_flow_blocks() {
        let data = make_data();
        let result = render(
            "{if condition=\"$a eq 1\"}A{/if}{if condition=\"$b eq 2\"}B{/if}",
            &data,
        );
        assert_eq!(result, "AB");
    }

    #[test]
    fn test_for_zero_iterations() {
        let data = make_data();
        let result = render("{for start=\"5\" end=\"3\" name=\"i\"}{$i}{/for}", &data);
        assert_eq!(result, "");
    }

    #[test]
    fn test_foreach_empty_no_empty_attr() {
        let data = make_data();
        let result = render(
            "{foreach name=\"empty_list\" id=\"item\"}{$item}{/foreach}",
            &data,
        );
        assert_eq!(result, "");
    }
}

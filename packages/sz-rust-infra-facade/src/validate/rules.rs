// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 内置验证规则 — 对齐 PHP `think\Validate` 类的内置规则方法
//!
//! 本模块实现 PHP `think\Validate` 类中除 `require`/`must`/`is`/`regex` 外的
//! 所有内置规则方法。
//!
//! ## PHP 对齐
//!
//! 所有规则函数签名统一为：
//! ```ignore
//! fn rule(value: &Value, rule: &str, data: &Value, field: &str) -> bool
//! ```
//!
//! 对齐 PHP `$this->$type($value, $rule, $data, $field, $title)` 调用。
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\Validate.php`
//!   - 第 717-728 行：`confirm`
//!   - 第 738-741 行：`different`
//!   - 第 751-754 行：`egt`
//!   - 第 764-767 行：`gt`
//!   - 第 777-780 行：`elt`
//!   - 第 790-793 行：`lt`
//!   - 第 802-805 行：`eq`
//!   - 第 926-933 行：`activeUrl`
//!   - 第 942-949 行：`ip`
//!   - 第 1109-1113 行：`dateFormat`
//!   - 第 1200-1209 行：`requireIf`
//!   - 第 1238-1247 行：`requireWith`
//!   - 第 1257-1266 行：`requireWithout`
//!   - 第 1275-1278 行：`in`
//!   - 第 1287-1290 行：`notIn`
//!   - 第 1299-1307 行：`between`
//!   - 第 1316-1324 行：`notBetween`
//!   - 第 1333-1351 行：`length`
//!   - 第 1360-1371 行：`max`
//!   - 第 1380-1391 行：`min`
//!   - 第 1401-1404 行：`after`
//!   - 第 1414-1417 行：`before`
//!   - 第 1427-1431 行：`afterWith`
//!   - 第 1441-1445 行：`beforeWith`
//!   - 第 1454-1471 行：`expire`
//!   - 第 1480-1483 行：`allowIp`
//!   - 第 1492-1495 行：`denyIp`

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;
use std::net::IpAddr;

use crate::validate::{is_empty_value, Validate};

// ============================================================================
// 内部辅助函数
// ============================================================================

/// 将 Value 转为字符串（对齐 PHP `(string) $value`）
fn value_as_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => {
            if *b {
                "1".to_string()
            } else {
                String::new()
            }
        }
        Value::Null => String::new(),
        _ => String::new(),
    }
}

/// 将 Value 转为 f64（对齐 PHP 数字字符串转数字）
///
/// PHP 松散比较中，数字字符串会被当作数字处理。
/// 本函数处理 Value::Number 和 Value::String 两种情况。
fn value_as_f64(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return Some(n);
    }
    if let Value::String(s) = value {
        return s.parse::<f64>().ok();
    }
    None
}

/// PHP 松散比较 `==` — 对齐 PHP `==` 运算符
///
/// ## PHP 行为
///
/// - 数字与数字字符串按数字比较（`1 == "1"` 为 true）
/// - 其他按字符串比较
fn value_loose_equals_str(value: &Value, other: &str) -> bool {
    // 尝试数字比较（PHP 松散比较：数字字符串按数字比较）
    if let Some(v_num) = value_as_f64(value) {
        if let Ok(o_num) = other.parse::<f64>() {
            return v_num == o_num;
        }
    }
    // 字符串比较
    value_as_string(value) == other
}

/// PHP 松散比较 `==`（Value 与 Value）
fn value_loose_equals(value: &Value, other: &Value) -> bool {
    // 尝试数字比较（PHP 松散比较：数字字符串按数字比较）
    if let (Some(v_num), Some(o_num)) = (value_as_f64(value), value_as_f64(other)) {
        return v_num == o_num;
    }
    // 字符串比较
    value_as_string(value) == value_as_string(other)
}

/// PHP 松散比较 `>=`/`>`/`<=`/`<`（Value 与 Value）
///
/// 返回 `Some(Ordering)` 表示可比较，`None` 表示不可比较（视为不满足）
fn value_loose_compare(value: &Value, other: &Value) -> Option<std::cmp::Ordering> {
    // 尝试数字比较（PHP 松散比较：数字字符串按数字比较）
    if let (Some(v_num), Some(o_num)) = (value_as_f64(value), value_as_f64(other)) {
        return v_num.partial_cmp(&o_num);
    }
    // 字符串比较
    Some(value_as_string(value).cmp(&value_as_string(other)))
}

/// 解析时间戳（对齐 PHP `strtotime`）
///
/// PHP `strtotime` 解析多种日期格式，返回 Unix 时间戳。
/// 本函数尝试常见格式解析，返回 Unix 时间戳（秒）。
fn parse_timestamp(value: &Value) -> Option<i64> {
    let s = match value {
        Value::String(s) => s.as_str(),
        Value::Number(n) => {
            // 数字直接作为时间戳
            if let Some(i) = n.as_i64() {
                return Some(i);
            }
            return None;
        }
        _ => return None,
    };

    // 尝试 RFC3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    // 尝试常见格式
    let formats: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%SZ",
    ];
    for fmt in formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp());
        }
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return d.and_hms_opt(0, 0, 0).map(|t| t.and_utc().timestamp());
        }
    }
    None
}

/// 将 PHP 日期格式字符串转换为 chrono 格式字符串
///
/// PHP 格式说明符参考：https://www.php.net/manual/en/datetime.format.php
fn php_date_format_to_chrono(php_format: &str) -> String {
    let mut result = String::new();
    let mut chars = php_format.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // 年
            'Y' => result.push_str("%Y"),
            'y' => result.push_str("%y"),
            // 月
            'm' => result.push_str("%m"),
            'n' => result.push_str("%_m"),
            // 日
            'd' => result.push_str("%d"),
            'j' => result.push_str("%_d"),
            // 时
            'H' => result.push_str("%H"),
            'G' => result.push_str("%_H"),
            // 分
            'i' => result.push_str("%M"),
            // 秒
            's' => result.push_str("%S"),
            // AM/PM
            'a' | 'A' => result.push_str("%P"),
            // 转义字符
            '\\' => {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
            _ => result.push(c),
        }
    }
    result
}

// ============================================================================
// 比较类规则
// ============================================================================

/// 验证是否等于某个值 — 对齐 PHP `eq`
///
/// 对齐 PHP `Validate.php` 第 802-805 行
///
/// ## PHP 行为
///
/// `return $value == $rule;`（松散比较）
///
/// - 数字与数字字符串按数字比较（`1 == "1"` 为 true）
/// - 其他按字符串比较
pub fn eq(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    value_loose_equals_str(value, rule)
}

/// 验证是否大于等于某个字段的值 — 对齐 PHP `egt`
///
/// 对齐 PHP `Validate.php` 第 751-754 行
///
/// ## PHP 行为
///
/// `return $value >= $this->getDataValue($data, $rule);`
///
/// **注意**：PHP `egt` 的 `$rule` 是字段名，比较 `value` 与 `data[rule]` 的值。
pub fn egt(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    matches!(
        value_loose_compare(value, &other),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

/// 验证是否大于某个字段的值 — 对齐 PHP `gt`
///
/// 对齐 PHP `Validate.php` 第 764-767 行
pub fn gt(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    matches!(
        value_loose_compare(value, &other),
        Some(std::cmp::Ordering::Greater)
    )
}

/// 验证是否小于等于某个字段的值 — 对齐 PHP `elt`
///
/// 对齐 PHP `Validate.php` 第 777-780 行
pub fn elt(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    matches!(
        value_loose_compare(value, &other),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Less)
    )
}

/// 验证是否小于某个字段的值 — 对齐 PHP `lt`
///
/// 对齐 PHP `Validate.php` 第 790-793 行
pub fn lt(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    matches!(
        value_loose_compare(value, &other),
        Some(std::cmp::Ordering::Less)
    )
}

/// 验证是否和某个字段的值是否一致 — 对齐 PHP `confirm`
///
/// 对齐 PHP `Validate.php` 第 717-728 行
///
/// ## PHP 行为
///
/// - 如果 `rule` 为空，根据 `field` 推断确认字段名（`field_confirm` 或 `field + '_confirm'`）
/// - 比较 `value` 与 `data[rule]` 的值（严格比较 `===`）
pub fn confirm(value: &Value, rule: &str, data: &Value, field: &str) -> bool {
    let confirm_field = if rule.is_empty() {
        if field.contains("_confirm") {
            field.split("_confirm").next().unwrap_or("").to_string()
        } else {
            format!("{}_confirm", field)
        }
    } else {
        rule.to_string()
    };
    let other = Validate::get_data_value(data, &confirm_field);
    // PHP 使用 === 严格比较
    value == &other
}

/// 验证是否和某个字段的值是否不同 — 对齐 PHP `different`
///
/// 对齐 PHP `Validate.php` 第 738-741 行
///
/// ## PHP 行为
///
/// `return $this->getDataValue($data, $rule) != $value;`（松散比较 `!=`）
pub fn different(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    !value_loose_equals(value, &other)
}

// ============================================================================
// 范围类规则
// ============================================================================

/// 验证是否在范围内 — 对齐 PHP `in`
///
/// 对齐 PHP `Validate.php` 第 1275-1278 行
///
/// ## PHP 行为
///
/// `return in_array($value, is_array($rule) ? $rule : explode(',', $rule));`
///
/// **注意**：PHP `in_array` 默认是松散比较。
pub fn in_rule(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let items: Vec<&str> = rule.split(',').collect();
    for item in items {
        let item = item.trim();
        if value_loose_equals_str(value, item) {
            return true;
        }
    }
    false
}

/// 验证是否不在某个范围 — 对齐 PHP `notIn`
///
/// 对齐 PHP `Validate.php` 第 1287-1290 行
pub fn not_in(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    !in_rule(value, rule, _data, _field)
}

/// between 验证数据 — 对齐 PHP `between`
///
/// 对齐 PHP `Validate.php` 第 1299-1307 行
///
/// ## PHP 行为
///
/// ```php
/// [$min, $max] = explode(',', $rule);
/// return $value >= $min && $value <= $max;
/// ```
pub fn between(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let parts: Vec<&str> = rule.split(',').collect();
    if parts.len() < 2 {
        return false;
    }
    let min = parts[0].trim();
    let max = parts[1].trim();
    // PHP 松散比较
    let ge_min = value_loose_compare_str(value, min)
        .map(|o| o != std::cmp::Ordering::Less)
        .unwrap_or(false);
    let le_max = value_loose_compare_str(value, max)
        .map(|o| o != std::cmp::Ordering::Greater)
        .unwrap_or(false);
    ge_min && le_max
}

/// notBetween 验证数据 — 对齐 PHP `notBetween`
///
/// 对齐 PHP `Validate.php` 第 1316-1324 行
pub fn not_between(value: &Value, rule: &str, data: &Value, field: &str) -> bool {
    !between(value, rule, data, field)
}

/// PHP 松散比较（Value 与 &str）
fn value_loose_compare_str(value: &Value, other: &str) -> Option<std::cmp::Ordering> {
    // 尝试数字比较（PHP 松散比较：数字字符串按数字比较）
    if let Some(v_num) = value_as_f64(value) {
        if let Ok(o_num) = other.parse::<f64>() {
            return v_num.partial_cmp(&o_num);
        }
    }
    // 字符串比较
    Some(value_as_string(value).as_str().cmp(other))
}

// ============================================================================
// 长度类规则
// ============================================================================

/// 计算值的长度（对齐 PHP `mb_strlen((string) $value)`）
///
/// - 数组：元素个数
/// - 字符串：Unicode 字符数（对齐 PHP `mb_strlen`）
/// - 其他：字符串表示的长度
fn value_length(value: &Value) -> usize {
    match value {
        Value::Array(a) => a.len(),
        Value::Object(o) => o.len(),
        Value::String(s) => s.chars().count(),
        _ => value_as_string(value).chars().count(),
    }
}

/// 验证数据长度 — 对齐 PHP `length`
///
/// 对齐 PHP `Validate.php` 第 1333-1351 行
///
/// ## PHP 行为
///
/// - 数组：`count($value)`
/// - 字符串：`mb_strlen((string) $value)`
/// - 如果 `rule` 包含 `,`，为长度区间 `[min, max]`
/// - 否则为指定长度
pub fn length(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let len = value_length(value);
    if let Some(idx) = rule.find(',') {
        let min_str = rule[..idx].trim();
        let max_str = rule[idx + 1..].trim();
        let min: usize = min_str.parse().unwrap_or(0);
        let max: usize = max_str.parse().unwrap_or(0);
        len >= min && len <= max
    } else {
        let target: usize = rule.parse().unwrap_or(0);
        len == target
    }
}

/// 验证数据最大长度 — 对齐 PHP `max`
///
/// 对齐 PHP `Validate.php` 第 1360-1371 行
pub fn max(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let len = value_length(value);
    let max: usize = rule.parse().unwrap_or(0);
    len <= max
}

/// 验证数据最小长度 — 对齐 PHP `min`
///
/// 对齐 PHP `Validate.php` 第 1380-1391 行
pub fn min(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let len = value_length(value);
    let min: usize = rule.parse().unwrap_or(0);
    len >= min
}

// ============================================================================
// 日期类规则
// ============================================================================

/// 验证时间和日期是否符合指定格式 — 对齐 PHP `dateFormat`
///
/// 对齐 PHP `Validate.php` 第 1109-1113 行
///
/// ## PHP 行为
///
/// ```php
/// $info = date_parse_from_format($rule, $value);
/// return 0 == $info['warning_count'] && 0 == $info['error_count'];
/// ```
pub fn date_format(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let s = match value {
        Value::String(s) => s.as_str(),
        _ => return false,
    };
    let chrono_fmt = php_date_format_to_chrono(rule);
    // 尝试 NaiveDateTime 解析
    if NaiveDateTime::parse_from_str(s, &chrono_fmt).is_ok() {
        return true;
    }
    // 尝试 NaiveDate 解析
    if NaiveDate::parse_from_str(s, &chrono_fmt).is_ok() {
        return true;
    }
    false
}

/// 验证日期 — 对齐 PHP `after`
///
/// 对齐 PHP `Validate.php` 第 1401-1404 行
///
/// ## PHP 行为
///
/// `return strtotime($value) >= strtotime($rule);`
pub fn after(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let value_ts = parse_timestamp(value);
    let rule_ts = parse_timestamp(&Value::String(rule.to_string()));
    match (value_ts, rule_ts) {
        (Some(v), Some(r)) => v >= r,
        _ => false,
    }
}

/// 验证日期 — 对齐 PHP `before`
///
/// 对齐 PHP `Validate.php` 第 1414-1417 行
pub fn before(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let value_ts = parse_timestamp(value);
    let rule_ts = parse_timestamp(&Value::String(rule.to_string()));
    match (value_ts, rule_ts) {
        (Some(v), Some(r)) => v <= r,
        _ => false,
    }
}

/// 验证日期 — 对齐 PHP `afterWith`
///
/// 对齐 PHP `Validate.php` 第 1427-1431 行
///
/// ## PHP 行为
///
/// ```php
/// $rule = $this->getDataValue($data, $rule);
/// return !is_null($rule) && strtotime($value) >= strtotime($rule);
/// ```
pub fn after_with(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    if other.is_null() {
        return false;
    }
    let value_ts = parse_timestamp(value);
    let rule_ts = parse_timestamp(&other);
    match (value_ts, rule_ts) {
        (Some(v), Some(r)) => v >= r,
        _ => false,
    }
}

/// 验证日期 — 对齐 PHP `beforeWith`
///
/// 对齐 PHP `Validate.php` 第 1441-1445 行
pub fn before_with(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    if other.is_null() {
        return false;
    }
    let value_ts = parse_timestamp(value);
    let rule_ts = parse_timestamp(&other);
    match (value_ts, rule_ts) {
        (Some(v), Some(r)) => v <= r,
        _ => false,
    }
}

/// 验证有效期 — 对齐 PHP `expire`
///
/// 对齐 PHP `Validate.php` 第 1454-1471 行
///
/// ## PHP 行为
///
/// ```php
/// [$start, $end] = explode(',', $rule);
/// if (!is_numeric($start)) { $start = strtotime($start); }
/// if (!is_numeric($end)) { $end = strtotime($end); }
/// return time() >= $start && time() <= $end;
/// ```
pub fn expire(_value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let parts: Vec<&str> = rule.split(',').collect();
    if parts.len() < 2 {
        return false;
    }
    let start_str = parts[0].trim();
    let end_str = parts[1].trim();

    // 对齐 PHP is_numeric 检查：数字直接作为时间戳，否则解析为时间戳
    let start_ts = if let Ok(n) = start_str.parse::<i64>() {
        Some(n)
    } else {
        parse_timestamp(&Value::String(start_str.to_string()))
    };
    let end_ts = if let Ok(n) = end_str.parse::<i64>() {
        Some(n)
    } else {
        parse_timestamp(&Value::String(end_str.to_string()))
    };

    match (start_ts, end_ts) {
        (Some(s), Some(e)) => {
            let now = Utc::now().timestamp();
            now >= s && now <= e
        }
        _ => false,
    }
}

// ============================================================================
// 条件必须类规则
// ============================================================================

/// 验证某个字段等于某个值的时候必须 — 对齐 PHP `requireIf`
///
/// 对齐 PHP `Validate.php` 第 1200-1209 行
///
/// ## PHP 行为
///
/// ```php
/// [$field, $val] = explode(',', $rule);
/// if ($this->getDataValue($data, $field) == $val) {
///     return !empty($value) || '0' == $value;
/// }
/// return true;
/// ```
pub fn require_if(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let parts: Vec<&str> = rule.split(',').collect();
    if parts.len() < 2 {
        return true;
    }
    let field_name = parts[0].trim();
    let expected_val = parts[1].trim();

    let actual = Validate::get_data_value(data, field_name);
    if value_loose_equals_str(&actual, expected_val) {
        // 必须验证：对齐 PHP `!empty($value) || '0' == $value`
        !is_empty_value(value) || matches!(value, Value::String(s) if s == "0")
    } else {
        true
    }
}

/// 验证某个字段有值的情况下必须 — 对齐 PHP `requireWith`
///
/// 对齐 PHP `Validate.php` 第 1238-1247 行
///
/// ## PHP 行为
///
/// ```php
/// $val = $this->getDataValue($data, $rule);
/// if (!empty($val)) {
///     return !empty($value) || '0' == $value;
/// }
/// return true;
/// ```
pub fn require_with(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    if !is_empty_value(&other) {
        !is_empty_value(value) || matches!(value, Value::String(s) if s == "0")
    } else {
        true
    }
}

/// 验证某个字段没有值的情况下必须 — 对齐 PHP `requireWithout`
///
/// 对齐 PHP `Validate.php` 第 1257-1266 行
pub fn require_without(value: &Value, rule: &str, data: &Value, _field: &str) -> bool {
    let other = Validate::get_data_value(data, rule);
    if is_empty_value(&other) {
        !is_empty_value(value) || matches!(value, Value::String(s) if s == "0")
    } else {
        true
    }
}

// ============================================================================
// IP 类规则
// ============================================================================

/// 验证是否有效 IP — 对齐 PHP `ip`
///
/// 对齐 PHP `Validate.php` 第 942-949 行
///
/// ## PHP 行为
///
/// ```php
/// if (!in_array($rule, ['ipv4', 'ipv6'])) { $rule = 'ipv4'; }
/// return $this->filter($value, [FILTER_VALIDATE_IP, 'ipv6' == $rule ? FILTER_FLAG_IPV6 : FILTER_FLAG_IPV4]);
/// ```
pub fn ip(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let s = match value {
        Value::String(s) => s.as_str(),
        _ => return false,
    };
    let parsed: Result<IpAddr, _> = s.parse();
    match parsed {
        Ok(IpAddr::V4(_)) => rule != "ipv6", // ipv4 或默认
        Ok(IpAddr::V6(_)) => rule == "ipv6",
        Err(_) => false,
    }
}

/// 验证 IP 许可 — 对齐 PHP `allowIp`
///
/// 对齐 PHP `Validate.php` 第 1480-1483 行
pub fn allow_ip(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    let s = match value {
        Value::String(s) => s.as_str(),
        _ => return false,
    };
    let allowed: Vec<&str> = rule.split(',').map(|x| x.trim()).collect();
    allowed.contains(&s)
}

/// 验证 IP 禁用 — 对齐 PHP `denyIp`
///
/// 对齐 PHP `Validate.php` 第 1492-1495 行
pub fn deny_ip(value: &Value, rule: &str, _data: &Value, _field: &str) -> bool {
    !allow_ip(value, rule, _data, _field)
}

// ============================================================================
// 域名类规则
// ============================================================================

/// 验证是否为有效的域名或 IP — 对齐 PHP `activeUrl`
///
/// 对齐 PHP `Validate.php` 第 926-933 行
///
/// ## PHP 行为
///
/// ```php
/// if (!in_array($rule, ['A', 'MX', 'NS', 'SOA', 'PTR', 'CNAME', 'AAAA', 'A6', 'SRV', 'NAPTR', 'TXT', 'ANY'])) {
///     $rule = 'MX';
/// }
/// return checkdnsrr($value, $rule);
/// ```
///
/// ## Rust 实现
///
/// `checkdnsrr` 通过 DNS 查询验证域名是否有效。Rust 实现使用
/// `std::net::ToSocketAddrs` 解析域名，能解析则视为有效（简化处理）。
pub fn active_url(value: &Value, _rule: &str, _data: &Value, _field: &str) -> bool {
    let s = match value {
        Value::String(s) => s.as_str(),
        _ => return false,
    };
    // 空字符串不是有效域名（避免 ":80" 被解析为有效地址）
    if s.is_empty() {
        return false;
    }
    // 简化：使用 DNS 解析验证域名有效性
    // 对齐 PHP checkdnsrr 的语义：能解析到记录即视为有效
    use std::net::ToSocketAddrs;
    let target = format!("{}:80", s);
    target.to_socket_addrs().is_ok()
}

// ============================================================================
// 内联单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // 比较类规则测试
    // ========================================================================

    #[test]
    fn test_eq_numeric() {
        assert!(eq(&json!(1), "1", &Value::Null, ""));
        assert!(eq(&json!("1"), "1", &Value::Null, ""));
        assert!(eq(&json!(1.5), "1.5", &Value::Null, ""));
        assert!(!eq(&json!(2), "1", &Value::Null, ""));
    }

    #[test]
    fn test_eq_string() {
        assert!(eq(&json!("hello"), "hello", &Value::Null, ""));
        assert!(!eq(&json!("hello"), "world", &Value::Null, ""));
    }

    #[test]
    fn test_egt_field_comparison() {
        let data = json!({"min_val": 10});
        assert!(egt(&json!(15), "min_val", &data, ""));
        assert!(egt(&json!(10), "min_val", &data, ""));
        assert!(!egt(&json!(5), "min_val", &data, ""));
    }

    #[test]
    fn test_gt_field_comparison() {
        let data = json!({"min_val": 10});
        assert!(gt(&json!(15), "min_val", &data, ""));
        assert!(!gt(&json!(10), "min_val", &data, ""));
        assert!(!gt(&json!(5), "min_val", &data, ""));
    }

    #[test]
    fn test_elt_field_comparison() {
        let data = json!({"max_val": 100});
        assert!(elt(&json!(50), "max_val", &data, ""));
        assert!(elt(&json!(100), "max_val", &data, ""));
        assert!(!elt(&json!(150), "max_val", &data, ""));
    }

    #[test]
    fn test_lt_field_comparison() {
        let data = json!({"max_val": 100});
        assert!(lt(&json!(50), "max_val", &data, ""));
        assert!(!lt(&json!(100), "max_val", &data, ""));
        assert!(!lt(&json!(150), "max_val", &data, ""));
    }

    #[test]
    fn test_confirm_explicit_field() {
        let data = json!({"password": "abc123", "password_confirm": "abc123"});
        assert!(confirm(
            &json!("abc123"),
            "password_confirm",
            &data,
            "password"
        ));
        assert!(!confirm(
            &json!("wrong"),
            "password_confirm",
            &data,
            "password"
        ));
    }

    #[test]
    fn test_confirm_auto_field_inference() {
        // PHP 行为：rule 为空时，从 field 推断 field_confirm
        let data = json!({"password": "abc123", "password_confirm": "abc123"});
        assert!(confirm(&json!("abc123"), "", &data, "password"));
        assert!(!confirm(&json!("wrong"), "", &data, "password"));
    }

    #[test]
    fn test_confirm_auto_field_strips_suffix() {
        // PHP 行为：field 包含 _confirm 时，取前缀作为确认字段
        let data = json!({"password": "abc123"});
        assert!(confirm(&json!("abc123"), "", &data, "password_confirm"));
    }

    #[test]
    fn test_different_loose_comparison() {
        let data = json!({"other": "abc"});
        assert!(different(&json!("xyz"), "other", &data, ""));
        assert!(!different(&json!("abc"), "other", &data, ""));
        // 松散比较：1 == "1" 为 true，所以 different 为 false
        let data2 = json!({"other": "1"});
        assert!(!different(&json!(1), "other", &data2, ""));
    }

    // ========================================================================
    // 范围类规则测试
    // ========================================================================

    #[test]
    fn test_in_rule() {
        assert!(in_rule(&json!(1), "1,2,3", &Value::Null, ""));
        assert!(in_rule(&json!("1"), "1,2,3", &Value::Null, ""));
        assert!(in_rule(
            &json!("active"),
            "active,inactive",
            &Value::Null,
            ""
        ));
        assert!(!in_rule(&json!(4), "1,2,3", &Value::Null, ""));
        assert!(!in_rule(&json!("xyz"), "active,inactive", &Value::Null, ""));
    }

    #[test]
    fn test_not_in() {
        assert!(!not_in(&json!(1), "1,2,3", &Value::Null, ""));
        assert!(not_in(&json!(4), "1,2,3", &Value::Null, ""));
    }

    #[test]
    fn test_between_numeric() {
        assert!(between(&json!(5), "1,10", &Value::Null, ""));
        assert!(between(&json!(1), "1,10", &Value::Null, ""));
        assert!(between(&json!(10), "1,10", &Value::Null, ""));
        assert!(!between(&json!(0), "1,10", &Value::Null, ""));
        assert!(!between(&json!(11), "1,10", &Value::Null, ""));
    }

    #[test]
    fn test_between_string_numeric() {
        // 松散比较："5" 在 "1,10" 区间内
        assert!(between(&json!("5"), "1,10", &Value::Null, ""));
    }

    #[test]
    fn test_not_between() {
        assert!(!not_between(&json!(5), "1,10", &Value::Null, ""));
        assert!(not_between(&json!(11), "1,10", &Value::Null, ""));
    }

    #[test]
    fn test_between_invalid_format() {
        assert!(!between(&json!(5), "1", &Value::Null, "")); // 缺少 max
    }

    // ========================================================================
    // 长度类规则测试
    // ========================================================================

    #[test]
    fn test_length_exact() {
        assert!(length(&json!("abc"), "3", &Value::Null, ""));
        assert!(!length(&json!("abc"), "5", &Value::Null, ""));
    }

    #[test]
    fn test_length_range() {
        assert!(length(&json!("abc"), "1,5", &Value::Null, ""));
        assert!(length(&json!("abcde"), "1,5", &Value::Null, ""));
        assert!(!length(&json!("abcdef"), "1,5", &Value::Null, ""));
    }

    #[test]
    fn test_length_unicode() {
        // 对齐 PHP mb_strlen：Unicode 字符按字符计数
        assert!(length(&json!("中文"), "2", &Value::Null, ""));
        assert!(!length(&json!("中文"), "4", &Value::Null, "")); // 不是字节长度
    }

    #[test]
    fn test_length_array() {
        assert!(length(&json!([1, 2, 3]), "3", &Value::Null, ""));
        assert!(!length(&json!([1, 2, 3]), "2", &Value::Null, ""));
    }

    #[test]
    fn test_max_length() {
        assert!(max(&json!("abc"), "5", &Value::Null, ""));
        assert!(max(&json!("abcde"), "5", &Value::Null, ""));
        assert!(!max(&json!("abcdef"), "5", &Value::Null, ""));
    }

    #[test]
    fn test_min_length() {
        assert!(min(&json!("abc"), "3", &Value::Null, ""));
        assert!(!min(&json!("ab"), "3", &Value::Null, ""));
    }

    // ========================================================================
    // 日期类规则测试
    // ========================================================================

    #[test]
    fn test_date_format_y_m_d() {
        assert!(date_format(&json!("2024-01-15"), "Y-m-d", &Value::Null, ""));
        assert!(!date_format(
            &json!("2024/01/15"),
            "Y-m-d",
            &Value::Null,
            ""
        ));
    }

    #[test]
    fn test_date_format_full() {
        assert!(date_format(
            &json!("2024-01-15 12:30:45"),
            "Y-m-d H:i:s",
            &Value::Null,
            ""
        ));
    }

    #[test]
    fn test_after_date() {
        assert!(after(&json!("2024-01-02"), "2024-01-01", &Value::Null, ""));
        assert!(after(&json!("2024-01-01"), "2024-01-01", &Value::Null, ""));
        assert!(!after(&json!("2023-12-31"), "2024-01-01", &Value::Null, ""));
    }

    #[test]
    fn test_before_date() {
        assert!(before(&json!("2023-12-31"), "2024-01-01", &Value::Null, ""));
        assert!(before(&json!("2024-01-01"), "2024-01-01", &Value::Null, ""));
        assert!(!before(
            &json!("2024-01-02"),
            "2024-01-01",
            &Value::Null,
            ""
        ));
    }

    #[test]
    fn test_after_with_field() {
        let data = json!({"start_date": "2024-01-01"});
        assert!(after_with(&json!("2024-01-02"), "start_date", &data, ""));
        assert!(!after_with(&json!("2023-12-31"), "start_date", &data, ""));
    }

    #[test]
    fn test_before_with_field() {
        let data = json!({"end_date": "2024-12-31"});
        assert!(before_with(&json!("2024-06-15"), "end_date", &data, ""));
        assert!(!before_with(&json!("2025-01-01"), "end_date", &data, ""));
    }

    #[test]
    fn test_after_with_null_field() {
        // PHP 行为：字段值为 null 时返回 false
        let data = json!({});
        assert!(!after_with(&json!("2024-01-02"), "missing", &data, ""));
    }

    #[test]
    fn test_expire_with_timestamps() {
        // 使用时间戳：过去的时间区间应返回 false
        let now = Utc::now().timestamp();
        let past_start = now - 7200; // 2 小时前
        let past_end = now - 3600; // 1 小时前
        let rule = format!("{},{}", past_start, past_end);
        assert!(!expire(&Value::Null, &rule, &Value::Null, ""));

        // 当前时间在区间内应返回 true
        let future_start = now - 60;
        let future_end = now + 60;
        let rule = format!("{},{}", future_start, future_end);
        assert!(expire(&Value::Null, &rule, &Value::Null, ""));
    }

    #[test]
    fn test_expire_with_date_strings() {
        // 使用日期字符串
        let rule = "2020-01-01,2030-12-31";
        assert!(expire(&Value::Null, rule, &Value::Null, ""));

        let rule = "2010-01-01,2015-12-31";
        assert!(!expire(&Value::Null, rule, &Value::Null, ""));
    }

    // ========================================================================
    // 条件必须类规则测试
    // ========================================================================

    #[test]
    fn test_require_if_condition_met() {
        // type=login 时 username 必须非空
        let data = json!({"type": "login"});
        assert!(require_if(&json!("alice"), "type,login", &data, ""));
        // 空值不满足 require
        assert!(!require_if(&json!(""), "type,login", &data, ""));
        // "0" 视为非空（PHP 特殊行为）
        assert!(require_if(&json!("0"), "type,login", &data, ""));
    }

    #[test]
    fn test_require_if_condition_not_met() {
        let data = json!({"type": "register"});
        // 条件不满足时返回 true（不验证）
        assert!(require_if(&json!(""), "type,login", &data, ""));
    }

    #[test]
    fn test_require_with_other_has_value() {
        let data = json!({"other_field": "some_value"});
        assert!(require_with(&json!("value"), "other_field", &data, ""));
        assert!(!require_with(&json!(""), "other_field", &data, ""));
    }

    #[test]
    fn test_require_with_other_empty() {
        let data = json!({"other_field": ""});
        // 其他字段为空时不验证
        assert!(require_with(&json!(""), "other_field", &data, ""));

        let data2 = json!({});
        assert!(require_with(&json!(""), "missing", &data2, ""));
    }

    #[test]
    fn test_require_without_other_empty() {
        let data = json!({"other_field": ""});
        // 其他字段为空时必须
        assert!(require_without(&json!("value"), "other_field", &data, ""));
        assert!(!require_without(&json!(""), "other_field", &data, ""));
    }

    #[test]
    fn test_require_without_other_has_value() {
        let data = json!({"other_field": "some_value"});
        // 其他字段有值时不验证
        assert!(require_without(&json!(""), "other_field", &data, ""));
    }

    // ========================================================================
    // IP 类规则测试
    // ========================================================================

    #[test]
    fn test_ip_v4() {
        assert!(ip(&json!("127.0.0.1"), "ipv4", &Value::Null, ""));
        assert!(ip(&json!("192.168.1.1"), "ipv4", &Value::Null, ""));
        assert!(ip(&json!("127.0.0.1"), "", &Value::Null, "")); // 默认 ipv4
        assert!(!ip(&json!("::1"), "ipv4", &Value::Null, ""));
        assert!(!ip(&json!("999.999.999.999"), "ipv4", &Value::Null, ""));
    }

    #[test]
    fn test_ip_v6() {
        assert!(ip(&json!("::1"), "ipv6", &Value::Null, ""));
        assert!(ip(&json!("2001:db8::1"), "ipv6", &Value::Null, ""));
        assert!(!ip(&json!("127.0.0.1"), "ipv6", &Value::Null, ""));
    }

    #[test]
    fn test_allow_ip() {
        assert!(allow_ip(
            &json!("127.0.0.1"),
            "127.0.0.1,192.168.1.1",
            &Value::Null,
            ""
        ));
        assert!(!allow_ip(
            &json!("10.0.0.1"),
            "127.0.0.1,192.168.1.1",
            &Value::Null,
            ""
        ));
    }

    #[test]
    fn test_deny_ip() {
        assert!(!deny_ip(
            &json!("127.0.0.1"),
            "127.0.0.1,192.168.1.1",
            &Value::Null,
            ""
        ));
        assert!(deny_ip(
            &json!("10.0.0.1"),
            "127.0.0.1,192.168.1.1",
            &Value::Null,
            ""
        ));
    }

    // ========================================================================
    // 域名类规则测试
    // ========================================================================

    #[test]
    fn test_active_url_valid_domain() {
        // 测试已知可解析的域名
        assert!(active_url(&json!("localhost"), "", &Value::Null, ""));
    }

    #[test]
    fn test_active_url_invalid() {
        assert!(!active_url(
            &json!("not.a.valid.domain.example.invalid"),
            "",
            &Value::Null,
            ""
        ));
        assert!(!active_url(&json!(""), "", &Value::Null, ""));
        assert!(!active_url(&json!(123), "", &Value::Null, ""));
    }

    // ========================================================================
    // 辅助函数测试
    // ========================================================================

    #[test]
    fn test_value_loose_equals_str_numeric() {
        assert!(value_loose_equals_str(&json!(1), "1"));
        assert!(value_loose_equals_str(&json!(1.0), "1"));
        assert!(value_loose_equals_str(&json!("1"), "1"));
        assert!(!value_loose_equals_str(&json!(2), "1"));
    }

    #[test]
    fn test_value_loose_equals_str_string() {
        assert!(value_loose_equals_str(&json!("hello"), "hello"));
        assert!(!value_loose_equals_str(&json!("hello"), "world"));
    }

    #[test]
    fn test_value_loose_compare_numeric() {
        use std::cmp::Ordering;
        assert_eq!(
            value_loose_compare(&json!(5), &json!(3)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            value_loose_compare(&json!(3), &json!(5)),
            Some(Ordering::Less)
        );
        assert_eq!(
            value_loose_compare(&json!(5), &json!(5)),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn test_value_length_string() {
        assert_eq!(value_length(&json!("abc")), 3);
        assert_eq!(value_length(&json!("中文")), 2); // Unicode 字符数
    }

    #[test]
    fn test_value_length_array() {
        assert_eq!(value_length(&json!([1, 2, 3])), 3);
        assert_eq!(value_length(&json!([])), 0);
    }

    #[test]
    fn test_parse_timestamp_iso() {
        let ts = parse_timestamp(&json!("2024-01-01 12:00:00"));
        assert!(ts.is_some());
    }

    #[test]
    fn test_parse_timestamp_numeric() {
        let ts = parse_timestamp(&json!(1700000000));
        assert_eq!(ts, Some(1700000000));
    }

    #[test]
    fn test_php_date_format_to_chrono_simple() {
        assert_eq!(php_date_format_to_chrono("Y-m-d"), "%Y-%m-%d");
        assert_eq!(
            php_date_format_to_chrono("Y/m/d H:i:s"),
            "%Y/%m/%d %H:%M:%S"
        );
    }
}

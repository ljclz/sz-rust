//! 缺失键降级策略（spec §5.4 规则 4）
//!
//! 降级链：当前语言缺失 → 默认语言 → 键名 → 占位符

use serde_json::Value;

/// 缺失键降级策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FallbackStrategy {
    /// 回退默认语言
    #[default]
    DefaultLanguage,
    /// 回退键名
    KeyName,
    /// 占位符（返回空字符串）
    Placeholder,
}

/// 执行降级查找
///
/// `current_msg`：当前语言的消息（可能为 None）
/// `default_msg`：默认语言的消息（可能为 None）
/// `key`：消息键名
/// `strategy`：降级策略
/// `missing_log`：记录缺失键（非静默返回空）
pub fn fallback_lookup(
    current_msg: Option<&Value>,
    default_msg: Option<&Value>,
    key: &str,
    strategy: FallbackStrategy,
    missing_log: &mut Vec<String>,
) -> Option<Value> {
    if let Some(v) = current_msg {
        return Some(v.clone());
    }

    match strategy {
        FallbackStrategy::DefaultLanguage => {
            if let Some(v) = default_msg {
                missing_log.push(format!("键 '{key}' 在当前语言缺失，回退默认语言"));
                Some(v.clone())
            } else {
                missing_log.push(format!("键 '{key}' 在当前语言和默认语言均缺失"));
                None
            }
        }
        FallbackStrategy::KeyName => {
            missing_log.push(format!("键 '{key}' 缺失，回退键名"));
            Some(Value::String(key.to_string()))
        }
        FallbackStrategy::Placeholder => {
            missing_log.push(format!("键 '{key}' 缺失，返回占位符"));
            Some(Value::String(format!("[{key}]")))
        }
    }
}

/// 将 Value 转为字符串
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_current_hit() {
        let mut log = Vec::new();
        let result = fallback_lookup(
            Some(&Value::String("你好".to_string())),
            None,
            "greeting",
            FallbackStrategy::DefaultLanguage,
            &mut log,
        );
        assert_eq!(result, Some(Value::String("你好".to_string())));
        assert!(log.is_empty());
    }

    #[test]
    fn test_fallback_to_default_language() {
        let mut log = Vec::new();
        let result = fallback_lookup(
            None,
            Some(&Value::String("Hello".to_string())),
            "greeting",
            FallbackStrategy::DefaultLanguage,
            &mut log,
        );
        assert_eq!(result, Some(Value::String("Hello".to_string())));
        assert_eq!(log.len(), 1);
        assert!(log[0].contains("回退默认语言"));
    }

    #[test]
    fn test_fallback_to_key_name() {
        let mut log = Vec::new();
        let result = fallback_lookup(None, None, "greeting", FallbackStrategy::KeyName, &mut log);
        assert_eq!(result, Some(Value::String("greeting".to_string())));
    }

    #[test]
    fn test_fallback_to_placeholder() {
        let mut log = Vec::new();
        let result = fallback_lookup(
            None,
            None,
            "greeting",
            FallbackStrategy::Placeholder,
            &mut log,
        );
        assert_eq!(result, Some(Value::String("[greeting]".to_string())));
    }

    #[test]
    fn test_fallback_both_missing_default_strategy() {
        let mut log = Vec::new();
        let result = fallback_lookup(
            None,
            None,
            "greeting",
            FallbackStrategy::DefaultLanguage,
            &mut log,
        );
        assert!(result.is_none());
        assert_eq!(log.len(), 1);
    }
}

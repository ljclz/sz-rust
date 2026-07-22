//! 控制器共享辅助函数 — 对齐 PHP 控制器公共模式
//!
//! ## PHP 对齐
//!
//! | PHP 模式 | Rust 等价 | 说明 |
//! |---------|----------|------|
//! | `json_decode($param['formData'], true)` | [`parse_form_data`] | 解析 formData JSON 字符串 |
//! | `$param['company_id']` 等直接访问 | [`get_i64_param`] / [`get_str_param`] | 类型安全取值 |
//! | `$param['app_id'] ?? 10001` | [`get_app_id`] | app_id 默认值 10001 |
//!
//! ## 设计原则
//!
//! - 辅助函数均为纯函数，无副作用
//! - 返回 `Option` 或 `Result`，不 panic
//! - 严格对齐 PHP 弱类型语义（如 `empty($val)` 判空）

use serde_json::Value;

/// 解析 formData 字段为 JSON Value（对齐 PHP `json_decode($param['formData'], true)`）
///
/// # PHP 对齐
///
/// ```php
/// $data = json_decode($param['formData'], true);
/// ```
///
/// # 参数
///
/// - `param`：控制器 postData 返回的参数对象
///
/// # 返回
///
/// - `Ok(Value::Object)` 或 `Ok(Value::Array)`：解析成功
/// - `Err(String)`：formData 字段缺失或解析失败
///
/// # 行为
///
/// - 若 `formData` 不存在，返回 `Err("formData 字段缺失")`
/// - 若 `formData` 为非字符串类型（如已是对象/数组），直接返回
/// - 若 `formData` 为字符串，调用 `serde_json::from_str` 解析
pub fn parse_form_data(param: &Value) -> Result<Value, String> {
    match param.get("formData") {
        None => Err("formData 字段缺失".to_string()),
        Some(Value::String(s)) => {
            serde_json::from_str(s).map_err(|e| format!("formData 解析失败: {e}"))
        }
        Some(v) if v.is_object() || v.is_array() => Ok(v.clone()),
        Some(_) => Err("formData 类型无效".to_string()),
    }
}

/// 获取 i64 参数（对齐 PHP `$param[$key]` 后 intval 转换）
///
/// # PHP 对齐
///
/// ```php
/// $company_id = $param['company_id'];  // 隐式类型转换
/// ```
///
/// # 参数
///
/// - `param`：控制器 postData 返回的参数对象
/// - `key`：参数名
///
/// # 返回
///
/// - `Some(i64)`：参数存在且可转换为 i64
/// - `None`：参数不存在或不可转换
pub fn get_i64_param(param: &Value, key: &str) -> Option<i64> {
    param.get(key).and_then(|v| v.as_i64())
}

/// 获取字符串参数（对齐 PHP `$param[$key]` 后字符串转换）
///
/// # 参数
///
/// - `param`：控制器 postData 返回的参数对象
/// - `key`：参数名
///
/// # 返回
///
/// - `Some(String)`：参数存在且为字符串
/// - `None`：参数不存在或非字符串
pub fn get_str_param(param: &Value, key: &str) -> Option<String> {
    param
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 获取 app_id（对齐 PHP `$param['app_id'] ?? 10001`）
///
/// # PHP 对齐
///
/// ```php
/// $data['app_id'] = $param['app_id'] ?? 10001;
/// ```
///
/// # 参数
///
/// - `param`：控制器 postData 返回的参数对象
///
/// # 返回
///
/// - `i64`：app_id 值，默认 10001
pub fn get_app_id(param: &Value) -> i64 {
    get_i64_param(param, "app_id").unwrap_or(10001)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------------------- parse_form_data 测试 --------------------

    #[test]
    fn test_parse_form_data_from_string() {
        // PHP: json_decode('{"name":"test"}', true)
        let param = json!({"formData": r#"{"name":"test","age":18}"#});
        let data = parse_form_data(&param).unwrap();
        assert_eq!(data["name"], "test");
        assert_eq!(data["age"], 18);
    }

    #[test]
    fn test_parse_form_data_already_object() {
        // formData 已是对象，直接返回
        let param = json!({"formData": {"name": "test"}});
        let data = parse_form_data(&param).unwrap();
        assert_eq!(data["name"], "test");
    }

    #[test]
    fn test_parse_form_data_missing() {
        let param = json!({"other": 1});
        let result = parse_form_data(&param);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("缺失"));
    }

    #[test]
    fn test_parse_form_data_invalid_json() {
        let param = json!({"formData": "not a json"});
        let result = parse_form_data(&param);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("解析失败"));
    }

    // -------------------- get_i64_param 测试 --------------------

    #[test]
    fn test_get_i64_param_exists() {
        let param = json!({"company_id": 42});
        assert_eq!(get_i64_param(&param, "company_id"), Some(42));
    }

    #[test]
    fn test_get_i64_param_missing() {
        let param = json!({"other": 1});
        assert_eq!(get_i64_param(&param, "company_id"), None);
    }

    #[test]
    fn test_get_i64_param_wrong_type() {
        let param = json!({"company_id": "not a number"});
        assert_eq!(get_i64_param(&param, "company_id"), None);
    }

    // -------------------- get_str_param 测试 --------------------

    #[test]
    fn test_get_str_param_exists() {
        let param = json!({"keyword": "test"});
        assert_eq!(get_str_param(&param, "keyword"), Some("test".to_string()));
    }

    #[test]
    fn test_get_str_param_missing() {
        let param = json!({"other": 1});
        assert_eq!(get_str_param(&param, "keyword"), None);
    }

    // -------------------- get_app_id 测试 --------------------

    #[test]
    fn test_get_app_id_exists() {
        let param = json!({"app_id": 20002});
        assert_eq!(get_app_id(&param), 20002);
    }

    #[test]
    fn test_get_app_id_default() {
        // PHP: $param['app_id'] ?? 10001
        let param = json!({"other": 1});
        assert_eq!(get_app_id(&param), 10001);
    }

    #[test]
    fn test_get_app_id_null_uses_default() {
        // PHP: $param['app_id'] ?? 10001（null 也触发默认值）
        let param = json!({"app_id": null});
        assert_eq!(get_app_id(&param), 10001);
    }
}

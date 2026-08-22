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
use sz_rust_core::orm::repository::{Repository, WhereCondition};
use sz_rust_core::orm::Value as OrmValue;

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

/// 从 Repository 查询列表并转为 JSON 数组（H-2 修复：业务层注入 Repository）
///
/// 对齐 PHP `Xxx::getAll($app_id)` / `Xxx::getLightList(...)` 静态方法模式。
/// PHP 端通过模型静态方法直接查库，Rust 端通过 Repository 参数注入，
/// 控制器调用此辅助函数完成「查询 + 序列化」流程。
///
/// # PHP 对齐
///
/// ```php
/// $list = Category::getAll($param['app_id']);
/// // $list 为数组，每元素为模型 toArray() 结果
/// ```
///
/// # 参数
///
/// - `repo`：Repository 实例（由业务层注入具体实现）
/// - `conditions`：查询条件（通常是 `app_id` + `is_delete=0`）
///
/// # 返回
///
/// - 成功：`Value::Array`，每元素为模型的 `to_json()` 输出
/// - 失败：`Value::Array(vec![])`（空数组，对齐 PHP 查询失败返回空数组的行为）
///
/// # 类型参数约束
///
/// `E` 必须实现 `Model` + `ModelExt`（提供 `to_json()` 序列化能力）和 `EntityAttributes`
/// （使 `InMemoryRepository` 可提取主键）。
pub fn fetch_list_as_json<E>(
    repo: &dyn Repository<E, Key = OrmValue>,
    conditions: &[WhereCondition],
) -> Value
where
    E: sz_rust_core::orm::Model + sz_rust_core::orm::ModelExt,
{
    match repo.find_by(conditions) {
        Ok(list) => Value::Array(list.iter().map(|e| e.to_json()).collect()),
        Err(_) => Value::Array(vec![]),
    }
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

    // -------------------- fetch_list_as_json 测试 --------------------

    use crate::model::{Category, Dept};
    use sz_rust_core::orm::repository::{InMemoryRepository, WhereOp};

    fn make_category(id: i64, name: &str, app_id: i64) -> Category {
        Category::new()
            .with_data("cat_id", json!(id))
            .with_data("cat_name", json!(name))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    #[test]
    fn test_fetch_list_as_json_returns_matching_records() {
        // H-2 验证：app_id + is_delete=0 过滤返回匹配记录
        let repo: InMemoryRepository<Category> = InMemoryRepository::from_vec(vec![
            make_category(1, "餐饮", 10001),
            make_category(2, "零售", 10001),
            make_category(3, "其他应用", 20002),
        ]);
        let conditions = [
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(10001)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let result = fetch_list_as_json(&repo, &conditions);
        let arr = result.as_array().expect("应返回数组");
        assert_eq!(arr.len(), 2, "app_id=10001 应返回 2 条记录");
        assert_eq!(arr[0]["cat_name"], "餐饮");
        assert_eq!(arr[1]["cat_name"], "零售");
    }

    #[test]
    fn test_fetch_list_as_json_returns_empty_when_no_match() {
        // H-2 验证：无匹配记录时返回空数组（对齐 PHP 查询失败返回空数组）
        let repo: InMemoryRepository<Category> =
            InMemoryRepository::from_vec(vec![make_category(1, "餐饮", 10001)]);
        let conditions = [
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(99999)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let result = fetch_list_as_json(&repo, &conditions);
        let arr = result.as_array().expect("应返回数组");
        assert!(arr.is_empty(), "无匹配时应返回空数组");
    }

    #[test]
    fn test_fetch_list_as_json_returns_empty_array_for_empty_repo() {
        // H-2 验证：空 Repository 返回空数组
        let repo: InMemoryRepository<Category> = InMemoryRepository::from_vec(vec![]);
        let conditions = [
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(10001)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let result = fetch_list_as_json(&repo, &conditions);
        assert!(result.as_array().unwrap().is_empty());
    }

    #[test]
    fn test_fetch_list_as_json_serializes_model_fields() {
        // H-2 验证：返回的 JSON 包含模型字段（对齐 PHP toArray()）
        let repo: InMemoryRepository<Category> =
            InMemoryRepository::from_vec(vec![make_category(42, "测试分类", 10001)]);
        let conditions = [
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(10001)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let result = fetch_list_as_json(&repo, &conditions);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["cat_id"], 42);
        assert_eq!(arr[0]["cat_name"], "测试分类");
        assert_eq!(arr[0]["app_id"], 10001);
    }

    #[test]
    fn test_fetch_list_as_json_works_with_dept_model() {
        // H-2 验证：函数对不同模型类型通用
        let dept = Dept::new()
            .with_data("dept_id", json!(1))
            .with_data("dept_name", json!("市场部"))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(10001));
        let repo: InMemoryRepository<Dept> = InMemoryRepository::from_vec(vec![dept]);
        let conditions = [
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(10001)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let result = fetch_list_as_json(&repo, &conditions);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["dept_name"], "市场部");
    }
}

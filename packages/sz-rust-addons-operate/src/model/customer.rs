//! Customer 模型 — 对齐 PHP `addons\operate\model\Customer`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer'` | [`Customer::table_name()`] | 表名 |
//! | `$pk = 'customer_id'` | [`Customer::pk_name()`] | 主键列名 |
//! | `$append = ['rentarea_text','status_text']` | [`Customer::append()`] + [`Customer::append_state`] | 静态 append |
//! | `getRentareaIdsAttr` | [`Customer::accessor_for`] "rentarea_ids" 分支 | CSV → Vec\<i64\> |
//! | `getStatusTextAttr` | [`Customer::accessor_for`] "status_text" 分支 | 枚举映射 |
//! | `getRentareaTextAttr` | [`Customer::accessor_for`] "rentarea_text" 分支 | NOTE(Phase 4) |
//! | `setRentareaIdsAttr` | [`Customer::mutator_for`] "rentarea_ids" 分支 | Vec\<i64\> → CSV |
//!
//! ## 未实现（标 NOTE）
//!
//! - **`getRentareaTextAttr` 静态反查**：PHP 调 `Rentarea::where(['customer_id'=>$data['customer_id'],'is_delete'=>0])->column('position')`
//!   Phase 2.10 无数据库连接，返回空字符串（与 PHP `customer_id` 为空时行为一致）。
//!   完整实现在 Phase 4（Repository 层）。

use crate::enums::ContractStatusEnum;
use crate::model::{csv_to_vec_i64, get_i64, impl_empty_relation_loader, vec_i64_to_csv};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 客户模型 — 对齐 PHP `addons\operate\model\Customer`
#[derive(Clone)]
pub struct Customer {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Customer {
    /// 创建空模型
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            get_cache: HashMap::new(),
            append_state: AppendState::new(),
        }
    }

    /// 链式设置字段值（测试用）
    pub fn with_data(mut self, key: &str, value: Value) -> Self {
        self.data.insert(key.to_string(), value);
        self
    }
}

impl Default for Customer {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Customer {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer"
    }

    fn pk_name() -> &'static str {
        "customer_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "customer_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("customer_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        // PHP 端用 `is_delete` 字段做软删除（在 `where` 中手动过滤）
        // Phase 3 接入 SoftDelete trait 后正式启用
        None
    }
}

impl ModelExt for Customer {
    fn columns() -> Vec<&'static str> {
        vec![
            "customer_id",
            "customer_name",
            "linkman_name",
            "status",
            "dept_id",
            "cat_id",
            "rentarea_ids",
            "app_id",
            "is_delete",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "customer_name",
            "status",
            "dept_id",
            "cat_id",
            "rentarea_ids",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["customer_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "customer_id" | "status" | "dept_id" | "cat_id" | "app_id" | "is_delete" => {
                v.as_i64().map(OrmValue::I64)
            }
            "customer_name" | "linkman_name" | "rentarea_ids" => {
                v.as_str().map(|s| OrmValue::String(s.to_string()))
            }
            "create_time" | "update_time" => v.as_i64().map(OrmValue::I64),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, sz_orm_core::Value>) {
        for (k, v) in map {
            let json_val = match v {
                sz_orm_core::Value::I64(i) => json!(i),
                sz_orm_core::Value::I32(i) => json!(i),
                sz_orm_core::Value::String(s) => json!(s),
                sz_orm_core::Value::Array(_) => json!(null),
                other => serde_json::to_value(&other).unwrap_or(json!(null)),
            };
            self.data.insert(k, json_val);
        }
    }
}

impl_empty_relation_loader!(Customer);

impl BaseModel for Customer {
    fn append() -> Vec<&'static str> {
        vec!["rentarea_text", "status_text"]
    }

    /// 无缓存路径（对齐 PHP `appendAttrToArray` 调 `getAttr` 但不写缓存）
    ///
    /// 业务模型同时实现 [`Appendable`] trait，建议用 `to_json_with_append_cached` 走缓存路径。
    fn get_appended_value(&self, field: &str) -> Option<Value> {
        let value = self.data.get(field);
        Some(self.accessor_for(field, value))
    }
}

impl Accessor for Customer {
    fn data_map(&self) -> &HashMap<String, Value> {
        &self.data
    }

    fn data_map_mut(&mut self) -> &mut HashMap<String, Value> {
        &mut self.data
    }

    fn accessor_cache(&self) -> &HashMap<String, Value> {
        &self.get_cache
    }

    fn accessor_cache_mut(&mut self) -> &mut HashMap<String, Value> {
        &mut self.get_cache
    }

    /// 访问器派发 — 对齐 PHP `getAttr` → `getXxxAttr`
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `rentarea_ids` | `getRentareaIdsAttr` | CSV → Vec\<i64\> |
    /// | `status_text` | `getStatusTextAttr` | 枚举映射中文 |
    /// | `rentarea_text` | `getRentareaTextAttr` | 静态反查（NOTE Phase 4） |
    fn accessor_for(&self, field: &str, _value: Option<&Value>) -> Value {
        match field {
            // PHP getRentareaIdsAttr($value): CSV → Vec<i64>
            "rentarea_ids" => {
                let raw = self
                    .data
                    .get("rentarea_ids")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arr: Vec<Value> = csv_to_vec_i64(raw).into_iter().map(|i| json!(i)).collect();
                Value::Array(arr)
            }
            // PHP getStatusTextAttr($value, $data): !empty($data['status']) ? customerStatusName : ''
            "status_text" => {
                let status = get_i64(&self.data, "status").unwrap_or(0);
                if status == 0 {
                    json!("")
                } else {
                    json!(ContractStatusEnum::customer_status_name(status))
                }
            }
            // PHP getRentareaTextAttr($value, $data): 静态反查 Rentarea 表
            // Phase 2.10 无数据库连接，返回空字符串（与 PHP customer_id 为空时一致）
            // NOTE(Phase 4): 完整实现 Rentarea::where(['customer_id'=>..., 'is_delete'=>0])->column('position')
            "rentarea_text" => json!(""),
            _ => Value::Null,
        }
    }
}

impl Mutator for Customer {
    /// 修改器派发 — 对齐 PHP `setAttr` → `setXxxAttr`
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `rentarea_ids` | `setRentareaIdsAttr` | Vec\<i64\> → CSV |
    fn mutator_for(
        &mut self,
        field: &str,
        value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        match field {
            // PHP setRentareaIdsAttr($value): array_filter + array_map('trim') + implode(',')
            "rentarea_ids" => {
                let csv = match value {
                    Value::Array(arr) => {
                        let i64_arr: Vec<i64> = arr
                            .iter()
                            .filter_map(|v| {
                                // PHP array_filter 过滤 empty（0/空字符串/null/空数组）
                                // PHP empty(0)=true, empty("0")=true, empty("")=true, empty(null)=true
                                if v.is_null() {
                                    return None;
                                }
                                if let Some(n) = v.as_i64() {
                                    if n == 0 {
                                        return None;
                                    }
                                    return Some(n);
                                }
                                if let Some(s) = v.as_str() {
                                    let trimmed = s.trim();
                                    if trimmed.is_empty() || trimmed == "0" {
                                        return None;
                                    }
                                    return trimmed.parse::<i64>().ok().filter(|n| *n != 0);
                                }
                                None
                            })
                            .collect();
                        vec_i64_to_csv(&i64_arr)
                    }
                    Value::String(s) => {
                        // PHP (array)$string 会把字符串转为单元素数组
                        // 但 setRentareaIdsAttr 业务上只接收数组，这里防御性处理
                        let trimmed = s.trim();
                        if trimmed.is_empty() || trimmed == "0" {
                            String::new()
                        } else {
                            trimmed.to_string()
                        }
                    }
                    _ => String::new(),
                };
                Some(MutatorResult::Value(Value::String(csv)))
            }
            _ => None,
        }
    }
}

impl Appendable for Customer {
    fn append_state(&self) -> &AppendState {
        &self.append_state
    }

    fn append_state_mut(&mut self) -> &mut AppendState {
        &mut self.append_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------- 元数据测试 --------------------

    #[test]
    fn test_table_name_aligns_php() {
        assert_eq!(Customer::table_name(), "customer");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Customer::pk_name(), "customer_id");
    }

    #[test]
    fn test_append_fields_aligns_php() {
        // PHP $append = ['rentarea_text', 'status_text']
        assert_eq!(Customer::append(), vec!["rentarea_text", "status_text"]);
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_rentarea_ids_csv_to_vec() {
        // PHP getRentareaIdsAttr("1,2,3") → [1, 2, 3]
        let model = Customer::new().with_data("rentarea_ids", json!("1,2,3"));
        let value = model.accessor_for("rentarea_ids", None);
        assert_eq!(value, json!([1, 2, 3]));
    }

    #[test]
    fn test_accessor_rentarea_ids_empty_string_returns_empty_vec() {
        // PHP getRentareaIdsAttr("") → []
        let model = Customer::new();
        let value = model.accessor_for("rentarea_ids", None);
        assert_eq!(value, json!([]));
    }

    #[test]
    fn test_accessor_rentarea_ids_filters_zero() {
        // PHP empty(0)=true → 0 被过滤
        let model = Customer::new().with_data("rentarea_ids", json!("0,1,2"));
        let value = model.accessor_for("rentarea_ids", None);
        assert_eq!(value, json!([1, 2]));
    }

    #[test]
    fn test_accessor_status_text_zero_returns_empty() {
        // PHP !empty(0)=false → ''
        let model = Customer::new().with_data("status", json!(0));
        let value = model.accessor_for("status_text", None);
        assert_eq!(value, json!(""));
    }

    #[test]
    fn test_accessor_status_text_one_returns_zu() {
        let model = Customer::new().with_data("status", json!(1));
        let value = model.accessor_for("status_text", None);
        assert_eq!(value, json!("在租"));
    }

    #[test]
    fn test_accessor_status_text_unknown_returns_unknown() {
        let model = Customer::new().with_data("status", json!(99));
        let value = model.accessor_for("status_text", None);
        assert_eq!(value, json!("未知"));
    }

    #[test]
    fn test_accessor_rentarea_text_returns_empty_in_phase_2_10() {
        // Phase 2.10 无数据库连接，rentarea_text 始终返回空字符串
        // 完整实现在 Phase 4（Repository 层）
        let model = Customer::new().with_data("customer_id", json!(1));
        let value = model.accessor_for("rentarea_text", None);
        assert_eq!(value, json!(""));
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_rentarea_ids_vec_to_csv() {
        // PHP setRentareaIdsAttr([1, 2, 3]) → "1,2,3"
        let mut model = Customer::new();
        let merged = HashMap::new();
        let result = model.mutator_for("rentarea_ids", &json!([1, 2, 3]), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String("1,2,3".to_string())))
        );
    }

    #[test]
    fn test_mutator_rentarea_ids_filters_empty_and_zero() {
        // PHP array_filter 过滤 empty（0/空字符串/null）
        let mut model = Customer::new();
        let merged = HashMap::new();
        let result = model.mutator_for("rentarea_ids", &json!([0, 1, "", 2, null, 3]), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String("1,2,3".to_string())))
        );
    }

    #[test]
    fn test_mutator_rentarea_ids_empty_vec_returns_empty_string() {
        let mut model = Customer::new();
        let merged = HashMap::new();
        let result = model.mutator_for("rentarea_ids", &json!([]), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String(String::new())))
        );
    }

    // -------------------- Appendable 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_includes_status_text() {
        let mut model = Customer::new()
            .with_data("customer_id", json!(1))
            .with_data("status", json!(1))
            .with_data("customer_name", json!("张三"));
        let json = model.to_json_with_append_cached();
        // 基础字段
        assert_eq!(json["customer_id"], 1);
        assert_eq!(json["customer_name"], "张三");
        assert_eq!(json["status"], 1);
        // append 字段
        assert_eq!(json["status_text"], "在租");
        assert_eq!(json["rentarea_text"], "");
    }

    #[test]
    fn test_to_json_with_append_cached_empty_status() {
        let mut model = Customer::new().with_data("customer_id", json!(1));
        let json = model.to_json_with_append_cached();
        // status 不存在 → !empty(null)=false → ''
        assert_eq!(json["status_text"], "");
        assert_eq!(json["rentarea_text"], "");
    }
}

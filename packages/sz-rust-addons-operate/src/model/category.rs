//! Category 模型 — 对齐 PHP `addons\operate\model\Category`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_category'` | [`Category::table_name()`] | 表名 |
//! | `$pk = 'cat_id'` | [`Category::pk_name()`] | 主键列名 |
//! | `$append = []` | [`Category::append()`]（默认空） | 无静态 append |
//!
//! ## 无访问器 / 无修改器
//!
//! PHP `Category` 未声明任何 `getXxxAttr` / `setXxxAttr`，
//! Rust 端 [`Category::accessor_for`] 返回原始值，
//! [`Category::mutator_for`] 返回 `None`。
//!
//! ## 未实现（标 TODO）
//!
//! - **业务方法**（detail/getList/add/edit/setDelete/getAll/getCustomerCategoryInfo 等）→ TODO(Phase 5+ 控制器层)
//! - **静态缓存**（Cache::get/set）→ TODO(Phase 6)

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 客户分类模型 — 对齐 PHP `addons\operate\model\Category`
#[derive(Clone)]
pub struct Category {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Category {
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

impl Default for Category {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Category {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_category"
    }

    fn pk_name() -> &'static str {
        "cat_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "cat_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("cat_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Category {
    fn columns() -> Vec<&'static str> {
        vec![
            "cat_id",
            "cat_name",
            "cat_sort",
            "is_delete",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["cat_name", "cat_sort"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["cat_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "cat_id" | "cat_sort" | "is_delete" | "app_id" | "create_time" | "update_time" => {
                v.as_i64().map(OrmValue::I64)
            }
            "cat_name" => v.as_str().map(|s| OrmValue::String(s.to_string())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, sz_orm_core::Value>) {
        for (k, v) in map {
            let json_val = match v {
                sz_orm_core::Value::I64(i) => json!(i),
                sz_orm_core::Value::I32(i) => json!(i),
                sz_orm_core::Value::F64(f) => json!(f),
                sz_orm_core::Value::String(s) => json!(s),
                sz_orm_core::Value::Array(_) => json!(null),
                other => serde_json::to_value(&other).unwrap_or(json!(null)),
            };
            self.data.insert(k, json_val);
        }
    }
}

impl_empty_relation_loader!(Category);

impl BaseModel for Category {
    // PHP Category 未声明 $append，使用默认空 Vec
}

impl Accessor for Category {
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

    /// PHP Category 未声明任何 getXxxAttr
    fn accessor_for(&self, _field: &str, value: Option<&Value>) -> Value {
        value.cloned().unwrap_or(Value::Null)
    }
}

impl Mutator for Category {
    /// PHP Category 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Category {
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
        assert_eq!(Category::table_name(), "customer_category");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Category::pk_name(), "cat_id");
    }

    #[test]
    fn test_append_fields_empty_aligns_php() {
        // PHP Category 未声明 $append
        assert!(Category::append().is_empty());
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = Category::columns();
        assert!(cols.contains(&"cat_id"));
        assert!(cols.contains(&"cat_name"));
        assert!(cols.contains(&"cat_sort"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"app_id"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = Category::fillable();
        assert!(!fillable.contains(&"cat_id"), "cat_id 应受保护不可批量赋值");
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"cat_name"));
        assert!(fillable.contains(&"cat_sort"));
    }

    #[test]
    fn test_guarded_includes_cat_id() {
        assert!(Category::guarded().contains(&"cat_id"));
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_returns_raw_value_for_all_fields() {
        // PHP Category 无访问器，getAttr 返回原始字段值
        let model = Category::new()
            .with_data("cat_id", json!(1))
            .with_data("cat_name", json!("餐饮"));
        assert_eq!(model.accessor_for("cat_id", None), Value::Null);
        assert_eq!(model.accessor_for("cat_name", None), Value::Null);
        assert_eq!(
            model.accessor_for("cat_name", Some(&json!("餐饮"))),
            json!("餐饮")
        );
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP Category 无修改器
        let mut model = Category::new();
        let merged = HashMap::new();
        assert_eq!(model.mutator_for("cat_name", &json!("测试"), &merged), None);
        assert_eq!(model.mutator_for("cat_sort", &json!(1), &merged), None);
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = Category::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = Category::new().with_data("cat_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = Category::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_no_append_fields() {
        // 无 append 字段，to_json_with_append_cached 等价于 to_json
        let mut model = Category::new()
            .with_data("cat_id", json!(1))
            .with_data("cat_name", json!("餐饮"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["cat_id"], 1);
        assert_eq!(json["cat_name"], "餐饮");
        assert_eq!(json.as_object().unwrap().len(), 2);
    }
}

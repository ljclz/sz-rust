//! Dept 模型 — 对齐 PHP `app\common\model\szoa\industry\IndustryDept`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'industry_dept'` | [`Dept::table_name()`] | 表名 |
//! | `$pk = 'dept_id'` | [`Dept::pk_name()`] | 主键列名 |
//! | `$append = []` | [`Dept::append()`]（默认空） | 无静态 append |
//!
//! ## 无访问器 / 无修改器
//!
//! PHP `IndustryDept` 未声明任何 `getXxxAttr` / `setXxxAttr`，
//! Rust 端 [`Dept::accessor_for`] 返回 `Value::Null`，
//! [`Dept::mutator_for`] 返回 `None`。
//!
//! ## 未实现（标 NOTE）
//!
//! - **4 个关联关系**（industry / operate / head / finance BelongsTo + personnel HasMany）→ NOTE(关联模块)
//! - **业务方法**（detail/getSimpleList/getDeptList 等）→ NOTE(控制器层)
//! - **静态缓存**（Cache::get/set）→ NOTE(Cache 模块)

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 部门模型 — 对齐 PHP `app\common\model\szoa\industry\IndustryDept`
#[derive(Clone)]
pub struct Dept {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Dept {
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

impl Default for Dept {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Dept {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "industry_dept"
    }

    fn pk_name() -> &'static str {
        "dept_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "dept_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("dept_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Dept {
    fn columns() -> Vec<&'static str> {
        vec![
            "dept_id",
            "parent_id",
            "industry_id",
            "dept_name",
            "dept_logo",
            "dept_sort",
            "is_show",
            "is_delete",
            "operate_uid",
            "head_uid",
            "finance_uid",
            "province",
            "city",
            "county",
            "street",
            "lng",
            "lat",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "parent_id",
            "industry_id",
            "dept_name",
            "dept_logo",
            "dept_sort",
            "is_show",
            "operate_uid",
            "head_uid",
            "finance_uid",
            "province",
            "city",
            "county",
            "street",
            "lng",
            "lat",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["dept_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "dept_id" | "parent_id" | "industry_id" | "dept_sort" | "is_show" | "is_delete"
            | "operate_uid" | "head_uid" | "finance_uid" | "app_id" | "create_time"
            | "update_time" => v.as_i64().map(OrmValue::I64),
            "dept_name" | "dept_logo" | "province" | "city" | "county" | "street" => {
                v.as_str().map(|s| OrmValue::String(s.to_string()))
            }
            "lng" | "lat" => v.as_f64().map(OrmValue::F64),
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

impl_empty_relation_loader!(Dept);

impl BaseModel for Dept {
    // PHP IndustryDept 未声明 $append，使用默认空 Vec
}

impl Accessor for Dept {
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

    /// PHP IndustryDept 未声明任何 getXxxAttr
    fn accessor_for(&self, _field: &str, value: Option<&Value>) -> Value {
        value.cloned().unwrap_or(Value::Null)
    }
}

impl Mutator for Dept {
    /// PHP IndustryDept 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Dept {
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
        assert_eq!(Dept::table_name(), "industry_dept");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Dept::pk_name(), "dept_id");
    }

    #[test]
    fn test_append_fields_empty_aligns_php() {
        // PHP IndustryDept 未声明 $append
        assert!(Dept::append().is_empty());
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = Dept::columns();
        // 验证关键字段都在
        assert!(cols.contains(&"dept_id"));
        assert!(cols.contains(&"parent_id"));
        assert!(cols.contains(&"industry_id"));
        assert!(cols.contains(&"dept_name"));
        assert!(cols.contains(&"dept_logo"));
        assert!(cols.contains(&"operate_uid"));
        assert!(cols.contains(&"head_uid"));
        assert!(cols.contains(&"finance_uid"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = Dept::fillable();
        assert!(
            !fillable.contains(&"dept_id"),
            "dept_id 应受保护不可批量赋值"
        );
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"dept_name"));
        assert!(fillable.contains(&"parent_id"));
    }

    #[test]
    fn test_guarded_includes_dept_id() {
        assert!(Dept::guarded().contains(&"dept_id"));
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_returns_raw_value_for_all_fields() {
        // PHP IndustryDept 无访问器，getAttr 返回原始字段值
        let model = Dept::new()
            .with_data("dept_id", json!(1))
            .with_data("dept_name", json!("市场部"))
            .with_data("parent_id", json!(0));
        assert_eq!(model.accessor_for("dept_id", None), Value::Null);
        assert_eq!(model.accessor_for("dept_name", None), Value::Null);
        assert_eq!(model.accessor_for("parent_id", None), Value::Null);
        // 传入 value 参数时返回原值
        assert_eq!(
            model.accessor_for("dept_name", Some(&json!("市场部"))),
            json!("市场部")
        );
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP IndustryDept 无修改器
        let mut model = Dept::new();
        let merged = HashMap::new();
        assert_eq!(
            model.mutator_for("dept_name", &json!("测试"), &merged),
            None
        );
        assert_eq!(model.mutator_for("parent_id", &json!(1), &merged), None);
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = Dept::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = Dept::new().with_data("dept_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = Dept::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_no_append_fields() {
        // 无 append 字段，to_json_with_append_cached 等价于 to_json
        let mut model = Dept::new()
            .with_data("dept_id", json!(1))
            .with_data("dept_name", json!("市场部"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["dept_id"], 1);
        assert_eq!(json["dept_name"], "市场部");
        // 无 append 字段，JSON 不应包含额外字段
        assert_eq!(json.as_object().unwrap().len(), 2);
    }
}

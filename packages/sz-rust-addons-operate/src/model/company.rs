//! Company 模型 — 对齐 PHP `addons\operate\model\Company`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_company'` | [`Company::table_name()`] | 表名 |
//! | `$pk = 'company_id'` | [`Company::pk_name()`] | 主键列名 |
//! | `$append = []` | [`Company::append()`]（默认空） | 无静态 append |
//!
//! ## 无访问器 / 无修改器
//!
//! PHP `Company` 未声明任何 `getXxxAttr` / `setXxxAttr`，
//! Rust 端 [`Company::accessor_for`] 返回原始值，
//! [`Company::mutator_for`] 返回 `None`。
//!
//! ## 未实现（标 NOTE）
//!
//! - **业务方法**（detail/info/getAll/getList/add/edit/setDelete 等）→ NOTE(Phase 5+ 控制器层)
//! - **静态缓存**（Cache::get/set，info/getAll 方法）→ NOTE(Phase 6)
//! - **关联关系**→ NOTE(Phase 4)

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 客户公司模型 — 对齐 PHP `addons\operate\model\Company`
#[derive(Clone)]
pub struct Company {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Company {
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

impl Default for Company {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Company {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_company"
    }

    fn pk_name() -> &'static str {
        "company_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "company_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("company_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Company {
    fn columns() -> Vec<&'static str> {
        vec![
            "company_id",
            "company_name",
            "company_linkman",
            "company_address",
            "sort",
            "is_delete",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["company_name", "company_linkman", "company_address", "sort"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["company_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "company_id" | "sort" | "is_delete" | "app_id" | "create_time" | "update_time" => {
                v.as_i64().map(OrmValue::I64)
            }
            "company_name" | "company_linkman" | "company_address" => {
                v.as_str().map(|s| OrmValue::String(s.to_string()))
            }
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

impl_empty_relation_loader!(Company);

impl BaseModel for Company {
    // PHP Company 未声明 $append，使用默认空 Vec
}

impl Accessor for Company {
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

    /// PHP Company 未声明任何 getXxxAttr
    fn accessor_for(&self, _field: &str, value: Option<&Value>) -> Value {
        value.cloned().unwrap_or(Value::Null)
    }
}

impl Mutator for Company {
    /// PHP Company 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Company {
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
        assert_eq!(Company::table_name(), "customer_company");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Company::pk_name(), "company_id");
    }

    #[test]
    fn test_append_fields_empty_aligns_php() {
        // PHP Company 未声明 $append
        assert!(Company::append().is_empty());
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = Company::columns();
        assert!(cols.contains(&"company_id"));
        assert!(cols.contains(&"company_name"));
        assert!(cols.contains(&"company_linkman"));
        assert!(cols.contains(&"company_address"));
        assert!(cols.contains(&"sort"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"app_id"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = Company::fillable();
        assert!(
            !fillable.contains(&"company_id"),
            "company_id 应受保护不可批量赋值"
        );
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"company_name"));
        assert!(fillable.contains(&"company_linkman"));
        assert!(fillable.contains(&"company_address"));
        assert!(fillable.contains(&"sort"));
    }

    #[test]
    fn test_guarded_includes_company_id() {
        assert!(Company::guarded().contains(&"company_id"));
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_returns_raw_value_for_all_fields() {
        // PHP Company 无访问器，getAttr 返回原始字段值
        let model = Company::new()
            .with_data("company_id", json!(1))
            .with_data("company_name", json!("盛庄酒店"));
        assert_eq!(model.accessor_for("company_id", None), Value::Null);
        assert_eq!(model.accessor_for("company_name", None), Value::Null);
        assert_eq!(
            model.accessor_for("company_name", Some(&json!("盛庄酒店"))),
            json!("盛庄酒店")
        );
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP Company 无修改器
        let mut model = Company::new();
        let merged = HashMap::new();
        assert_eq!(
            model.mutator_for("company_name", &json!("测试"), &merged),
            None
        );
        assert_eq!(model.mutator_for("sort", &json!(1), &merged), None);
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = Company::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = Company::new().with_data("company_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = Company::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_no_append_fields() {
        // 无 append 字段，to_json_with_append_cached 等价于 to_json
        let mut model = Company::new()
            .with_data("company_id", json!(1))
            .with_data("company_name", json!("盛庄酒店"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["company_id"], 1);
        assert_eq!(json["company_name"], "盛庄酒店");
        assert_eq!(json.as_object().unwrap().len(), 2);
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_company_no_accessor_no_mutator_no_append() {
        // R5: PHP Company 是最简模型，无访问器/修改器/append
        // Rust 端行为必须与 PHP 完全一致
        let model = Company::new()
            .with_data("company_id", json!(1))
            .with_data("company_name", json!("测试公司"))
            .with_data("sort", json!(100));

        // 访问器返回原始值
        assert_eq!(
            model.accessor_for("company_name", Some(&json!("测试公司"))),
            json!("测试公司")
        );
        assert_eq!(model.accessor_for("sort", Some(&json!(100))), json!(100));

        // append 为空
        assert!(Company::append().is_empty());

        // 序列化不追加额外字段
        let mut model_for_json = model;
        let json = model_for_json.to_json_with_append_cached();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert!(!json.as_object().unwrap().contains_key("company_name_text"));
    }

    #[test]
    fn test_r5_php_company_soft_delete_via_is_delete_field() {
        // R5: PHP Company 通过 is_delete=1 实现软删除（setDelete 方法）
        // Rust 端 Model::soft_delete_field() 返回 None（对齐 PHP BaseModel 不使用 think-orm 软删除特性）
        // 实际软删除由业务层（setDelete 方法）手动设置 is_delete=1
        assert_eq!(Company::soft_delete_field(), None);
    }
}

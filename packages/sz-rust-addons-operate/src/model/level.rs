//! Level 模型 — 对齐 PHP `addons\operate\model\Level`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_level'` | [`Level::table_name()`] | 表名 |
//! | `$pk = 'level_id'` | [`Level::pk_name()`] | 主键列名 |
//! | `$append = []` | [`Level::append()`]（默认空） | 无静态 append |
//!
//! ## 无访问器 / 无修改器
//!
//! PHP `Level` 未声明任何 `getXxxAttr` / `setXxxAttr`，
//! Rust 端 [`Level::accessor_for`] 返回原始值，
//! [`Level::mutator_for`] 返回 `None`。
//!
//! ## 未实现（标 NOTE）
//!
//! - **业务方法**（detail/getList/getAll/getCustomerLevelInfo/getSelectCustomerLevel/add/edit/setDelete）→ NOTE(Phase 5+ 控制器层)
//! - **静态缓存**（Cache::get/set，getAll/getCustomerLevelInfo/getSelectCustomerLevel 方法）→ NOTE(Phase 6)
//! - **关联关系**→ NOTE(Phase 4)

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 客户等级模型 — 对齐 PHP `addons\operate\model\Level`
#[derive(Clone)]
pub struct Level {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Level {
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

impl Default for Level {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Level {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_level"
    }

    fn pk_name() -> &'static str {
        "level_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "level_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("level_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Level {
    fn columns() -> Vec<&'static str> {
        vec![
            "level_id",
            "level_name",
            "level_sort",
            "is_delete",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["level_name", "level_sort"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["level_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "level_id" | "level_sort" | "is_delete" | "app_id" | "create_time" | "update_time" => {
                v.as_i64().map(OrmValue::I64)
            }
            "level_name" => v.as_str().map(|s| OrmValue::String(s.to_string())),
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

impl_empty_relation_loader!(Level);

impl BaseModel for Level {
    // PHP Level 未声明 $append，使用默认空 Vec
}

impl Accessor for Level {
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

    /// PHP Level 未声明任何 getXxxAttr
    fn accessor_for(&self, _field: &str, value: Option<&Value>) -> Value {
        value.cloned().unwrap_or(Value::Null)
    }
}

impl Mutator for Level {
    /// PHP Level 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Level {
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
        assert_eq!(Level::table_name(), "customer_level");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Level::pk_name(), "level_id");
    }

    #[test]
    fn test_append_fields_empty_aligns_php() {
        // PHP Level 未声明 $append
        assert!(Level::append().is_empty());
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = Level::columns();
        assert!(cols.contains(&"level_id"));
        assert!(cols.contains(&"level_name"));
        assert!(cols.contains(&"level_sort"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"app_id"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = Level::fillable();
        assert!(
            !fillable.contains(&"level_id"),
            "level_id 应受保护不可批量赋值"
        );
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"level_name"));
        assert!(fillable.contains(&"level_sort"));
    }

    #[test]
    fn test_guarded_includes_level_id() {
        assert!(Level::guarded().contains(&"level_id"));
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_returns_raw_value_for_all_fields() {
        // PHP Level 无访问器，getAttr 返回原始字段值
        let model = Level::new()
            .with_data("level_id", json!(1))
            .with_data("level_name", json!("VIP"));
        assert_eq!(model.accessor_for("level_id", None), Value::Null);
        assert_eq!(model.accessor_for("level_name", None), Value::Null);
        assert_eq!(
            model.accessor_for("level_name", Some(&json!("VIP"))),
            json!("VIP")
        );
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP Level 无修改器
        let mut model = Level::new();
        let merged = HashMap::new();
        assert_eq!(
            model.mutator_for("level_name", &json!("测试"), &merged),
            None
        );
        assert_eq!(model.mutator_for("level_sort", &json!(1), &merged), None);
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = Level::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = Level::new().with_data("level_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = Level::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_no_append_fields() {
        // 无 append 字段，to_json_with_append_cached 等价于 to_json
        let mut model = Level::new()
            .with_data("level_id", json!(1))
            .with_data("level_name", json!("VIP"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["level_id"], 1);
        assert_eq!(json["level_name"], "VIP");
        assert_eq!(json.as_object().unwrap().len(), 2);
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_level_no_accessor_no_mutator_no_append() {
        // R5: PHP Level 是最简模型，无访问器/修改器/append
        let model = Level::new()
            .with_data("level_id", json!(1))
            .with_data("level_name", json!("普通会员"))
            .with_data("level_sort", json!(10));

        // 访问器返回原始值
        assert_eq!(
            model.accessor_for("level_name", Some(&json!("普通会员"))),
            json!("普通会员")
        );
        assert_eq!(
            model.accessor_for("level_sort", Some(&json!(10))),
            json!(10)
        );

        // append 为空
        assert!(Level::append().is_empty());

        // 序列化不追加额外字段
        let mut model_for_json = model;
        let json = model_for_json.to_json_with_append_cached();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert!(!json.as_object().unwrap().contains_key("level_name_text"));
    }

    #[test]
    fn test_r5_php_level_soft_delete_via_is_delete_field() {
        // R5: PHP Level 通过 is_delete=1 实现软删除（setDelete 方法）
        assert_eq!(Level::soft_delete_field(), None);
    }
}

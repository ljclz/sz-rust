//! Rentarea 模型 — 对齐 PHP `addons\operate\model\Rentarea`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_rentarea'` | [`Rentarea::table_name()`] | 表名 |
//! | `$pk = 'rentarea_id'` | [`Rentarea::pk_name()`] | 主键列名 |
//! | `$append = ['unit','type_name']` | [`Rentarea::append()`] | 静态 append |
//! | `getUnitAttr` | [`Rentarea::accessor_for`] "unit" 分支 | area_type=2→'米'，否则'㎡' |
//! | `getTypeNameAttr` | [`Rentarea::accessor_for`] "type_name" 分支 | area_type=2→'长度'，否则'平方面积' |
//!
//! ## 无修改器
//!
//! PHP `Rentarea` 未声明任何 `setXxxAttr`，Rust 端 [`Rentarea::mutator_for`] 返回 `None`。

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 租赁区域模型 — 对齐 PHP `addons\operate\model\Rentarea`
#[derive(Clone)]
pub struct Rentarea {
    data: HashMap<String, Value>,
    get_cache: HashMap<String, Value>,
    append_state: AppendState,
}

impl Rentarea {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            get_cache: HashMap::new(),
            append_state: AppendState::new(),
        }
    }

    pub fn with_data(mut self, key: &str, value: Value) -> Self {
        self.data.insert(key.to_string(), value);
        self
    }
}

impl Default for Rentarea {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Rentarea {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_rentarea"
    }

    fn pk_name() -> &'static str {
        "rentarea_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "rentarea_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("rentarea_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Rentarea {
    fn columns() -> Vec<&'static str> {
        vec![
            "rentarea_id",
            "customer_id",
            "area",
            "used_area",
            "position",
            "area_type",
            "area_name",
            "dept_id",
            "cat_id",
            "rent",
            "rent_day",
            "remarks",
            "status",
            "app_id",
            "is_delete",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "customer_id",
            "area",
            "used_area",
            "position",
            "area_type",
            "area_name",
            "dept_id",
            "cat_id",
            "rent",
            "rent_day",
            "remarks",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["rentarea_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "rentarea_id" | "customer_id" | "area_type" | "dept_id" | "cat_id" | "status"
            | "app_id" | "is_delete" | "create_time" | "update_time" => {
                v.as_i64().map(OrmValue::I64)
            }
            "area" | "used_area" | "rent" => v.as_f64().map(OrmValue::F64),
            "position" | "area_name" | "rent_day" | "remarks" => {
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

impl_empty_relation_loader!(Rentarea);

impl BaseModel for Rentarea {
    fn append() -> Vec<&'static str> {
        vec!["unit", "type_name"]
    }

    fn get_appended_value(&self, field: &str) -> Option<Value> {
        let value = self.data.get(field);
        Some(self.accessor_for(field, value))
    }
}

impl Accessor for Rentarea {
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

    /// 访问器派发 — 对齐 PHP `getUnitAttr` / `getTypeNameAttr`
    ///
    /// 两个访问器都基于 `$data['area_type']` 判断：
    /// - `area_type == 2` → 长度类型（'米' / '长度'）
    /// - 其他 → 面积类型（'㎡' / '平方面积'）
    ///
    /// **PHP `!empty($data['area_type'])` 行为**：
    /// - `area_type` 不存在 / 0 / "0" → false → 返回面积类型
    /// - `area_type` = 2 → true → 检查 `== 2` → 返回长度类型
    fn accessor_for(&self, field: &str, _value: Option<&Value>) -> Value {
        let area_type = get_i64(&self.data, "area_type").unwrap_or(0);
        // PHP !empty($data['area_type']): 0 视为空
        let is_length_type = area_type != 0 && area_type == 2;
        match field {
            // PHP getUnitAttr: !empty($data['area_type']) && == 2 ? '米' : '㎡'
            "unit" => {
                if is_length_type {
                    json!("米")
                } else {
                    json!("㎡")
                }
            }
            // PHP getTypeNameAttr: !empty($data['area_type']) && == 2 ? '长度' : '平方面积'
            "type_name" => {
                if is_length_type {
                    json!("长度")
                } else {
                    json!("平方面积")
                }
            }
            _ => Value::Null,
        }
    }
}

impl Mutator for Rentarea {
    /// PHP Rentarea 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Rentarea {
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

    #[test]
    fn test_table_name_aligns_php() {
        assert_eq!(Rentarea::table_name(), "customer_rentarea");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Rentarea::pk_name(), "rentarea_id");
    }

    #[test]
    fn test_append_fields_aligns_php() {
        assert_eq!(Rentarea::append(), vec!["unit", "type_name"]);
    }

    #[test]
    fn test_accessor_unit_area_type_2_returns_meter() {
        let model = Rentarea::new().with_data("area_type", json!(2));
        assert_eq!(model.accessor_for("unit", None), json!("米"));
        assert_eq!(model.accessor_for("type_name", None), json!("长度"));
    }

    #[test]
    fn test_accessor_unit_area_type_1_returns_square_meter() {
        let model = Rentarea::new().with_data("area_type", json!(1));
        assert_eq!(model.accessor_for("unit", None), json!("㎡"));
        assert_eq!(model.accessor_for("type_name", None), json!("平方面积"));
    }

    #[test]
    fn test_accessor_unit_area_type_0_returns_square_meter() {
        // PHP !empty(0)=false → 走 else 分支
        let model = Rentarea::new().with_data("area_type", json!(0));
        assert_eq!(model.accessor_for("unit", None), json!("㎡"));
        assert_eq!(model.accessor_for("type_name", None), json!("平方面积"));
    }

    #[test]
    fn test_accessor_unit_area_type_missing_returns_square_meter() {
        // PHP !empty(null)=false → 走 else 分支
        let model = Rentarea::new();
        assert_eq!(model.accessor_for("unit", None), json!("㎡"));
        assert_eq!(model.accessor_for("type_name", None), json!("平方面积"));
    }

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP Rentarea 未声明任何 setXxxAttr
        let mut model = Rentarea::new();
        let merged = HashMap::new();
        assert_eq!(
            model.mutator_for("area_type", &json!(2), &merged),
            None,
            "Rentarea 无修改器，应返回 None"
        );
    }

    #[test]
    fn test_to_json_with_append_cached_includes_unit_and_type_name() {
        let mut model = Rentarea::new()
            .with_data("rentarea_id", json!(1))
            .with_data("area_type", json!(2))
            .with_data("position", json!("A区01"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["area_type"], 2);
        assert_eq!(json["position"], "A区01");
        assert_eq!(json["unit"], "米");
        assert_eq!(json["type_name"], "长度");
    }
}

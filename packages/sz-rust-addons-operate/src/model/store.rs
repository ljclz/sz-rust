//! Store 模型 — 对齐 PHP `addons\operate\model\Store`
//!
//! ## PHP 对齐
//!
//! PHP `Store extends IndustryDept`，继承父类的：
//! - `$name = 'industry_dept'`（表名）
//! - `$pk = 'dept_id'`（主键）
//! - 全部字段、访问器、修改器、append（IndustryDept 均未声明）
//!
//! Rust 端无继承，Store 独立 struct 复刻 IndustryDept 字段集。
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | 继承 `$name = 'industry_dept'` | [`Store::table_name()`] | 表名 |
//! | 继承 `$pk = 'dept_id'` | [`Store::pk_name()`] | 主键列名 |
//! | 继承 `$append = []` | [`Store::append()`]（默认空） | 无静态 append |
//! | `getList($param)` | TODO(Phase 5) | 业务方法 |
//! | `getStat($param)` | TODO(Phase 5) | 业务方法 |
//!
//! ## 无访问器 / 无修改器
//!
//! PHP `Store` 自身未声明任何 `getXxxAttr` / `setXxxAttr`，
//! 父类 `IndustryDept` 也未声明。Rust 端 [`Store::accessor_for`]
//! 返回 `Value::Null`，[`Store::mutator_for`] 返回 `None`。
//!
//! ## 未实现（标 TODO）
//!
//! - **业务方法 `getList`**：PHP 端调用 `Personnel::where()->count()`、
//!   `Customer::where()->count()`、`Rentarea::where()->count()/SUM()`，
//!   并对结果列表做 `usort` 按 `rentarea_scale` 降序排序。Phase 2.10 无
//!   数据库连接，留 TODO(Phase 5 控制器层 + Phase 4 Repository)。
//! - **业务方法 `getStat`**：PHP 端调用 `IndustryDept::where()->column()`、
//!   `Rentarea::where()->count()/SUM()`、`Customer::where()->count()`。
//!   同上原因留 TODO。
//! - **辅助函数 `getPersonnelInfo`**：PHP 全局函数，Rust 端待 Phase 5 移植。

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 商铺模型 — 对齐 PHP `addons\operate\model\Store`（extends IndustryDept）
///
/// PHP 继承结构：`Store extends IndustryDept extends BaseModel`。
/// Rust 端无继承，独立 struct 持有与 [`crate::model::Dept`] 相同的字段集。
#[derive(Clone)]
pub struct Store {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Store {
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

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Store {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        // PHP Store extends IndustryDept，继承 $name = 'industry_dept'
        "industry_dept"
    }

    fn pk_name() -> &'static str {
        // PHP Store 继承 $pk = 'dept_id'
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

impl ModelExt for Store {
    /// 字段集与 [`crate::model::Dept`] 完全相同（PHP 继承自 IndustryDept）
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

impl_empty_relation_loader!(Store);

impl BaseModel for Store {
    // PHP Store + IndustryDept 均未声明 $append，使用默认空 Vec
}

impl Accessor for Store {
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

    /// PHP Store + IndustryDept 均未声明任何 getXxxAttr
    fn accessor_for(&self, _field: &str, value: Option<&Value>) -> Value {
        value.cloned().unwrap_or(Value::Null)
    }
}

impl Mutator for Store {
    /// PHP Store + IndustryDept 均未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Store {
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
    fn test_table_name_inherits_industry_dept() {
        // PHP Store extends IndustryDept，继承 $name = 'industry_dept'
        assert_eq!(Store::table_name(), "industry_dept");
    }

    #[test]
    fn test_pk_name_inherits_industry_dept() {
        // PHP Store 继承 $pk = 'dept_id'
        assert_eq!(Store::pk_name(), "dept_id");
    }

    #[test]
    fn test_append_fields_empty_aligns_php() {
        // PHP Store + IndustryDept 均未声明 $append
        assert!(Store::append().is_empty());
    }

    #[test]
    fn test_columns_inherit_all_industry_dept_fields() {
        // PHP Store 继承 IndustryDept 全部字段
        let cols = Store::columns();
        assert!(cols.contains(&"dept_id"));
        assert!(cols.contains(&"parent_id"));
        assert!(cols.contains(&"industry_id"));
        assert!(cols.contains(&"dept_name"));
        assert!(cols.contains(&"dept_logo"));
        assert!(cols.contains(&"dept_sort"));
        assert!(cols.contains(&"is_show"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"operate_uid"));
        assert!(cols.contains(&"head_uid"));
        assert!(cols.contains(&"finance_uid"));
        assert!(cols.contains(&"province"));
        assert!(cols.contains(&"city"));
        assert!(cols.contains(&"county"));
        assert!(cols.contains(&"street"));
        assert!(cols.contains(&"lng"));
        assert!(cols.contains(&"lat"));
        assert!(cols.contains(&"app_id"));
        assert!(cols.contains(&"create_time"));
        assert!(cols.contains(&"update_time"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = Store::fillable();
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
        assert!(Store::guarded().contains(&"dept_id"));
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_returns_raw_value_for_all_fields() {
        // PHP Store 无访问器，getAttr 返回原始字段值
        let model = Store::new()
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
        // PHP Store 无修改器
        let mut model = Store::new();
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
        let model = Store::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = Store::new().with_data("dept_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = Store::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_no_append_fields() {
        // 无 append 字段，to_json_with_append_cached 等价于 to_json
        let mut model = Store::new()
            .with_data("dept_id", json!(1))
            .with_data("dept_name", json!("市场部"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["dept_id"], 1);
        assert_eq!(json["dept_name"], "市场部");
        // 无 append 字段，JSON 不应包含额外字段
        assert_eq!(json.as_object().unwrap().len(), 2);
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_store_inherits_industry_dept_metadata() {
        // R5: PHP Store extends IndustryDept
        // Rust 端 Store 与 Dept 元数据完全相同
        assert_eq!(Store::table_name(), crate::Dept::table_name());
        assert_eq!(Store::pk_name(), crate::Dept::pk_name());
        assert_eq!(Store::columns(), crate::Dept::columns());
        assert_eq!(Store::fillable(), crate::Dept::fillable());
        assert_eq!(Store::guarded(), crate::Dept::guarded());
        assert_eq!(Store::append(), crate::Dept::append());
    }

    #[test]
    fn test_r5_php_store_business_methods_documented_as_todo() {
        // R5: PHP Store 声明 getList($param) 和 getStat($param)
        // Phase 2.10 无数据库连接，业务方法留 TODO(Phase 5)
        // 此测试验证 Store 模型本身可正常构造，业务方法待 Phase 5 实现
        let model = Store::new().with_data("dept_id", json!(1));
        assert_eq!(model.pk(), 1);
    }

    #[test]
    fn test_r5_php_store_no_own_accessors_or_mutators() {
        // R5: PHP Store 自身未声明任何 getXxxAttr / setXxxAttr
        // 父类 IndustryDept 也未声明
        let mut model = Store::new().with_data("dept_name", json!("测试"));
        let merged = HashMap::new();
        // 访问器对所有字段返回 None（传入 None）或原值（传入 Some）
        assert_eq!(model.accessor_for("dept_name", None), Value::Null);
        assert_eq!(
            model.accessor_for("dept_name", Some(&json!("测试"))),
            json!("测试")
        );
        // 修改器对所有字段返回 None
        assert_eq!(
            model.mutator_for("dept_name", &json!("新值"), &merged),
            None
        );
    }
}

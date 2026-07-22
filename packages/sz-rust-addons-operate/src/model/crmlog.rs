//! Crmlog 模型 — 对齐 PHP `addons\operate\model\Crmlog`
//!
//! ## PHP namespace bug 说明
//!
//! **PHP 源码 bug**：文件物理路径为 `addons/operate/model/Crmlog.php`，
//! 但 PHP `namespace` 声明为 `addons\finance\model`（应为 `addons\operate\model`）。
//! 这是 PHP 端的历史遗留 bug，Rust 端按文件物理位置归入 `sz-rust-addons-operate`，
//! 与 PHP 文件位置保持一致。
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'crm_log'` | [`Crmlog::table_name()`] | 表名 |
//! | `$pk = 'log_id'` | [`Crmlog::pk_name()`] | 主键列名 |
//! | `$append = []` | [`Crmlog::append()`]（默认空） | 无静态 append |
//! | `payment()` belongsTo | TODO(Phase 4) | `Payment` mid→payment_id |
//! | `personnel()` belongsTo | TODO(Phase 4) | `IndustryPersonnel` check_uid→uid |
//!
//! ## 无访问器 / 无修改器
//!
//! PHP `Crmlog` 未声明任何 `getXxxAttr` / `setXxxAttr`。
//!
//! ## 未实现（标 TODO）
//!
//! - **业务方法**（detail/getAll/getList/setDelete）→ TODO(Phase 5+ 控制器层)
//! - **关联关系**（payment/personnel belongsTo）→ TODO(Phase 4)

use crate::model::{get_i64, impl_empty_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// CRM 日志模型 — 对齐 PHP `addons\operate\model\Crmlog`
///
/// **注意**：PHP 端 namespace 为 `addons\finance\model`（历史 bug），
/// Rust 端按文件物理位置归入 `sz-rust-addons-operate`。
#[derive(Clone)]
pub struct Crmlog {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl Crmlog {
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

impl Default for Crmlog {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Crmlog {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "crm_log"
    }

    fn pk_name() -> &'static str {
        "log_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "log_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("log_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Crmlog {
    fn columns() -> Vec<&'static str> {
        vec![
            "log_id",
            "mid",
            "check_uid",
            "uid",
            "status",
            "status_name",
            "explain",
            "model_table",
            "is_delete",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "mid",
            "check_uid",
            "uid",
            "status",
            "status_name",
            "explain",
            "model_table",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["log_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "log_id" | "mid" | "check_uid" | "uid" | "status" | "is_delete" | "app_id"
            | "create_time" | "update_time" => v.as_i64().map(OrmValue::I64),
            "status_name" | "explain" | "model_table" => {
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

impl_empty_relation_loader!(Crmlog);

impl BaseModel for Crmlog {
    // PHP Crmlog 未声明 $append，使用默认空 Vec
}

impl Accessor for Crmlog {
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

    /// PHP Crmlog 未声明任何 getXxxAttr
    fn accessor_for(&self, _field: &str, value: Option<&Value>) -> Value {
        value.cloned().unwrap_or(Value::Null)
    }
}

impl Mutator for Crmlog {
    /// PHP Crmlog 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for Crmlog {
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
        assert_eq!(Crmlog::table_name(), "crm_log");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Crmlog::pk_name(), "log_id");
    }

    #[test]
    fn test_append_fields_empty_aligns_php() {
        // PHP Crmlog 未声明 $append
        assert!(Crmlog::append().is_empty());
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = Crmlog::columns();
        assert!(cols.contains(&"log_id"));
        assert!(cols.contains(&"mid"));
        assert!(cols.contains(&"check_uid"));
        assert!(cols.contains(&"uid"));
        assert!(cols.contains(&"status"));
        assert!(cols.contains(&"status_name"));
        assert!(cols.contains(&"explain"));
        assert!(cols.contains(&"model_table"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"app_id"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = Crmlog::fillable();
        assert!(!fillable.contains(&"log_id"), "log_id 应受保护不可批量赋值");
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"mid"));
        assert!(fillable.contains(&"status"));
        assert!(fillable.contains(&"explain"));
    }

    #[test]
    fn test_guarded_includes_log_id() {
        assert!(Crmlog::guarded().contains(&"log_id"));
    }

    // -------------------- 访问器测试 --------------------

    #[test]
    fn test_accessor_returns_raw_value_for_all_fields() {
        // PHP Crmlog 无访问器，getAttr 返回原始字段值
        let model = Crmlog::new()
            .with_data("log_id", json!(1))
            .with_data("status_name", json!("已审核"));
        assert_eq!(model.accessor_for("log_id", None), Value::Null);
        assert_eq!(model.accessor_for("status_name", None), Value::Null);
        assert_eq!(
            model.accessor_for("status_name", Some(&json!("已审核"))),
            json!("已审核")
        );
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP Crmlog 无修改器
        let mut model = Crmlog::new();
        let merged = HashMap::new();
        assert_eq!(model.mutator_for("status", &json!(1), &merged), None);
        assert_eq!(
            model.mutator_for("status_name", &json!("测试"), &merged),
            None
        );
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = Crmlog::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = Crmlog::new().with_data("log_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = Crmlog::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_no_append_fields() {
        // 无 append 字段，to_json_with_append_cached 等价于 to_json
        let mut model = Crmlog::new()
            .with_data("log_id", json!(1))
            .with_data("status_name", json!("已审核"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["log_id"], 1);
        assert_eq!(json["status_name"], "已审核");
        assert_eq!(json.as_object().unwrap().len(), 2);
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_crmlog_no_accessor_no_mutator_no_append() {
        // R5: PHP Crmlog 无访问器/修改器/append
        let model = Crmlog::new()
            .with_data("log_id", json!(1))
            .with_data("status", json!(1))
            .with_data("status_name", json!("已审核"));

        // 访问器返回原始值
        assert_eq!(
            model.accessor_for("status_name", Some(&json!("已审核"))),
            json!("已审核")
        );
        assert_eq!(model.accessor_for("status", Some(&json!(1))), json!(1));

        // append 为空
        assert!(Crmlog::append().is_empty());

        // 序列化不追加额外字段
        let mut model_for_json = model;
        let json = model_for_json.to_json_with_append_cached();
        assert_eq!(json.as_object().unwrap().len(), 3);
    }

    #[test]
    fn test_r5_php_crmlog_soft_delete_via_is_delete_field() {
        // R5: PHP Crmlog 通过 is_delete=1 实现软删除（setDelete 方法）
        assert_eq!(Crmlog::soft_delete_field(), None);
    }

    #[test]
    fn test_r5_php_crmlog_namespace_bug_documented() {
        // R5: PHP 源码 bug — 文件位于 addons/operate/model/ 但 namespace 是 addons\finance\model
        // Rust 端按文件物理位置归入 sz-rust-addons-operate
        // 此测试仅作为文档化记录，验证模型可正常创建
        let model = Crmlog::new().with_data("log_id", json!(1));
        assert_eq!(model.pk(), 1);
        assert_eq!(Crmlog::table_name(), "crm_log");
    }
}

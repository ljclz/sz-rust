//! ContractLog 模型 — 对齐 PHP `addons\operate\model\ContractLog`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_contract_log'` | [`ContractLog::table_name()`] | 表名 |
//! | `$pk = 'log_id'` | [`ContractLog::pk_name()`] | 主键列名 |
//! | `$append = ['type_info']` | [`ContractLog::append()`] | 静态 append |
//! | `getBeforeNumAttr` | [`ContractLog::accessor_for`] "before_num" | float 强转 |
//! | `getAfterNumAttr` | [`ContractLog::accessor_for`] "after_num" | float 强转 |
//! | `getChageNumAttr` | [`ContractLog::accessor_for`] "chage_num" | float 强转（PHP typo: chage） |
//! | `getTypeInfoAttr` | [`ContractLog::accessor_for`] "type_info" | type 字段映射 |
//! | `personnel()` belongsTo | NOTE(关联模块) | `IndustryPersonnel` uid→uid |
//! | `contract()` belongsTo | NOTE(关联模块) | `Contract` contract_id→contract_id |
//! | `dept()` belongsTo | NOTE(关联模块) | `IndustryDept` dept_id→dept_id |
//! | `customer()` belongsTo | NOTE(关联模块) | `Customer` customer_id→customer_id |
//!
//! ## PHP `if($value) return (float)$value; return 0;` 行为复刻
//!
//! ContractLog 的 3 个 float 访问器（before_num/after_num/chage_num）使用
//! `if($value) return (float)$value; return 0;` 模式，
//! 与 Contract 的 `$value ? (float)$value : 0` 语义完全相同，
//! 复用 `contract::php_price_attr`。
//!
//! ## PHP `getTypeInfoAttr` 行为
//!
//! ```php
//! public function getTypeInfoAttr($value,$data): array {
//!     if(!empty($data['type']) && $data['type'] == 1) {
//!         return ['text'=>'增加','color'=>'orange'];
//!     }
//!     return ['text'=>'减少','color'=>'green'];
//! }
//! ```
//!
//! - `!empty($data['type'])` → type 非 0/空/"0"/null/false
//! - `$data['type'] == 1` → PHP 松散比较：1=="1"==true
//! - 满足条件返回增加(橙色)，否则返回减少(绿色)
//!
//! ## 未实现（标 NOTE）
//!
//! - **业务方法**（detail/getLogs/getList/getStat/add）→ NOTE(控制器层)
//! - **关联关系**（personnel/contract/dept/customer belongsTo）→ NOTE(关联模块)

use crate::model::contract::php_price_attr;
use crate::model::{get_i64, impl_relation_loader};
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};
use sz_rust_core::orm::{Model, ModelExt, TimestampFields};

/// 合同变更日志模型 — 对齐 PHP `addons\operate\model\ContractLog`
#[derive(Clone)]
pub struct ContractLog {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
    /// 已加载的关联数据（H-1 修复：真实 RelationLoader 存储）
    relations: HashMap<String, sz_rust_core::orm::Value>,
}

impl ContractLog {
    /// 创建空模型
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            get_cache: HashMap::new(),
            append_state: AppendState::new(),
            relations: HashMap::new(),
        }
    }

    /// 链式设置字段值（测试用）
    pub fn with_data(mut self, key: &str, value: Value) -> Self {
        self.data.insert(key.to_string(), value);
        self
    }

    /// PHP `getTypeInfoAttr` 行为复刻
    ///
    /// 条件：`!empty($data['type']) && $data['type'] == 1`
    /// - type=1 → `{"text":"增加","color":"orange"}`
    /// - type=0 或空 → `{"text":"减少","color":"green"}`
    fn type_info_attr(data: &HashMap<String, Value>) -> Value {
        let type_value = data.get("type");
        let is_type_one = match type_value {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => {
                // PHP empty(0)=true, empty(0.0)=true
                // PHP $data['type'] == 1: 1==1 true, 1.0==1 true
                n.as_i64()
                    .map(|i| i != 0 && i == 1)
                    .unwrap_or_else(|| n.as_f64().map(|f| f != 0.0 && f == 1.0).unwrap_or(false))
            }
            Some(Value::String(s)) => {
                let trimmed = s.trim();
                // PHP empty("0")=true, empty("")=true
                // PHP "1" == 1 true (loose comparison)
                !trimmed.is_empty() && trimmed != "0" && trimmed.parse::<i64>().ok() == Some(1)
            }
            _ => false,
        };
        if is_type_one {
            json!({"text": "增加", "color": "orange"})
        } else {
            json!({"text": "减少", "color": "green"})
        }
    }
}

impl Default for ContractLog {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for ContractLog {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_contract_log"
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

impl ModelExt for ContractLog {
    fn columns() -> Vec<&'static str> {
        vec![
            "log_id",
            "contract_id",
            "uid",
            "type",
            "customer_id",
            "dept_id",
            "before_num",
            "after_num",
            "chage_num",
            "is_delete",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "contract_id",
            "uid",
            "type",
            "customer_id",
            "dept_id",
            "before_num",
            "after_num",
            "chage_num",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["log_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "log_id" | "contract_id" | "uid" | "type" | "customer_id" | "dept_id" | "is_delete"
            | "app_id" | "create_time" | "update_time" => v.as_i64().map(OrmValue::I64),
            "before_num" | "after_num" | "chage_num" => {
                v.as_f64().map(OrmValue::F64).or_else(|| {
                    v.as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(OrmValue::F64)
                })
            }
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, sz_rust_core::orm::Value>) {
        for (k, v) in map {
            let json_val = match v {
                sz_rust_core::orm::Value::I64(i) => json!(i),
                sz_rust_core::orm::Value::I32(i) => json!(i),
                sz_rust_core::orm::Value::F64(f) => json!(f),
                sz_rust_core::orm::Value::String(s) => json!(s),
                sz_rust_core::orm::Value::Array(_) => json!(null),
                other => serde_json::to_value(&other).unwrap_or(json!(null)),
            };
            self.data.insert(k, json_val);
        }
    }
}

impl_relation_loader!(ContractLog);

impl BaseModel for ContractLog {
    fn append() -> Vec<&'static str> {
        vec!["type_info"]
    }
}

impl Accessor for ContractLog {
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

    /// PHP ContractLog 访问器派发
    ///
    /// - `before_num` / `after_num` / `chage_num`：`if($value) return (float)$value; return 0;`
    ///   复用 `contract::php_price_attr`
    /// - `type_info`：基于 `$data['type']` 返回 `{"text":"增加","color":"orange"}` 或
    ///   `{"text":"减少","color":"green"}`
    fn accessor_for(&self, field: &str, value: Option<&Value>) -> Value {
        match field {
            "before_num" | "after_num" | "chage_num" => php_price_attr(value),
            "type_info" => Self::type_info_attr(&self.data),
            _ => value.cloned().unwrap_or(Value::Null),
        }
    }
}

impl Mutator for ContractLog {
    /// PHP ContractLog 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for ContractLog {
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
        assert_eq!(ContractLog::table_name(), "customer_contract_log");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(ContractLog::pk_name(), "log_id");
    }

    #[test]
    fn test_append_fields_aligns_php() {
        // PHP $append = ['type_info']
        let append = ContractLog::append();
        assert_eq!(append, vec!["type_info"]);
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = ContractLog::columns();
        assert!(cols.contains(&"log_id"));
        assert!(cols.contains(&"contract_id"));
        assert!(cols.contains(&"uid"));
        assert!(cols.contains(&"type"));
        assert!(cols.contains(&"customer_id"));
        assert!(cols.contains(&"dept_id"));
        assert!(cols.contains(&"before_num"));
        assert!(cols.contains(&"after_num"));
        assert!(cols.contains(&"chage_num"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"app_id"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = ContractLog::fillable();
        assert!(!fillable.contains(&"log_id"), "log_id 应受保护不可批量赋值");
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"contract_id"));
        assert!(fillable.contains(&"type"));
        assert!(fillable.contains(&"before_num"));
    }

    #[test]
    fn test_guarded_includes_log_id() {
        assert!(ContractLog::guarded().contains(&"log_id"));
    }

    // -------------------- 访问器测试：float 强转 --------------------

    #[test]
    fn test_before_num_accessor_aligns_php_float_cast() {
        // PHP: if($value) return (float)$value; return 0;
        let model = ContractLog::new();
        // 非零值 → float
        assert_eq!(
            model.accessor_for("before_num", Some(&json!(100.50))),
            json!(100.5)
        );
        assert_eq!(model.accessor_for("before_num", Some(&json!(0))), json!(0));
        assert_eq!(model.accessor_for("before_num", None), json!(0));
    }

    #[test]
    fn test_after_num_accessor_aligns_php_float_cast() {
        let model = ContractLog::new();
        assert_eq!(
            model.accessor_for("after_num", Some(&json!(200.25))),
            json!(200.25)
        );
        assert_eq!(model.accessor_for("after_num", Some(&json!(0))), json!(0));
    }

    #[test]
    fn test_chage_num_accessor_aligns_php_float_cast() {
        // 注意 PHP typo: chage_num（非 change_num）
        let model = ContractLog::new();
        assert_eq!(
            model.accessor_for("chage_num", Some(&json!(50.5))),
            json!(50.5)
        );
        assert_eq!(model.accessor_for("chage_num", Some(&json!(0))), json!(0));
    }

    #[test]
    fn test_float_accessor_string_value_parses_to_float() {
        // PHP (float)"100.5" = 100.5
        let model = ContractLog::new();
        assert_eq!(
            model.accessor_for("before_num", Some(&json!("100.5"))),
            json!(100.5)
        );
    }

    // -------------------- 访问器测试：type_info --------------------

    #[test]
    fn test_type_info_attr_type_1_returns_increase_orange() {
        // PHP: !empty($data['type']) && $data['type'] == 1 → 增加(orange)
        let model = ContractLog::new().with_data("type", json!(1));
        let result = model.accessor_for("type_info", None);
        assert_eq!(result["text"], "增加");
        assert_eq!(result["color"], "orange");
    }

    #[test]
    fn test_type_info_attr_type_0_returns_decrease_green() {
        // PHP: empty(0)=true → 减少(green)
        let model = ContractLog::new().with_data("type", json!(0));
        let result = model.accessor_for("type_info", None);
        assert_eq!(result["text"], "减少");
        assert_eq!(result["color"], "green");
    }

    #[test]
    fn test_type_info_attr_type_missing_returns_decrease_green() {
        // PHP: empty(null)=true → 减少(green)
        let model = ContractLog::new();
        let result = model.accessor_for("type_info", None);
        assert_eq!(result["text"], "减少");
        assert_eq!(result["color"], "green");
    }

    #[test]
    fn test_type_info_attr_type_string_1_returns_increase_orange() {
        // PHP: !empty("1")=true && "1"==1 → 增加(orange)
        let model = ContractLog::new().with_data("type", json!("1"));
        let result = model.accessor_for("type_info", None);
        assert_eq!(result["text"], "增加");
        assert_eq!(result["color"], "orange");
    }

    #[test]
    fn test_type_info_attr_type_string_0_returns_decrease_green() {
        // PHP: empty("0")=true → 减少(green)
        let model = ContractLog::new().with_data("type", json!("0"));
        let result = model.accessor_for("type_info", None);
        assert_eq!(result["text"], "减少");
        assert_eq!(result["color"], "green");
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP ContractLog 无修改器
        let mut model = ContractLog::new();
        let merged = HashMap::new();
        assert_eq!(model.mutator_for("before_num", &json!(100), &merged), None);
        assert_eq!(model.mutator_for("type", &json!(1), &merged), None);
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = ContractLog::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = ContractLog::new().with_data("log_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = ContractLog::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_includes_type_info() {
        // append=['type_info']，序列化时应自动追加 type_info 字段
        let mut model = ContractLog::new()
            .with_data("log_id", json!(1))
            .with_data("type", json!(1))
            .with_data("before_num", json!(100.5));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["log_id"], 1);
        assert_eq!(json["type"], 1);
        assert_eq!(json["before_num"], 100.5);
        // type_info 是 append 字段，应自动追加
        assert_eq!(json["type_info"]["text"], "增加");
        assert_eq!(json["type_info"]["color"], "orange");
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_contract_log_float_accessor_zero_handling() {
        // R5: PHP if($value) return (float)$value; return 0;
        // 0/0.0/"0"/""/null → 0（int）
        let model = ContractLog::new();
        assert_eq!(model.accessor_for("before_num", Some(&json!(0))), json!(0));
        assert_eq!(
            model.accessor_for("before_num", Some(&json!(0.0))),
            json!(0)
        );
        assert_eq!(
            model.accessor_for("before_num", Some(&json!("0"))),
            json!(0)
        );
        assert_eq!(model.accessor_for("before_num", Some(&json!(""))), json!(0));
        assert_eq!(model.accessor_for("before_num", None), json!(0));
    }

    #[test]
    fn test_r5_php_contract_log_type_info_php_bug_replication() {
        // R5: PHP getTypeInfoAttr 默认返回减少(green)（即使 type 字段不存在）
        // 这与 PHP empty(null)=true 行为一致
        let model = ContractLog::new();
        let result = model.accessor_for("type_info", None);
        assert_eq!(result["text"], "减少");
        assert_eq!(result["color"], "green");
    }

    #[test]
    fn test_r5_php_contract_log_chage_num_field_name_typo() {
        // R5: PHP 源码 typo — chage_num（非 change_num）
        // Rust 端严格对齐 PHP 字段名
        let cols = ContractLog::columns();
        assert!(
            cols.contains(&"chage_num"),
            "字段名应为 chage_num（对齐 PHP typo）"
        );
        assert!(!cols.contains(&"change_num"), "不应存在 change_num 字段");
    }

    #[test]
    fn test_r5_php_contract_log_soft_delete_via_is_delete_field() {
        // R5: PHP ContractLog 通过 is_delete=1 实现软删除
        assert_eq!(ContractLog::soft_delete_field(), None);
    }
}

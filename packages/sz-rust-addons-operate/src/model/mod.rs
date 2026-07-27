//! 模型模块 — 5 个核心业务模型
//!
//! 对齐 PHP `addons/operate/model/` 与 `app/common/model/szoa/industry/`。
//!
//! ## 共享实现策略
//!
//! 5 个业务模型都用 `HashMap<String, serde_json::Value>` 持有数据（PHP `$this->data` 风格），
//! 通过 `sz_rust_core::model::BaseModel` + `sz_rust_core::model::Accessor` + `sz_rust_core::model::Mutator` +
//! `sz_rust_core::model::Appendable` trait 组合获得完整 PHP 对齐能力。
//!
//! 每个业务模型重写 `accessor_for` / `mutator_for` / `append` / `hidden` / `get_appended_value`
//! 实现特定 PHP 行为。

pub mod category;
pub mod company;
pub mod contract;
pub mod contract_log;
pub mod crmlog;
pub mod customer;
pub mod customer_pay;
pub mod dept;
pub mod level;
pub mod rentarea;
pub mod store;

// 重导出业务模型，使控制器可通过 `crate::model::Xxx` 引用
pub use category::Category;
pub use company::Company;
pub use contract::Contract;
pub use contract_log::ContractLog;
pub use crmlog::Crmlog;
pub use customer::Customer;
pub use customer_pay::CustomerPay;
pub use dept::Dept;
pub use level::Level;
pub use rentarea::Rentarea;
pub use store::Store;

use serde_json::Value;
use std::collections::HashMap;

use sz_orm_core::repository::EntityAttributes;

/// 为模型实现 `EntityAttributes` trait（委托给 `ModelExt::get_column_value`）
///
/// 使模型可与 `InMemoryRepository` 配合使用，支持 `find_by` 条件查询。
/// 实现委托给各模型的 `get_column_value` 方法（已由 `ModelExt` trait 定义）。
///
/// ## 主键别名 "id"
///
/// `InMemoryRepository::key_of` 默认通过 `entity.get_attribute("id")` 提取主键，
/// 但业务模型主键名各异（`company_id`/`level_id`/`cat_id` 等）。
/// 因此当 `field == "id"` 时，返回 `Model::pk()` 的值，
/// 使 `save`/`find_by_id` 等方法能正确定位记录。
macro_rules! impl_entity_attributes {
    ($model:ty) => {
        impl EntityAttributes for $model {
            fn get_attribute(&self, field: &str) -> Option<sz_orm_core::Value> {
                if field == "id" {
                    // 主键别名：返回主键值（用于 InMemoryRepository::key_of）
                    use sz_orm_core::Model;
                    Some(sz_orm_core::Value::I64(self.pk()))
                } else {
                    <Self as sz_orm_core::ModelExt>::get_column_value(self, field)
                }
            }
        }
    };
}

impl_entity_attributes!(category::Category);
impl_entity_attributes!(company::Company);
impl_entity_attributes!(contract::Contract);
impl_entity_attributes!(contract_log::ContractLog);
impl_entity_attributes!(crmlog::Crmlog);
impl_entity_attributes!(customer::Customer);
impl_entity_attributes!(customer_pay::CustomerPay);
impl_entity_attributes!(dept::Dept);
impl_entity_attributes!(level::Level);
impl_entity_attributes!(rentarea::Rentarea);
impl_entity_attributes!(store::Store);

/// 业务模型共享的元数据补全宏
///
/// 5 个模型都需要实现 `RelationLoader`，且当前关联关系尚未实现，
/// 统一返回空实现。完整接入关联关系后移除。
///
/// **注意**：宏内使用完全限定路径 `sz_orm_core::RelationLoader` 和 `sz_orm_core::Value`，
/// 避免在调用点（如 `customer.rs`）依赖 trait/类型已导入。
///
/// ## `from_value` 共享行为说明
///
/// 5 个业务模型的 `ModelExt::from_value` 实现采用相同模式：
///
/// ```ignore
/// fn from_value(&mut self, map: HashMap<String, sz_orm_core::Value>) {
///     for (k, v) in map {
///         let json_val = match v {
///             sz_orm_core::Value::I64(i) => json!(i),
///             sz_orm_core::Value::I32(i) => json!(i),
///             sz_orm_core::Value::F64(f) => json!(f),
///             sz_orm_core::Value::String(s) => json!(s),
///             sz_orm_core::Value::Array(_) => json!(null),  // 防御性：业务字段均为标量
///             other => serde_json::to_value(&other).unwrap_or(json!(null)),
///         };
///         self.data.insert(k, json_val);
///     }
/// }
/// ```
///
/// **`Value::Array(_) => json!(null)` 防御性处理说明**：
/// 5 个业务模型（Customer/Contract/Rentarea/Dept/Category）的所有字段均为标量
/// （i64/f64/String），数据库 schema 中不存在数组类型字段。
/// 若 `from_value` 收到 `Value::Array`，说明上游传入非法数据，
/// 统一映射为 `null`（避免 panic，由后续 `get_column_value` 的 `as_i64`/`as_f64`/`as_str` 返回 `None` 兜底）。
macro_rules! impl_empty_relation_loader {
    ($model:ty) => {
        impl sz_orm_core::RelationLoader for $model {
            fn get_relation(&self, _name: &str) -> Option<&sz_orm_core::Value> {
                None
            }
            fn set_relation_data(&mut self, _name: &str, _data: sz_orm_core::Value) {}
            fn get_relation_fk_value(&self, _fk_name: &str) -> String {
                String::new()
            }
        }
    };
}

pub(crate) use impl_empty_relation_loader;

/// 业务模型共享工具：从 HashMap 取 i64
pub(crate) fn get_i64(data: &HashMap<String, Value>, key: &str) -> Option<i64> {
    data.get(key).and_then(|v| v.as_i64())
}

/// 业务模型共享工具：从 HashMap 取字符串
#[allow(dead_code)] // 预留给后续业务模型使用
pub(crate) fn get_str(data: &HashMap<String, Value>, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 业务模型共享工具：将 Vec<i64> 转为逗号分隔字符串（对齐 PHP `implode(',', $arr)`）
pub(crate) fn vec_i64_to_csv(arr: &[i64]) -> String {
    arr.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 业务模型共享工具：将逗号分隔字符串转 Vec<i64>（对齐 PHP `getRentareaIdsAttr`）
///
/// PHP 行为（`Customer::getRentareaIdsAttr`）：
/// ```php
/// if(!empty($value)){
///     $arr = explode("," , $value);
///     foreach($arr as $key=>$item){
///         if(!empty($item)) $arr[$key] = intval($item);
///         else unset($arr[$key]);
///     }
///     return $arr;
/// }else{
///     return [];
/// }
/// ```
///
/// 即：空字符串返回空数组；非空项 `intval`；空项过滤掉。
///
/// **PHP `empty()` 特殊行为**：`empty(0)` 和 `empty("0")` 都返回 true，
/// 因此 `"0,1,2"` 在 PHP 端会过滤掉 0，返回 `[1, 2]`。
/// Rust 端严格对齐：parse 失败映射为 0，再过滤 0。
pub(crate) fn csv_to_vec_i64(value: &str) -> Vec<i64> {
    if value.is_empty() {
        return Vec::new();
    }
    value
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            // PHP intval 解析失败返回 0，empty(0)=true 过滤掉
            // Rust 端：parse 失败映射为 0，再过滤 0（严格对齐 PHP empty 语义）
            let n: i64 = trimmed.parse().unwrap_or(0);
            if n == 0 {
                None
            } else {
                Some(n)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_to_vec_i64_aligns_php_explode_intval_filter() {
        // PHP: "1,2,3" → [1, 2, 3]
        assert_eq!(csv_to_vec_i64("1,2,3"), vec![1, 2, 3]);
        // PHP: "" → []
        assert_eq!(csv_to_vec_i64(""), Vec::<i64>::new());
        // PHP: "1,,3" → [1, 3]（空项被 unset）
        assert_eq!(csv_to_vec_i64("1,,3"), vec![1, 3]);
        // PHP: "1,abc,3" → [1, 3]（intval("abc")=0，但 empty(0)=true → unset）
        // Rust 端 parse 失败返回 None，等价过滤
        assert_eq!(csv_to_vec_i64("1,abc,3"), vec![1, 3]);
    }

    #[test]
    fn test_vec_i64_to_csv_aligns_php_implode() {
        assert_eq!(vec_i64_to_csv(&[1, 2, 3]), "1,2,3");
        assert_eq!(vec_i64_to_csv(&[]), "");
    }
}

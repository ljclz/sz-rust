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

use sz_rust_core::orm::repository::EntityAttributes;

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
            fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
                if field == "id" {
                    // 主键别名：返回主键值（用于 InMemoryRepository::key_of）
                    use sz_rust_core::orm::Model;
                    Some(sz_rust_core::orm::Value::I64(self.pk()))
                } else {
                    <Self as sz_rust_core::orm::ModelExt>::get_column_value(self, field)
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

/// 真实 RelationLoader 实现宏 — H-1 修复
///
/// 为业务模型实现真实的关联数据加载能力：
/// - `get_relation(name)`: 从 `self.relations` HashMap 读取已加载的关联数据
/// - `set_relation_data(name, data)`: 将关联数据写入 `self.relations` HashMap
/// - `get_relation_fk_value(fk_name)`: 从 `self.data_map()` 读取外键字段的字符串值
///
/// ## 使用要求
///
/// 模型必须满足以下条件：
/// 1. 拥有 `relations: HashMap<String, sz_rust_core::orm::Value>` 字段
/// 2. 实现 `sz_rust_core::model::Accessor` trait（提供 `data_map()` 方法）
///
/// ## 外键值提取策略
///
/// `get_relation_fk_value` 通过 `Accessor::data_map()` 获取字段值，
/// 将 JSON Value 转为字符串：
/// - `Null` → 空字符串（表示无外键关联）
/// - `String(s)` → s（直接返回字符串值）
/// - `Number(n)` → n.to_string()（数字转字符串）
/// - `Bool(b)` → b.to_string()
/// - 其他 → JSON 序列化字符串
///
/// 这与 sz-orm-core 的 `BelongsTo` 关系期望一致：
/// 空字符串表示无关联，非空字符串用于 SQL 查询。
///
/// ## 示例
///
/// ```ignore
/// pub struct Customer {
///     data: HashMap<String, Value>,
///     get_cache: HashMap<String, Value>,
///     append_state: AppendState,
///     relations: HashMap<String, sz_rust_core::orm::Value>,  // 新增字段
/// }
///
/// impl_relation_loader!(Customer);
/// ```
macro_rules! impl_relation_loader {
    ($model:ty) => {
        impl sz_rust_core::orm::RelationLoader for $model {
            /// 获取已加载的关联数据
            ///
            /// # 参数
            ///
            /// - `name`: 关联名称（如 "rentarea", "logs", "customer"）
            ///
            /// # 返回
            ///
            /// - `Some(&Value)`: 已加载的关联数据
            /// - `None`: 关联未加载
            fn get_relation(&self, name: &str) -> Option<&sz_rust_core::orm::Value> {
                self.relations.get(name)
            }

            /// 写入已加载的关联数据
            ///
            /// # 参数
            ///
            /// - `name`: 关联名称
            /// - `data`: 关联数据（ORM Value 类型）
            fn set_relation_data(&mut self, name: &str, data: sz_rust_core::orm::Value) {
                self.relations.insert(name.to_string(), data);
            }

            /// 获取关系对应的外键值
            ///
            /// 从 `self.data_map()` 读取指定外键字段的值并转为字符串。
            /// 空字符串表示无外键关联（`BelongsTo` 关系会跳过查询）。
            ///
            /// # 参数
            ///
            /// - `fk_name`: 外键字段名（如 "customer_id", "contract_id", "dept_id"）
            ///
            /// # 返回
            ///
            /// 外键值的字符串表示，字段不存在或为 null 时返回空字符串
            fn get_relation_fk_value(&self, fk_name: &str) -> String {
                use sz_rust_core::model::Accessor;
                self.data_map()
                    .get(fk_name)
                    .map(|v| match v {
                        serde_json::Value::Null => String::new(),
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default()
            }
        }
    };
}

pub(crate) use impl_relation_loader;

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

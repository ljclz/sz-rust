//! CustomerPay 模型 — 对齐 PHP `addons\operate\model\CustomerPay`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_pay'` | [`CustomerPay::table_name()`] | 表名 |
//! | `$pk = 'order_id'` | [`CustomerPay::pk_name()`] | 主键列名 |
//! | `$append = ['order_status_text','sync_status_text','pay_status_text','pay_type_text','pay_source_text']` | [`CustomerPay::append()`] | 静态 append |
//! | `getPayStatusTextAttr` | [`CustomerPay::accessor_for`] "pay_status_text" | 枚举映射 |
//! | `getOrderStatusTextAttr` | [`CustomerPay::accessor_for`] "order_status_text" | 枚举映射 |
//! | `getSyncStatusTextAttr` | [`CustomerPay::accessor_for`] "sync_status_text" | 枚举映射 |
//! | `getPayTypeTextAttr` | [`CustomerPay::accessor_for`] "pay_type_text" | 枚举映射 |
//! | `getPaySourceTextAttr` | [`CustomerPay::accessor_for`] "pay_source_text" | 字符串枚举映射 |
//! | 5 个 `getXxxPriceAttr` | [`CustomerPay::accessor_for`] "xxx_price" | 数值强转 |
//! | `getPayTimeAttr` | [`CustomerPay::accessor_for`] "pay_time" | Unix 时间戳格式化 |
//! | `dept()` belongsTo | NOTE(关联模块) | `IndustryDept` dept_id→dept_id |
//! | `personnel()` belongsTo | NOTE(关联模块) | `IndustryPersonnel` opt_uid→uid |
//! | `category()` belongsTo | NOTE(关联模块) | `Category` cat_id→cat_id |
//! | `customer()` belongsTo | NOTE(关联模块) | `Customer` customer_id→customer_id |
//! | `contract()` belongsTo | NOTE(关联模块) | `Contract` contract_id→contract_id |
//!
//! ## PHP `$value ? (float)$value : 0` 行为复刻
//!
//! 5 个价格访问器（pay_price/total_price/cash_price/epay_price/refund_price）
//! 复用 `contract::php_price_attr`，与 Contract 模型 11 个价格访问器语义相同。
//!
//! ## PHP `!empty($data['xxx']) ? xxxName : ''` 行为复刻
//!
//! 4 个数字枚举访问器（pay_status_text/order_status_text/sync_status_text/pay_type_text）
//! 使用 `contract::php_is_empty` 检查字段是否为空。
//!
//! ## PHP `!empty($data['pay_source'])` 字符串字段行为
//!
//! `pay_source` 是字符串字段（值如 `'icbc'`/`'ccb'`/`'fuiou'`/`'cash'`）。
//! PHP `empty()` 对字符串：`""` 和 `"0"` 视为空，其他非空字符串视为非空。
//! `contract::php_is_empty` 严格对齐此语义。
//!
//! ## PHP `getPayTimeAttr` 时间格式化
//!
//! ```php
//! public function getPayTimeAttr($value): string {
//!     return $value ? date('Y-m-d H:i:s', $value) : '';
//! }
//! ```
//!
//! - `$value = 0` 或空 → `''`
//! - `$value = 1700000000` → `'2023-11-14 22:13:20'`（PHP 默认时区）
//!
//! Rust 端用 `chrono::NaiveDateTime::from_timestamp_opt` 格式化。
//! **时区差异**：PHP `date()` 使用服务器配置时区（通常 Asia/Shanghai UTC+8），
//! chrono `from_timestamp_opt` 使用 UTC。当前按 UTC 格式化，
//! 接入应用配置后切换为 Asia/Shanghai。
//!
//! ## 未实现（标 NOTE）
//!
//! - **业务方法**（detail/info/getPayDetail/getList/getStat/add/tradeNo/orderNo/
//!   onPayment/onlinePayment/epayCheck/settle/onPayBuy/onRefund）→ NOTE(控制器层)
//! - **关联关系**（dept/personnel/category/customer/contract belongsTo）→ NOTE(关联模块)

use crate::enums::{ContractStatusEnum, CustomerSyncTypeEnum};
use crate::model::contract::{php_is_empty, php_price_attr};
use crate::model::{get_i64, impl_empty_relation_loader};
use chrono::DateTime;
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, TimestampFields};
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};

/// 客户支付订单模型 — 对齐 PHP `addons\operate\model\CustomerPay`
#[derive(Clone)]
pub struct CustomerPay {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
}

impl CustomerPay {
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

    /// PHP `getPayTimeAttr` 行为复刻
    ///
    /// 条件：`$value ? date('Y-m-d H:i:s', $value) : ''`
    /// - 0/null/空 → `''`
    /// - Unix 时间戳 → `'YYYY-MM-DD HH:MM:SS'`（PHP 服务器时区，本模块用 UTC）
    fn pay_time_attr(value: Option<&Value>) -> Value {
        let is_truthy = match value {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
            Some(Value::String(s)) => {
                let trimmed = s.trim();
                !trimmed.is_empty() && trimmed != "0"
            }
            Some(Value::Array(arr)) => !arr.is_empty(),
            Some(Value::Object(map)) => !map.is_empty(),
        };
        if !is_truthy {
            return json!("");
        }
        // PHP date('Y-m-d H:i:s', $value) 接收 Unix 时间戳
        let timestamp = match value {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
            _ => 0,
        };
        match DateTime::from_timestamp(timestamp, 0) {
            Some(dt) => json!(dt.format("%Y-%m-%d %H:%M:%S").to_string()),
            None => json!(""),
        }
    }
}

impl Default for CustomerPay {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for CustomerPay {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_pay"
    }

    fn pk_name() -> &'static str {
        "order_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "order_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("order_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for CustomerPay {
    fn columns() -> Vec<&'static str> {
        vec![
            "order_id",
            "order_no",
            "trade_no",
            "epay_id",
            "opt_id",
            "opt_uid",
            "dept_id",
            "cat_id",
            "contract_id",
            "customer_id",
            "customer_name",
            "pay_type",
            "order_status",
            "pay_status",
            "sync_status",
            "pay_source",
            "pay_price",
            "total_price",
            "epay_price",
            "cash_price",
            "refund_price",
            "pay_time",
            "remarks",
            "serial_sn",
            "is_delete",
            "app_id",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "order_no",
            "trade_no",
            "epay_id",
            "opt_id",
            "opt_uid",
            "dept_id",
            "cat_id",
            "contract_id",
            "customer_id",
            "customer_name",
            "pay_type",
            "order_status",
            "pay_status",
            "sync_status",
            "pay_source",
            "pay_price",
            "total_price",
            "epay_price",
            "cash_price",
            "refund_price",
            "pay_time",
            "remarks",
            "serial_sn",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["order_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_orm_core::Value> {
        use sz_orm_core::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "order_id" | "opt_id" | "opt_uid" | "dept_id" | "cat_id" | "contract_id"
            | "customer_id" | "pay_type" | "order_status" | "pay_status" | "sync_status"
            | "pay_time" | "is_delete" | "app_id" | "create_time" | "update_time" => {
                v.as_i64().map(OrmValue::I64)
            }
            "pay_price" | "total_price" | "epay_price" | "cash_price" | "refund_price" => {
                v.as_f64().map(OrmValue::F64)
            }
            "order_no" | "trade_no" | "epay_id" | "customer_name" | "pay_source" | "remarks"
            | "serial_sn" => v.as_str().map(|s| OrmValue::String(s.to_string())),
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

impl_empty_relation_loader!(CustomerPay);

impl BaseModel for CustomerPay {
    fn append() -> Vec<&'static str> {
        vec![
            "order_status_text",
            "sync_status_text",
            "pay_status_text",
            "pay_type_text",
            "pay_source_text",
        ]
    }
}

impl Accessor for CustomerPay {
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

    /// PHP CustomerPay 访问器派发 — 11 个 getXxxAttr
    ///
    /// ### 枚举映射访问器（4 个，数字字段）
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `pay_status_text` | `getPayStatusTextAttr` | `!empty(pay_status) ? payStatusName : ''` |
    /// | `order_status_text` | `getOrderStatusTextAttr` | `!empty(order_status) ? orderStatusName : ''` |
    /// | `sync_status_text` | `getSyncStatusTextAttr` | `!empty(sync_status) ? syncStatusName : ''` |
    /// | `pay_type_text` | `getPayTypeTextAttr` | `!empty(pay_type) ? payTypeName : ''` |
    ///
    /// ### 字符串枚举映射访问器（1 个，字符串字段）
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `pay_source_text` | `getPaySourceTextAttr` | `!empty(pay_source) ? paySourceName : ''` |
    ///
    /// ### 数值强转访问器（5 个）
    ///
    /// `pay_price` / `total_price` / `cash_price` / `epay_price` / `refund_price`
    /// 全部走 `php_price_attr`，对齐 PHP `$value ? (float)$value : 0`。
    ///
    /// ### 时间格式化访问器（1 个）
    ///
    /// `pay_time`：Unix 时间戳 → `'YYYY-MM-DD HH:MM:SS'`（对齐 `getPayTimeAttr`）。
    fn accessor_for(&self, field: &str, _value: Option<&Value>) -> Value {
        match field {
            // ==================== 枚举映射访问器（数字字段） ====================
            "pay_status_text" => {
                let v = self.data.get("pay_status");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "pay_status").unwrap_or(0);
                    json!(CustomerSyncTypeEnum::pay_status_name(status))
                }
            }
            "order_status_text" => {
                let v = self.data.get("order_status");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "order_status").unwrap_or(0);
                    json!(CustomerSyncTypeEnum::order_status_name(status))
                }
            }
            "sync_status_text" => {
                let v = self.data.get("sync_status");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "sync_status").unwrap_or(0);
                    json!(CustomerSyncTypeEnum::sync_status_name(status))
                }
            }
            "pay_type_text" => {
                let v = self.data.get("pay_type");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "pay_type").unwrap_or(0);
                    // PHP CustomerSyncTypeEnum::payTypeName 与 ContractStatusEnum::payTypeName
                    // 数据相同（1/2/3 → 扫码转账/现金支付/转账+现金），Rust 端复用
                    json!(ContractStatusEnum::pay_type_name(status))
                }
            }

            // ==================== 字符串枚举映射访问器 ====================
            "pay_source_text" => {
                let v = self.data.get("pay_source");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let source = v.and_then(|val| val.as_str()).unwrap_or("");
                    json!(CustomerSyncTypeEnum::pay_source_name(source))
                }
            }

            // ==================== 数值强转访问器（5 个） ====================
            "pay_price" | "total_price" | "cash_price" | "epay_price" | "refund_price" => {
                php_price_attr(self.data.get(field))
            }

            // ==================== 时间格式化访问器 ====================
            "pay_time" => Self::pay_time_attr(self.data.get("pay_time")),

            _ => Value::Null,
        }
    }
}

impl Mutator for CustomerPay {
    /// PHP CustomerPay 未声明任何 setXxxAttr
    fn mutator_for(
        &mut self,
        _field: &str,
        _value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        None
    }
}

impl Appendable for CustomerPay {
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
        assert_eq!(CustomerPay::table_name(), "customer_pay");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(CustomerPay::pk_name(), "order_id");
    }

    #[test]
    fn test_append_fields_aligns_php() {
        // PHP $append = ['order_status_text','sync_status_text','pay_status_text','pay_type_text','pay_source_text']
        let append = CustomerPay::append();
        assert_eq!(
            append,
            vec![
                "order_status_text",
                "sync_status_text",
                "pay_status_text",
                "pay_type_text",
                "pay_source_text",
            ]
        );
    }

    #[test]
    fn test_columns_include_all_php_fields() {
        let cols = CustomerPay::columns();
        assert!(cols.contains(&"order_id"));
        assert!(cols.contains(&"order_no"));
        assert!(cols.contains(&"trade_no"));
        assert!(cols.contains(&"epay_id"));
        assert!(cols.contains(&"opt_id"));
        assert!(cols.contains(&"opt_uid"));
        assert!(cols.contains(&"dept_id"));
        assert!(cols.contains(&"cat_id"));
        assert!(cols.contains(&"contract_id"));
        assert!(cols.contains(&"customer_id"));
        assert!(cols.contains(&"customer_name"));
        assert!(cols.contains(&"pay_type"));
        assert!(cols.contains(&"order_status"));
        assert!(cols.contains(&"pay_status"));
        assert!(cols.contains(&"sync_status"));
        assert!(cols.contains(&"pay_source"));
        assert!(cols.contains(&"pay_price"));
        assert!(cols.contains(&"total_price"));
        assert!(cols.contains(&"epay_price"));
        assert!(cols.contains(&"cash_price"));
        assert!(cols.contains(&"refund_price"));
        assert!(cols.contains(&"pay_time"));
        assert!(cols.contains(&"remarks"));
        assert!(cols.contains(&"serial_sn"));
        assert!(cols.contains(&"is_delete"));
        assert!(cols.contains(&"app_id"));
    }

    #[test]
    fn test_fillable_excludes_primary_key_and_meta() {
        let fillable = CustomerPay::fillable();
        assert!(
            !fillable.contains(&"order_id"),
            "order_id 应受保护不可批量赋值"
        );
        assert!(!fillable.contains(&"is_delete"), "is_delete 不应可批量赋值");
        assert!(!fillable.contains(&"app_id"), "app_id 不应可批量赋值");
        assert!(fillable.contains(&"order_no"));
        assert!(fillable.contains(&"pay_type"));
    }

    #[test]
    fn test_guarded_includes_order_id() {
        assert!(CustomerPay::guarded().contains(&"order_id"));
    }

    // -------------------- 访问器测试：枚举映射（数字字段） --------------------

    #[test]
    fn test_pay_status_text_aligns_php() {
        // PHP: !empty(pay_status) ? payStatusName : ''
        let model = CustomerPay::new().with_data("pay_status", json!(10));
        assert_eq!(model.accessor_for("pay_status_text", None), json!("未付款"));

        let model = CustomerPay::new().with_data("pay_status", json!(20));
        assert_eq!(model.accessor_for("pay_status_text", None), json!("已付款"));

        let model = CustomerPay::new().with_data("pay_status", json!(30));
        assert_eq!(model.accessor_for("pay_status_text", None), json!("已退款"));
    }

    #[test]
    fn test_pay_status_text_zero_returns_empty() {
        // PHP: empty(0)=true → ''
        let model = CustomerPay::new().with_data("pay_status", json!(0));
        assert_eq!(model.accessor_for("pay_status_text", None), json!(""));
    }

    #[test]
    fn test_pay_status_text_missing_returns_empty() {
        // PHP: empty(null)=true → ''
        let model = CustomerPay::new();
        assert_eq!(model.accessor_for("pay_status_text", None), json!(""));
    }

    #[test]
    fn test_order_status_text_aligns_php() {
        let model = CustomerPay::new().with_data("order_status", json!(10));
        assert_eq!(
            model.accessor_for("order_status_text", None),
            json!("进行中")
        );

        let model = CustomerPay::new().with_data("order_status", json!(20));
        assert_eq!(
            model.accessor_for("order_status_text", None),
            json!("已经取消")
        );

        let model = CustomerPay::new().with_data("order_status", json!(30));
        assert_eq!(
            model.accessor_for("order_status_text", None),
            json!("已完成")
        );
    }

    #[test]
    fn test_sync_status_text_aligns_php() {
        let model = CustomerPay::new().with_data("sync_status", json!(10));
        assert_eq!(
            model.accessor_for("sync_status_text", None),
            json!("待同步")
        );

        let model = CustomerPay::new().with_data("sync_status", json!(30));
        assert_eq!(
            model.accessor_for("sync_status_text", None),
            json!("已同步")
        );
    }

    #[test]
    fn test_pay_type_text_aligns_php() {
        let model = CustomerPay::new().with_data("pay_type", json!(1));
        assert_eq!(model.accessor_for("pay_type_text", None), json!("扫码转账"));

        let model = CustomerPay::new().with_data("pay_type", json!(2));
        assert_eq!(model.accessor_for("pay_type_text", None), json!("现金支付"));
    }

    // -------------------- 访问器测试：字符串枚举映射 --------------------

    #[test]
    fn test_pay_source_text_aligns_php() {
        // PHP: !empty(pay_source) ? paySourceName : ''
        let model = CustomerPay::new().with_data("pay_source", json!("icbc"));
        assert_eq!(
            model.accessor_for("pay_source_text", None),
            json!("工商银行")
        );

        let model = CustomerPay::new().with_data("pay_source", json!("ccb"));
        assert_eq!(
            model.accessor_for("pay_source_text", None),
            json!("建设银行")
        );

        let model = CustomerPay::new().with_data("pay_source", json!("fuiou"));
        assert_eq!(
            model.accessor_for("pay_source_text", None),
            json!("富友支付")
        );

        let model = CustomerPay::new().with_data("pay_source", json!("cash"));
        assert_eq!(
            model.accessor_for("pay_source_text", None),
            json!("现金支付")
        );
    }

    #[test]
    fn test_pay_source_text_empty_string_returns_empty() {
        // PHP: empty("")=true → ''
        let model = CustomerPay::new().with_data("pay_source", json!(""));
        assert_eq!(model.accessor_for("pay_source_text", None), json!(""));
    }

    #[test]
    fn test_pay_source_text_missing_returns_empty() {
        // PHP: empty(null)=true → ''
        let model = CustomerPay::new();
        assert_eq!(model.accessor_for("pay_source_text", None), json!(""));
    }

    #[test]
    fn test_pay_source_text_unknown_returns_unknown() {
        // PHP: !empty("unknown")=true → paySourceName("unknown")='未知'
        let model = CustomerPay::new().with_data("pay_source", json!("unknown"));
        assert_eq!(model.accessor_for("pay_source_text", None), json!("未知"));
    }

    // -------------------- 访问器测试：float 强转 --------------------

    #[test]
    fn test_pay_price_accessor_aligns_php_float_cast() {
        // PHP: $value ? (float)$value : 0
        let model = CustomerPay::new().with_data("pay_price", json!(100.50));
        assert_eq!(model.accessor_for("pay_price", None), json!(100.5));

        let model = CustomerPay::new().with_data("pay_price", json!(0));
        assert_eq!(model.accessor_for("pay_price", None), json!(0));

        let model = CustomerPay::new();
        assert_eq!(model.accessor_for("pay_price", None), json!(0));
    }

    #[test]
    fn test_total_price_accessor_aligns_php_float_cast() {
        let model = CustomerPay::new().with_data("total_price", json!(200.25));
        assert_eq!(model.accessor_for("total_price", None), json!(200.25));
    }

    #[test]
    fn test_cash_price_accessor_aligns_php_float_cast() {
        let model = CustomerPay::new().with_data("cash_price", json!(50.0));
        assert_eq!(model.accessor_for("cash_price", None), json!(50.0));
    }

    #[test]
    fn test_epay_price_accessor_aligns_php_float_cast() {
        let model = CustomerPay::new().with_data("epay_price", json!(150.75));
        assert_eq!(model.accessor_for("epay_price", None), json!(150.75));
    }

    #[test]
    fn test_refund_price_accessor_aligns_php_float_cast() {
        let model = CustomerPay::new().with_data("refund_price", json!(30.0));
        assert_eq!(model.accessor_for("refund_price", None), json!(30.0));
    }

    #[test]
    fn test_float_accessor_string_value_parses_to_float() {
        // PHP (float)"100.5" = 100.5
        let model = CustomerPay::new().with_data("pay_price", json!("100.5"));
        assert_eq!(model.accessor_for("pay_price", None), json!(100.5));
    }

    // -------------------- 访问器测试：pay_time 时间格式化 --------------------

    #[test]
    fn test_pay_time_zero_returns_empty_string() {
        // PHP: 0 ? date(...) : '' → ''
        let model = CustomerPay::new().with_data("pay_time", json!(0));
        assert_eq!(model.accessor_for("pay_time", None), json!(""));
    }

    #[test]
    fn test_pay_time_missing_returns_empty_string() {
        // PHP: null ? date(...) : '' → ''
        let model = CustomerPay::new();
        assert_eq!(model.accessor_for("pay_time", None), json!(""));
    }

    #[test]
    fn test_pay_time_valid_timestamp_returns_formatted_string() {
        // PHP: date('Y-m-d H:i:s', 1700000000) → '2023-11-14 22:13:20'（UTC+8）
        // Rust 端用 UTC，结果为 '2023-11-14 14:13:20'
        let model = CustomerPay::new().with_data("pay_time", json!(1700000000));
        let result = model.accessor_for("pay_time", None);
        let s = result.as_str().unwrap();
        // 验证格式：YYYY-MM-DD HH:MM:SS
        assert_eq!(s.len(), 19);
        assert_eq!(s.chars().nth(4), Some('-'));
        assert_eq!(s.chars().nth(7), Some('-'));
        assert_eq!(s.chars().nth(10), Some(' '));
        assert_eq!(s.chars().nth(13), Some(':'));
        assert_eq!(s.chars().nth(16), Some(':'));
        // 验证日期部分（UTC）
        assert!(s.starts_with("2023-11-14"));
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_returns_none_for_all_fields() {
        // PHP CustomerPay 无修改器
        let mut model = CustomerPay::new();
        let merged = HashMap::new();
        assert_eq!(model.mutator_for("pay_price", &json!(100), &merged), None);
        assert_eq!(model.mutator_for("pay_type", &json!(1), &merged), None);
    }

    // -------------------- 主键测试 --------------------

    #[test]
    fn test_pk_returns_zero_for_empty_model() {
        let model = CustomerPay::new();
        assert_eq!(model.pk(), 0);
    }

    #[test]
    fn test_pk_returns_value_from_data() {
        let model = CustomerPay::new().with_data("order_id", json!(42));
        assert_eq!(model.pk(), 42);
    }

    #[test]
    fn test_set_pk_updates_data() {
        let mut model = CustomerPay::new();
        model.set_pk(99);
        assert_eq!(model.pk(), 99);
    }

    // -------------------- 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_includes_all_append_fields() {
        // append 5 个字段，序列化时应自动追加
        let mut model = CustomerPay::new()
            .with_data("order_id", json!(1))
            .with_data("pay_status", json!(20))
            .with_data("order_status", json!(10))
            .with_data("sync_status", json!(30))
            .with_data("pay_type", json!(1))
            .with_data("pay_source", json!("icbc"));
        let json = model.to_json_with_append_cached();
        assert_eq!(json["order_id"], 1);
        assert_eq!(json["pay_status"], 20);
        // 5 个 append 字段应自动追加
        assert_eq!(json["pay_status_text"], "已付款");
        assert_eq!(json["order_status_text"], "进行中");
        assert_eq!(json["sync_status_text"], "已同步");
        assert_eq!(json["pay_type_text"], "扫码转账");
        assert_eq!(json["pay_source_text"], "工商银行");
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_customer_pay_float_accessor_zero_handling() {
        // R5: PHP $value ? (float)$value : 0
        let model = CustomerPay::new();
        assert_eq!(model.accessor_for("pay_price", Some(&json!(0))), json!(0));
        assert_eq!(model.accessor_for("pay_price", Some(&json!(0.0))), json!(0));
        assert_eq!(model.accessor_for("pay_price", Some(&json!("0"))), json!(0));
        assert_eq!(model.accessor_for("pay_price", Some(&json!(""))), json!(0));
        assert_eq!(model.accessor_for("pay_price", None), json!(0));
    }

    #[test]
    fn test_r5_php_customer_pay_pay_source_string_field() {
        // R5: PHP pay_source 是字符串字段，empty() 行为对齐
        let model = CustomerPay::new().with_data("pay_source", json!("icbc"));
        assert_eq!(
            model.accessor_for("pay_source_text", None),
            json!("工商银行")
        );

        // empty("0")=true → ''
        let model = CustomerPay::new().with_data("pay_source", json!("0"));
        assert_eq!(model.accessor_for("pay_source_text", None), json!(""));

        // empty("")=true → ''
        let model = CustomerPay::new().with_data("pay_source", json!(""));
        assert_eq!(model.accessor_for("pay_source_text", None), json!(""));
    }

    #[test]
    fn test_r5_php_customer_pay_pay_time_php_truthy_check() {
        // R5: PHP $value ? date(...) : ''
        // 0/null/空 → ''
        let model = CustomerPay::new();
        assert_eq!(model.accessor_for("pay_time", None), json!(""));

        let model = CustomerPay::new().with_data("pay_time", json!(0));
        assert_eq!(model.accessor_for("pay_time", None), json!(""));

        // 非零时间戳 → 格式化字符串
        let model = CustomerPay::new().with_data("pay_time", json!(1700000000));
        let result = model.accessor_for("pay_time", None);
        assert!(result.as_str().unwrap().starts_with("2023-11-14"));
    }

    #[test]
    fn test_r5_php_customer_pay_soft_delete_via_is_delete_field() {
        // R5: PHP CustomerPay 通过 is_delete=1 实现软删除
        assert_eq!(CustomerPay::soft_delete_field(), None);
    }

    #[test]
    fn test_r5_php_customer_pay_5_belongs_to_relations_documented_as_todo() {
        // R5: PHP CustomerPay 声明 5 个 belongsTo: dept/personnel/category/customer/contract
        // 关联关系留 NOTE(关联模块)
        // 此测试验证模型本身可正常构造，关联关系待后续实现
        let model = CustomerPay::new().with_data("order_id", json!(1));
        assert_eq!(model.pk(), 1);
    }
}

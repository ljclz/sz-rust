//! Contract 模型 — 对齐 PHP `addons\operate\model\Contract`
//!
//! ## PHP 对齐
//!
//! | PHP 属性/方法 | Rust 等价 | 说明 |
//! |---------------|----------|------|
//! | `$name = 'customer_contract'` | [`Contract::table_name()`] | 表名 |
//! | `$pk = 'contract_id'` | [`Contract::pk_name()`] | 主键列名 |
//! | `$append = ['signing_text','contract_text','pay_status_text','pay_type_text','remaining_day','logs']` | [`Contract::append()`] | 静态 append |
//! | `getSigningTextAttr` | [`Contract::accessor_for`] "signing_text" | 枚举映射 |
//! | `getContractTextAttr` | [`Contract::accessor_for`] "contract_text" | 枚举映射 |
//! | `getPayStatusTextAttr` | [`Contract::accessor_for`] "pay_status_text" | 枚举映射 |
//! | `getPayTypeTextAttr` | [`Contract::accessor_for`] "pay_type_text" | 枚举映射 |
//! | `getRemainingDayAttr` | [`Contract::accessor_for`] "remaining_day" | 日期差 |
//! | `getLogsAttr` | [`Contract::accessor_for`] "logs" | NOTE(Repository 层) |
//! | 11 个 `getXxxPriceAttr` | [`Contract::accessor_for`] "xxx_price" | 数值强转 |
//! | `getPayDetailAttr` | [`Contract::accessor_for`] "pay_detail" | 序列化解码 |
//! | `getFilesAttr` | [`Contract::accessor_for`] "files" | JSON 解码 |
//! | `setPayDetailAttr` | [`Contract::mutator_for`] "pay_detail" | 序列化 |
//! | `setFilesAttr` | [`Contract::mutator_for`] "files" | JSON 编码 |
//!
//! ## PHP `$value ? (float)$value : 0` 行为复刻
//!
//! PHP 真值判断（`?:` 运算符）对 11 个价格字段：
//! - `null` / `0` / `0.0` / `"0"` / `""` → 视为 false → 返回 int `0`
//! - 其他非空数值字符串 → 视为 true → 返回 `(float)$value`
//! - 非数值字符串（如 `"abc"`）→ 视为 true（PHP 字符串真值规则）→ 返回 `(float)"abc" = 0.0`
//!
//! Rust 端用 `php_price_attr` 严格对齐：
//! - 数值类型：`0` / `0.0` 返回 `json!(0)`，非零返回 `json!(f64)`
//! - 字符串：`""` / `"0"` 返回 `json!(0)`，其他解析为 f64（解析失败返回 `0.0`）
//!
//! ## PHP 序列化策略
//!
//! PHP `pay_detail` 字段使用 `iserializer` / `unserialize`（PHP serialize 格式）。
//! Rust 端统一改用 JSON（`serde_json`）。
//! **不兼容说明**：若需与 PHP 数据库现存量兼容，需在数据迁移层做格式转换。

use crate::enums::ContractStatusEnum;
use crate::model::{get_i64, impl_relation_loader};
use chrono::NaiveDate;
use serde_json::{json, Value};
use std::collections::HashMap;
use sz_rust_core::model::{Accessor, AppendState, Appendable, BaseModel, Mutator, MutatorResult};
use sz_rust_core::orm::{Model, ModelExt, TimestampFields};

/// PHP `$value ? (float)$value : 0` 行为复刻
///
/// 11 个价格访问器共用此函数。详见模块文档「PHP `$value ? (float)$value : 0` 行为复刻」。
pub(crate) fn php_price_attr(value: Option<&Value>) -> Value {
    let is_truthy = match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            // PHP (bool)"" = false, (bool)"0" = false, 其他非空字符串 = true
            !trimmed.is_empty() && trimmed != "0"
        }
        Some(Value::Array(arr)) => !arr.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
    };
    if is_truthy {
        // PHP (float)$value：字符串解析失败返回 0.0，数值取 f64
        // 注意：serde_json::Value::as_f64() 只对 Number 有效，字符串需手动 parse
        let f = match value {
            Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
            Some(v) => v.as_f64().unwrap_or(0.0),
            None => 0.0,
        };
        json!(f)
    } else {
        // PHP int 0（JSON 序列化为整数 0，非 0.0）
        json!(0)
    }
}

/// PHP `empty($value)` 行为复刻（针对字符串/数值/数组）
///
/// PHP `empty()` 返回 true 的情况：
/// - `null` / `false` / `0` / `0.0` / `""` / `"0"` / `"0.0"` / `[]`
///
/// **注意**：PHP `empty("0.0")` 在 PHP 8+ 返回 false（只有 "0" 是空字符串）。
/// 此函数严格对齐 PHP 8 语义。
pub(crate) fn php_is_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f == 0.0).unwrap_or(true),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            // PHP 8: empty("") = true, empty("0") = true, empty("0.0") = false
            trimmed.is_empty() || trimmed == "0"
        }
        Some(Value::Array(arr)) => arr.is_empty(),
        Some(Value::Object(map)) => map.is_empty(),
    }
}

/// 合同模型 — 对齐 PHP `addons\operate\model\Contract`
#[derive(Clone)]
pub struct Contract {
    /// 数据存储（对齐 PHP `$this->data`）
    data: HashMap<String, Value>,
    /// 访问器缓存（对齐 PHP `$this->get`）
    get_cache: HashMap<String, Value>,
    /// 动态 append 状态（对齐 PHP `$this->append`）
    append_state: AppendState,
    /// 已加载的关联数据（H-1 修复：真实 RelationLoader 存储）
    relations: HashMap<String, sz_rust_core::orm::Value>,
}

impl Contract {
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
}

impl Default for Contract {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Contract {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "customer_contract"
    }

    fn pk_name() -> &'static str {
        "contract_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        get_i64(&self.data, "contract_id").unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.data.insert("contract_id".to_string(), json!(pk));
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }

    fn soft_delete_field() -> Option<&'static str> {
        None
    }
}

impl ModelExt for Contract {
    fn columns() -> Vec<&'static str> {
        vec![
            "contract_id",
            "customer_id",
            "dept_id",
            "cat_id",
            "company_id",
            "contract_name",
            "serial_sn",
            "contract_status",
            "signing_status",
            "pay_status",
            "pay_type",
            "start_day",
            "end_day",
            "contract_price",
            "rent_price",
            "manage_price",
            "sanitation_price",
            "electricity_price",
            "water_price",
            "security_price",
            "paid_price",
            "capacity_price",
            "decoration_price",
            "decoration_deposit",
            "pay_detail",
            "files",
            "remarks",
            "app_id",
            "is_delete",
            "create_time",
            "update_time",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "customer_id",
            "dept_id",
            "cat_id",
            "company_id",
            "contract_name",
            "contract_status",
            "signing_status",
            "pay_status",
            "pay_type",
            "start_day",
            "end_day",
            "contract_price",
            "rent_price",
            "manage_price",
            "sanitation_price",
            "electricity_price",
            "water_price",
            "security_price",
            "paid_price",
            "capacity_price",
            "decoration_price",
            "decoration_deposit",
            "pay_detail",
            "files",
            "remarks",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["contract_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as OrmValue;
        let v = self.data.get(column)?;
        match column {
            "contract_id" | "customer_id" | "dept_id" | "cat_id" | "company_id"
            | "contract_status" | "signing_status" | "pay_status" | "pay_type" | "app_id"
            | "is_delete" | "create_time" | "update_time" => v.as_i64().map(OrmValue::I64),
            "contract_price" | "rent_price" | "manage_price" | "sanitation_price"
            | "electricity_price" | "water_price" | "security_price" | "paid_price"
            | "capacity_price" | "decoration_price" | "decoration_deposit" => {
                v.as_f64().map(OrmValue::F64)
            }
            "contract_name" | "serial_sn" | "start_day" | "end_day" | "pay_detail" | "files"
            | "remarks" => v.as_str().map(|s| OrmValue::String(s.to_string())),
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

impl_relation_loader!(Contract);

impl BaseModel for Contract {
    fn append() -> Vec<&'static str> {
        vec![
            "signing_text",
            "contract_text",
            "pay_status_text",
            "pay_type_text",
            "remaining_day",
            "logs",
        ]
    }

    fn get_appended_value(&self, field: &str) -> Option<Value> {
        let value = self.data.get(field);
        Some(self.accessor_for(field, value))
    }
}

impl Accessor for Contract {
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

    /// 访问器派发 — 对齐 PHP 19 个 `getXxxAttr`
    ///
    /// ### 枚举映射访问器（4 个）
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `signing_text` | `getSigningTextAttr` | `!empty(signing_status) ? signingName : ''` |
    /// | `contract_text` | `getContractTextAttr` | `!empty(contract_status) ? contractStatusName : ''` |
    /// | `pay_status_text` | `getPayStatusTextAttr` | `!empty(pay_status) ? payStatusName : ''` |
    /// | `pay_type_text` | `getPayTypeTextAttr` | `!empty(pay_type) ? payTypeName : ''` |
    ///
    /// ### 数值强转访问器（11 个）
    ///
    /// `contract_price` / `rent_price` / `manage_price` / `sanitation_price` /
    /// `electricity_price` / `water_price` / `security_price` / `paid_price` /
    /// `capacity_price` / `decoration_price` / `decoration_deposit`
    /// 全部走 `php_price_attr`，对齐 PHP `$value ? (float)$value : 0`。
    ///
    /// ### 日期计算访问器（1 个）
    ///
    /// `remaining_day`：基于 `end_day` 与今天的天数差（对齐 `getRemainingDayAttr`）。
    ///
    /// ### 序列化解码访问器（2 个）
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `pay_detail` | `getPayDetailAttr` | `!empty ? unserialize : []`（Rust 用 JSON） |
    /// | `files` | `getFilesAttr` | `empty ? [] : json_decode` |
    ///
    /// ### 虚拟字段访问器（1 个）
    ///
    /// `logs`：PHP 调 `ContractLog::getLogs($data['contract_id'])`。
    /// 当前无数据库连接，返回空数组（对齐 PHP `contract_id` 为空时 `getLogs(0)` 行为）。
    /// NOTE(Repository 层): 完整实现 ContractLog::getLogs。
    fn accessor_for(&self, field: &str, _value: Option<&Value>) -> Value {
        match field {
            // ==================== 枚举映射访问器 ====================
            "signing_text" => {
                let v = self.data.get("signing_status");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "signing_status").unwrap_or(0);
                    json!(ContractStatusEnum::signing_name(status))
                }
            }
            "contract_text" => {
                let v = self.data.get("contract_status");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "contract_status").unwrap_or(0);
                    json!(ContractStatusEnum::contract_status_name(status))
                }
            }
            "pay_status_text" => {
                let v = self.data.get("pay_status");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "pay_status").unwrap_or(0);
                    json!(ContractStatusEnum::pay_status_name(status))
                }
            }
            "pay_type_text" => {
                let v = self.data.get("pay_type");
                if php_is_empty(v) {
                    json!("")
                } else {
                    let status = get_i64(&self.data, "pay_type").unwrap_or(0);
                    json!(ContractStatusEnum::pay_type_name(status))
                }
            }

            // ==================== 数值强转访问器（11 个） ====================
            "contract_price" | "rent_price" | "manage_price" | "sanitation_price"
            | "electricity_price" | "water_price" | "security_price" | "paid_price"
            | "capacity_price" | "decoration_price" | "decoration_deposit" => {
                php_price_attr(self.data.get(field))
            }

            // ==================== 日期计算访问器 ====================
            // PHP: empty($data['end_day']) ? 0 : (int)floor((strtotime(end_day) - strtotime(today)) / 86400)
            //
            // **时区策略**：PHP `date('Y-m-d')` 取 `date.timezone` 配置的时区，
            // Rust `chrono::Local::now()` 取系统 TZ 环境变量。
            // 部署时需保证 PHP `date.timezone` 与系统 TZ 一致（如都设为 `Asia/Shanghai`）。
            //
            // **解析失败行为差异**：PHP `strtotime("invalid")` 返回 `false`，
            // 后续 `(false - ts) / 86400` 会得到奇怪的负数；
            // Rust 端解析失败统一返回 0（防御性，业务中 end_day 不会是非法字符串）。
            "remaining_day" => {
                let end_day = self
                    .data
                    .get("end_day")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if end_day.is_empty() {
                    return json!(0);
                }
                // PHP strtotime 支持多种格式，Rust 端先尝试 Y-m-d，再尝试 Y-m-d H:M:S
                let parsed = NaiveDate::parse_from_str(end_day, "%Y-%m-%d").or_else(|_| {
                    // 截取前 10 字符尝试日期部分
                    let prefix = end_day.get(..10).unwrap_or(end_day);
                    NaiveDate::parse_from_str(prefix, "%Y-%m-%d")
                });
                let end_date = match parsed {
                    Ok(d) => d,
                    Err(_) => return json!(0),
                };
                let today = chrono::Local::now().date_naive();
                let diff = (end_date - today).num_days();
                json!(diff)
            }

            // ==================== 序列化解码访问器 ====================
            // PHP getPayDetailAttr: !empty($value) ? unserialize($value) : []
            // Rust 端用 JSON（不兼容 PHP serialize 格式，需数据迁移）
            "pay_detail" => {
                let v = self.data.get("pay_detail");
                if php_is_empty(v) {
                    json!([])
                } else {
                    let s = v.and_then(|val| val.as_str()).unwrap_or("");
                    serde_json::from_str(s).unwrap_or(json!([]))
                }
            }
            // PHP getFilesAttr: empty($value) ? [] : json_decode($value, true)
            "files" => {
                let v = self.data.get("files");
                if php_is_empty(v) {
                    json!([])
                } else {
                    let s = v.and_then(|val| val.as_str()).unwrap_or("");
                    serde_json::from_str(s).unwrap_or(json!([]))
                }
            }

            // ==================== 虚拟字段访问器 ====================
            // PHP getLogsAttr: ContractLog::getLogs($data['contract_id'])
            // H-1 修复：从已加载的 "logs" 关联数据中提取日志记录
            // 若关联未加载（Repository 层未调用 set_relation_data），返回空数组
            "logs" => {
                let logs: Vec<Value> = self
                    .relations
                    .get("logs")
                    .and_then(|v| match v {
                        sz_rust_core::orm::Value::Array(items) => Some(
                            items
                                .iter()
                                .filter_map(|item| {
                                    match item {
                                        sz_rust_core::orm::Value::Object(map) => {
                                            // 将 ORM Value HashMap 转为 serde_json Value Object
                                            let mut json_map = serde_json::Map::new();
                                            for (k, val) in map {
                                                let json_val = match val {
                                                    sz_rust_core::orm::Value::I64(i) => json!(i),
                                                    sz_rust_core::orm::Value::I32(i) => json!(i),
                                                    sz_rust_core::orm::Value::F64(f) => json!(f),
                                                    sz_rust_core::orm::Value::String(s) => json!(s),
                                                    sz_rust_core::orm::Value::Bool(b) => json!(b),
                                                    sz_rust_core::orm::Value::Null => json!(null),
                                                    other => serde_json::to_value(other)
                                                        .unwrap_or(json!(null)),
                                                };
                                                json_map.insert(k.clone(), json_val);
                                            }
                                            Some(Value::Object(json_map))
                                        }
                                        _ => None,
                                    }
                                })
                                .collect(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_default();
                Value::Array(logs)
            }

            _ => Value::Null,
        }
    }
}

impl Mutator for Contract {
    /// 修改器派发 — 对齐 PHP `setPayDetailAttr` / `setFilesAttr`
    ///
    /// | field | PHP 方法 | 行为 |
    /// |-------|---------|------|
    /// | `pay_detail` | `setPayDetailAttr` | `!empty ? iserializer : ''`（Rust 用 JSON） |
    /// | `files` | `setFilesAttr` | `empty ? '[]' : json_encode` |
    fn mutator_for(
        &mut self,
        field: &str,
        value: &Value,
        _merged_data: &HashMap<String, Value>,
    ) -> Option<MutatorResult> {
        match field {
            // PHP setPayDetailAttr: !empty($value) ? iserializer($value) : ''
            // Rust 端用 serde_json::to_string（不兼容 PHP iserializer 格式）
            "pay_detail" => {
                if php_is_empty(Some(value)) {
                    Some(MutatorResult::Value(Value::String(String::new())))
                } else {
                    let serialized = serde_json::to_string(value).unwrap_or_default();
                    Some(MutatorResult::Value(Value::String(serialized)))
                }
            }
            // PHP setFilesAttr: empty($value) ? '[]' : json_encode($value)
            "files" => {
                if php_is_empty(Some(value)) {
                    Some(MutatorResult::Value(Value::String("[]".to_string())))
                } else {
                    let serialized =
                        serde_json::to_string(value).unwrap_or_else(|_| "[]".to_string());
                    Some(MutatorResult::Value(Value::String(serialized)))
                }
            }
            _ => None,
        }
    }
}

impl Appendable for Contract {
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
        assert_eq!(Contract::table_name(), "customer_contract");
    }

    #[test]
    fn test_pk_name_aligns_php() {
        assert_eq!(Contract::pk_name(), "contract_id");
    }

    #[test]
    fn test_append_fields_aligns_php() {
        // PHP $append = ['signing_text','contract_text','pay_status_text','pay_type_text','remaining_day','logs']
        assert_eq!(
            Contract::append(),
            vec![
                "signing_text",
                "contract_text",
                "pay_status_text",
                "pay_type_text",
                "remaining_day",
                "logs"
            ]
        );
    }

    // -------------------- 枚举映射访问器测试 --------------------

    #[test]
    fn test_accessor_signing_text_zero_returns_empty() {
        // PHP !empty(0)=false → ''
        let model = Contract::new().with_data("signing_status", json!(0));
        assert_eq!(model.accessor_for("signing_text", None), json!(""));
    }

    #[test]
    fn test_accessor_signing_text_one_returns_pending() {
        let model = Contract::new().with_data("signing_status", json!(1));
        assert_eq!(model.accessor_for("signing_text", None), json!("待签约"));
    }

    #[test]
    fn test_accessor_signing_text_two_returns_signed() {
        let model = Contract::new().with_data("signing_status", json!(2));
        assert_eq!(model.accessor_for("signing_text", None), json!("已签约"));
    }

    #[test]
    fn test_accessor_signing_text_three_returns_terminated() {
        let model = Contract::new().with_data("signing_status", json!(3));
        assert_eq!(model.accessor_for("signing_text", None), json!("已解约"));
    }

    #[test]
    fn test_accessor_signing_text_unknown_returns_unknown() {
        let model = Contract::new().with_data("signing_status", json!(99));
        assert_eq!(model.accessor_for("signing_text", None), json!("未知"));
    }

    #[test]
    fn test_accessor_signing_text_missing_returns_empty() {
        // PHP !empty(null)=false → ''
        let model = Contract::new();
        assert_eq!(model.accessor_for("signing_text", None), json!(""));
    }

    #[test]
    fn test_accessor_contract_text_aligns_php() {
        let model = Contract::new().with_data("contract_status", json!(1));
        assert_eq!(model.accessor_for("contract_text", None), json!("待生效"));
        let model = Contract::new().with_data("contract_status", json!(2));
        assert_eq!(model.accessor_for("contract_text", None), json!("有效期"));
        let model = Contract::new().with_data("contract_status", json!(3));
        assert_eq!(model.accessor_for("contract_text", None), json!("已失效"));
        let model = Contract::new();
        assert_eq!(model.accessor_for("contract_text", None), json!(""));
    }

    #[test]
    fn test_accessor_pay_status_text_aligns_php() {
        let model = Contract::new().with_data("pay_status", json!(1));
        assert_eq!(model.accessor_for("pay_status_text", None), json!("待缴费"));
        let model = Contract::new().with_data("pay_status", json!(2));
        assert_eq!(model.accessor_for("pay_status_text", None), json!("已缴费"));
        let model = Contract::new().with_data("pay_status", json!(3));
        assert_eq!(model.accessor_for("pay_status_text", None), json!("缴费中"));
        let model = Contract::new();
        assert_eq!(model.accessor_for("pay_status_text", None), json!(""));
    }

    #[test]
    fn test_accessor_pay_type_text_aligns_php() {
        let model = Contract::new().with_data("pay_type", json!(1));
        assert_eq!(model.accessor_for("pay_type_text", None), json!("扫码转账"));
        let model = Contract::new().with_data("pay_type", json!(2));
        assert_eq!(model.accessor_for("pay_type_text", None), json!("现金支付"));
        let model = Contract::new().with_data("pay_type", json!(3));
        assert_eq!(
            model.accessor_for("pay_type_text", None),
            json!("转账+现金")
        );
        let model = Contract::new();
        assert_eq!(model.accessor_for("pay_type_text", None), json!(""));
    }

    // -------------------- 数值强转访问器测试 --------------------

    #[test]
    fn test_accessor_contract_price_zero_returns_int_zero() {
        // PHP $value=0 → 0 (int)
        let model = Contract::new().with_data("contract_price", json!(0));
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(0));
        // 整数 0 不是浮点 0.0
        assert!(v.is_i64(), "PHP $value=0 应返回 int 0，非 float 0.0");
    }

    #[test]
    fn test_accessor_contract_price_string_zero_returns_int_zero() {
        // PHP $value="0" → 0 (int)
        let model = Contract::new().with_data("contract_price", json!("0"));
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(0));
        assert!(v.is_i64());
    }

    #[test]
    fn test_accessor_contract_price_empty_string_returns_int_zero() {
        // PHP $value="" → 0 (int)
        let model = Contract::new().with_data("contract_price", json!(""));
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(0));
        assert!(v.is_i64());
    }

    #[test]
    fn test_accessor_contract_price_missing_returns_int_zero() {
        // PHP $value=null → 0 (int)
        let model = Contract::new();
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(0));
        assert!(v.is_i64());
    }

    #[test]
    fn test_accessor_contract_price_nonzero_returns_float() {
        // PHP $value="100.5" → (float)"100.5" = 100.5
        let model = Contract::new().with_data("contract_price", json!("100.5"));
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(100.5));
        assert!(v.is_f64(), "PHP 非零值应返回 float");
    }

    #[test]
    fn test_accessor_contract_price_numeric_nonzero_returns_float() {
        // PHP $value=100.5 → (float)100.5 = 100.5
        let model = Contract::new().with_data("contract_price", json!(100.5));
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(100.5));
        assert!(v.is_f64());
    }

    #[test]
    fn test_accessor_all_price_fields_align_php() {
        // 11 个价格字段统一行为测试
        let price_fields = [
            "contract_price",
            "rent_price",
            "manage_price",
            "sanitation_price",
            "electricity_price",
            "water_price",
            "security_price",
            "paid_price",
            "capacity_price",
            "decoration_price",
            "decoration_deposit",
        ];
        for field in price_fields {
            // 0 → int 0
            let model = Contract::new().with_data(field, json!(0));
            let v = model.accessor_for(field, None);
            assert_eq!(v, json!(0), "{}: 0 应返回 int 0", field);
            assert!(v.is_i64(), "{}: 0 应为 i64", field);

            // 非零 → float
            let model = Contract::new().with_data(field, json!(200.75));
            let v = model.accessor_for(field, None);
            assert_eq!(v, json!(200.75), "{}: 200.75 应返回 float", field);
            assert!(v.is_f64(), "{}: 200.75 应为 f64", field);
        }
    }

    #[test]
    fn test_accessor_price_string_zero_point_zero_returns_float_zero() {
        // PHP $value="0.0" → truthy（PHP 8: empty("0.0")=false）→ (float)"0.0" = 0.0
        // Rust 端严格对齐 PHP 8 语义
        let model = Contract::new().with_data("contract_price", json!("0.0"));
        let v = model.accessor_for("contract_price", None);
        assert_eq!(v, json!(0.0));
        assert!(v.is_f64(), "PHP \"0.0\" truthy 应返回 float 0.0");
    }

    // -------------------- 日期计算访问器测试 --------------------

    #[test]
    fn test_accessor_remaining_day_empty_returns_zero() {
        // PHP empty($data['end_day']) → 0
        let model = Contract::new();
        assert_eq!(model.accessor_for("remaining_day", None), json!(0));
    }

    #[test]
    fn test_accessor_remaining_day_invalid_date_returns_zero() {
        // PHP strtotime("invalid") = false → (false - ts) / 86400 → 负数
        // 但 PHP 实际行为：strtotime("invalid") 返回 false，floor((false - ts)/86400) = floor(-ts/86400)
        // Rust 端解析失败统一返回 0（防御性，与 PHP 行为不完全一致，但 end_day 不会是非法字符串）
        let model = Contract::new().with_data("end_day", json!("invalid-date"));
        assert_eq!(model.accessor_for("remaining_day", None), json!(0));
    }

    #[test]
    fn test_accessor_remaining_day_future_date_returns_positive() {
        // 未来日期 → 正数天数差
        let today = chrono::Local::now().date_naive();
        let future = today + chrono::Duration::days(30);
        let end_day = future.format("%Y-%m-%d").to_string();
        let model = Contract::new().with_data("end_day", json!(end_day));
        assert_eq!(model.accessor_for("remaining_day", None), json!(30));
    }

    #[test]
    fn test_accessor_remaining_day_past_date_returns_negative() {
        // 过去日期 → 负数天数差
        let today = chrono::Local::now().date_naive();
        let past = today - chrono::Duration::days(10);
        let end_day = past.format("%Y-%m-%d").to_string();
        let model = Contract::new().with_data("end_day", json!(end_day));
        assert_eq!(model.accessor_for("remaining_day", None), json!(-10));
    }

    #[test]
    fn test_accessor_remaining_day_today_returns_zero() {
        // end_day = 今天 → 0
        let today = chrono::Local::now().date_naive();
        let end_day = today.format("%Y-%m-%d").to_string();
        let model = Contract::new().with_data("end_day", json!(end_day));
        assert_eq!(model.accessor_for("remaining_day", None), json!(0));
    }

    #[test]
    fn test_accessor_remaining_day_datetime_format_returns_diff() {
        // PHP strtotime 支持 "Y-m-d H:i:s" 格式，Rust 端截取前 10 字符
        let today = chrono::Local::now().date_naive();
        let future = today + chrono::Duration::days(7);
        let end_day = format!("{} 23:59:59", future.format("%Y-%m-%d"));
        let model = Contract::new().with_data("end_day", json!(end_day));
        assert_eq!(model.accessor_for("remaining_day", None), json!(7));
    }

    // -------------------- 序列化解码访问器测试 --------------------

    #[test]
    fn test_accessor_pay_detail_empty_returns_empty_array() {
        // PHP !empty("")=false → []
        let model = Contract::new().with_data("pay_detail", json!(""));
        assert_eq!(model.accessor_for("pay_detail", None), json!([]));
    }

    #[test]
    fn test_accessor_pay_detail_missing_returns_empty_array() {
        let model = Contract::new();
        assert_eq!(model.accessor_for("pay_detail", None), json!([]));
    }

    #[test]
    fn test_accessor_pay_detail_valid_json_returns_array() {
        // Rust 端用 JSON（PHP 端用 unserialize，不兼容）
        let model = Contract::new().with_data("pay_detail", json!(r#"{"key":"value","num":100}"#));
        let v = model.accessor_for("pay_detail", None);
        assert_eq!(v["key"], "value");
        assert_eq!(v["num"], 100);
    }

    #[test]
    fn test_accessor_pay_detail_invalid_json_returns_empty_array() {
        // PHP unserialize 失败抛异常；Rust 端 serde_json 解析失败返回 []
        let model = Contract::new().with_data("pay_detail", json!("invalid-serialized"));
        assert_eq!(model.accessor_for("pay_detail", None), json!([]));
    }

    #[test]
    fn test_accessor_files_empty_returns_empty_array() {
        // PHP empty($value)=true → []
        let model = Contract::new().with_data("files", json!(""));
        assert_eq!(model.accessor_for("files", None), json!([]));
    }

    #[test]
    fn test_accessor_files_missing_returns_empty_array() {
        let model = Contract::new();
        assert_eq!(model.accessor_for("files", None), json!([]));
    }

    #[test]
    fn test_accessor_files_valid_json_returns_array() {
        let model = Contract::new().with_data(
            "files",
            json!(r#"[{"name":"a.pdf","url":"/uploads/a.pdf"}]"#),
        );
        let v = model.accessor_for("files", None);
        assert!(v.is_array());
        assert_eq!(v[0]["name"], "a.pdf");
        assert_eq!(v[0]["url"], "/uploads/a.pdf");
    }

    #[test]
    fn test_accessor_files_invalid_json_returns_empty_array() {
        // PHP json_decode 失败返回 null；Rust 端解析失败返回 []
        let model = Contract::new().with_data("files", json!("not-json"));
        assert_eq!(model.accessor_for("files", None), json!([]));
    }

    // -------------------- 虚拟字段访问器测试 --------------------

    #[test]
    fn test_accessor_logs_returns_empty_without_db() {
        // 当前无数据库连接，logs 始终返回空数组
        // 完整实现在 Repository 层
        let model = Contract::new().with_data("contract_id", json!(1));
        assert_eq!(model.accessor_for("logs", None), json!([]));
    }

    // -------------------- 修改器测试 --------------------

    #[test]
    fn test_mutator_pay_detail_empty_returns_empty_string() {
        // PHP setPayDetailAttr: !empty($value) ? iserializer($value) : ''
        let mut model = Contract::new();
        let merged = HashMap::new();
        let result = model.mutator_for("pay_detail", &json!([]), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String(String::new())))
        );
    }

    #[test]
    fn test_mutator_pay_detail_null_returns_empty_string() {
        let mut model = Contract::new();
        let merged = HashMap::new();
        let result = model.mutator_for("pay_detail", &json!(null), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String(String::new())))
        );
    }

    #[test]
    fn test_mutator_pay_detail_nonempty_returns_json_string() {
        // Rust 端用 serde_json（PHP 端用 iserializer，不兼容）
        let mut model = Contract::new();
        let merged = HashMap::new();
        let result = model.mutator_for("pay_detail", &json!({"key": "value"}), &merged);
        if let Some(MutatorResult::Value(Value::String(s))) = result {
            let parsed: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed["key"], "value");
        } else {
            panic!("应返回 MutatorResult::Value(String)");
        }
    }

    #[test]
    fn test_mutator_files_empty_returns_bracket_string() {
        // PHP setFilesAttr: empty($value) ? '[]' : json_encode($value)
        let mut model = Contract::new();
        let merged = HashMap::new();
        let result = model.mutator_for("files", &json!([]), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String("[]".to_string())))
        );
    }

    #[test]
    fn test_mutator_files_null_returns_bracket_string() {
        let mut model = Contract::new();
        let merged = HashMap::new();
        let result = model.mutator_for("files", &json!(null), &merged);
        assert_eq!(
            result,
            Some(MutatorResult::Value(Value::String("[]".to_string())))
        );
    }

    #[test]
    fn test_mutator_files_nonempty_returns_json_string() {
        let mut model = Contract::new();
        let merged = HashMap::new();
        let result = model.mutator_for("files", &json!([{"name": "a.pdf"}]), &merged);
        if let Some(MutatorResult::Value(Value::String(s))) = result {
            assert_eq!(s, r#"[{"name":"a.pdf"}]"#);
        } else {
            panic!("应返回 MutatorResult::Value(String)");
        }
    }

    #[test]
    fn test_mutator_unknown_field_returns_none() {
        let mut model = Contract::new();
        let merged = HashMap::new();
        assert_eq!(
            model.mutator_for("contract_name", &json!("测试"), &merged),
            None,
            "Contract 仅声明 pay_detail / files 修改器，其他字段应返回 None"
        );
    }

    // -------------------- Appendable 序列化测试 --------------------

    #[test]
    fn test_to_json_with_append_cached_includes_all_append_fields() {
        let mut model = Contract::new()
            .with_data("contract_id", json!(1))
            .with_data("contract_name", json!("2026年合同"))
            .with_data("signing_status", json!(2))
            .with_data("contract_status", json!(1))
            .with_data("pay_status", json!(1))
            .with_data("pay_type", json!(2))
            .with_data("contract_price", json!(0))
            .with_data("end_day", json!("2099-12-31"));
        let json = model.to_json_with_append_cached();
        // 基础字段
        assert_eq!(json["contract_id"], 1);
        assert_eq!(json["contract_name"], "2026年合同");
        assert_eq!(json["signing_status"], 2);
        assert_eq!(json["contract_status"], 1);
        // append 字段
        assert_eq!(json["signing_text"], "已签约");
        assert_eq!(json["contract_text"], "待生效");
        assert_eq!(json["pay_status_text"], "待缴费");
        assert_eq!(json["pay_type_text"], "现金支付");
        assert_eq!(json["logs"], json!([]));
        // remaining_day 为正数（2099-12-31 在今天之后）
        let remaining = json["remaining_day"].as_i64().unwrap();
        assert!(remaining > 0, "remaining_day 应为正数（未来日期）");
    }

    #[test]
    fn test_to_json_with_append_cached_empty_data_returns_empty_strings() {
        let mut model = Contract::new().with_data("contract_id", json!(1));
        let json = model.to_json_with_append_cached();
        // 所有枚举映射字段为空字符串
        assert_eq!(json["signing_text"], "");
        assert_eq!(json["contract_text"], "");
        assert_eq!(json["pay_status_text"], "");
        assert_eq!(json["pay_type_text"], "");
        // remaining_day 为 0（end_day 缺失）
        assert_eq!(json["remaining_day"], 0);
        // logs 为空数组
        assert_eq!(json["logs"], json!([]));
    }

    // -------------------- php_is_empty / php_price_attr 单元测试 --------------------

    #[test]
    fn test_php_is_empty_aligns_php8_semantics() {
        // PHP empty() 返回 true 的情况
        assert!(php_is_empty(None));
        assert!(php_is_empty(Some(&json!(null))));
        assert!(php_is_empty(Some(&json!(false))));
        assert!(php_is_empty(Some(&json!(0))));
        assert!(php_is_empty(Some(&json!(0.0))));
        assert!(php_is_empty(Some(&json!(""))));
        assert!(php_is_empty(Some(&json!("0"))));
        assert!(php_is_empty(Some(&json!([]))));

        // PHP empty() 返回 false 的情况
        assert!(!php_is_empty(Some(&json!(true))));
        assert!(!php_is_empty(Some(&json!(1))));
        assert!(!php_is_empty(Some(&json!(-1))));
        assert!(!php_is_empty(Some(&json!(0.1))));
        assert!(!php_is_empty(Some(&json!("0.0")))); // PHP 8: empty("0.0") = false
        assert!(!php_is_empty(Some(&json!("abc"))));
        assert!(!php_is_empty(Some(&json!([1]))));
        assert!(!php_is_empty(Some(&json!({"k": "v"}))));
    }

    #[test]
    fn test_php_price_attr_aligns_php_truthiness() {
        // false 分支：返回 int 0
        assert_eq!(php_price_attr(None), json!(0));
        assert_eq!(php_price_attr(Some(&json!(0))), json!(0));
        assert_eq!(php_price_attr(Some(&json!(0.0))), json!(0));
        assert_eq!(php_price_attr(Some(&json!(""))), json!(0));
        assert_eq!(php_price_attr(Some(&json!("0"))), json!(0));

        // true 分支：返回 float
        assert_eq!(php_price_attr(Some(&json!(100))), json!(100.0));
        assert_eq!(php_price_attr(Some(&json!(100.5))), json!(100.5));
        assert_eq!(php_price_attr(Some(&json!("100.5"))), json!(100.5));
        // PHP "abc" truthy → (float)"abc" = 0.0
        assert_eq!(php_price_attr(Some(&json!("abc"))), json!(0.0));
    }
}

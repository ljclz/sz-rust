use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 结算模型实体（对齐 PHP Settlement 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settlement {
    /// 结算主键 ID
    pub settle_id: Option<i64>,
    /// 商户 ID
    pub merchant_id: i64,
    /// 结算日期（格式：YYYY-MM-DD）
    pub settle_date: String,
    /// 结算总金额（单位：分）
    pub total_fen: i64,
    /// 订单数量
    pub order_count: i32,
    /// 手续费（单位：分）
    pub fee_fen: i64,
    /// 状态（0=待结算，1=已结算，2=已失败）
    pub status: i8,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Settlement {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "settlement"
    }

    fn pk_name() -> &'static str {
        "settle_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.settle_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.settle_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Settlement {
    fn columns() -> Vec<&'static str> {
        vec![
            "settle_id",
            "merchant_id",
            "settle_date",
            "total_fen",
            "order_count",
            "fee_fen",
            "status",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "merchant_id",
            "settle_date",
            "total_fen",
            "order_count",
            "fee_fen",
            "status",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["settle_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "settle_id" => self.settle_id.map(Value::I64),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "settle_date" => Some(Value::String(self.settle_date.clone())),
            "total_fen" => Some(Value::I64(self.total_fen)),
            "order_count" => Some(Value::I32(self.order_count)),
            "fee_fen" => Some(Value::I64(self.fee_fen)),
            "status" => Some(Value::I32(self.status as i32)),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("settle_id").and_then(|v| v.as_i64()) {
            self.settle_id = Some(v);
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("settle_date").and_then(|v| v.as_str()) {
            self.settle_date = v.to_string();
        }
        if let Some(v) = map.get("total_fen").and_then(|v| v.as_i64()) {
            self.total_fen = v;
        }
        if let Some(v) = map.get("order_count").and_then(|v| v.as_i64()) {
            self.order_count = v as i32;
        }
        if let Some(v) = map.get("fee_fen").and_then(|v| v.as_i64()) {
            self.fee_fen = v;
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Settlement {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}

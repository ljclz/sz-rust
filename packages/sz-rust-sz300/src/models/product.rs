use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 商品模型实体（对齐 PHP Product 模型，对应数据表 good）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    /// 商品主键 ID
    pub good_id: Option<i64>,
    /// 商户 ID
    pub merchant_id: i64,
    /// 类目 ID
    pub cat_id: i64,
    /// 商品名称
    pub name: String,
    /// 条形码
    pub barcode: String,
    /// 单价（单位：分）
    pub price: i64,
    /// 计价单位（如：个、斤、千克）
    pub unit: String,
    /// AI 分类 ID
    pub ai_class_id: i64,
    /// 商品图片 URL
    pub image: String,
    /// 状态（0=下架，1=上架）
    pub status: i8,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Product {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "good"
    }

    fn pk_name() -> &'static str {
        "good_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.good_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.good_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Product {
    fn columns() -> Vec<&'static str> {
        vec![
            "good_id",
            "merchant_id",
            "cat_id",
            "name",
            "barcode",
            "price",
            "unit",
            "ai_class_id",
            "image",
            "status",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "merchant_id",
            "cat_id",
            "name",
            "barcode",
            "price",
            "unit",
            "ai_class_id",
            "image",
            "status",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["good_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "good_id" => self.good_id.map(Value::I64),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "cat_id" => Some(Value::I64(self.cat_id)),
            "name" => Some(Value::String(self.name.clone())),
            "barcode" => Some(Value::String(self.barcode.clone())),
            "price" => Some(Value::I64(self.price)),
            "unit" => Some(Value::String(self.unit.clone())),
            "ai_class_id" => Some(Value::I64(self.ai_class_id)),
            "image" => Some(Value::String(self.image.clone())),
            "status" => Some(Value::I32(self.status as i32)),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("good_id").and_then(|v| v.as_i64()) {
            self.good_id = Some(v);
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("cat_id").and_then(|v| v.as_i64()) {
            self.cat_id = v;
        }
        if let Some(v) = map.get("name").and_then(|v| v.as_str()) {
            self.name = v.to_string();
        }
        if let Some(v) = map.get("barcode").and_then(|v| v.as_str()) {
            self.barcode = v.to_string();
        }
        if let Some(v) = map.get("price").and_then(|v| v.as_i64()) {
            self.price = v;
        }
        if let Some(v) = map.get("unit").and_then(|v| v.as_str()) {
            self.unit = v.to_string();
        }
        if let Some(v) = map.get("ai_class_id").and_then(|v| v.as_i64()) {
            self.ai_class_id = v;
        }
        if let Some(v) = map.get("image").and_then(|v| v.as_str()) {
            self.image = v.to_string();
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

impl RelationLoader for Product {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}

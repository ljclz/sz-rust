// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 商品模型实体（对齐 PHP Product 模型，对应数据表 good）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    /// 商品主键 ID
    pub good_id: Option<i64>,
    /// 商户 ID
    pub merchant_id: i64,
    /// 类目 ID（NULL 表示未分类；016_nullable_good_cats.sql 后改 NULLABLE）
    pub cat_id: Option<i64>,
    /// 商品名称
    pub name: String,
    /// 条形码
    pub barcode: String,
    /// 单价（单位：分）
    pub price: i64,
    /// 计价单位（如：个、斤、千克）
    pub unit: String,
    /// AI 分类 ID（NULL 表示未 AI 分类；016_nullable_good_cats.sql 后改 NULLABLE）
    pub ai_class_id: Option<i64>,
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
            "cat_id" => self.cat_id.map(Value::I64),
            "name" => Some(Value::String(self.name.clone())),
            "barcode" => Some(Value::String(self.barcode.clone())),
            "price" => Some(Value::I64(self.price)),
            "unit" => Some(Value::String(self.unit.clone())),
            "ai_class_id" => self.ai_class_id.map(Value::I64),
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
            self.cat_id = Some(v);
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
            self.ai_class_id = Some(v);
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_preserves_nullable_cats() {
        // cat_id / ai_class_id 的 Some 与 None 两种情况
        let p = Product {
            good_id: Some(88),
            merchant_id: 5,
            cat_id: Some(3),
            name: "活鲈鱼".into(),
            barcode: "6900000000001".into(),
            price: 3000,
            unit: "斤".into(),
            ai_class_id: Some(12),
            image: "https://cdn.example.com/img/88.jpg".into(),
            status: 1,
            created_at: Some("2026-05-05 00:00:00".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let back: Product = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.good_id, Some(88));
        assert_eq!(back.merchant_id, 5);
        assert_eq!(back.cat_id, Some(3));
        assert_eq!(back.name, "活鲈鱼");
        assert_eq!(back.barcode, "6900000000001");
        assert_eq!(back.price, 3000);
        assert_eq!(back.unit, "斤");
        assert_eq!(back.ai_class_id, Some(12));
        assert_eq!(back.image, "https://cdn.example.com/img/88.jpg");
        assert_eq!(back.status, 1);
        assert_eq!(back.created_at.as_deref(), Some("2026-05-05 00:00:00"));
        assert_eq!(back.updated_at, None);

        let unclassified = Product {
            good_id: None,
            merchant_id: 1,
            cat_id: None,
            name: "未分类商品".into(),
            barcode: String::new(),
            price: 0,
            unit: "个".into(),
            ai_class_id: None,
            image: String::new(),
            status: 0,
            created_at: None,
            updated_at: None,
        };
        let json2 = serde_json::to_string(&unclassified).expect("serialize");
        let back2: Product = serde_json::from_str(&json2).expect("deserialize");
        assert_eq!(back2.cat_id, None);
        assert_eq!(back2.ai_class_id, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Product::table_name(), "good");
        assert_eq!(Product::pk_name(), "good_id");
        let mut p = Product {
            good_id: None,
            merchant_id: 0,
            cat_id: None,
            name: String::new(),
            barcode: String::new(),
            price: 0,
            unit: String::new(),
            ai_class_id: None,
            image: String::new(),
            status: 0,
            created_at: None,
            updated_at: None,
        };
        assert_eq!(p.pk(), 0);
        assert!(p.get_column_value("good_id").is_none());
        p.set_pk(66);
        assert_eq!(p.pk(), 66);
        assert!(p.get_column_value("good_id").is_some());
        assert_eq!(Product::columns().len(), 12);
        assert_eq!(Product::fillable().len(), 9);
        assert_eq!(Product::guarded().len(), 1);
        assert!(p.get_column_value("cat_id").is_none());
        assert!(p.get_column_value("ai_class_id").is_none());
        assert!(p.get_column_value("name").is_some());
        assert!(p.get_column_value("nope").is_none());
        assert!(Product::timestamp_fields().is_some());
    }

    #[test]
    fn from_value_populates_all_fields_then_empty_keeps() {
        use std::collections::HashMap;
        let mut p = Product {
            good_id: None,
            merchant_id: 0,
            cat_id: None,
            name: String::new(),
            barcode: String::new(),
            price: 0,
            unit: String::new(),
            ai_class_id: None,
            image: String::new(),
            status: 0,
            created_at: None,
            updated_at: None,
        };
        let mut map = HashMap::new();
        map.insert("good_id".to_string(), Value::I64(88));
        map.insert("merchant_id".to_string(), Value::I64(5));
        map.insert("cat_id".to_string(), Value::I64(3));
        map.insert("name".to_string(), Value::String("鱼".into()));
        map.insert("barcode".to_string(), Value::String("690".into()));
        map.insert("price".to_string(), Value::I64(3000));
        map.insert("unit".to_string(), Value::String("斤".into()));
        map.insert("ai_class_id".to_string(), Value::I64(12));
        map.insert("image".to_string(), Value::String("img".into()));
        map.insert("status".to_string(), Value::I64(1));
        map.insert("created_at".to_string(), Value::String("c".into()));
        map.insert("updated_at".to_string(), Value::String("u".into()));
        p.from_value(map);
        assert_eq!(p.good_id, Some(88));
        assert_eq!(p.merchant_id, 5);
        assert_eq!(p.cat_id, Some(3));
        assert_eq!(p.name, "鱼");
        assert_eq!(p.barcode, "690");
        assert_eq!(p.price, 3000);
        assert_eq!(p.unit, "斤");
        assert_eq!(p.ai_class_id, Some(12));
        assert_eq!(p.image, "img");
        assert_eq!(p.status, 1);
        assert_eq!(p.created_at.as_deref(), Some("c"));
        assert_eq!(p.updated_at.as_deref(), Some("u"));
        p.from_value(HashMap::new());
        assert_eq!(p.good_id, Some(88));
        assert_eq!(p.merchant_id, 5);
        assert_eq!(p.name, "鱼");
    }
}

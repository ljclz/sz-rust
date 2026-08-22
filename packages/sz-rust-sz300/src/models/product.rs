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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample() -> Product {
        Product {
            good_id: Some(1),
            merchant_id: 7,
            cat_id: 3,
            name: "白菜".into(),
            barcode: "6900000000001".into(),
            price: 250,
            unit: "斤".into(),
            ai_class_id: 10,
            image: "/uploads/baicao.jpg".into(),
            status: 1,
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Product::table_name(), "good");
        assert_eq!(Product::pk_name(), "good_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut p = sample();
        assert_eq!(p.pk(), 1);
        p.good_id = None;
        assert_eq!(p.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut p = sample();
        p.set_pk(88);
        assert_eq!(p.good_id, Some(88));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = Product::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(Product::columns().len(), 12);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Product::fillable();
        assert!(!f.contains(&"good_id"));
        assert!(f.contains(&"name"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(Product::guarded(), vec!["good_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let p = sample();
        assert_eq!(p.get_column_value("good_id"), Some(Value::I64(1)));
        assert_eq!(
            p.get_column_value("name"),
            Some(Value::String("白菜".into()))
        );
        assert_eq!(p.get_column_value("price"), Some(Value::I64(250)));
        assert_eq!(p.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(p.get_column_value("ai_class_id"), Some(Value::I64(10)));
        assert_eq!(p.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let p = Product {
            good_id: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(p.get_column_value("good_id"), None);
        assert_eq!(p.get_column_value("created_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut p = Product {
            good_id: None,
            merchant_id: 0,
            cat_id: 0,
            name: String::new(),
            barcode: String::new(),
            price: 0,
            unit: String::new(),
            ai_class_id: 0,
            image: String::new(),
            status: 0,
            created_at: None,
            updated_at: None,
        };
        let mut m = HashMap::new();
        m.insert("good_id".into(), Value::I64(9));
        m.insert("merchant_id".into(), Value::I64(1));
        m.insert("cat_id".into(), Value::I64(2));
        m.insert("name".into(), Value::String("萝卜".into()));
        m.insert("barcode".into(), Value::String("123".into()));
        m.insert("price".into(), Value::I64(300));
        m.insert("unit".into(), Value::String("个".into()));
        m.insert("ai_class_id".into(), Value::I64(5));
        m.insert("image".into(), Value::String("/img.jpg".into()));
        m.insert("status".into(), Value::I64(0));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        m.insert("updated_at".into(), Value::String("2026-01-02".into()));
        p.from_value(m);
        assert_eq!(p.good_id, Some(9));
        assert_eq!(p.name, "萝卜");
        assert_eq!(p.price, 300);
        assert_eq!(p.status, 0);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut p = sample();
        p.from_value(HashMap::new());
        assert_eq!(p.good_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let p = sample();
        assert!(p.get_relation("any").is_none());
        assert_eq!(p.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut p = sample();
        p.set_relation_data("x", Value::Null);
        assert!(p.get_relation("x").is_none());
    }
}

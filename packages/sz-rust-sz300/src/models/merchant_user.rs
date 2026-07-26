use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 商户用户模型实体（对齐 PHP MerchantUser 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantUser {
    /// 用户主键 ID
    pub user_id: Option<i64>,
    /// 所属商户 ID
    pub merchant_id: i64,
    /// 登录用户名
    pub username: String,
    /// 密码哈希值
    pub password: String,
    /// 联系电话
    pub phone: String,
    /// 角色（0=普通用户，1=管理员）
    pub role: i8,
    /// 最后登录时间
    pub last_login_at: Option<String>,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for MerchantUser {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "merchant_user"
    }

    fn pk_name() -> &'static str {
        "user_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.user_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.user_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for MerchantUser {
    fn columns() -> Vec<&'static str> {
        vec![
            "user_id",
            "merchant_id",
            "username",
            "password",
            "phone",
            "role",
            "last_login_at",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "merchant_id",
            "username",
            "password",
            "phone",
            "role",
            "last_login_at",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["user_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "user_id" => self.user_id.map(Value::I64),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "username" => Some(Value::String(self.username.clone())),
            "password" => Some(Value::String(self.password.clone())),
            "phone" => Some(Value::String(self.phone.clone())),
            "role" => Some(Value::I32(self.role as i32)),
            "last_login_at" => self
                .last_login_at
                .as_ref()
                .map(|s| Value::String(s.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("user_id").and_then(|v| v.as_i64()) {
            self.user_id = Some(v);
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("username").and_then(|v| v.as_str()) {
            self.username = v.to_string();
        }
        if let Some(v) = map.get("password").and_then(|v| v.as_str()) {
            self.password = v.to_string();
        }
        if let Some(v) = map.get("phone").and_then(|v| v.as_str()) {
            self.phone = v.to_string();
        }
        if let Some(v) = map.get("role").and_then(|v| v.as_i64()) {
            self.role = v as i8;
        }
        if let Some(v) = map.get("last_login_at").and_then(|v| v.as_str()) {
            self.last_login_at = Some(v.to_string());
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for MerchantUser {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}

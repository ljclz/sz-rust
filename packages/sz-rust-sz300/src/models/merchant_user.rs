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
    /// 密码哈希值（序列化时自动脱敏，防止通过 API 响应泄漏）
    #[serde(skip_serializing)]
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

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// P0-SEC-01：password 字段序列化时必须脱敏（不出现在 JSON 响应中）
    #[test]
    fn test_p0_sec_01_password_not_in_serialized_json() {
        let user = MerchantUser {
            user_id: Some(42),
            merchant_id: 7,
            username: "admin".to_string(),
            password: "$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/X4.G.2I.2I.2I.2I".to_string(),
            phone: "13800138000".to_string(),
            role: 1,
            last_login_at: None,
            created_at: None,
            updated_at: None,
        };

        let json = serde_json::to_string(&user).expect("序列化应成功");

        // password 绝不应出现在序列化输出中
        assert!(
            !json.contains("password"),
            "P0-SEC-01: password 字段出现在序列化 JSON 中（安全脱敏失败）: {json}"
        );
        assert!(
            !json.contains("$2b$12"),
            "P0-SEC-01: bcrypt 哈希泄漏到 JSON 响应中: {json}"
        );

        // 其他非敏感字段应正常序列化
        assert!(json.contains("admin"), "username 应出现在 JSON 中");
        assert!(json.contains("13800138000"), "phone 应出现在 JSON 中");
    }

    /// P0-SEC-01：password 字段反序列化时仍可正常读取（脱敏仅影响序列化）
    #[test]
    fn test_p0_sec_01_password_deserializes_normally() {
        let json = json!({
            "user_id": 99,
            "merchant_id": 1,
            "username": "testuser",
            "password": "bcrypt_hash_xyz",
            "phone": "13900139000",
            "role": 0
        });

        let user: MerchantUser = serde_json::from_value(json).expect("反序列化应成功");
        assert_eq!(user.password, "bcrypt_hash_xyz");
        assert_eq!(user.username, "testuser");
        assert_eq!(user.user_id, Some(99));
    }

    // ---- Model / ModelExt / RelationLoader trait 覆盖 ----

    fn sample_user() -> MerchantUser {
        MerchantUser {
            user_id: Some(1),
            merchant_id: 7,
            username: "admin".into(),
            password: "hash".into(),
            phone: "13800138000".into(),
            role: 1,
            last_login_at: Some("2026-01-01".into()),
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(MerchantUser::table_name(), "merchant_user");
        assert_eq!(MerchantUser::pk_name(), "user_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut u = sample_user();
        assert_eq!(u.pk(), 1);
        u.user_id = None;
        assert_eq!(u.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut u = sample_user();
        u.set_pk(42);
        assert_eq!(u.user_id, Some(42));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = MerchantUser::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(MerchantUser::columns().len(), 9);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = MerchantUser::fillable();
        assert!(!f.contains(&"user_id"));
        assert!(f.contains(&"username"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(MerchantUser::guarded(), vec!["user_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let u = sample_user();
        assert_eq!(u.get_column_value("user_id"), Some(Value::I64(1)));
        assert_eq!(u.get_column_value("merchant_id"), Some(Value::I64(7)));
        assert_eq!(
            u.get_column_value("username"),
            Some(Value::String("admin".into()))
        );
        assert_eq!(
            u.get_column_value("password"),
            Some(Value::String("hash".into()))
        );
        assert_eq!(u.get_column_value("role"), Some(Value::I32(1)));
        assert_eq!(u.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let u = MerchantUser {
            user_id: None,
            last_login_at: None,
            created_at: None,
            updated_at: None,
            ..sample_user()
        };
        assert_eq!(u.get_column_value("user_id"), None);
        assert_eq!(u.get_column_value("last_login_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut u = MerchantUser {
            user_id: None,
            merchant_id: 0,
            username: String::new(),
            password: String::new(),
            phone: String::new(),
            role: 0,
            last_login_at: None,
            created_at: None,
            updated_at: None,
        };
        let mut m = HashMap::new();
        m.insert("user_id".into(), Value::I64(9));
        m.insert("merchant_id".into(), Value::I64(2));
        m.insert("username".into(), Value::String("test".into()));
        m.insert("password".into(), Value::String("pw".into()));
        m.insert("phone".into(), Value::String("139".into()));
        m.insert("role".into(), Value::I64(0));
        m.insert("last_login_at".into(), Value::String("2026-01-01".into()));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        m.insert("updated_at".into(), Value::String("2026-01-02".into()));
        u.from_value(m);
        assert_eq!(u.user_id, Some(9));
        assert_eq!(u.username, "test");
        assert_eq!(u.role, 0);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut u = sample_user();
        u.from_value(HashMap::new());
        assert_eq!(u.user_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let u = sample_user();
        assert!(u.get_relation("any").is_none());
        assert_eq!(u.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut u = sample_user();
        u.set_relation_data("x", Value::Null);
        assert!(u.get_relation("x").is_none());
    }
}

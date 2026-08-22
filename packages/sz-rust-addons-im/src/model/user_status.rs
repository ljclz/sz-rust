use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct UserStatus {
    pub id: i64,
    pub user_id: i64,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub last_seen: i64,
    #[serde(default)]
    pub device_type: String,
}

impl UserStatus {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for UserStatus {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "user_id" => Some(V::I64(self.user_id)),
            "is_online" => Some(V::Bool(self.is_online)),
            "last_seen" => Some(V::I64(self.last_seen)),
            "device_type" => Some(V::String(self.device_type.clone())),
            _ => None,
        }
    }
}

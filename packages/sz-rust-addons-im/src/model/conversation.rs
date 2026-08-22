use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Conversation {
    pub id: i64,
    pub user1_id: i64,
    pub user2_id: i64,
    #[serde(default)]
    pub last_message: String,
    #[serde(default)]
    pub last_message_at: i64,
    #[serde(default)]
    pub unread_count: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Conversation {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "user1_id" => Some(V::I64(self.user1_id)),
            "user2_id" => Some(V::I64(self.user2_id)),
            "last_message" => Some(V::String(self.last_message.clone())),
            "last_message_at" => Some(V::I64(self.last_message_at)),
            "unread_count" => Some(V::I64(self.unread_count)),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub sender_id: i64,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub msg_type: String,
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub created_at: i64,
}

impl Message {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Message {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "conversation_id" => Some(V::I64(self.conversation_id)),
            "sender_id" => Some(V::I64(self.sender_id)),
            "content" => Some(V::String(self.content.clone())),
            "msg_type" => Some(V::String(self.msg_type.clone())),
            "is_read" => Some(V::Bool(self.is_read)),
            "created_at" => Some(V::I64(self.created_at)),
            _ => None,
        }
    }
}

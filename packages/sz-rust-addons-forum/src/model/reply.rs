use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Reply {
    pub id: i64,
    pub topic_id: i64,
    pub author_id: i64,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub created_at: i64,
}

impl Reply {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Reply {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "topic_id" => Some(V::I64(self.topic_id)),
            "author_id" => Some(V::I64(self.author_id)),
            "content" => Some(V::String(self.content.clone())),
            "created_at" => Some(V::I64(self.created_at)),
            _ => None,
        }
    }
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Topic {
    pub id: i64,
    pub board_id: i64,
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub author_id: i64,
    #[serde(default)]
    pub reply_count: i64,
    #[serde(default)]
    pub view_count: i64,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub is_closed: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl Topic {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Topic {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "board_id" => Some(V::I64(self.board_id)),
            "title" => Some(V::String(self.title.clone())),
            "content" => Some(V::String(self.content.clone())),
            "author_id" => Some(V::I64(self.author_id)),
            "reply_count" => Some(V::I64(self.reply_count)),
            "view_count" => Some(V::I64(self.view_count)),
            "is_pinned" => Some(V::Bool(self.is_pinned)),
            "is_closed" => Some(V::Bool(self.is_closed)),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}

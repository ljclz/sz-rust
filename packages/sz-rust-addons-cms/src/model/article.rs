use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Article {
    pub id: i64,
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub category_id: i64,
    #[serde(default)]
    pub author_id: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub view_count: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl Article {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Article {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "title" => Some(V::String(self.title.clone())),
            "content" => Some(V::String(self.content.clone())),
            "summary" => Some(V::String(self.summary.clone())),
            "category_id" => Some(V::I64(self.category_id)),
            "author_id" => Some(V::I64(self.author_id)),
            "status" => Some(V::String(self.status.clone())),
            "view_count" => Some(V::I64(self.view_count)),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}

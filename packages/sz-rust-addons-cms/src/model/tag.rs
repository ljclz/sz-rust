use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub article_count: i64,
    #[serde(default)]
    pub created_at: i64,
}

impl Tag {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Tag {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "name" => Some(V::String(self.name.clone())),
            "article_count" => Some(V::I64(self.article_count)),
            "created_at" => Some(V::I64(self.created_at)),
            _ => None,
        }
    }
}

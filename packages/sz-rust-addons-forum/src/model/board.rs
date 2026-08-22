use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Board {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sort: i64,
    #[serde(default)]
    pub topic_count: i64,
    #[serde(default)]
    pub created_at: i64,
}

impl Board {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Board {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "name" => Some(V::String(self.name.clone())),
            "description" => Some(V::String(self.description.clone())),
            "sort" => Some(V::I64(self.sort)),
            "topic_count" => Some(V::I64(self.topic_count)),
            "created_at" => Some(V::I64(self.created_at)),
            _ => None,
        }
    }
}

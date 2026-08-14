use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Category {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub parent_id: i64,
    #[serde(default)]
    pub sort: i64,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl Category {
    pub fn new() -> Self {
        Self::default()
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Category {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "name" => Some(V::String(self.name.clone())),
            "parent_id" => Some(V::I64(self.parent_id)),
            "sort" => Some(V::I64(self.sort)),
            "description" => Some(V::String(self.description.clone())),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}

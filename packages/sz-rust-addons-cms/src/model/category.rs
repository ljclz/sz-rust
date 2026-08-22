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

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::orm::repository::EntityAttributes;
    use sz_rust_core::orm::Value as V;

    #[test]
    fn new_returns_default() {
        let c = Category::new();
        assert_eq!(c.id, 0);
        assert!(c.name.is_empty());
        assert_eq!(c.parent_id, 0);
        assert_eq!(c.sort, 0);
        assert!(c.description.is_empty());
        assert_eq!(c.created_at, 0);
        assert_eq!(c.updated_at, 0);
    }

    #[test]
    fn get_attribute_returns_all_fields() {
        let c = Category {
            id: 1,
            name: "Tech".to_string(),
            parent_id: 2,
            sort: 3,
            description: "desc".to_string(),
            created_at: 100,
            updated_at: 200,
        };
        assert_eq!(c.get_attribute("id"), Some(V::I64(1)));
        assert_eq!(c.get_attribute("name"), Some(V::String("Tech".to_string())));
        assert_eq!(c.get_attribute("parent_id"), Some(V::I64(2)));
        assert_eq!(c.get_attribute("sort"), Some(V::I64(3)));
        assert_eq!(
            c.get_attribute("description"),
            Some(V::String("desc".to_string()))
        );
        assert_eq!(c.get_attribute("created_at"), Some(V::I64(100)));
        assert_eq!(c.get_attribute("updated_at"), Some(V::I64(200)));
        assert_eq!(c.get_attribute("unknown"), None);
    }
}

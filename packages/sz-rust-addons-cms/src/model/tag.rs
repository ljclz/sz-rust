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

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::orm::repository::EntityAttributes;
    use sz_rust_core::orm::Value as V;

    #[test]
    fn new_returns_default() {
        let t = Tag::new();
        assert_eq!(t.id, 0);
        assert!(t.name.is_empty());
        assert_eq!(t.article_count, 0);
        assert_eq!(t.created_at, 0);
    }

    #[test]
    fn get_attribute_returns_all_fields() {
        let t = Tag {
            id: 1,
            name: "rust".to_string(),
            article_count: 5,
            created_at: 100,
        };
        assert_eq!(t.get_attribute("id"), Some(V::I64(1)));
        assert_eq!(t.get_attribute("name"), Some(V::String("rust".to_string())));
        assert_eq!(t.get_attribute("article_count"), Some(V::I64(5)));
        assert_eq!(t.get_attribute("created_at"), Some(V::I64(100)));
        assert_eq!(t.get_attribute("unknown"), None);
    }
}

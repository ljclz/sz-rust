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

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::orm::repository::EntityAttributes;
    use sz_rust_core::orm::Value as V;

    #[test]
    fn new_returns_default() {
        let a = Article::new();
        assert_eq!(a.id, 0);
        assert!(a.title.is_empty());
        assert!(a.content.is_empty());
        assert!(a.summary.is_empty());
        assert_eq!(a.category_id, 0);
        assert_eq!(a.author_id, 0);
        assert!(a.status.is_empty());
        assert_eq!(a.view_count, 0);
        assert_eq!(a.created_at, 0);
        assert_eq!(a.updated_at, 0);
    }

    #[test]
    fn get_attribute_returns_all_fields() {
        let a = Article {
            id: 1,
            title: "T".to_string(),
            content: "C".to_string(),
            summary: "S".to_string(),
            category_id: 2,
            author_id: 3,
            status: "draft".to_string(),
            view_count: 10,
            created_at: 100,
            updated_at: 200,
        };
        assert_eq!(a.get_attribute("id"), Some(V::I64(1)));
        assert_eq!(a.get_attribute("title"), Some(V::String("T".to_string())));
        assert_eq!(a.get_attribute("content"), Some(V::String("C".to_string())));
        assert_eq!(a.get_attribute("summary"), Some(V::String("S".to_string())));
        assert_eq!(a.get_attribute("category_id"), Some(V::I64(2)));
        assert_eq!(a.get_attribute("author_id"), Some(V::I64(3)));
        assert_eq!(
            a.get_attribute("status"),
            Some(V::String("draft".to_string()))
        );
        assert_eq!(a.get_attribute("view_count"), Some(V::I64(10)));
        assert_eq!(a.get_attribute("created_at"), Some(V::I64(100)));
        assert_eq!(a.get_attribute("updated_at"), Some(V::I64(200)));
        assert_eq!(a.get_attribute("unknown"), None);
    }
}

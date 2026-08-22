use serde_json::{json, Value};
use sz_rust_core::orm::repository::Repository;
use sz_rust_core::orm::Value as OrmValue;

use crate::model::tag::Tag;

pub struct TagController;

impl TagController {
    pub async fn list<R: Repository<Tag, Key = OrmValue>>(repo: &R) -> Value {
        match repo.paginate_by(&[], 1, 10000) {
            Ok(pr) => json!({"code": 0, "msg": "ok", "data": pr.items}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Tag, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut tag: Tag = match serde_json::from_value(body) {
            Ok(t) => t,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if tag.name.is_empty() {
            return json!({"code": 400, "msg": "name is required", "data": null});
        }
        tag.id = 0;
        match repo.save(tag) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Tag, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use sz_rust_core::orm::repository::InMemoryRepository;

    type TagRepo = Arc<InMemoryRepository<Tag>>;

    // --- list 边界 ---

    #[tokio::test]
    async fn list_returns_empty_when_no_tags() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        let result = TagController::list(&*repo).await;
        assert_eq!(result["code"], 0);
        assert!(result["data"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_returns_all_tags() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        repo.save(Tag {
            id: 1,
            name: "rust".to_string(),
            ..Default::default()
        })
        .unwrap();
        repo.save(Tag {
            id: 2,
            name: "web".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = TagController::list(&*repo).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"].as_array().unwrap().len(), 2);
    }

    // --- create 边界 ---

    #[tokio::test]
    async fn create_returns_400_on_deserialize_failure() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        // name 字段类型不匹配
        let result = TagController::create(&*repo, json!({"name": 123})).await;
        assert_eq!(result["code"], 400);
    }

    #[tokio::test]
    async fn create_rejects_empty_name() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        let result = TagController::create(&*repo, json!({"id": 0, "name": ""})).await;
        assert_eq!(result["code"], 400);
        assert_eq!(result["msg"], "name is required");
    }

    #[tokio::test]
    async fn create_succeeds_with_valid_name() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        let result = TagController::create(&*repo, json!({"id": 0, "name": "rust"})).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["name"], "rust");
    }

    // --- delete 边界 ---

    #[tokio::test]
    async fn delete_returns_404_when_not_found() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        let result = TagController::delete(&*repo, 999).await;
        assert_eq!(result["code"], 404);
    }

    #[tokio::test]
    async fn delete_succeeds_when_found() {
        let repo: TagRepo = Arc::new(InMemoryRepository::new());
        repo.save(Tag {
            id: 1,
            name: "rust".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = TagController::delete(&*repo, 1).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["rows"], 1);
    }
}

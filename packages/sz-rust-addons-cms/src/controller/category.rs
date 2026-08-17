use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::category::Category;

pub struct CategoryController;

impl CategoryController {
    pub async fn list<R: Repository<Category, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(kw) = keyword.as_deref() {
            if kw.is_empty() {
                Vec::new()
            } else {
                vec![WhereCondition::new(
                    "name",
                    WhereOp::Like,
                    OrmValue::String(kw.to_string()),
                )]
            }
        } else {
            Vec::new()
        };

        match repo.paginate_by(&conditions, page, page_size) {
            Ok(pr) => json!({
                "code": 0, "msg": "ok",
                "data": {"list": pr.items, "total": pr.total, "page": pr.page, "page_size": pr.page_size}
            }),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Category, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut category: Category = match serde_json::from_value(body) {
            Ok(c) => c,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if category.name.is_empty() {
            return json!({"code": 400, "msg": "name is required", "data": null});
        }
        category.id = 0;
        match repo.save(category) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Category, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(c)) => json!({"code": 0, "msg": "ok", "data": c}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Category, Key = OrmValue>>(repo: &R, id: i64) -> Value {
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

    type CategoryRepo = Arc<InMemoryRepository<Category>>;

    // --- list 边界 ---

    #[tokio::test]
    async fn list_with_empty_keyword_returns_all() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        repo.save(Category {
            id: 1,
            name: "Tech".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = CategoryController::list(&*repo, 1, 20, Some(String::new())).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn list_with_keyword_filters_by_name() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        repo.save(Category {
            id: 1,
            name: "Tech".to_string(),
            ..Default::default()
        })
        .unwrap();
        repo.save(Category {
            id: 2,
            name: "News".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = CategoryController::list(&*repo, 1, 20, Some("Tech".to_string())).await;
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn list_with_pagination() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        for i in 1..=3 {
            repo.save(Category {
                id: i,
                name: format!("C{i}"),
                ..Default::default()
            })
            .unwrap();
        }
        let result = CategoryController::list(&*repo, 1, 2, None).await;
        assert_eq!(result["data"]["total"], 3);
        assert_eq!(result["data"]["page_size"], 2);
    }

    // --- create 边界 ---

    #[tokio::test]
    async fn create_returns_400_on_deserialize_failure() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        // name 字段类型不匹配
        let result = CategoryController::create(&*repo, json!({"name": 123})).await;
        assert_eq!(result["code"], 400);
    }

    #[tokio::test]
    async fn create_rejects_empty_name() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        let result = CategoryController::create(&*repo, json!({"id": 0, "name": ""})).await;
        assert_eq!(result["code"], 400);
        assert_eq!(result["msg"], "name is required");
    }

    #[tokio::test]
    async fn create_succeeds_with_valid_name() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        let result = CategoryController::create(&*repo, json!({"id": 0, "name": "Tech"})).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["name"], "Tech");
    }

    // --- get 边界 ---

    #[tokio::test]
    async fn get_returns_404_when_not_found() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        let result = CategoryController::get(&*repo, 999).await;
        assert_eq!(result["code"], 404);
    }

    #[tokio::test]
    async fn get_returns_category_when_found() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        repo.save(Category {
            id: 1,
            name: "Tech".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = CategoryController::get(&*repo, 1).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["name"], "Tech");
    }

    // --- delete 边界 ---

    #[tokio::test]
    async fn delete_returns_404_when_not_found() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        let result = CategoryController::delete(&*repo, 999).await;
        assert_eq!(result["code"], 404);
    }

    #[tokio::test]
    async fn delete_succeeds_when_found() {
        let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
        repo.save(Category {
            id: 1,
            name: "Tech".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = CategoryController::delete(&*repo, 1).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["rows"], 1);
    }
}

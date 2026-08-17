use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::article::Article;

/// CMS Article 合法状态流转表。
///
/// 规则（对齐 design.md 2.1.3）：
/// - draft → published（发布）
/// - draft → archived（直接下架）
/// - published → archived（下架）
///
/// archived 为终态，不可再流转。
pub const ALLOWED_TRANSITIONS: &[(&str, &str)] = &[
    ("draft", "published"),
    ("draft", "archived"),
    ("published", "archived"),
];

/// 校验状态流转是否合法。
pub fn is_valid_transition(from: &str, to: &str) -> bool {
    ALLOWED_TRANSITIONS
        .iter()
        .any(|(f, t)| *f == from && *t == to)
}

pub struct ArticleController;

impl ArticleController {
    pub async fn list<R: Repository<Article, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
        category_id: Option<i64>,
        status: Option<String>,
    ) -> Value {
        let mut conditions: Vec<WhereCondition> = Vec::new();
        if let Some(kw) = keyword.as_deref() {
            if !kw.is_empty() {
                conditions.push(WhereCondition::new(
                    "title",
                    WhereOp::Like,
                    OrmValue::String(kw.to_string()),
                ));
            }
        }
        if let Some(cid) = category_id {
            if cid > 0 {
                conditions.push(WhereCondition::new(
                    "category_id",
                    WhereOp::Eq,
                    OrmValue::I64(cid),
                ));
            }
        }
        if let Some(st) = status.as_deref() {
            if !st.is_empty() {
                conditions.push(WhereCondition::new(
                    "status",
                    WhereOp::Eq,
                    OrmValue::String(st.to_string()),
                ));
            }
        }

        match repo.paginate_by(&conditions, page, page_size) {
            Ok(pr) => json!({
                "code": 0, "msg": "ok",
                "data": {"list": pr.items, "total": pr.total, "page": pr.page, "page_size": pr.page_size}
            }),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    /// 发布文章：校验当前 status == "draft"，更新为 "published"。
    pub async fn publish<R: Repository<Article, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        let key = OrmValue::I64(id);
        let mut article = match repo.find_by_id(&key) {
            Ok(Some(a)) => a,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if !is_valid_transition(&article.status, "published") {
            return json!({
                "code": 422,
                "msg": format!("文章当前状态为 {}，不可直接发布", article.status),
                "data": null
            });
        }
        article.status = "published".to_string();
        article.updated_at = chrono::Utc::now().timestamp();
        match repo.save(article) {
            Ok(saved) => json!({"code": 0, "msg": "published", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    /// 归档文章：校验当前 status ∈ {"draft", "published"}，更新为 "archived"。
    pub async fn archive<R: Repository<Article, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        let key = OrmValue::I64(id);
        let mut article = match repo.find_by_id(&key) {
            Ok(Some(a)) => a,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if !is_valid_transition(&article.status, "archived") {
            return json!({
                "code": 422,
                "msg": format!("文章当前状态为 {}，不可归档", article.status),
                "data": null
            });
        }
        article.status = "archived".to_string();
        article.updated_at = chrono::Utc::now().timestamp();
        match repo.save(article) {
            Ok(saved) => json!({"code": 0, "msg": "archived", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Article, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut article: Article = match serde_json::from_value(body) {
            Ok(a) => a,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if article.title.is_empty() {
            return json!({"code": 400, "msg": "title is required", "data": null});
        }
        if article.status.is_empty() {
            article.status = "draft".to_string();
        }
        article.id = 0;
        match repo.save(article) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Article, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(a)) => json!({"code": 0, "msg": "ok", "data": a}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<Article, Key = OrmValue>>(
        repo: &R,
        id: i64,
        body: Value,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut article = match repo.find_by_id(&key) {
            Ok(Some(a)) => a,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(obj) = body.as_object() {
            macro_rules! patch {
                ($field:ident, str) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(s) = v.as_str() {
                            article.$field = s.to_string();
                        }
                    }
                };
                ($field:ident, i64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_i64() {
                            article.$field = n;
                        }
                    }
                };
            }
            patch!(title, str);
            patch!(content, str);
            patch!(summary, str);
            patch!(category_id, i64);
            patch!(author_id, i64);
            patch!(status, str);
        }
        match repo.save(article) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Article, Key = OrmValue>>(repo: &R, id: i64) -> Value {
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

    type ArticleRepo = Arc<InMemoryRepository<Article>>;

    fn repo_with_one(id: i64, status: &str) -> ArticleRepo {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        repo.save(Article {
            id,
            title: format!("Article {id}"),
            status: status.to_string(),
            ..Default::default()
        })
        .unwrap();
        repo
    }

    // --- 状态机校验 ---

    #[test]
    fn is_valid_transition_allows_draft_to_published() {
        assert!(is_valid_transition("draft", "published"));
    }

    #[test]
    fn is_valid_transition_allows_draft_to_archived() {
        assert!(is_valid_transition("draft", "archived"));
    }

    #[test]
    fn is_valid_transition_allows_published_to_archived() {
        assert!(is_valid_transition("published", "archived"));
    }

    #[test]
    fn is_valid_transition_rejects_archived_to_anything() {
        assert!(!is_valid_transition("archived", "draft"));
        assert!(!is_valid_transition("archived", "published"));
        assert!(!is_valid_transition("archived", "archived"));
    }

    #[test]
    fn is_valid_transition_rejects_published_to_draft() {
        assert!(!is_valid_transition("published", "draft"));
    }

    // --- publish 边界 ---

    #[tokio::test]
    async fn publish_returns_404_when_not_found() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        let result = ArticleController::publish(&*repo, 999).await;
        assert_eq!(result["code"], 404);
        assert_eq!(result["msg"], "not found");
    }

    #[tokio::test]
    async fn publish_rejects_already_published() {
        let repo = repo_with_one(1, "published");
        let result = ArticleController::publish(&*repo, 1).await;
        assert_eq!(result["code"], 422);
        assert!(result["msg"].as_str().unwrap().contains("不可直接发布"));
    }

    // --- archive 边界 ---

    #[tokio::test]
    async fn archive_draft_to_archived_succeeds() {
        let repo = repo_with_one(1, "draft");
        let result = ArticleController::archive(&*repo, 1).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["status"], "archived");
    }

    #[tokio::test]
    async fn archive_published_to_archived_succeeds() {
        let repo = repo_with_one(1, "published");
        let result = ArticleController::archive(&*repo, 1).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["status"], "archived");
    }

    #[tokio::test]
    async fn archive_returns_404_when_not_found() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        let result = ArticleController::archive(&*repo, 999).await;
        assert_eq!(result["code"], 404);
    }

    #[tokio::test]
    async fn archive_rejects_already_archived() {
        let repo = repo_with_one(1, "archived");
        let result = ArticleController::archive(&*repo, 1).await;
        assert_eq!(result["code"], 422);
        assert!(result["msg"].as_str().unwrap().contains("不可归档"));
    }

    // --- get 边界 ---

    #[tokio::test]
    async fn get_returns_404_when_not_found() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        let result = ArticleController::get(&*repo, 999).await;
        assert_eq!(result["code"], 404);
    }

    #[tokio::test]
    async fn get_returns_article_when_found() {
        let repo = repo_with_one(1, "draft");
        let result = ArticleController::get(&*repo, 1).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["id"], 1);
    }

    // --- update 边界 ---

    #[tokio::test]
    async fn update_returns_404_when_not_found() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        let result = ArticleController::update(&*repo, 999, json!({"title": "X"})).await;
        assert_eq!(result["code"], 404);
    }

    #[tokio::test]
    async fn update_patches_all_fields() {
        let repo = repo_with_one(1, "draft");
        let result = ArticleController::update(
            &*repo,
            1,
            json!({
                "title": "Updated",
                "content": "Content",
                "summary": "Summary",
                "category_id": 7,
                "author_id": 3,
                "status": "published"
            }),
        )
        .await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["title"], "Updated");
        assert_eq!(result["data"]["content"], "Content");
        assert_eq!(result["data"]["summary"], "Summary");
        assert_eq!(result["data"]["category_id"], 7);
        assert_eq!(result["data"]["author_id"], 3);
        assert_eq!(result["data"]["status"], "published");
    }

    #[tokio::test]
    async fn update_with_non_object_body_keeps_article_unchanged() {
        let repo = repo_with_one(1, "draft");
        let result = ArticleController::update(&*repo, 1, json!("not an object")).await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["title"], "Article 1");
    }

    // --- delete 边界 ---

    #[tokio::test]
    async fn delete_returns_404_when_not_found() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        let result = ArticleController::delete(&*repo, 999).await;
        assert_eq!(result["code"], 404);
    }

    // --- create 边界 ---

    #[tokio::test]
    async fn create_returns_400_on_deserialize_failure() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        // title 字段类型不匹配，反序列化失败
        let result = ArticleController::create(&*repo, json!({"title": 123})).await;
        assert_eq!(result["code"], 400);
    }

    #[tokio::test]
    async fn create_with_explicit_status_keeps_it() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        let result = ArticleController::create(
            &*repo,
            json!({"id": 0, "title": "T", "status": "published"}),
        )
        .await;
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["status"], "published");
    }

    // --- list 边界 ---

    #[tokio::test]
    async fn list_with_empty_keyword_string_ignores_filter() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        repo.save(Article {
            id: 1,
            title: "A".to_string(),
            status: "draft".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = ArticleController::list(&*repo, 1, 20, Some(String::new()), None, None).await;
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn list_with_zero_category_id_ignores_filter() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        repo.save(Article {
            id: 1,
            title: "A".to_string(),
            status: "draft".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = ArticleController::list(&*repo, 1, 20, None, Some(0), None).await;
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn list_with_empty_status_string_ignores_filter() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        repo.save(Article {
            id: 1,
            title: "A".to_string(),
            status: "draft".to_string(),
            ..Default::default()
        })
        .unwrap();
        let result = ArticleController::list(&*repo, 1, 20, None, None, Some(String::new())).await;
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn list_combines_multiple_filters() {
        let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
        repo.save(Article {
            id: 1,
            title: "Rust Intro".to_string(),
            status: "draft".to_string(),
            category_id: 5,
            ..Default::default()
        })
        .unwrap();
        repo.save(Article {
            id: 2,
            title: "Rust Advanced".to_string(),
            status: "published".to_string(),
            category_id: 5,
            ..Default::default()
        })
        .unwrap();
        let result = ArticleController::list(
            &*repo,
            1,
            20,
            Some("%Rust%".to_string()),
            Some(5),
            Some("draft".to_string()),
        )
        .await;
        assert_eq!(result["data"]["total"], 1);
    }
}

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

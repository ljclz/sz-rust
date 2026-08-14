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

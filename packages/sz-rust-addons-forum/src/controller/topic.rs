use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::topic::Topic;

pub struct TopicController;

impl TopicController {
    pub async fn list<R: Repository<Topic, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
        board_id: Option<i64>,
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
        if let Some(bid) = board_id {
            if bid > 0 {
                conditions.push(WhereCondition::new(
                    "board_id",
                    WhereOp::Eq,
                    OrmValue::I64(bid),
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

    pub async fn create<R: Repository<Topic, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut topic: Topic = match serde_json::from_value(body) {
            Ok(t) => t,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if topic.title.is_empty() {
            return json!({"code": 400, "msg": "title is required", "data": null});
        }
        topic.id = 0;
        match repo.save(topic) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Topic, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(t)) => json!({"code": 0, "msg": "ok", "data": t}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Topic, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

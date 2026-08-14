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

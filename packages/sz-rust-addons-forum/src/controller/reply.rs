use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::reply::Reply;

pub struct ReplyController;

impl ReplyController {
    pub async fn list_by_topic<R: Repository<Reply, Key = OrmValue>>(
        repo: &R,
        topic_id: i64,
    ) -> Value {
        let conditions = vec![WhereCondition::new(
            "topic_id",
            WhereOp::Eq,
            OrmValue::I64(topic_id),
        )];
        match repo.paginate_by(&conditions, 1, 10000) {
            Ok(pr) => json!({"code": 0, "msg": "ok", "data": pr.items}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Reply, Key = OrmValue>>(
        repo: &R,
        topic_id: i64,
        body: Value,
    ) -> Value {
        let mut reply: Reply = match serde_json::from_value(body) {
            Ok(r) => r,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if reply.content.is_empty() {
            return json!({"code": 400, "msg": "content is required", "data": null});
        }
        reply.id = 0;
        reply.topic_id = topic_id;
        match repo.save(reply) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

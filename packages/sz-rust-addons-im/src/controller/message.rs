use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::message::Message;

pub struct MessageController;

impl MessageController {
    pub async fn list_by_conversation<R: Repository<Message, Key = OrmValue>>(
        repo: &R,
        conversation_id: i64,
    ) -> Value {
        let conditions = vec![WhereCondition::new(
            "conversation_id",
            WhereOp::Eq,
            OrmValue::I64(conversation_id),
        )];
        match repo.paginate_by(&conditions, 1, 10000) {
            Ok(pr) => json!({"code": 0, "msg": "ok", "data": pr.items}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Message, Key = OrmValue>>(
        repo: &R,
        conversation_id: i64,
        body: Value,
    ) -> Value {
        let mut msg: Message = match serde_json::from_value(body) {
            Ok(m) => m,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if msg.content.is_empty() {
            return json!({"code": 400, "msg": "content is required", "data": null});
        }
        if msg.msg_type.is_empty() {
            msg.msg_type = "text".to_string();
        }
        msg.id = 0;
        msg.conversation_id = conversation_id;
        match repo.save(msg) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

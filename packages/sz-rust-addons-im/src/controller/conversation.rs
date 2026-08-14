use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::conversation::Conversation;

pub struct ConversationController;

impl ConversationController {
    pub async fn list<R: Repository<Conversation, Key = OrmValue>>(
        repo: &R,
        user_id: Option<i64>,
        page: u64,
        page_size: u64,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(uid) = user_id {
            vec![WhereCondition::new(
                "user1_id",
                WhereOp::Eq,
                OrmValue::I64(uid),
            )]
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

    pub async fn create<R: Repository<Conversation, Key = OrmValue>>(
        repo: &R,
        body: Value,
    ) -> Value {
        let mut conv: Conversation = match serde_json::from_value(body) {
            Ok(c) => c,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if conv.user1_id == 0 || conv.user2_id == 0 {
            return json!({"code": 400, "msg": "user1_id and user2_id are required", "data": null});
        }
        conv.id = 0;
        match repo.save(conv) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

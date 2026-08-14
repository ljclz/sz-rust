use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::user_status::UserStatus;

pub struct UserStatusController;

impl UserStatusController {
    pub async fn get<R: Repository<UserStatus, Key = OrmValue>>(repo: &R, user_id: i64) -> Value {
        let conditions = vec![WhereCondition::new(
            "user_id",
            WhereOp::Eq,
            OrmValue::I64(user_id),
        )];
        match repo.paginate_by(&conditions, 1, 1) {
            Ok(pr) if !pr.items.is_empty() => json!({"code": 0, "msg": "ok", "data": pr.items[0]}),
            Ok(_) => {
                json!({"code": 0, "msg": "ok", "data": {"user_id": user_id, "is_online": false, "last_seen": 0, "device_type": ""}})
            }
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<UserStatus, Key = OrmValue>>(
        repo: &R,
        user_id: i64,
        body: Value,
    ) -> Value {
        let conditions = vec![WhereCondition::new(
            "user_id",
            WhereOp::Eq,
            OrmValue::I64(user_id),
        )];
        match repo.paginate_by(&conditions, 1, 1) {
            Ok(pr) if !pr.items.is_empty() => {
                let mut status = pr.items[0].clone();
                if let Some(obj) = body.as_object() {
                    if let Some(v) = obj.get("is_online") {
                        if let Some(b) = v.as_bool() {
                            status.is_online = b;
                        }
                    }
                    if let Some(v) = obj.get("device_type") {
                        if let Some(s) = v.as_str() {
                            status.device_type = s.to_string();
                        }
                    }
                }
                match repo.save(status) {
                    Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
                    Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
                }
            }
            Ok(_) => {
                let mut status = UserStatus::new();
                status.user_id = user_id;
                if let Some(obj) = body.as_object() {
                    if let Some(v) = obj.get("is_online") {
                        if let Some(b) = v.as_bool() {
                            status.is_online = b;
                        }
                    }
                    if let Some(v) = obj.get("device_type") {
                        if let Some(s) = v.as_str() {
                            status.device_type = s.to_string();
                        }
                    }
                }
                status.id = 0;
                match repo.save(status) {
                    Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
                    Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
                }
            }
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

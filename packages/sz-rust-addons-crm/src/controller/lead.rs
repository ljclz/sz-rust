//! Lead（线索）控制器
//!
//! 业务方法通过 `&dyn Repository` 参数接收仓储，由 `lib.rs` 的薄包装 handler 调用。

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::lead::Lead;

pub struct LeadController;

impl LeadController {
    pub async fn list<R: Repository<Lead, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        status: Option<String>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(s) = status.as_deref() {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![WhereCondition::new(
                    "status",
                    WhereOp::Eq,
                    OrmValue::String(s.to_string()),
                )]
            }
        } else {
            Vec::new()
        };
        match repo.paginate_by(&conditions, page, page_size) {
            Ok(pr) => {
                json!({"code": 0, "msg": "ok", "data": {"list": pr.items, "total": pr.total, "page": pr.page, "page_size": pr.page_size}})
            }
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Lead, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut lead: Lead = match serde_json::from_value(body) {
            Ok(l) => l,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if lead.name.is_empty() {
            return json!({"code": 400, "msg": "name 必填", "data": null});
        }
        lead.id = 0;
        match repo.save(lead) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Lead, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(c)) => json!({"code": 0, "msg": "ok", "data": c}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<Lead, Key = OrmValue>>(
        repo: &R,
        id: i64,
        body: Value,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut lead = match repo.find_by_id(&key) {
            Ok(Some(c)) => c,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(obj) = body.as_object() {
            macro_rules! patch {
                ($field:ident, str) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(s) = v.as_str() {
                            lead.$field = s.to_string();
                        }
                    }
                };
                ($field:ident, f64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_f64() {
                            lead.$field = n;
                        }
                    }
                };
                ($field:ident, i64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_i64() {
                            lead.$field = n;
                        }
                    }
                };
            }
            patch!(name, str);
            patch!(status, str);
            patch!(source, str);
            patch!(phone, str);
            patch!(email, str);
            patch!(company, str);
            patch!(estimated_amount, f64);
            patch!(owner_id, i64);
            patch!(remark, str);
        }
        match repo.save(lead) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Lead, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn convert<R: Repository<Lead, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        let key = OrmValue::I64(id);
        let mut lead = match repo.find_by_id(&key) {
            Ok(Some(l)) => l,
            Ok(None) => return json!({"code": 404, "msg": "线索不存在", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        lead.status = "converted".to_string();
        match repo.save(lead) {
            Ok(saved) => json!({
                "code": 0, "msg": "converted",
                "data": {"lead": saved, "note": "模板方法：在此创建关联的 Contact 和 Deal 记录"}
            }),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

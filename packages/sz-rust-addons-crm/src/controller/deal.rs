//! Deal（商机）控制器
//!
//! 业务方法通过 `&dyn Repository` 参数接收仓储，由 `lib.rs` 的薄包装 handler 调用。

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::deal::Deal;

pub struct DealController;

impl DealController {
    pub async fn list<R: Repository<Deal, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        stage: Option<String>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(s) = stage.as_deref() {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![WhereCondition::new(
                    "stage",
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

    pub async fn create<R: Repository<Deal, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut deal: Deal = match serde_json::from_value(body) {
            Ok(d) => d,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if deal.name.is_empty() {
            return json!({"code": 400, "msg": "name 必填", "data": null});
        }
        deal.id = 0;
        match repo.save(deal) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Deal, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(d)) => json!({"code": 0, "msg": "ok", "data": d}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<Deal, Key = OrmValue>>(
        repo: &R,
        id: i64,
        body: Value,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut deal = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(obj) = body.as_object() {
            macro_rules! patch {
                ($field:ident, str) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(s) = v.as_str() {
                            deal.$field = s.to_string();
                        }
                    }
                };
                ($field:ident, f64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_f64() {
                            deal.$field = n;
                        }
                    }
                };
                ($field:ident, i64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_i64() {
                            deal.$field = n;
                        }
                    }
                };
                ($field:ident, u8) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_u64() {
                            deal.$field = n as u8;
                        }
                    }
                };
            }
            patch!(name, str);
            patch!(stage, str);
            patch!(amount, f64);
            patch!(contact_id, i64);
            patch!(lead_id, i64);
            patch!(owner_id, i64);
            patch!(remark, str);
            patch!(probability, u8);
        }
        match repo.save(deal) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Deal, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn pipeline<R: Repository<Deal, Key = OrmValue>>(repo: &R) -> Value {
        let stages = [
            "prospect",
            "qualified",
            "proposal",
            "negotiation",
            "closed_won",
            "closed_lost",
        ];
        let all_deals = match repo.find_all() {
            Ok(deals) => deals,
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        let mut pipeline: Vec<Value> = Vec::new();
        for stage in stages {
            let deals_in_stage: Vec<_> = all_deals.iter().filter(|d| d.stage == stage).collect();
            let total_amount: f64 = deals_in_stage.iter().map(|d| d.amount).sum();
            pipeline.push(json!({
                "stage": stage,
                "count": deals_in_stage.len(),
                "total_amount": total_amount,
            }));
        }
        json!({"code": 0, "msg": "ok", "data": {"pipeline": pipeline}})
    }
}

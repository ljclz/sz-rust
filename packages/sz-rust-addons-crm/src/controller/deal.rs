//! Deal（商机）控制器
//!
//! 业务方法通过 `&dyn Repository` 参数接收仓储，由 `lib.rs` 的薄包装 handler 调用。

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::deal::Deal;

/// CRM Deal 合法阶段流转表（对齐 design.md 2.1.3 + spec 6.4）。
///
/// 6 阶段：initial → requirement_confirmed → quoted → negotiating → won/lost
pub const ALLOWED_TRANSITIONS: &[(&str, &str)] = &[
    ("initial", "requirement_confirmed"),
    ("requirement_confirmed", "quoted"),
    ("quoted", "negotiating"),
    ("negotiating", "won"),
    ("negotiating", "lost"),
];

/// 校验阶段流转是否合法。
pub fn is_valid_transition(from: &str, to: &str) -> bool {
    ALLOWED_TRANSITIONS
        .iter()
        .any(|(f, t)| *f == from && *t == to)
}

/// pipeline 阶段名列表（spec 6.4 定义）。
pub const PIPELINE_STAGES: &[&str] = &[
    "initial",
    "requirement_confirmed",
    "quoted",
    "negotiating",
    "won",
    "lost",
];

pub struct DealController;

impl DealController {
    pub async fn list<R: Repository<Deal, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
        stage: Option<String>,
    ) -> Value {
        let mut conditions: Vec<WhereCondition> = Vec::new();
        if let Some(kw) = keyword.as_deref() {
            if !kw.is_empty() {
                conditions.push(WhereCondition::new(
                    "name",
                    WhereOp::Like,
                    OrmValue::String(kw.to_string()),
                ));
            }
        }
        if let Some(s) = stage.as_deref() {
            if !s.is_empty() {
                conditions.push(WhereCondition::new(
                    "stage",
                    WhereOp::Eq,
                    OrmValue::String(s.to_string()),
                ));
            }
        }
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
        let all_deals = match repo.find_all() {
            Ok(deals) => deals,
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        let mut pipeline: Vec<Value> = Vec::new();
        for stage in PIPELINE_STAGES {
            let deals_in_stage: Vec<_> = all_deals.iter().filter(|d| d.stage == *stage).collect();
            let total_amount: f64 = deals_in_stage.iter().map(|d| d.amount).sum();
            pipeline.push(json!({
                "stage": stage,
                "count": deals_in_stage.len(),
                "total_amount": total_amount,
            }));
        }
        json!({"code": 0, "msg": "ok", "data": {"pipeline": pipeline}})
    }

    /// 阶段流转：校验合法流转表，更新 stage + updated_at。
    ///
    /// 非法流转返回 `ValidationError("商机阶段不可从 {current} 回退至 {new}")`。
    pub async fn update_stage<R: Repository<Deal, Key = OrmValue>>(
        repo: &R,
        id: i64,
        new_stage: &str,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut deal = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if !is_valid_transition(&deal.stage, new_stage) {
            return json!({
                "code": 422,
                "msg": format!("商机阶段不可从 {} 回退至 {}", deal.stage, new_stage),
                "data": null
            });
        }
        deal.stage = new_stage.to_string();
        deal.updated_at = chrono::Utc::now().timestamp();
        match repo.save(deal) {
            Ok(saved) => json!({"code": 0, "msg": "stage_updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

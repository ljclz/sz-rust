//! Order（订单）控制器

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::order::Order;

pub struct OrderController;

impl OrderController {
    pub async fn list<R: Repository<Order, Key = OrmValue>>(
        repo: &R, page: u64, page_size: u64, status: Option<String>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(s) = status.as_deref() {
            if s.is_empty() { Vec::new() }
            else { vec![WhereCondition::new("status", WhereOp::Eq, OrmValue::String(s.to_string()))] }
        } else { Vec::new() };
        match repo.paginate_by(&conditions, page, page_size) {
            Ok(pr) => json!({"code": 0, "msg": "ok", "data": {"list": pr.items, "total": pr.total, "page": pr.page, "page_size": pr.page_size}}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn create<R: Repository<Order, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut order: Order = match serde_json::from_value(body) {
            Ok(d) => d,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if order.user_id == 0 || order.order_no.is_empty() {
            return json!({"code": 400, "msg": "user_id 和 order_no 必填", "data": null});
        }
        order.id = 0;
        match repo.save(order) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Order, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(d)) => json!({"code": 0, "msg": "ok", "data": d}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<Order, Key = OrmValue>>(repo: &R, id: i64, body: Value) -> Value {
        let key = OrmValue::I64(id);
        let mut order = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(obj) = body.as_object() {
            macro_rules! patch {
                ($field:ident, str) => { if let Some(v) = obj.get(stringify!($field)) { if let Some(s) = v.as_str() { order.$field = s.to_string(); } } };
                ($field:ident, f64) => { if let Some(v) = obj.get(stringify!($field)) { if let Some(n) = v.as_f64() { order.$field = n; } } };
                ($field:ident, i64) => { if let Some(v) = obj.get(stringify!($field)) { if let Some(n) = v.as_i64() { order.$field = n; } } };
            }
            patch!(order_no, str); patch!(user_id, i64);
            patch!(total_amount, f64); patch!(paid_amount, f64);
            patch!(status, str); patch!(shipping_address, str); patch!(remark, str);
        }
        match repo.save(order) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Order, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn cancel<R: Repository<Order, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        let key = OrmValue::I64(id);
        let mut order = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if order.status != "pending" && order.status != "paid" {
            return json!({"code": 400, "msg": "仅 pending/paid 状态可取消", "data": null});
        }
        order.status = "cancelled".to_string();
        match repo.save(order) {
            Ok(saved) => json!({"code": 0, "msg": "cancelled", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

//! PurchaseOrder（采购单）控制器

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::purchase_order::PurchaseOrder;

pub struct PurchaseOrderController;

impl PurchaseOrderController {
    pub async fn list<R: Repository<PurchaseOrder, Key = OrmValue>>(
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

    pub async fn create<R: Repository<PurchaseOrder, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut order: PurchaseOrder = match serde_json::from_value(body) {
            Ok(d) => d,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if order.supplier_id == 0 || order.product_id == 0 {
            return json!({"code": 400, "msg": "supplier_id 和 product_id 必填", "data": null});
        }
        order.id = 0;
        match repo.save(order) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<PurchaseOrder, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(d)) => json!({"code": 0, "msg": "ok", "data": d}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<PurchaseOrder, Key = OrmValue>>(repo: &R, id: i64, body: Value) -> Value {
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
            patch!(supplier_id, i64); patch!(product_id, i64);
            patch!(quantity, i64); patch!(unit_price, f64); patch!(total_amount, f64);
            patch!(status, str); patch!(order_date, i64); patch!(remark, str);
        }
        match repo.save(order) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<PurchaseOrder, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn approve<R: Repository<PurchaseOrder, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        let key = OrmValue::I64(id);
        let mut order = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if order.status != "pending" {
            return json!({"code": 400, "msg": "仅 pending 状态的采购单可审批", "data": null});
        }
        order.status = "approved".to_string();
        match repo.save(order) {
            Ok(saved) => json!({"code": 0, "msg": "approved", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

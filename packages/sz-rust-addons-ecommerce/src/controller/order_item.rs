//! OrderItem（订单项）控制器

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::order_item::OrderItem;

pub struct OrderItemController;

impl OrderItemController {
    pub async fn list<R: Repository<OrderItem, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        order_id: Option<i64>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(oid) = order_id {
            vec![WhereCondition::new(
                "order_id",
                WhereOp::Eq,
                OrmValue::I64(oid),
            )]
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

    pub async fn create<R: Repository<OrderItem, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut item: OrderItem = match serde_json::from_value(body) {
            Ok(d) => d,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if item.order_id == 0 || item.product_id == 0 {
            return json!({"code": 400, "msg": "order_id 和 product_id 必填", "data": null});
        }
        item.id = 0;
        match repo.save(item) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<OrderItem, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

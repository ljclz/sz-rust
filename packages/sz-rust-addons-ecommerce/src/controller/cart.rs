//! Cart（购物车）控制器

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::cart::CartItem;

pub struct CartController;

impl CartController {
    pub async fn list<R: Repository<CartItem, Key = OrmValue>>(repo: &R, user_id: i64) -> Value {
        let conditions = vec![WhereCondition::new(
            "user_id",
            WhereOp::Eq,
            OrmValue::I64(user_id),
        )];
        match repo.paginate_by(&conditions, 1, 1000) {
            Ok(pr) => {
                json!({"code": 0, "msg": "ok", "data": {"list": pr.items, "total_count": pr.total}})
            }
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn add<R: Repository<CartItem, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let new_item: CartItem = match serde_json::from_value(body) {
            Ok(d) => d,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if new_item.user_id == 0 || new_item.product_id == 0 {
            return json!({"code": 400, "msg": "user_id 和 product_id 必填", "data": null});
        }
        if new_item.quantity <= 0 {
            return json!({"code": 400, "msg": "商品数量必须大于 0", "data": null});
        }
        let conditions = vec![
            WhereCondition::new("user_id", WhereOp::Eq, OrmValue::I64(new_item.user_id)),
            WhereCondition::new(
                "product_id",
                WhereOp::Eq,
                OrmValue::I64(new_item.product_id),
            ),
        ];
        match repo.paginate_by(&conditions, 1, 1) {
            Ok(pr) => {
                if let Some(mut existing) = pr.items.into_iter().next() {
                    existing.quantity += new_item.quantity;
                    existing.updated_at = new_item.updated_at;
                    match repo.save(existing) {
                        Ok(saved) => json!({"code": 0, "msg": "merged", "data": saved}),
                        Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
                    }
                } else {
                    let mut item = new_item;
                    item.id = 0;
                    match repo.save(item) {
                        Ok(saved) => json!({"code": 0, "msg": "added", "data": saved}),
                        Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
                    }
                }
            }
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update_qty<R: Repository<CartItem, Key = OrmValue>>(
        repo: &R,
        id: i64,
        quantity: i64,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut item = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        item.quantity = quantity;
        match repo.save(item) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<CartItem, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn clear<R: Repository<CartItem, Key = OrmValue>>(repo: &R, user_id: i64) -> Value {
        let conditions = vec![WhereCondition::new(
            "user_id",
            WhereOp::Eq,
            OrmValue::I64(user_id),
        )];
        match repo.delete_by(&conditions) {
            Ok(n) => json!({"code": 0, "msg": "cleared", "data": {"rows": n}}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

//! Supplier（供应商）控制器

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::supplier::Supplier;

pub struct SupplierController;

impl SupplierController {
    pub async fn list<R: Repository<Supplier, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(k) = keyword.as_deref() {
            if k.is_empty() {
                Vec::new()
            } else {
                vec![WhereCondition::new(
                    "name",
                    WhereOp::Like,
                    OrmValue::String(format!("%{}%", k)),
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

    pub async fn create<R: Repository<Supplier, Key = OrmValue>>(repo: &R, body: Value) -> Value {
        let mut supplier: Supplier = match serde_json::from_value(body) {
            Ok(d) => d,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if supplier.name.is_empty() {
            return json!({"code": 400, "msg": "name 必填", "data": null});
        }
        supplier.id = 0;
        match repo.save(supplier) {
            Ok(saved) => json!({"code": 0, "msg": "created", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn get<R: Repository<Supplier, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(d)) => json!({"code": 0, "msg": "ok", "data": d}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn update<R: Repository<Supplier, Key = OrmValue>>(
        repo: &R,
        id: i64,
        body: Value,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut supplier = match repo.find_by_id(&key) {
            Ok(Some(d)) => d,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(obj) = body.as_object() {
            macro_rules! patch {
                ($field:ident, str) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(s) = v.as_str() {
                            supplier.$field = s.to_string();
                        }
                    }
                };
                ($field:ident, i64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_i64() {
                            supplier.$field = n;
                        }
                    }
                };
                ($field:ident, u8) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_u64() {
                            supplier.$field = n as u8;
                        }
                    }
                };
            }
            patch!(name, str);
            patch!(contact, str);
            patch!(phone, str);
            patch!(email, str);
            patch!(address, str);
            patch!(credit_level, u8);
            patch!(remark, str);
        }
        match repo.save(supplier) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    pub async fn delete<R: Repository<Supplier, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

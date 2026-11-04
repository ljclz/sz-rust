// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Contact（联系人）控制器
//!
//! 业务方法通过 `&dyn Repository` 参数接收仓储，由 `lib.rs` 的薄包装 handler 调用。
//! 此设计避免 trait object 出现在 axum State 中（trait object 无法满足 Handler 泛型推断）。

use std::sync::Arc;

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;
use sz_rust_core::plugin::InMemoryEventBus;

use crate::event_publisher;
use crate::interop::{validate_owner_exists, SharedUserRepo};
use crate::model::contact::Contact;

/// Contact 业务控制器
pub struct ContactController;

impl ContactController {
    /// 联系人列表（分页）
    pub async fn list<R: Repository<Contact, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
    ) -> Value {
        let conditions: Vec<WhereCondition> = if let Some(kw) = keyword.as_deref() {
            if kw.is_empty() {
                Vec::new()
            } else {
                vec![WhereCondition::new(
                    "name",
                    WhereOp::Like,
                    OrmValue::String(kw.to_string()),
                )]
            }
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

    /// 创建联系人
    ///
    /// - `shared_users` 为 `Some` 且 `owner_id != 0` 时校验 owner 存在且同租户。
    /// - `event_bus` 为 `Some` 时持久化成功后发布 `crm.contact.created` 事件。
    pub async fn create<R: Repository<Contact, Key = OrmValue>>(
        repo: &R,
        body: Value,
        shared_users: Option<&SharedUserRepo>,
        event_bus: Option<&Arc<InMemoryEventBus>>,
        tenant_id: i64,
    ) -> Value {
        let mut contact: Contact = match serde_json::from_value(body) {
            Ok(c) => c,
            Err(e) => return json!({"code": 400, "msg": e.to_string(), "data": null}),
        };
        if contact.name.is_empty() {
            return json!({"code": 400, "msg": "name 必填", "data": null});
        }
        if let Some(repo_users) = shared_users {
            if let Err(err) = validate_owner_exists(repo_users, tenant_id, contact.owner_id).await {
                return json!({"code": err["code"], "msg": err["msg"], "data": null});
            }
        }
        contact.id = 0;
        let saved = match repo.save(contact) {
            Ok(s) => s,
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(bus) = event_bus {
            let _ = event_publisher::publish_contact_created(
                bus.as_ref(),
                tenant_id,
                saved.id,
                saved.owner_id,
            )
            .await;
        }
        json!({"code": 0, "msg": "created", "data": saved})
    }

    /// 获取单个联系人
    pub async fn get<R: Repository<Contact, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.find_by_id(&OrmValue::I64(id)) {
            Ok(Some(c)) => json!({"code": 0, "msg": "ok", "data": c}),
            Ok(None) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    /// 更新联系人（部分更新）
    pub async fn update<R: Repository<Contact, Key = OrmValue>>(
        repo: &R,
        id: i64,
        body: Value,
    ) -> Value {
        let key = OrmValue::I64(id);
        let mut contact = match repo.find_by_id(&key) {
            Ok(Some(c)) => c,
            Ok(None) => return json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if let Some(obj) = body.as_object() {
            macro_rules! patch {
                ($field:ident, str) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(s) = v.as_str() {
                            contact.$field = s.to_string();
                        }
                    }
                };
                ($field:ident, i64) => {
                    if let Some(v) = obj.get(stringify!($field)) {
                        if let Some(n) = v.as_i64() {
                            contact.$field = n;
                        }
                    }
                };
            }
            patch!(name, str);
            patch!(phone, str);
            patch!(email, str);
            patch!(customer_id, i64);
            patch!(owner_id, i64);
            patch!(position, str);
            patch!(remark, str);
        }
        match repo.save(contact) {
            Ok(saved) => json!({"code": 0, "msg": "updated", "data": saved}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }

    /// 删除联系人
    pub async fn delete<R: Repository<Contact, Key = OrmValue>>(repo: &R, id: i64) -> Value {
        match repo.delete(&OrmValue::I64(id)) {
            Ok(n) if n > 0 => json!({"code": 0, "msg": "deleted", "data": {"rows": n}}),
            Ok(_) => json!({"code": 404, "msg": "not found", "data": null}),
            Err(e) => json!({"code": 500, "msg": e.to_string(), "data": null}),
        }
    }
}

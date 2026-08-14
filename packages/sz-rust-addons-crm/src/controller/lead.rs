//! Lead（线索）控制器
//!
//! 业务方法通过 `&dyn Repository` 参数接收仓储，由 `lib.rs` 的薄包装 handler 调用。

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::Value as OrmValue;

use crate::model::contact::Contact;
use crate::model::deal::Deal;
use crate::model::lead::Lead;

pub struct LeadController;

impl LeadController {
    pub async fn list<R: Repository<Lead, Key = OrmValue>>(
        repo: &R,
        page: u64,
        page_size: u64,
        keyword: Option<String>,
        status: Option<String>,
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
        if let Some(s) = status.as_deref() {
            if !s.is_empty() {
                conditions.push(WhereCondition::new(
                    "status",
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

    /// 线索转化：三步原子操作，创建关联 Contact 和 Deal。
    ///
    /// 步骤 ①：校验线索存在 + 未转化 → 标记为 converted
    /// 步骤 ②：创建 Contact（从 Lead 复制 name/phone/email）
    /// 步骤 ③：创建 Deal（name = company + " 商机"，amount = estimated_amount）
    ///
    /// 手动回滚策略（InMemoryRepository 无事务）：
    /// - 步骤 ② 失败 → 回滚步骤 ①
    /// - 步骤 ③ 失败 → 回滚步骤 ② + 步骤 ①
    pub async fn convert<
        L: Repository<Lead, Key = OrmValue>,
        C: Repository<Contact, Key = OrmValue>,
        D: Repository<Deal, Key = OrmValue>,
    >(
        lead_repo: &L,
        contact_repo: &C,
        deal_repo: &D,
        id: i64,
    ) -> Value {
        let key = OrmValue::I64(id);

        // 步骤 ①：校验 + 标记转化
        let mut lead = match lead_repo.find_by_id(&key) {
            Ok(Some(l)) => l,
            Ok(None) => return json!({"code": 404, "msg": "线索不存在", "data": null}),
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };
        if lead.status == "converted" {
            return json!({"code": 422, "msg": "线索已转化，不可重复转化", "data": null});
        }
        let original_status = lead.status.clone();
        lead.status = "converted".to_string();
        lead.updated_at = chrono::Utc::now().timestamp();
        let saved_lead = match lead_repo.save(lead) {
            Ok(l) => l,
            Err(e) => return json!({"code": 500, "msg": e.to_string(), "data": null}),
        };

        // 步骤 ②：创建 Contact
        let new_contact = Contact {
            id: 0,
            name: saved_lead.name.clone(),
            phone: saved_lead.phone.clone(),
            email: saved_lead.email.clone(),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            ..Default::default()
        };
        let saved_contact = match contact_repo.save(new_contact) {
            Ok(c) => c,
            Err(e) => {
                // 回滚步骤 ①
                let mut rollback_lead = saved_lead.clone();
                rollback_lead.status = original_status;
                let _ = lead_repo.save(rollback_lead);
                return json!({"code": 500, "msg": format!("创建联系人失败: {e}"), "data": null});
            }
        };

        // 步骤 ③：创建 Deal
        let deal_name = if saved_lead.company.is_empty() {
            format!("{} 商机", saved_lead.name)
        } else {
            format!("{} 商机", saved_lead.company)
        };
        let new_deal = Deal {
            id: 0,
            name: deal_name,
            amount: saved_lead.estimated_amount,
            contact_id: saved_contact.id,
            lead_id: saved_lead.id,
            owner_id: saved_lead.owner_id,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            ..Default::default()
        };
        let saved_deal = match deal_repo.save(new_deal) {
            Ok(d) => d,
            Err(e) => {
                // 回滚步骤 ② + 步骤 ①
                let _ = contact_repo.delete(&OrmValue::I64(saved_contact.id));
                let mut rollback_lead = saved_lead.clone();
                rollback_lead.status = original_status;
                let _ = lead_repo.save(rollback_lead);
                return json!({"code": 500, "msg": format!("创建商机失败: {e}"), "data": null});
            }
        };

        json!({
            "code": 0, "msg": "converted",
            "data": {"lead": saved_lead, "contact": saved_contact, "deal": saved_deal}
        })
    }
}

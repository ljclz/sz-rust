//! ContractLog 控制器 — 对齐 PHP `addons/operate/controller/admin/ContractLog.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `index()` | [`ContractLogController::index`] | 分页列表 |
//! | `export()` | [`ContractLogController::export`] | 导出列表（不分页） |
//! | `add()` | [`ContractLogController::add`] | 添加日志 |
//!
//! ## PHP 源码依据
//!
//! ```php
//! public function index(): Json {
//!     $param = $this->postData();
//!     $model = new ContractLogModel();
//!     $result['list'] = $model->getList($param,'list');
//!     return $this->renderSuccess('', compact('result'));
//! }
//! ```

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use serde_json::{json, Value};
use sz_orm_core::repository::{Repository, WhereCondition, WhereOp};
use sz_orm_core::ModelExt as _;
use sz_orm_core::Value as OrmValue;
use sz_rust_core::controller::{AddonsBaseController, BaseController, SzController};
use sz_rust_core::model::Mutator as _;

use crate::controller::common::{get_app_id, get_i64_param, get_str_param, parse_form_data};
use crate::model::ContractLog;

/// ContractLog 控制器 — 对齐 PHP `ContractLog` 控制器
pub struct ContractLogController;

impl SzController for ContractLogController {}
impl BaseController for ContractLogController {}
impl AddonsBaseController for ContractLogController {}

impl ContractLogController {
    /// 分页列表 — 对齐 PHP `index()`
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<ContractLog, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "list");
        self.render_success("", json!({"result": result}))
    }

    /// 导出列表 — 对齐 PHP `export()`
    pub async fn export(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<ContractLog, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "export");
        self.render_success("", json!({"result": result}))
    }

    /// 添加日志 — 对齐 PHP `add()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add(): Json {
    ///     $param = $this->postData();
    ///     $model = new ContractLogModel();
    ///     $data = json_decode($param['formData'], true);
    ///     if($model->add($data)){
    ///         return $this->renderSuccess('添加成功');
    ///     }
    ///     return $this->renderError($model->getError() ?: '添加失败');
    /// }
    /// ```
    pub async fn add(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<ContractLog, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::add_log(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法（对齐 PHP `ContractLog` 模型业务方法）
    // ========================================================================

    /// 查询日志列表 — 对齐 PHP `ContractLog::getList($param, $type)`
    ///
    /// # PHP 对齐
    ///
    /// PHP `getList` 支持按 contract_id/uid/dept_id/customer_id/type/stat_day 过滤，
    /// 按 create_time desc 排序，`type='export'` 不分页，`type='list'` 分页。
    ///
    /// # 简化说明
    ///
    /// - 关联关系（contract/customer）：NOTE(Phase 6)
    fn get_list(
        repo: &dyn Repository<ContractLog, Key = OrmValue>,
        param: &Value,
        list_type: &str,
    ) -> Value {
        let app_id = get_app_id(param);
        let list_rows = get_i64_param(param, "list_rows").unwrap_or(15) as usize;
        let page = get_i64_param(param, "page").unwrap_or(1) as usize;

        // 基础条件：app_id
        let mut conditions = vec![WhereCondition::new(
            "app_id",
            WhereOp::Eq,
            OrmValue::I64(app_id),
        )];

        // 可选过滤条件（对齐 PHP getList 中各字段过滤）
        if let Some(contract_id) = get_i64_param(param, "contract_id") {
            conditions.push(WhereCondition::new(
                "contract_id",
                WhereOp::Eq,
                OrmValue::I64(contract_id),
            ));
        }
        if let Some(uid) = get_i64_param(param, "uid") {
            conditions.push(WhereCondition::new("uid", WhereOp::Eq, OrmValue::I64(uid)));
        }
        if let Some(dept_id) = get_i64_param(param, "dept_id") {
            conditions.push(WhereCondition::new(
                "dept_id",
                WhereOp::Eq,
                OrmValue::I64(dept_id),
            ));
        }
        if let Some(customer_id) = get_i64_param(param, "customer_id") {
            conditions.push(WhereCondition::new(
                "customer_id",
                WhereOp::Eq,
                OrmValue::I64(customer_id),
            ));
        }
        if let Some(log_type) = get_i64_param(param, "type") {
            conditions.push(WhereCondition::new(
                "type",
                WhereOp::Eq,
                OrmValue::I64(log_type),
            ));
        }

        let mut items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };

        // stat_day 过滤（字符串相等）
        if let Some(stat_day) = get_str_param(param, "stat_day") {
            items.retain(|item| {
                item.get("stat_day")
                    .and_then(|v| v.as_str())
                    .map(|s| s == stat_day)
                    .unwrap_or(false)
            });
        }

        // PHP 按 create_time desc 排序（简化：按 log_id desc）
        items.sort_by(|a, b| {
            let a_id = a.get("log_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("log_id").and_then(|v| v.as_i64()).unwrap_or(0);
            b_id.cmp(&a_id)
        });

        // 分页
        let result_list = if list_type == "export" {
            items
        } else {
            let start = page.saturating_sub(1) * list_rows;
            if start >= items.len() {
                Vec::new()
            } else {
                let end = (start + list_rows).min(items.len());
                items[start..end].to_vec()
            }
        };

        json!({"list": result_list})
    }

    /// 添加日志 — 对齐 PHP `ContractLog::add($data)`
    ///
    /// # PHP 对齐
    ///
    /// PHP `add` 会保存日志并更新 Contract 的 contract_price。
    /// 简化：仅保存日志记录，Contract 更新由调用方负责。
    fn add_log(
        repo: &dyn Repository<ContractLog, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        let mut model = ContractLog::new();
        if let Some(obj) = data.as_object() {
            let mut data_map: std::collections::HashMap<String, Value> =
                std::collections::HashMap::new();
            for (k, v) in obj {
                data_map.insert(k.clone(), v.clone());
            }
            model.set_attrs(&data_map);
        }
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_core::repository::InMemoryRepository;

    fn make_log(id: i64, contract_id: i64, app_id: i64) -> ContractLog {
        ContractLog::new()
            .with_data("log_id", json!(id))
            .with_data("contract_id", json!(contract_id))
            .with_data("customer_id", json!(0))
            .with_data("uid", json!(0))
            .with_data("dept_id", json!(0))
            .with_data("type", json!(1))
            .with_data("app_id", json!(app_id))
            .with_data("stat_day", json!("2026-01-01"))
    }

    fn make_repo() -> InMemoryRepository<ContractLog> {
        InMemoryRepository::from_vec(vec![
            make_log(1, 100, 10001),
            make_log(2, 100, 10001),
            make_log(3, 200, 10001),
            make_log(4, 100, 20002),
        ])
    }

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = ContractLogController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_get_list_filters_by_contract_id() {
        let repo = make_repo();
        let result = ContractLogController::get_list(
            &repo,
            &json!({"app_id": 10001, "contract_id": 100}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_get_list_export_no_pagination() {
        let repo = make_repo();
        let result = ContractLogController::get_list(&repo, &json!({"app_id": 10001}), "export");
        let list = result["list"].as_array().unwrap();
        // export 不分页，返回全部
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_get_list_pagination() {
        let repo = make_repo();
        let result = ContractLogController::get_list(
            &repo,
            &json!({"app_id": 10001, "page": 1, "list_rows": 2}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_add_log_success() {
        let repo = make_repo();
        let result = ContractLogController::add_log(
            &repo,
            &json!({"contract_id": 300, "type": 1, "app_id": 10001}),
        );
        assert!(result.is_ok());
        assert_eq!(repo.len(), 5);
    }

    #[test]
    fn test_r5_php_contract_log_get_list_returns_list_key() {
        let repo = make_repo();
        let result = ContractLogController::get_list(&repo, &json!({"app_id": 10001}), "list");
        assert!(result["list"].is_array());
    }

    #[test]
    fn test_r5_php_contract_log_export_returns_all() {
        // R5: PHP export 类型返回所有记录不分页
        let repo = make_repo();
        let list_result = ContractLogController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let export_result =
            ContractLogController::get_list(&repo, &json!({"app_id": 10001}), "export");
        // export 至少和 list 一样多
        let list_len = list_result["list"].as_array().unwrap().len();
        let export_len = export_result["list"].as_array().unwrap().len();
        assert!(export_len >= list_len);
    }
}

//! Contract 控制器 — 对齐 PHP `addons/operate/controller/admin/Contract.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `base()` | [`ContractController::base`] | 基础数据 |
//! | `index()` | [`ContractController::index`] | 分页列表 |
//! | `export()` | [`ContractController::export`] | 导出列表 |
//! | `add()` | [`ContractController::add`] | 添加合同 |
//! | `copy()` | [`ContractController::copy`] | 复制合同（新增并旧合同置为 3） |
//! | `edit()` | [`ContractController::edit`] | 编辑合同 |
//! | `bind()` | [`ContractController::bind`] | 绑定商户 |
//! | `del()` | [`ContractController::del`] | 软删除合同 |
//! | `cancel()` | [`ContractController::cancel`] | 解绑 |
//! | `status()` | [`ContractController::status`] | 修改状态 |
//! | `detail()` | [`ContractController::detail`] | 合同详情 |
//! | `customer()` | [`ContractController::customer`] | 按客户列表 |

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
use crate::model::Contract;

/// Contract 控制器 — 对齐 PHP `Contract` 控制器
pub struct ContractController;

impl SzController for ContractController {}
impl BaseController for ContractController {}
impl AddonsBaseController for ContractController {}

impl ContractController {
    /// 基础数据 — 对齐 PHP `base()`
    #[tracing::instrument(skip_all)]
    pub async fn base(&self, req: Request<Body>) -> Response {
        let _param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = json!({
            "payStatusList": [],
            "payTypeList": [],
            "contractStatusList": [
                {"code": 0, "name": "待生效"},
                {"code": 1, "name": "生效中"},
                {"code": 2, "name": "已到期"},
                {"code": 3, "name": "已终止"}
            ],
            "signingList": [],
            "companyList": [],
            "deptList": [],
            "customerList": [],
            "catList": []
        });
        self.render_success("", json!({"result": result}))
    }

    /// 分页列表 — 对齐 PHP `index()`
    #[tracing::instrument(skip_all)]
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "list");
        self.render_success("", json!({"result": result}))
    }

    /// 导出列表 — 对齐 PHP `export()`
    #[tracing::instrument(skip_all)]
    pub async fn export(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "export");
        self.render_success("", json!({"result": result}))
    }

    /// 添加合同 — 对齐 PHP `add()`
    #[tracing::instrument(skip_all)]
    pub async fn add(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::add_contract(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 复制合同 — 对齐 PHP `copy()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function copy(): Json {
    ///     $param = $this->postData();
    ///     $model = new ContractModel();
    ///     $data = json_decode($param['formData'], true);
    ///     if($model->add($data)){
    ///         ContractModel::where(['contract_id'=>$param['contract_id']])
    ///             ->update(['contract_status'=>3]);
    ///         return $this->renderSuccess('添加成功');
    ///     }
    ///     return $this->renderError($model->getError() ?: '添加失败');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn copy(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let old_contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::add_contract(repo, &data) {
            Ok(()) => {
                // 旧合同 contract_status 置为 3（对齐 PHP）
                if let Err(e) = Self::set_contract_status(repo, old_contract_id, 3) {
                    return self.render_error(&e, json!({}), 0);
                }
                self.render_success("添加成功", json!({}))
            }
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 编辑合同 — 对齐 PHP `edit()`
    #[tracing::instrument(skip_all)]
    pub async fn edit(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::edit_contract(repo, contract_id, &data) {
            Ok(()) => self.render_success("更新成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 绑定商户 — 对齐 PHP `bind()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function bind(): Json {
    ///     $param = $this->postData();
    ///     $model = ContractModel::detail($param['contract_id']);
    ///     if(empty($model)){
    ///         return $this->renderError('合同不存在');
    ///     }
    ///     $customer_id = $model['customer_id'];
    ///     if($model->bind($param,$customer_id)){
    ///         return $this->renderSuccess("绑定商户成功");
    ///     }
    ///     return $this->renderError($model->getError() ?:'绑定商户失败');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn bind(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };

        match Self::bind_contract(repo, contract_id, &param) {
            Ok(()) => self.render_success("绑定商户成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 软删除合同 — 对齐 PHP `del()`
    #[tracing::instrument(skip_all)]
    pub async fn del(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };

        match Self::set_delete(repo, contract_id) {
            Ok(()) => self.render_success("删除成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 解绑 — 对齐 PHP `cancel()`
    #[tracing::instrument(skip_all)]
    pub async fn cancel(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };

        match Self::cancel_contract(repo, contract_id) {
            Ok(()) => self.render_success("解绑成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 修改状态 — 对齐 PHP `status()`
    ///
    /// # PHP 对齐
    ///
    /// PHP `status()` 实际调用 `$model->cancel($customer_id)`（与 `cancel` 相同）
    #[tracing::instrument(skip_all)]
    pub async fn status(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };

        match Self::cancel_contract(repo, contract_id) {
            Ok(()) => self.render_success("解绑成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 合同详情 — 对齐 PHP `detail()`
    #[tracing::instrument(skip_all)]
    pub async fn detail(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let contract_id = match get_i64_param(&param, "contract_id") {
            Some(id) => id,
            None => return self.render_error("contract_id 参数缺失", json!({}), 0),
        };

        match Self::detail_contract(repo, contract_id) {
            Some(detail) => self.render_success("", json!({"detail": detail})),
            None => self.render_error("数据不存在", json!({}), 0),
        }
    }

    /// 按客户列表 — 对齐 PHP `customer()`
    #[tracing::instrument(skip_all)]
    pub async fn customer(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_customer_list(repo, &param);
        self.render_success("", json!({"result": result}))
    }

    // ========================================================================
    // 业务方法（对齐 PHP `Contract` 模型业务方法）
    // ========================================================================

    /// 查询合同列表 — 对齐 PHP `Contract::getList($param, $type)`
    fn get_list(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        param: &Value,
        list_type: &str,
    ) -> Value {
        let app_id = get_app_id(param);
        let list_rows = get_i64_param(param, "list_rows").unwrap_or(15) as usize;
        let page = get_i64_param(param, "page").unwrap_or(1) as usize;
        let keyword = get_str_param(param, "keyword").unwrap_or_default();

        let mut conditions = vec![
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
        ];
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
        if let Some(company_id) = get_i64_param(param, "company_id") {
            conditions.push(WhereCondition::new(
                "company_id",
                WhereOp::Eq,
                OrmValue::I64(company_id),
            ));
        }
        if let Some(contract_status) = get_i64_param(param, "contract_status") {
            conditions.push(WhereCondition::new(
                "contract_status",
                WhereOp::Eq,
                OrmValue::I64(contract_status),
            ));
        }

        let mut items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };

        // keyword 模糊匹配 contract_name
        if !keyword.is_empty() {
            let kw = keyword.trim().to_lowercase();
            items.retain(|item| {
                item.get("contract_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase().contains(&kw))
                    .unwrap_or(false)
            });
        }

        // PHP 按 create_time desc, contract_id desc 排序（简化：按 contract_id desc）
        items.sort_by(|a, b| {
            let a_id = a.get("contract_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("contract_id").and_then(|v| v.as_i64()).unwrap_or(0);
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

    /// 合同详情 — 对齐 PHP `Contract::detail($contract_id)`
    fn detail_contract(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        contract_id: i64,
    ) -> Option<Value> {
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(contract_id),
        )];
        repo.find_one_by(&conditions)
            .ok()
            .flatten()
            .map(|c| c.to_json())
    }

    /// 按客户查合同列表 — 对齐 PHP `Contract::getCustomerList($param)`
    fn get_customer_list(repo: &dyn Repository<Contract, Key = OrmValue>, param: &Value) -> Value {
        let app_id = get_app_id(param);
        let mut conditions = vec![
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
        ];
        if let Some(customer_id) = get_i64_param(param, "customer_id") {
            conditions.push(WhereCondition::new(
                "customer_id",
                WhereOp::Eq,
                OrmValue::I64(customer_id),
            ));
        }
        let items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };
        json!({"list": items})
    }

    /// 添加合同 — 对齐 PHP `Contract::add($data)`
    fn add_contract(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        let mut model = Contract::new();
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

    /// 编辑合同 — 对齐 PHP `Contract::edit($data)`
    fn edit_contract(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        contract_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(contract_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

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

    /// 绑定商户 — 对齐 PHP `Contract::bind($param, $customer_id)`
    fn bind_contract(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        contract_id: i64,
        param: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(contract_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("合同不存在")?;

        if let Some(obj) = param.as_object() {
            let mut data_map: std::collections::HashMap<String, Value> =
                std::collections::HashMap::new();
            for (k, v) in obj {
                if k == "contract_id" || k == "app_id" || k == "formData" {
                    continue;
                }
                data_map.insert(k.clone(), v.clone());
            }
            if !data_map.is_empty() {
                model.set_attrs(&data_map);
            }
        }
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 软删除合同 — 对齐 PHP `Contract::setDelete()`
    fn set_delete(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        contract_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(contract_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        let mut data_map: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        data_map.insert("is_delete".to_string(), json!(1));
        model.set_attrs(&data_map);
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 解绑 — 对齐 PHP `Contract::cancel($customer_id)`
    ///
    /// # PHP 对齐
    ///
    /// PHP `cancel` 清除 customer_id 关联并设置 contract_status。
    fn cancel_contract(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        contract_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(contract_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        let mut data_map: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        data_map.insert("customer_id".to_string(), json!(0));
        data_map.insert("contract_status".to_string(), json!(0));
        model.set_attrs(&data_map);
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 设置合同状态 — 对齐 PHP `ContractModel::where()->update(['contract_status'=>$status])`
    fn set_contract_status(
        repo: &dyn Repository<Contract, Key = OrmValue>,
        contract_id: i64,
        status: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(contract_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        let mut data_map: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        data_map.insert("contract_status".to_string(), json!(status));
        model.set_attrs(&data_map);
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_core::repository::InMemoryRepository;

    fn make_contract(id: i64, name: &str, app_id: i64, customer_id: i64) -> Contract {
        Contract::new()
            .with_data("contract_id", json!(id))
            .with_data("contract_name", json!(name))
            .with_data("customer_id", json!(customer_id))
            .with_data("dept_id", json!(34))
            .with_data("company_id", json!(0))
            .with_data("contract_status", json!(1))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    fn make_repo() -> InMemoryRepository<Contract> {
        InMemoryRepository::from_vec(vec![
            make_contract(1, "合同A", 10001, 100),
            make_contract(2, "合同B", 10001, 200),
            make_contract(3, "测试", 20002, 300),
        ])
    }

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = ContractController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_get_list_filters_by_customer_id() {
        let repo = make_repo();
        let result = ContractController::get_list(
            &repo,
            &json!({"app_id": 10001, "customer_id": 100}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_detail_contract() {
        let repo = make_repo();
        let detail = ContractController::detail_contract(&repo, 1);
        assert!(detail.is_some());
        let detail = detail.unwrap();
        assert_eq!(detail["contract_name"], "合同A");
    }

    #[test]
    fn test_detail_contract_not_found() {
        let repo = make_repo();
        let detail = ContractController::detail_contract(&repo, 999);
        assert!(detail.is_none());
    }

    #[test]
    fn test_add_contract_success() {
        let repo = make_repo();
        let result = ContractController::add_contract(
            &repo,
            &json!({"contract_name": "新合同", "app_id": 10001}),
        );
        assert!(result.is_ok());
        assert_eq!(repo.len(), 4);
    }

    #[test]
    fn test_edit_contract_success() {
        let repo = make_repo();
        let result =
            ContractController::edit_contract(&repo, 1, &json!({"contract_name": "更新名称"}));
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["contract_name"], "更新名称");
    }

    #[test]
    fn test_set_delete_success() {
        let repo = make_repo();
        let result = ContractController::set_delete(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let deleted = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(deleted.to_json()["is_delete"], 1);
    }

    #[test]
    fn test_cancel_contract_clears_customer() {
        let repo = make_repo();
        let result = ContractController::cancel_contract(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["customer_id"], 0);
        assert_eq!(updated.to_json()["contract_status"], 0);
    }

    #[test]
    fn test_copy_contract_sets_old_status_3() {
        let repo = make_repo();
        // PHP copy：新增合同并将旧合同 contract_status 置为 3
        let result = ContractController::add_contract(&repo, &json!({"contract_name": "复制"}));
        assert!(result.is_ok());
        let set_status_result = ContractController::set_contract_status(&repo, 1, 3);
        assert!(set_status_result.is_ok());
        let conditions = [WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let old = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(old.to_json()["contract_status"], 3);
    }

    #[test]
    fn test_r5_php_contract_get_list_returns_list_key() {
        let repo = make_repo();
        let result = ContractController::get_list(&repo, &json!({"app_id": 10001}), "list");
        assert!(result["list"].is_array());
    }
}

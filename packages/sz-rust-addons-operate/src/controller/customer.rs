//! Customer 控制器 — 对齐 PHP `addons/operate/controller/admin/Customer.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `base()` | [`CustomerController::base`] | 基础数据（部门/分类/等级/状态/支付方式） |
//! | `index()` | [`CustomerController::index`] | 分页列表 |
//! | `export()` | [`CustomerController::export`] | 导出列表（不分页） |
//! | `selectDeptList()` | [`CustomerController::select_dept_list`] | 按部门查客户 |
//! | `selectCatList()` | [`CustomerController::select_cat_list`] | 按分类查客户 |
//! | `add()` | [`CustomerController::add`] | 添加客户 |
//! | `edit()` | [`CustomerController::edit`] | 编辑客户 |
//! | `del()` | [`CustomerController::del`] | 软删除客户 |
//! | `bind()` | [`CustomerController::bind`] | 绑定铺位 |
//! | `cancel()` | [`CustomerController::cancel`] | 撤场 |
//! | `status()` | [`CustomerController::status`] | 修改状态 |
//! | `sync()` | [`CustomerController::sync`] | 同步 contract_id |
//!
//! ## PHP 源码依据
//!
//! ```php
//! public function index(): Json {
//!     $param = $this->postData();
//!     $model = new CustomerModel();
//!     $result = $model->getList($param,'list');
//!     return $this->renderSuccess('', compact('result'));
//! }
//! ```

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use serde_json::{json, Value};
use sz_orm_core::repository::{Repository, WhereCondition, WhereOp};
use sz_orm_core::Value as OrmValue;
use sz_orm_core::{Model as _, ModelExt as _};
use sz_rust_core::controller::{AddonsBaseController, BaseController, SzController};
use sz_rust_core::model::Mutator as _;

use crate::controller::common::{get_app_id, get_i64_param, get_str_param, parse_form_data};
use crate::model::{Contract, Customer};

/// Customer 控制器 — 对齐 PHP `Customer` 控制器
pub struct CustomerController;

impl SzController for CustomerController {}
impl BaseController for CustomerController {}
impl AddonsBaseController for CustomerController {}

impl CustomerController {
    /// 基础数据 — 对齐 PHP `base()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function base(): Json {
    ///     $param = $this->postData();
    ///     $result = [
    ///         'deptList' => Dept::getLightList(34),
    ///         'catList' => Category::getAll($param['app_id']),
    ///         'levelList' => Level::getAll($param['app_id']),
    ///         'statusList' => ContractStatusEnum::customerStatusList(),
    ///         'payTypeList' => CustomerSyncTypeEnum::payTypeList(),
    ///         'syncStatusList' => CustomerSyncTypeEnum::syncStatusList(),
    ///         'bankInfo' => ['bank_name' => 'ccb', 'bank_card' => '...', 'bank_account' => '...']
    ///     ];
    ///     return $this->renderSuccess('', compact('result'));
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn base(&self, req: Request<Body>) -> Response {
        let _param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        // PHP 整合部门/分类/等级/状态等基础数据，Rust 端返回基础结构，
        // 业务层（apps/oapc）可注入具体 Repository 补全 deptList/catList/levelList。
        let result = json!({
            "deptList": [],
            "catList": [],
            "levelList": [],
            "statusList": [
                {"code": 0, "name": "禁用"},
                {"code": 1, "name": "启用"}
            ],
            "payTypeList": [
                {"code": 1, "name": "现金"},
                {"code": 2, "name": "银行"}
            ],
            "syncStatusList": [
                {"code": 0, "name": "未同步"},
                {"code": 1, "name": "已同步"}
            ],
            "bankInfo": {
                "bank_name": "ccb",
                "bank_card": "105011773995373",
                "bank_account": "090378126"
            }
        });
        self.render_success("", json!({"result": result}))
    }

    /// 分页列表 — 对齐 PHP `index()`
    #[tracing::instrument(skip_all)]
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
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
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "export");
        self.render_success("", json!({"result": result}))
    }

    /// 按部门查客户 — 对齐 PHP `selectDeptList()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function selectDeptList(): Json {
    ///     $param = $this->postData();
    ///     $model = new CustomerModel();
    ///     $result = $model->selectDeptCustomer($param['dept_id'],$param['app_id']);
    ///     return $this->renderSuccess('', ['result'=>$result]);
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn select_dept_list(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let dept_id = match get_i64_param(&param, "dept_id") {
            Some(id) => id,
            None => return self.render_error("dept_id 参数缺失", json!({}), 0),
        };
        let app_id = get_app_id(&param);
        let result = Self::select_dept_customer(repo, dept_id, app_id);
        self.render_success("", json!({"result": result}))
    }

    /// 按分类查客户 — 对齐 PHP `selectCatList()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function selectCatList(): Json {
    ///     $param = $this->postData();
    ///     $model = new CustomerModel();
    ///     $result = $model->selectCatCustomer($param['dept_id'],34,$param['app_id']);
    ///     return $this->renderSuccess('', ['result'=>$result]);
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn select_cat_list(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let dept_id = match get_i64_param(&param, "dept_id") {
            Some(id) => id,
            None => return self.render_error("dept_id 参数缺失", json!({}), 0),
        };
        let app_id = get_app_id(&param);
        // PHP 第二参数硬编码 34（dept_id 过滤），Rust 端严格对齐
        let result = Self::select_cat_customer(repo, dept_id, 34, app_id);
        self.render_success("", json!({"result": result}))
    }

    /// 添加客户 — 对齐 PHP `add()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add(): Json {
    ///     $param = $this->postData();
    ///     $model = new CustomerModel();
    ///     $data = json_decode($param['formData'], true);
    ///     $data['app_id'] = $param['app_id'] ?? 10001;
    ///     if($model->add($data,$this->user['parent_id'])){
    ///         return $this->renderSuccess('添加成功');
    ///     }
    ///     return $this->renderError($model->getError() ?: '添加失败');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn add(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let mut data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        // PHP: $data['app_id'] = $param['app_id'] ?? 10001;
        if let Some(obj) = data.as_object_mut() {
            if !obj.contains_key("app_id") {
                obj.insert("app_id".to_string(), json!(get_app_id(&param)));
            }
        }

        match Self::add_customer(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 编辑客户 — 对齐 PHP `edit()`
    #[tracing::instrument(skip_all)]
    pub async fn edit(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let customer_id = match get_i64_param(&param, "customer_id") {
            Some(id) => id,
            None => return self.render_error("customer_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::edit_customer(repo, customer_id, &data) {
            Ok(()) => self.render_success("更新成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 软删除客户 — 对齐 PHP `del()`
    #[tracing::instrument(skip_all)]
    pub async fn del(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let customer_id = match get_i64_param(&param, "customer_id") {
            Some(id) => id,
            None => return self.render_error("customer_id 参数缺失", json!({}), 0),
        };

        match Self::set_delete(repo, customer_id) {
            Ok(()) => self.render_success("删除成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 绑定铺位 — 对齐 PHP `bind()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function bind(): Json {
    ///     $param = $this->postData();
    ///     $data = json_decode($param['formData'], true);
    ///     $model = CustomerModel::detail($data['customer_id']);
    ///     if($model->bind($data,$this->user['parent_id'])){
    ///         return $this->renderSuccess("绑定铺位成功");
    ///     }
    ///     return $this->renderError($model->getError() ?:'绑定铺位失败');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn bind(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        let customer_id = match get_i64_param(&data, "customer_id") {
            Some(id) => id,
            None => return self.render_error("customer_id 参数缺失", json!({}), 0),
        };

        match Self::bind_customer(repo, customer_id, &data) {
            Ok(()) => self.render_success("绑定铺位成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 撤场 — 对齐 PHP `cancel()`
    #[tracing::instrument(skip_all)]
    pub async fn cancel(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let customer_id = match get_i64_param(&param, "customer_id") {
            Some(id) => id,
            None => return self.render_error("customer_id 参数缺失", json!({}), 0),
        };

        match Self::cancel_customer(repo, customer_id) {
            Ok(()) => self.render_success("撤场成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 修改状态 — 对齐 PHP `status()`
    #[tracing::instrument(skip_all)]
    pub async fn status(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let customer_id = match get_i64_param(&param, "customer_id") {
            Some(id) => id,
            None => return self.render_error("customer_id 参数缺失", json!({}), 0),
        };

        match Self::set_status(repo, customer_id, &param) {
            Ok(()) => self.render_success("修改成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 同步 contract_id — 对齐 PHP `sync()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function sync(): Json {
    ///     $param = $this->postData();
    ///     $customer = new CustomerModel();
    ///     if (!empty($param['dept_id'])) {
    ///         $customer = $customer->where('dept_id', $param['dept_id']);
    ///     }
    ///     $list = $customer->where(['contract_id'=>0])->column('customer_id');
    ///     $num = 0;
    ///     $total = count($list);
    ///     foreach ($list as $vo) {
    ///         if (empty($vo)) continue;
    ///         $contract_id = Contract::where(['customer_id'=>$vo])
    ///             ->order(['create_time'=>'desc','contract_id' => 'desc'])
    ///             ->value('contract_id');
    ///         if (!empty($contract_id)) {
    ///             $res = CustomerModel::where(['customer_id'=>$vo])
    ///                 ->whereNotIn('contract_id',$contract_id)
    ///                 ->update(['contract_id'=>$contract_id]);
    ///             if ($res) $num++;
    ///         }
    ///     }
    ///     return $this->renderSuccess('操作成功'.$num.'条,失败:'.($total - $num));
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn sync(
        &self,
        req: Request<Body>,
        cust_repo: &dyn Repository<Customer, Key = OrmValue>,
        contract_repo: &dyn Repository<Contract, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };

        // 构造查询条件
        let mut conditions = vec![WhereCondition::new(
            "contract_id",
            WhereOp::Eq,
            OrmValue::I64(0),
        )];
        if let Some(dept_id) = get_i64_param(&param, "dept_id") {
            conditions.push(WhereCondition::new(
                "dept_id",
                WhereOp::Eq,
                OrmValue::I64(dept_id),
            ));
        }

        let customers = match cust_repo.find_by(&conditions) {
            Ok(list) => list,
            Err(e) => return self.render_error(format!("查询失败: {e}"), json!({}), 0),
        };

        let total = customers.len();
        let mut num = 0usize;
        for cust in customers {
            let customer_id = cust.pk();
            if customer_id == 0 {
                continue;
            }
            // 查询客户最新合同
            let contract_conditions = [WhereCondition::new(
                "customer_id",
                WhereOp::Eq,
                OrmValue::I64(customer_id),
            )];
            let mut contracts = match contract_repo.find_by(&contract_conditions) {
                Ok(list) => list,
                Err(_) => continue,
            };
            // 按 contract_id desc 排序（对齐 PHP order）
            contracts.sort_by_key(|b| std::cmp::Reverse(b.pk()));
            if let Some(latest) = contracts.first() {
                let contract_id = latest.pk();
                if contract_id == 0 {
                    continue;
                }
                // 更新 customer.contract_id
                if Self::update_contract_id(cust_repo, customer_id, contract_id).is_ok() {
                    num += 1;
                }
            }
        }

        let msg = format!("操作成功{num}条,失败:{}", total - num);
        self.render_success(&msg, json!({}))
    }

    // ========================================================================
    // 业务方法（对齐 PHP `Customer` 模型业务方法）
    // ========================================================================

    /// 查询客户列表 — 对齐 PHP `Customer::getList($param, $type)`
    ///
    /// # 简化说明
    ///
    /// - 关联关系（dept/cat/level/company）：NOTE(Phase 6)
    /// - 复杂搜索（keyword 多字段模糊）：简化为 customer_name 模糊匹配
    fn get_list(
        repo: &dyn Repository<Customer, Key = OrmValue>,
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
        if let Some(cat_id) = get_i64_param(param, "cat_id") {
            conditions.push(WhereCondition::new(
                "cat_id",
                WhereOp::Eq,
                OrmValue::I64(cat_id),
            ));
        }
        if let Some(level_id) = get_i64_param(param, "level_id") {
            conditions.push(WhereCondition::new(
                "level_id",
                WhereOp::Eq,
                OrmValue::I64(level_id),
            ));
        }
        if let Some(status) = get_i64_param(param, "status") {
            conditions.push(WhereCondition::new(
                "status",
                WhereOp::Eq,
                OrmValue::I64(status),
            ));
        }

        let mut items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };

        // keyword 模糊匹配（简化：仅 customer_name）
        if !keyword.is_empty() {
            let kw = keyword.trim().to_lowercase();
            items.retain(|item| {
                item.get("customer_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase().contains(&kw))
                    .unwrap_or(false)
            });
        }

        // PHP 按 create_time desc 排序（简化：按 customer_id desc）
        items.sort_by(|a, b| {
            let a_id = a.get("customer_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("customer_id").and_then(|v| v.as_i64()).unwrap_or(0);
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

    /// 按部门查客户 — 对齐 PHP `Customer::selectDeptCustomer($dept_id, $app_id)`
    fn select_dept_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        dept_id: i64,
        app_id: i64,
    ) -> Value {
        let conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!([]),
        };
        json!(items)
    }

    /// 按分类查客户 — 对齐 PHP `Customer::selectCatCustomer($dept_id, $cat_id, $app_id)`
    fn select_cat_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        dept_id: i64,
        cat_id: i64,
        app_id: i64,
    ) -> Value {
        let conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
            WhereCondition::new("cat_id", WhereOp::Eq, OrmValue::I64(cat_id)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!([]),
        };
        json!(items)
    }

    /// 添加客户 — 对齐 PHP `Customer::add($data, $parent_id)`
    fn add_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        let mut model = Customer::new();
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

    /// 编辑客户 — 对齐 PHP `Customer::edit($data, $parent_id)`
    fn edit_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        customer_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
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

    /// 软删除客户 — 对齐 PHP `Customer::setDelete($parent_id)`
    fn set_delete(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        customer_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
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

    /// 绑定铺位 — 对齐 PHP `Customer::bind($data, $parent_id)`
    ///
    /// # PHP 对齐
    ///
    /// PHP `bind` 更新 customer 的 rentarea_ids 字段。
    /// 简化：直接设置 rentarea_ids 字段。
    fn bind_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        customer_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
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

    /// 撤场 — 对齐 PHP `Customer::cancel($parent_id)`
    ///
    /// # PHP 对齐
    ///
    /// PHP `cancel` 设置 customer 的 status=2（撤场），清空 rentarea_ids。
    fn cancel_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        customer_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        let mut data_map: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        data_map.insert("status".to_string(), json!(2));
        data_map.insert("rentarea_ids".to_string(), json!(""));
        model.set_attrs(&data_map);
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 修改状态 — 对齐 PHP `Customer::status($param)`
    fn set_status(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        customer_id: i64,
        param: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        // PHP 传入 status 字段
        if let Some(obj) = param.as_object() {
            let mut data_map: std::collections::HashMap<String, Value> =
                std::collections::HashMap::new();
            for (k, v) in obj {
                // 排除控制器参数（customer_id 等）
                if k == "customer_id" || k == "app_id" || k == "formData" {
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

    /// 更新 customer.contract_id — 对齐 PHP `CustomerModel::where()->update(['contract_id'=>$id])`
    fn update_contract_id(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        customer_id: i64,
        contract_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        let mut data_map: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        data_map.insert("contract_id".to_string(), json!(contract_id));
        model.set_attrs(&data_map);
        repo.save(model).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_core::repository::InMemoryRepository;

    fn make_customer(id: i64, name: &str, app_id: i64, dept_id: i64) -> Customer {
        Customer::new()
            .with_data("customer_id", json!(id))
            .with_data("customer_name", json!(name))
            .with_data("dept_id", json!(dept_id))
            .with_data("cat_id", json!(34))
            .with_data("level_id", json!(0))
            .with_data("status", json!(1))
            .with_data("contract_id", json!(0))
            .with_data("rentarea_ids", json!(""))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    fn make_repo() -> InMemoryRepository<Customer> {
        InMemoryRepository::from_vec(vec![
            make_customer(1, "客户A", 10001, 34),
            make_customer(2, "客户B", 10001, 34),
            make_customer(3, "测试", 20002, 34),
        ])
    }

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = CustomerController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_get_list_filters_by_keyword() {
        let repo = make_repo();
        let result = CustomerController::get_list(
            &repo,
            &json!({"app_id": 10001, "keyword": "客户A"}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["customer_name"], "客户A");
    }

    #[test]
    fn test_get_list_export_no_pagination() {
        let repo = make_repo();
        let result = CustomerController::get_list(&repo, &json!({"app_id": 10001}), "export");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_select_dept_customer() {
        let repo = make_repo();
        let result = CustomerController::select_dept_customer(&repo, 34, 10001);
        let list = result.as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_select_cat_customer() {
        let repo = make_repo();
        let result = CustomerController::select_cat_customer(&repo, 34, 34, 10001);
        let list = result.as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_add_customer_success() {
        let repo = make_repo();
        let result = CustomerController::add_customer(
            &repo,
            &json!({"customer_name": "新客户", "app_id": 10001}),
        );
        assert!(result.is_ok());
        assert_eq!(repo.len(), 4);
    }

    #[test]
    fn test_edit_customer_success() {
        let repo = make_repo();
        let result =
            CustomerController::edit_customer(&repo, 1, &json!({"customer_name": "更新名称"}));
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["customer_name"], "更新名称");
    }

    #[test]
    fn test_set_delete_success() {
        let repo = make_repo();
        let result = CustomerController::set_delete(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let deleted = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(deleted.to_json()["is_delete"], 1);
    }

    #[test]
    fn test_bind_customer_success() {
        let repo = make_repo();
        let result = CustomerController::bind_customer(
            &repo,
            1,
            &json!({"customer_id": 1, "rentarea_ids": "10,11"}),
        );
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["rentarea_ids"], "10,11");
    }

    #[test]
    fn test_cancel_customer_sets_status_2() {
        let repo = make_repo();
        let result = CustomerController::cancel_customer(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["status"], 2);
        assert_eq!(updated.to_json()["rentarea_ids"], "");
    }

    #[test]
    fn test_set_status_success() {
        let repo = make_repo();
        let result =
            CustomerController::set_status(&repo, 1, &json!({"customer_id": 1, "status": 0}));
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["status"], 0);
    }

    #[test]
    fn test_r5_php_customer_get_list_returns_list_key() {
        let repo = make_repo();
        let result = CustomerController::get_list(&repo, &json!({"app_id": 10001}), "list");
        assert!(result["list"].is_array());
    }
}

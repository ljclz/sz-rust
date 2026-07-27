//! Rentarea 控制器 — 对齐 PHP `addons/operate/controller/admin/Rentarea.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `base()` | [`RentareaController::base`] | 基础数据 |
//! | `index()` | [`RentareaController::index`] | 分页列表 |
//! | `export()` | [`RentareaController::export`] | 导出列表 |
//! | `add()` | [`RentareaController::add`] | 添加铺位 |
//! | `edit()` | [`RentareaController::edit`] | 编辑铺位 |
//! | `del()` | [`RentareaController::del`] | 软删除铺位 |
//! | `bind()` | [`RentareaController::bind`] | 绑定商户 |
//! | `cancel()` | [`RentareaController::cancel`] | 一键空置 |
//! | `status()` | [`RentareaController::status`] | 修改状态 |
//! | `selectLevelList()` | [`RentareaController::select_level_list`] | 按等级查铺位 |
//! | `sync()` | [`RentareaController::sync`] | 同步商户铺位关联 |

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
use crate::model::{Customer, Rentarea};

/// Rentarea 控制器 — 对齐 PHP `Rentarea` 控制器
pub struct RentareaController;

impl SzController for RentareaController {}
impl BaseController for RentareaController {}
impl AddonsBaseController for RentareaController {}

impl RentareaController {
    /// 基础数据 — 对齐 PHP `base()`
    #[tracing::instrument(skip_all)]
    pub async fn base(&self, req: Request<Body>) -> Response {
        let _param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = json!({
            "deptList": [],
            "catList": [],
            "levelList": []
        });
        self.render_success("", json!({"result": result}))
    }

    /// 分页列表 — 对齐 PHP `index()`
    #[tracing::instrument(skip_all)]
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
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
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "export");
        self.render_success("", json!({"result": result}))
    }

    /// 添加铺位 — 对齐 PHP `add()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add(): Json {
    ///     $param = $this->postData();
    ///     $model = new RentareaModel();
    ///     $data = json_decode($param['formData'], true);
    ///     $data['app_id'] = $this->user['app_id'];
    ///     if($model->add($data)){
    ///         return $this->renderSuccess('添加成功');
    ///     }
    ///     return $this->renderError($model->getError() ?: '添加失败');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn add(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let mut data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        // PHP: $data['app_id'] = $this->user['app_id'];
        if let Some(obj) = data.as_object_mut() {
            if !obj.contains_key("app_id") {
                obj.insert("app_id".to_string(), json!(get_app_id(&param)));
            }
        }

        match Self::add_rentarea(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 编辑铺位 — 对齐 PHP `edit()`
    #[tracing::instrument(skip_all)]
    pub async fn edit(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let rentarea_id = match get_i64_param(&param, "rentarea_id") {
            Some(id) => id,
            None => return self.render_error("rentarea_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::edit_rentarea(repo, rentarea_id, &data) {
            Ok(()) => self.render_success("更新成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 软删除铺位 — 对齐 PHP `del()`
    #[tracing::instrument(skip_all)]
    pub async fn del(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let rentarea_id = match get_i64_param(&param, "rentarea_id") {
            Some(id) => id,
            None => return self.render_error("rentarea_id 参数缺失", json!({}), 0),
        };

        match Self::set_delete(repo, rentarea_id) {
            Ok(()) => self.render_success("删除成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 绑定商户 — 对齐 PHP `bind()`
    #[tracing::instrument(skip_all)]
    pub async fn bind(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let rentarea_id = match get_i64_param(&param, "rentarea_id") {
            Some(id) => id,
            None => return self.render_error("rentarea_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::bind_rentarea(repo, rentarea_id, &data) {
            Ok(()) => self.render_success("绑定商户成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 一键空置 — 对齐 PHP `cancel()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function cancel(): Json {
    ///     $param = $this->postData();
    ///     $model = RentareaModel::detail($param['rentarea_id']);
    ///     $data = [
    ///         'status' => 2, 'rent' => 0, 'rent_day' => 0,
    ///         'customer_id' => 0, 'area_name' => '空置'
    ///     ];
    ///     if(!$model->status($data,$model['customer_id'])){
    ///         return $this->renderError('一键空置失败');
    ///     }
    ///     return $this->renderSuccess("一键空置成功");
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn cancel(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let rentarea_id = match get_i64_param(&param, "rentarea_id") {
            Some(id) => id,
            None => return self.render_error("rentarea_id 参数缺失", json!({}), 0),
        };

        // PHP 硬编码空置数据
        let vacant_data = json!({
            "status": 2,
            "rent": 0,
            "rent_day": 0,
            "customer_id": 0,
            "area_name": "空置"
        });

        match Self::set_status(repo, rentarea_id, &vacant_data) {
            Ok(()) => self.render_success("一键空置成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 修改状态 — 对齐 PHP `status()`
    #[tracing::instrument(skip_all)]
    pub async fn status(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let rentarea_id = match get_i64_param(&param, "rentarea_id") {
            Some(id) => id,
            None => return self.render_error("rentarea_id 参数缺失", json!({}), 0),
        };

        match Self::set_status(repo, rentarea_id, &param) {
            Ok(()) => self.render_success("修改成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 按等级查铺位 — 对齐 PHP `selectLevelList()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function selectLevelList(): Json {
    ///     $param = $this->postData();
    ///     $result = RentareaModel::selectLevelRentarea($param['dept_id'],$param['app_id']);
    ///     return $this->renderSuccess('', ['result'=>$result]);
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn select_level_list(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
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
        let result = Self::select_level_rentarea(repo, dept_id, app_id);
        self.render_success("", json!({"result": result}))
    }

    /// 同步商户铺位关联 — 对齐 PHP `sync()`
    ///
    /// # PHP 对齐
    ///
    /// PHP `sync` 根据 cat_id + area_name 匹配 customer 与 rentarea，
    /// 更新 rentarea.customer_id 和 customer.rentarea_ids。
    ///
    /// # 简化说明
    ///
    /// - 事务：NOTE(事务模块)
    /// - 关联更新（customer.rentarea_ids）：已实现
    #[tracing::instrument(skip_all)]
    pub async fn sync(
        &self,
        req: Request<Body>,
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };

        let result = Self::sync_rentarea_customer(rentarea_repo, customer_repo, &param);
        match result {
            Ok(msg) => self.render_success(&msg, json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法（对齐 PHP `Rentarea` 模型业务方法）
    // ========================================================================

    /// 查询铺位列表 — 对齐 PHP `Rentarea::getList($param, $type)`
    fn get_list(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
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
        if let Some(customer_id) = get_i64_param(param, "customer_id") {
            conditions.push(WhereCondition::new(
                "customer_id",
                WhereOp::Eq,
                OrmValue::I64(customer_id),
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

        // keyword 模糊匹配 area_name
        if !keyword.is_empty() {
            let kw = keyword.trim().to_lowercase();
            items.retain(|item| {
                item.get("area_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase().contains(&kw))
                    .unwrap_or(false)
            });
        }

        // PHP 按 rentarea_id desc 排序
        items.sort_by(|a, b| {
            let a_id = a.get("rentarea_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("rentarea_id").and_then(|v| v.as_i64()).unwrap_or(0);
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

    /// 按等级查铺位 — 对齐 PHP `Rentarea::selectLevelRentarea($dept_id, $app_id)`
    fn select_level_rentarea(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
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

    /// 添加铺位 — 对齐 PHP `Rentarea::add($data)`
    fn add_rentarea(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        let mut model = Rentarea::new();
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

    /// 编辑铺位 — 对齐 PHP `Rentarea::edit($data)`
    fn edit_rentarea(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        rentarea_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(rentarea_id),
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

    /// 软删除铺位 — 对齐 PHP `Rentarea::setDelete()`
    fn set_delete(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        rentarea_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(rentarea_id),
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

    /// 绑定商户 — 对齐 PHP `Rentarea::bind($data, $customer_id)`
    fn bind_rentarea(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        rentarea_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(rentarea_id),
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

    /// 修改状态 — 对齐 PHP `Rentarea::status($data, $customer_id)`
    fn set_status(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        rentarea_id: i64,
        param: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(rentarea_id),
        )];
        let mut model = repo
            .find_one_by(&conditions)
            .map_err(|e| e.to_string())?
            .ok_or("数据不存在")?;

        if let Some(obj) = param.as_object() {
            let mut data_map: std::collections::HashMap<String, Value> =
                std::collections::HashMap::new();
            for (k, v) in obj {
                if k == "rentarea_id" || k == "app_id" || k == "formData" {
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

    /// 同步 rentarea 与 customer 关联 — 对齐 PHP `sync()`
    ///
    /// # PHP 对齐
    ///
    /// PHP 根据 `cat_id + area_name` 匹配 customer 与 rentarea，
    /// 更新 rentarea.customer_id = first_customer_id，
    /// 并将 rentarea_id 加入 customer.rentarea_ids（去重）。
    fn sync_rentarea_customer(
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
        param: &Value,
    ) -> Result<String, String> {
        let dept_id = get_i64_param(param, "dept_id");

        // 加载 rentarea 列表
        let mut rentarea_conditions = vec![WhereCondition::new(
            "is_delete",
            WhereOp::Eq,
            OrmValue::I64(0),
        )];
        if let Some(did) = dept_id {
            rentarea_conditions.push(WhereCondition::new(
                "dept_id",
                WhereOp::Eq,
                OrmValue::I64(did),
            ));
        }
        let rentarea_list = rentarea_repo
            .find_by(&rentarea_conditions)
            .map_err(|e| e.to_string())?;

        // 加载 customer 列表
        let mut customer_conditions = vec![WhereCondition::new(
            "is_delete",
            WhereOp::Eq,
            OrmValue::I64(0),
        )];
        if let Some(did) = dept_id {
            customer_conditions.push(WhereCondition::new(
                "dept_id",
                WhereOp::Eq,
                OrmValue::I64(did),
            ));
        }
        let customer_list = customer_repo
            .find_by(&customer_conditions)
            .map_err(|e| e.to_string())?;

        // PHP: $normalize = mb_strtolower(trim((string)$s), 'UTF-8')
        let normalize = |s: &str| s.trim().to_lowercase();
        // 构造 customer 映射：key = "cat_id|normalized_customer_name"
        let mut cust_map: std::collections::HashMap<String, Vec<Customer>> =
            std::collections::HashMap::new();
        for c in customer_list {
            let c_json = c.to_json();
            let cat_id = c_json.get("cat_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let name = c_json
                .get("customer_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = format!("{}|{}", cat_id, normalize(name));
            if key == "|" || key.is_empty() {
                continue;
            }
            cust_map.entry(key).or_default().push(c);
        }

        let mut rent_updated = 0usize;
        let mut cust_updated = 0usize;

        for ra in rentarea_list {
            let ra_json = ra.to_json();
            let cat_id = ra_json.get("cat_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let area_name = ra_json
                .get("area_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = format!("{}|{}", cat_id, normalize(area_name));
            if key == "|" || key.is_empty() {
                continue;
            }
            let matched = match cust_map.get(&key) {
                Some(list) if !list.is_empty() => list,
                _ => continue,
            };
            let first_customer_id = matched[0].pk();
            let rentarea_id = ra.pk();

            // 更新 rentarea.customer_id
            let current_customer_id = ra_json
                .get("customer_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if current_customer_id != first_customer_id {
                let mut data_map: std::collections::HashMap<String, Value> =
                    std::collections::HashMap::new();
                data_map.insert("customer_id".to_string(), json!(first_customer_id));
                let mut ra_model = ra;
                ra_model.set_attrs(&data_map);
                if rentarea_repo.save(ra_model).is_ok() {
                    rent_updated += 1;
                }
            }

            // 更新 customer.rentarea_ids
            for mc in matched {
                let mc_json = mc.to_json();
                let exist = mc_json
                    .get("rentarea_ids")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut arr: Vec<i64> = if exist.is_empty() {
                    Vec::new()
                } else {
                    exist
                        .split(',')
                        .filter_map(|s| {
                            let trimmed = s.trim();
                            if trimmed.is_empty() {
                                None
                            } else {
                                let n: i64 = trimmed.parse().unwrap_or(0);
                                if n == 0 {
                                    None
                                } else {
                                    Some(n)
                                }
                            }
                        })
                        .collect()
                };
                if !arr.contains(&rentarea_id) {
                    arr.push(rentarea_id);
                    let new_ids = arr
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("rentarea_ids".to_string(), json!(new_ids));
                    let mut mc_model = mc.clone();
                    mc_model.set_attrs(&data_map);
                    if customer_repo.save(mc_model).is_ok() {
                        cust_updated += 1;
                    }
                }
            }
        }

        Ok(format!(
            "已更新摊位:{rent_updated} 条, 更新商户 rentarea_ids:{cust_updated} 条"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_core::repository::InMemoryRepository;

    fn make_rentarea(id: i64, name: &str, app_id: i64, dept_id: i64, cat_id: i64) -> Rentarea {
        Rentarea::new()
            .with_data("rentarea_id", json!(id))
            .with_data("area_name", json!(name))
            .with_data("dept_id", json!(dept_id))
            .with_data("cat_id", json!(cat_id))
            .with_data("customer_id", json!(0))
            .with_data("status", json!(1))
            .with_data("rent", json!(0))
            .with_data("rent_day", json!(0))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    fn make_customer(id: i64, name: &str, app_id: i64, dept_id: i64, cat_id: i64) -> Customer {
        Customer::new()
            .with_data("customer_id", json!(id))
            .with_data("customer_name", json!(name))
            .with_data("dept_id", json!(dept_id))
            .with_data("cat_id", json!(cat_id))
            .with_data("rentarea_ids", json!(""))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    fn make_repo() -> InMemoryRepository<Rentarea> {
        InMemoryRepository::from_vec(vec![
            make_rentarea(1, "A01", 10001, 34, 100),
            make_rentarea(2, "A02", 10001, 34, 100),
            make_rentarea(3, "测试", 20002, 34, 100),
        ])
    }

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = RentareaController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_get_list_filters_by_keyword() {
        let repo = make_repo();
        let result = RentareaController::get_list(
            &repo,
            &json!({"app_id": 10001, "keyword": "A01"}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_add_rentarea_success() {
        let repo = make_repo();
        let result =
            RentareaController::add_rentarea(&repo, &json!({"area_name": "B01", "app_id": 10001}));
        assert!(result.is_ok());
        assert_eq!(repo.len(), 4);
    }

    #[test]
    fn test_edit_rentarea_success() {
        let repo = make_repo();
        let result = RentareaController::edit_rentarea(&repo, 1, &json!({"area_name": "更新名称"}));
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["area_name"], "更新名称");
    }

    #[test]
    fn test_set_delete_success() {
        let repo = make_repo();
        let result = RentareaController::set_delete(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let deleted = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(deleted.to_json()["is_delete"], 1);
    }

    #[test]
    fn test_cancel_sets_vacant_state() {
        let repo = make_repo();
        // PHP cancel 设置空置状态
        let vacant = json!({
            "status": 2,
            "rent": 0,
            "rent_day": 0,
            "customer_id": 0,
            "area_name": "空置"
        });
        let result = RentareaController::set_status(&repo, 1, &vacant);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "rentarea_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        let json = updated.to_json();
        assert_eq!(json["status"], 2);
        assert_eq!(json["area_name"], "空置");
        assert_eq!(json["customer_id"], 0);
    }

    #[test]
    fn test_select_level_rentarea() {
        let repo = make_repo();
        let result = RentareaController::select_level_rentarea(&repo, 34, 10001);
        let list = result.as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_sync_rentarea_customer_matches() {
        let rentarea_repo = make_repo();
        let customer_repo =
            InMemoryRepository::from_vec(vec![make_customer(100, "A01", 10001, 34, 100)]);

        let result = RentareaController::sync_rentarea_customer(
            &rentarea_repo,
            &customer_repo,
            &json!({"app_id": 10001}),
        );
        assert!(result.is_ok());
        let msg = result.unwrap();
        // 应至少更新 1 条 rentarea（cat_id=100, area_name="A01" 匹配）
        assert!(msg.contains("已更新摊位:1 条"));
    }

    #[test]
    fn test_r5_php_rentarea_get_list_returns_list_key() {
        let repo = make_repo();
        let result = RentareaController::get_list(&repo, &json!({"app_id": 10001}), "list");
        assert!(result["list"].is_array());
    }
}

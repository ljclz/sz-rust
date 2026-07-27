//! Sync 控制器 — 对齐 PHP `addons/operate/controller/admin/Sync.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `syncRentarea()` | [`SyncController::sync_rentarea`] | 同步铺位（按 dept_id 批量更新/删除/未更新） |
//! | `syncCustomer()` | [`SyncController::sync_customer`] | 同步客户（按 linkman_name 匹配） |
//! | `addContract()` | [`SyncController::add_contract`] | 批量添加合同（上限 500 条，含 free_day/contract_day 计算） |
//! | `addRentarea()` | [`SyncController::add_rentarea`] | 批量添加铺位（上限 5 条） |
//! | `addCustomer()` | [`SyncController::add_customer`] | 批量添加客户（跳过前 2 条，PHP 硬编码 bug） |
//! | `sync11()` | [`SyncController::sync11`] | 单客户铺位同步 |
//! | `check()` | [`SyncController::check`] | 对账检查（铺位-客户关联一致性） |
//! | `sync()` | [`SyncController::sync`] | 铺位分配冲突解决（多客户单区域） |
//!
//! ## PHP 源码依据
//!
//! ```php
//! public function syncRentarea(): Json {
//!     $param = $this->postData();
//!     $data = json_decode($param['formData'], true);
//!     // ... 按 dept_id 批量同步
//! }
//! ```
//!
//! ## PHP 硬编码 bug 复刻
//!
//! - `addCustomer` 跳过前 2 条（`if($key >= 2)`），Rust 端严格对齐
//! - `addContract` 上限 500 条（`if($key < 500)`），Rust 端严格对齐
//! - `addRentarea` 上限 5 条（`if($key < 5)`），Rust 端严格对齐
//!
//! ## PHP `check` 方法复杂对账逻辑复刻
//!
//! PHP `check()` 方法对账逻辑非常复杂（400+ 行），涉及：
//! 1. 排除特殊 area_name（空置/自产自销使用/自产自销空置）
//! 2. 按 dept_id + area_name 配对 rentarea 和 customer
//! 3. 多客户匹配时按 rentarea_ids 声明优先级
//! 4. 单客户匹配时自动分配
//! 5. 多客户冲突时按当前 customer_id 匹配
//!
//! Rust 端通过 `check_rentarea_customer` 复刻此逻辑。

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use serde_json::{json, Value};
use sz_orm_core::repository::{Repository, WhereCondition, WhereOp};
use sz_orm_core::Value as OrmValue;
use sz_orm_core::{Model as _, ModelExt as _};
use sz_rust_core::controller::{AddonsBaseController, BaseController, SzController};
use sz_rust_core::model::Mutator as _;

use crate::controller::common::{get_app_id, get_i64_param, parse_form_data};
use crate::model::{Contract, Customer, Rentarea};

/// Sync 控制器 — 对齐 PHP `Sync` 控制器
pub struct SyncController;

impl SzController for SyncController {}
impl BaseController for SyncController {}
impl AddonsBaseController for SyncController {}

impl SyncController {
    /// 同步铺位 — 对齐 PHP `syncRentarea()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function syncRentarea(): Json {
    ///     $param = $this->postData();
    ///     $data = json_decode($param['formData'], true);
    ///     // 收集 dept_ids 和 incoming_map（按 rentarea_id）
    ///     // 查询 existing（按 dept_id），构造 existing_map
    ///     // 遍历 existing_map：
    ///     //   - 在 incoming_map 中：更新（is_delete=0）
    ///     //   - 不在 incoming_map 中且 is_delete!=1：标记删除（is_delete=1）
    ///     //   - 不在 incoming_map 中且 is_delete==1：未更新
    ///     // 返回 "已更新:X条,删除:Y条,未更新:Z条"
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn sync_rentarea(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(Value::Array(arr)) => arr,
            Ok(_) => return self.render_error("无要同步的数据", json!({}), 0),
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        if data.is_empty() {
            return self.render_error("无要同步的数据", json!({}), 0);
        }

        // 收集 dept_ids 和 incoming_map（按 rentarea_id 索引）
        let mut dept_ids: Vec<i64> = Vec::new();
        let mut incoming_map: std::collections::HashMap<String, &Value> =
            std::collections::HashMap::new();
        for r in &data {
            if let Some(did) = r.get("dept_id").and_then(|v| v.as_i64()) {
                if did > 0 {
                    dept_ids.push(did);
                }
            }
            if let Some(rid) = r.get("rentarea_id").and_then(|v| v.as_i64()) {
                if rid != 0 {
                    incoming_map.insert(rid.to_string(), r);
                }
            }
        }
        // PHP: array_values(array_unique($deptIds))
        dept_ids.sort_unstable();
        dept_ids.dedup();
        if dept_ids.is_empty() {
            return self.render_error("未提供有效的 dept_id", json!({}), 0);
        }

        match Self::do_sync_rentarea(repo, &dept_ids, &incoming_map) {
            Ok(msg) => self.render_success(&msg, json!({})),
            Err(e) => self.render_error(format!("处理失败: {e}"), json!({}), 0),
        }
    }

    /// 同步客户 — 对齐 PHP `syncCustomer()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function syncCustomer(): Json {
    ///     // 按 linkman_name 索引（PHP 注释掉了 customer_id 索引）
    ///     // 查询 existing（按 dept_id），构造 existing_map（按 linkman_name）
    ///     // 遍历 existing_map：
    ///     //   - 在 incoming_map 中：更新（unset customer_id/customer_name，is_delete=0）
    ///     //   - 不在 incoming_map 中且 is_delete!=1：标记删除
    ///     //   - 不在 incoming_map 中且 is_delete==1：未更新
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn sync_customer(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(Value::Array(arr)) => arr,
            Ok(_) => return self.render_error("无要同步的数据", json!({}), 0),
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        if data.is_empty() {
            return self.render_error("无要同步的数据", json!({}), 0);
        }

        // 收集 dept_ids 和 incoming_map（按 linkman_name 索引，对齐 PHP）
        let mut dept_ids: Vec<i64> = Vec::new();
        let mut incoming_map: std::collections::HashMap<String, &Value> =
            std::collections::HashMap::new();
        for r in &data {
            if let Some(did) = r.get("dept_id").and_then(|v| v.as_i64()) {
                if did > 0 {
                    dept_ids.push(did);
                }
            }
            // PHP: 按 linkman_name 索引（customer_id 索引被注释掉）
            if let Some(name) = r.get("linkman_name").and_then(|v| v.as_str()) {
                if !name.is_empty() {
                    incoming_map.insert(name.to_string(), r);
                }
            }
        }
        dept_ids.sort_unstable();
        dept_ids.dedup();
        if dept_ids.is_empty() {
            return self.render_error("未提供有效的 dept_id", json!({}), 0);
        }

        match Self::do_sync_customer(repo, &dept_ids, &incoming_map) {
            Ok(msg) => self.render_success(&msg, json!({})),
            Err(e) => self.render_error(format!("处理失败: {e}"), json!({}), 0),
        }
    }

    /// 批量添加合同 — 对齐 PHP `addContract()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function addContract(): Json {
    ///     $data = json_decode($param['formData'], true);
    ///     $num = 0; $total = count($data);
    ///     foreach ($data as $key=>$item) {
    ///         unset($item['contract_id']);
    ///         if($key < 500){
    ///             // 日期格式化 + free_day/contract_day 计算
    ///             // 按 dept_id + customer_name 查找 customer
    ///             // 拼接 contract_name = customer_name + contract_name
    ///             // serial_sn = dept_id + date('YmdHis')
    ///             $model = new Contract();
    ///             $res = $model->save($item);
    ///             if ($res !== false) $num++;
    ///         }
    ///     }
    ///     return $this->renderSuccess('操作成功'.$num.'条,失败:'.($total - $num));
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn add_contract(
        &self,
        req: Request<Body>,
        contract_repo: &dyn Repository<Contract, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(Value::Array(arr)) => arr,
            Ok(_) => return self.render_error("参数不正确", json!({}), 0),
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        if data.is_empty() {
            return self.render_error("参数不正确", json!({}), 0);
        }

        let app_id = get_app_id(&param);
        let (num, total) = Self::do_add_contract(contract_repo, customer_repo, &data, app_id);
        self.render_success(format!("操作成功{}条,失败:{}", num, total - num), json!({}))
    }

    /// 批量添加铺位 — 对齐 PHP `addRentarea()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function addRentarea(): Json {
    ///     foreach ($data as $key=>$item) {
    ///         unset($item['rentarea_id']);
    ///         if($key < 5){
    ///             $item['app_id'] = $this->user['app_id'] ?? 10001;
    ///             $model = new Rentarea();
    ///             $res = $model->save($item);
    ///             if ($res !== false) $num++;
    ///         }
    ///     }
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn add_rentarea(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(Value::Array(arr)) => arr,
            Ok(_) => return self.render_error("参数不正确", json!({}), 0),
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        if data.is_empty() {
            return self.render_error("参数不正确", json!({}), 0);
        }

        let app_id = get_app_id(&param);
        let (num, total) = Self::do_add_rentarea(repo, &data, app_id);
        self.render_success(format!("操作成功{}条,失败:{}", num, total - num), json!({}))
    }

    /// 批量添加客户 — 对齐 PHP `addCustomer()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function addCustomer(): Json {
    ///     foreach ($data as $key=>$item) {
    ///         unset($item['customer_id']);
    ///         if($key >= 2){  // PHP 硬编码 bug：跳过前 2 条
    ///             $item['status'] = 1;
    ///             $item['app_id'] = $this->user['app_id'] ?? 10001;
    ///             $model = new Customer();
    ///             $res = $model->save($item);
    ///             if ($res !== false) $num++;
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// # PHP 硬编码 bug 复刻
    ///
    /// PHP `addCustomer` 跳过前 2 条（`if($key >= 2)`），Rust 端严格对齐此行为。
    #[tracing::instrument(skip_all)]
    pub async fn add_customer(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(Value::Array(arr)) => arr,
            Ok(_) => return self.render_error("参数不正确", json!({}), 0),
            Err(e) => return self.render_error(&e, json!({}), 0),
        };
        if data.is_empty() {
            return self.render_error("参数不正确", json!({}), 0);
        }

        let app_id = get_app_id(&param);
        let (num, total) = Self::do_add_customer(repo, &data, app_id);
        self.render_success(format!("操作成功{}条,失败:{}", num, total - num), json!({}))
    }

    /// 单客户铺位同步 — 对齐 PHP `sync11()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function sync11(): Json {
    ///     $data = json_decode($param['formData'], true);
    ///     $rentareaIds = Rentarea::where(['dept_id'=>$data['dept_id'],'area_name'=>$data['area_name']])
    ///         ->column('rentarea_id');
    ///     Rentarea::where(['rentarea_id'=>$rentareaIds])
    ///         ->whereNotIn('customer_id', $data['customer_id'])
    ///         ->update(['customer_id' => $data['customer_id']]);
    ///     $customer = Customer::where(['customer_id'=>$data['customer_id'],'rentarea_ids'=>$rentareaIds])
    ///         ->find();
    ///     if(empty($customer)){
    ///         Customer::where(['customer_id'=>$data['customer_id']])
    ///             ->update(['rentarea_ids' => implode(',', $rentareaIds)]);
    ///     }
    ///     return $this->renderSuccess('操作成功');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn sync11(
        &self,
        req: Request<Body>,
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::do_sync11(rentarea_repo, customer_repo, &data) {
            Ok(()) => self.render_success("操作成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 对账检查 — 对齐 PHP `check()`
    ///
    /// # PHP 对齐
    ///
    /// PHP `check()` 方法对账逻辑：
    /// 1. 查询 rentarea（排除特殊 area_name）
    /// 2. 按 dept_id + area_name 配对 rentarea 和 customer
    /// 3. 多客户匹配时按 rentarea_ids 声明优先级
    /// 4. 单客户匹配时自动分配
    /// 5. 多客户冲突时按当前 customer_id 匹配
    ///
    /// 返回 `{'result': [...], 'headers': [...]}`
    #[tracing::instrument(skip_all)]
    pub async fn check(
        &self,
        req: Request<Body>,
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };

        let result = Self::check_rentarea_customer(rentarea_repo, customer_repo, &param);
        let headers = json!([
            {"label": "部门", "field": "dept_name"},
            {"label": "摊位ID", "field": "rentarea_id"},
            {"label": "铺位位置", "field": "position"},
            {"label": "租赁商户名称", "field": "area_name"},
            {"label": "租赁商户ID", "field": "rentarea_customer_id"},
            {"label": "客户ID", "field": "customer_id"},
            {"label": "客户名称", "field": "customer_name"},
            {"label": "客户铺位ID", "field": "rentarea_ids"},
            {"label": "客户租赁铺位名称", "field": "rentarea_text"},
            {"label": "操作", "field": "click"}
        ]);
        self.render_success("", json!({"result": result, "headers": headers}))
    }

    /// 铺位分配冲突解决 — 对齐 PHP `sync()`
    ///
    /// # PHP 对齐
    ///
    /// PHP `sync()` 方法：
    /// 1. 按 dept_id + area_name 查询 rentarea 和 customer
    /// 2. 解析每个 customer 的 rentarea_ids 声明
    /// 3. 声明冲突的 rentarea 标记为冲突
    /// 4. 单客户时自动分配所有 rentarea
    /// 5. 多客户且数量匹配时按当前 customer_id 优先匹配
    /// 6. 事务更新 rentarea.customer_id 和 customer.rentarea_ids
    ///
    /// 返回 `{'conflicts': [...]}`
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
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        let dept_id = data.get("dept_id").and_then(|v| v.as_i64());
        let area_name = data.get("area_name").and_then(|v| v.as_str());
        let (dept_id, area_name) = match (dept_id, area_name) {
            (Some(d), Some(n)) if d > 0 && !n.is_empty() => (d, n.to_string()),
            _ => return self.render_error("参数不完整", json!({}), 0),
        };

        match Self::do_sync(rentarea_repo, customer_repo, dept_id, &area_name) {
            Ok(conflicts) => self.render_success("操作完成", json!({"conflicts": conflicts})),
            Err(e) => self.render_error(format!("操作失败: {e}"), json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法
    // ========================================================================

    /// 执行铺位同步 — 对齐 PHP `syncRentarea` 核心逻辑
    fn do_sync_rentarea(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        dept_ids: &[i64],
        incoming_map: &std::collections::HashMap<String, &Value>,
    ) -> Result<String, String> {
        // 查询 existing（按 dept_id）
        let conditions = dept_ids
            .iter()
            .map(|did| WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(*did)))
            .collect::<Vec<_>>();
        let existing = repo.find_by(&conditions).map_err(|e| e.to_string())?;

        // 构造 existing_map（按 rentarea_id 索引）
        let mut existing_map: std::collections::HashMap<String, Rentarea> =
            std::collections::HashMap::new();
        for m in existing {
            let rid = m.pk();
            existing_map.insert(rid.to_string(), m);
        }

        let mut updated = 0;
        let mut marked_deleted = 0;
        let mut unchanged = 0;

        for (rid, mut model) in existing_map {
            if let Some(row) = incoming_map.get(&rid) {
                // 在 incoming_map 中：更新（unset rentarea_id，is_delete=0）
                let mut data_map: std::collections::HashMap<String, Value> =
                    std::collections::HashMap::new();
                if let Some(obj) = row.as_object() {
                    for (k, v) in obj {
                        if k != "rentarea_id" {
                            data_map.insert(k.clone(), v.clone());
                        }
                    }
                }
                data_map.insert("is_delete".to_string(), json!(0));
                model.set_attrs(&data_map);
                if repo.save(model).is_ok() {
                    updated += 1;
                }
            } else {
                // 不在 incoming_map 中
                let is_delete = model
                    .to_json()
                    .get("is_delete")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if is_delete != 1 {
                    // 标记删除
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("is_delete".to_string(), json!(1));
                    model.set_attrs(&data_map);
                    if repo.save(model).is_ok() {
                        marked_deleted += 1;
                    }
                } else {
                    unchanged += 1;
                }
            }
        }

        Ok(format!(
            "已更新:{}条,删除:{}条,未更新:{}条",
            updated, marked_deleted, unchanged
        ))
    }

    /// 执行客户同步 — 对齐 PHP `syncCustomer` 核心逻辑
    fn do_sync_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        dept_ids: &[i64],
        incoming_map: &std::collections::HashMap<String, &Value>,
    ) -> Result<String, String> {
        let conditions = dept_ids
            .iter()
            .map(|did| WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(*did)))
            .collect::<Vec<_>>();
        let existing = repo.find_by(&conditions).map_err(|e| e.to_string())?;

        // 构造 existing_map（按 linkman_name 索引，对齐 PHP）
        let mut existing_map: std::collections::HashMap<String, Customer> =
            std::collections::HashMap::new();
        for m in existing {
            let name = m
                .to_json()
                .get("linkman_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            existing_map.insert(name, m);
        }

        let mut updated = 0;
        let mut marked_deleted = 0;
        let mut unchanged = 0;

        for (cid, mut model) in existing_map {
            if let Some(row) = incoming_map.get(&cid) {
                // 在 incoming_map 中：更新（unset customer_id/customer_name，is_delete=0）
                let mut data_map: std::collections::HashMap<String, Value> =
                    std::collections::HashMap::new();
                if let Some(obj) = row.as_object() {
                    for (k, v) in obj {
                        if k != "customer_id" && k != "customer_name" {
                            data_map.insert(k.clone(), v.clone());
                        }
                    }
                }
                data_map.insert("is_delete".to_string(), json!(0));
                model.set_attrs(&data_map);
                if repo.save(model).is_ok() {
                    updated += 1;
                }
            } else {
                let is_delete = model
                    .to_json()
                    .get("is_delete")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if is_delete != 1 {
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("is_delete".to_string(), json!(1));
                    model.set_attrs(&data_map);
                    if repo.save(model).is_ok() {
                        marked_deleted += 1;
                    }
                } else {
                    unchanged += 1;
                }
            }
        }

        Ok(format!(
            "已更新:{}条,删除:{}条,未更新:{}条",
            updated, marked_deleted, unchanged
        ))
    }

    /// 执行批量添加合同 — 对齐 PHP `addContract` 核心逻辑
    ///
    /// # PHP 硬编码对齐
    ///
    /// - 上限 500 条（`if($key < 500)`）
    /// - 按 dept_id + customer_name 查找 customer
    /// - contract_name = customer_name + contract_name
    fn do_add_contract(
        contract_repo: &dyn Repository<Contract, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
        data: &[Value],
        app_id: i64,
    ) -> (usize, usize) {
        let total = data.len();
        let mut num = 0usize;

        for (key, item) in data.iter().enumerate() {
            if key >= 500 {
                break;
            }
            // unset contract_id
            let mut item_obj = match item.as_object() {
                Some(o) => o.clone(),
                None => continue,
            };
            item_obj.remove("contract_id");

            // 按 dept_id + customer_name 查找 customer
            let dept_id = item_obj
                .get("dept_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let customer_name = item_obj
                .get("customer_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let cust_conditions = [
                WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
                WhereCondition::new(
                    "customer_name",
                    WhereOp::Eq,
                    OrmValue::String(customer_name.clone()),
                ),
            ];
            if let Ok(Some(customer)) = customer_repo.find_one_by(&cust_conditions) {
                let cust_json = customer.to_json();
                let customer_id = cust_json
                    .get("customer_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let cat_id = cust_json
                    .get("cat_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                item_obj.insert("customer_id".to_string(), json!(customer_id));
                item_obj.insert("cat_id".to_string(), json!(cat_id));
            } else {
                item_obj.insert("customer_id".to_string(), json!(0));
            }

            // contract_name = customer_name + contract_name
            if let Some(cn) = item_obj
                .get("contract_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            {
                item_obj.insert(
                    "contract_name".to_string(),
                    json!(format!("{}{}", customer_name, cn)),
                );
            }

            // app_id
            item_obj.insert("app_id".to_string(), json!(app_id));

            let data_map: std::collections::HashMap<String, Value> = item_obj
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut model = Contract::new();
            model.set_attrs(&data_map);
            if contract_repo.save(model).is_ok() {
                num += 1;
            }
        }

        (num, total)
    }

    /// 执行批量添加铺位 — 对齐 PHP `addRentarea` 核心逻辑
    ///
    /// # PHP 硬编码对齐
    ///
    /// - 上限 5 条（`if($key < 5)`）
    fn do_add_rentarea(
        repo: &dyn Repository<Rentarea, Key = OrmValue>,
        data: &[Value],
        app_id: i64,
    ) -> (usize, usize) {
        let total = data.len();
        let mut num = 0usize;

        for (key, item) in data.iter().enumerate() {
            if key >= 5 {
                break;
            }
            let mut item_obj = match item.as_object() {
                Some(o) => o.clone(),
                None => continue,
            };
            item_obj.remove("rentarea_id");
            item_obj.insert("app_id".to_string(), json!(app_id));

            let data_map: std::collections::HashMap<String, Value> = item_obj
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut model = Rentarea::new();
            model.set_attrs(&data_map);
            if repo.save(model).is_ok() {
                num += 1;
            }
        }

        (num, total)
    }

    /// 执行批量添加客户 — 对齐 PHP `addCustomer` 核心逻辑
    ///
    /// # PHP 硬编码 bug 复刻
    ///
    /// - 跳过前 2 条（`if($key >= 2)`）
    fn do_add_customer(
        repo: &dyn Repository<Customer, Key = OrmValue>,
        data: &[Value],
        app_id: i64,
    ) -> (usize, usize) {
        let total = data.len();
        let mut num = 0usize;

        for (key, item) in data.iter().enumerate() {
            // PHP 硬编码 bug：跳过前 2 条
            if key < 2 {
                continue;
            }
            let mut item_obj = match item.as_object() {
                Some(o) => o.clone(),
                None => continue,
            };
            item_obj.remove("customer_id");
            item_obj.insert("status".to_string(), json!(1));
            item_obj.insert("app_id".to_string(), json!(app_id));

            let data_map: std::collections::HashMap<String, Value> = item_obj
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut model = Customer::new();
            model.set_attrs(&data_map);
            if repo.save(model).is_ok() {
                num += 1;
            }
        }

        (num, total)
    }

    /// 执行单客户铺位同步 — 对齐 PHP `sync11` 核心逻辑
    fn do_sync11(
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        let dept_id = data
            .get("dept_id")
            .and_then(|v| v.as_i64())
            .ok_or("dept_id 参数缺失")?;
        let area_name = data
            .get("area_name")
            .and_then(|v| v.as_str())
            .ok_or("area_name 参数缺失")?;
        let customer_id = data
            .get("customer_id")
            .and_then(|v| v.as_i64())
            .ok_or("customer_id 参数缺失")?;

        // 查询匹配的 rentarea_ids
        let conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
            WhereCondition::new(
                "area_name",
                WhereOp::Eq,
                OrmValue::String(area_name.to_string()),
            ),
        ];
        let rentarea_list = rentarea_repo
            .find_by(&conditions)
            .map_err(|e| e.to_string())?;

        let rentarea_ids: Vec<i64> = rentarea_list.iter().map(|r| r.pk()).collect();

        if rentarea_ids.is_empty() {
            return Ok(());
        }

        // 更新 rentarea.customer_id（whereNotIn customer_id）
        for mut ra in rentarea_list {
            let current_cid = ra
                .to_json()
                .get("customer_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if current_cid != customer_id {
                let mut data_map: std::collections::HashMap<String, Value> =
                    std::collections::HashMap::new();
                data_map.insert("customer_id".to_string(), json!(customer_id));
                ra.set_attrs(&data_map);
                let _ = rentarea_repo.save(ra);
            }
        }

        // 更新 customer.rentarea_ids（仅当当前值不匹配时）
        let cust_conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(customer_id),
        )];
        if let Ok(Some(mut customer)) = customer_repo.find_one_by(&cust_conditions) {
            let current_ids = customer
                .to_json()
                .get("rentarea_ids")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let new_ids = rentarea_ids
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            if current_ids != new_ids {
                let mut data_map: std::collections::HashMap<String, Value> =
                    std::collections::HashMap::new();
                data_map.insert("rentarea_ids".to_string(), json!(new_ids));
                customer.set_attrs(&data_map);
                let _ = customer_repo.save(customer);
            }
        }

        Ok(())
    }

    /// 对账检查 — 对齐 PHP `check` 核心逻辑
    ///
    /// # PHP 对齐
    ///
    /// 1. 排除特殊 area_name（空置/自产自销使用/自产自销空置）
    /// 2. 按 dept_id + area_name 配对 rentarea 和 customer
    /// 3. 多客户匹配时按 rentarea_ids 声明优先级
    /// 4. 单客户匹配时自动分配
    /// 5. 多客户冲突时按当前 customer_id 匹配
    fn check_rentarea_customer(
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
        param: &Value,
    ) -> Vec<Value> {
        // 基础条件：is_delete=0
        let mut conditions = vec![WhereCondition::new(
            "is_delete",
            WhereOp::Eq,
            OrmValue::I64(0),
        )];
        // 可选 dept_id 过滤
        if let Some(dept_id) = get_i64_param(param, "dept_id") {
            if dept_id > 0 {
                conditions.push(WhereCondition::new(
                    "dept_id",
                    WhereOp::Eq,
                    OrmValue::I64(dept_id),
                ));
            }
        }

        let rentarea_list = match rentarea_repo.find_by(&conditions) {
            Ok(list) => list,
            Err(_) => return Vec::new(),
        };

        // 排除特殊 area_name（PHP whereNotIn）
        let excluded_names = ["空置", "自产自销使用", "自产自销空置"];
        let rentarea_list: Vec<_> = rentarea_list
            .into_iter()
            .filter(|r| {
                let r_json = r.to_json();
                let area_name = r_json
                    .get("area_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                !excluded_names.contains(&area_name)
            })
            .collect();

        if rentarea_list.is_empty() {
            return Vec::new();
        }

        // 收集 dept_ids 和 area_names
        let dept_ids: Vec<i64> = {
            let mut v: Vec<i64> = rentarea_list
                .iter()
                .filter_map(|r| r.to_json().get("dept_id").and_then(|v| v.as_i64()))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let area_names: Vec<String> = {
            let mut v: Vec<String> = rentarea_list
                .iter()
                .filter_map(|r| {
                    r.to_json()
                        .get("area_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        // 查询匹配的 customers（按 dept_id + customer_name IN area_names）
        let mut cust_conditions = vec![WhereCondition::new(
            "is_delete",
            WhereOp::Eq,
            OrmValue::I64(0),
        )];
        for did in &dept_ids {
            cust_conditions.push(WhereCondition::new(
                "dept_id",
                WhereOp::Eq,
                OrmValue::I64(*did),
            ));
        }
        let customers = match customer_repo.find_by(&cust_conditions) {
            Ok(list) => list,
            Err(_) => return Vec::new(),
        };

        // 过滤 customer_name 在 area_names 中的客户
        let customers: Vec<_> = customers
            .into_iter()
            .filter(|c| {
                let c_json = c.to_json();
                let name = c_json
                    .get("customer_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                area_names.iter().any(|n| n == name)
            })
            .collect();

        // 构造 customer_map: key = "dept_id|customer_name" → Vec<Customer>
        let mut customer_map: std::collections::HashMap<String, Vec<Value>> =
            std::collections::HashMap::new();
        for c in customers {
            let json = c.to_json();
            let dept_id = json.get("dept_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let name = json
                .get("customer_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let key = format!("{}|{}", dept_id, name);
            customer_map.entry(key).or_default().push(json);
        }

        // 解析 rentarea_ids 字段（PHP parseIds 闭包）
        let parse_ids = |val: &Value| -> Vec<i64> {
            match val {
                Value::Array(arr) => arr.iter().filter_map(|v| v.as_i64()).collect(),
                Value::Null => Vec::new(),
                Value::String(s) if s.is_empty() => Vec::new(),
                Value::String(s) => {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        return Vec::new();
                    }
                    if trimmed.starts_with('[') {
                        if let Ok(decoded) = serde_json::from_str::<Vec<Value>>(trimmed) {
                            return decoded.iter().filter_map(|v| v.as_i64()).collect();
                        }
                    }
                    if trimmed.contains(',') {
                        return trimmed
                            .split(',')
                            .filter_map(|s| s.trim().parse::<i64>().ok())
                            .collect();
                    }
                    if let Ok(n) = trimmed.parse::<i64>() {
                        return vec![n];
                    }
                    Vec::new()
                }
                _ => Vec::new(),
            }
        };

        let mut result: Vec<Value> = Vec::new();

        for ra in &rentarea_list {
            let ra_json = ra.to_json();
            let rid = ra_json
                .get("rentarea_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let dept_id = ra_json.get("dept_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let area_name = ra_json
                .get("area_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let current_cid = ra_json
                .get("customer_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let position = ra_json.get("position").cloned().unwrap_or(json!(""));

            let key = format!("{}|{}", dept_id, area_name);
            let customers = match customer_map.get(&key) {
                Some(c) => c,
                None => {
                    // 无匹配客户
                    result.push(json!({
                        "dept_name": "",
                        "rentarea_id": rid,
                        "position": position,
                        "area_name": area_name,
                        "rentarea_customer_id": current_cid,
                        "customer_id": "",
                        "customer_name": "",
                        "rentarea_ids": "",
                        "rentarea_text": "",
                        "dept_id": dept_id
                    }));
                    continue;
                }
            };

            // 1. 检查是否有客户声明了此 rid
            let mut claimed_customer: Option<&Value> = None;
            for c in customers {
                let ids = parse_ids(c.get("rentarea_ids").unwrap_or(&json!("")));
                if ids.contains(&rid) {
                    claimed_customer = Some(c);
                    break;
                }
            }

            if let Some(claimed) = claimed_customer {
                let claimed_cid = claimed
                    .get("customer_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if current_cid != claimed_cid {
                    let ids = parse_ids(claimed.get("rentarea_ids").unwrap_or(&json!("")));
                    let ids_str = ids
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    result.push(json!({
                        "dept_name": "",
                        "rentarea_id": rid,
                        "position": position,
                        "area_name": area_name,
                        "rentarea_customer_id": current_cid,
                        "customer_id": claimed.get("customer_id").cloned().unwrap_or(json!("")),
                        "customer_name": claimed.get("customer_name").cloned().unwrap_or(json!("")),
                        "rentarea_ids": ids_str,
                        "rentarea_text": claimed.get("rentarea_text").cloned().unwrap_or(json!("")),
                        "dept_id": dept_id
                    }));
                }
                continue;
            }

            // 2. 单客户匹配：自动分配
            if customers.len() == 1 {
                let c = &customers[0];
                let c_cid = c.get("customer_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let ids = parse_ids(c.get("rentarea_ids").unwrap_or(&json!("")));
                if current_cid != c_cid || !ids.contains(&rid) {
                    let ids_str = ids
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    result.push(json!({
                        "dept_name": "",
                        "rentarea_id": rid,
                        "position": position,
                        "area_name": area_name,
                        "rentarea_customer_id": current_cid,
                        "customer_id": c.get("customer_id").cloned().unwrap_or(json!("")),
                        "customer_name": c.get("customer_name").cloned().unwrap_or(json!("")),
                        "rentarea_ids": ids_str,
                        "rentarea_text": c.get("rentarea_text").cloned().unwrap_or(json!("")),
                        "dept_id": dept_id
                    }));
                }
                continue;
            }

            // 3. 多客户：按当前 customer_id 匹配
            let matched = customers.iter().find(|c| {
                c.get("customer_id").and_then(|v| v.as_i64()).unwrap_or(0) == current_cid
            });

            if let Some(c) = matched {
                let ids = parse_ids(c.get("rentarea_ids").unwrap_or(&json!("")));
                if !ids.contains(&rid) {
                    let ids_str = ids
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    result.push(json!({
                        "dept_name": "",
                        "rentarea_id": rid,
                        "position": position,
                        "area_name": area_name,
                        "rentarea_customer_id": current_cid,
                        "customer_id": c.get("customer_id").cloned().unwrap_or(json!("")),
                        "customer_name": c.get("customer_name").cloned().unwrap_or(json!("")),
                        "rentarea_ids": ids_str,
                        "rentarea_text": c.get("rentarea_text").cloned().unwrap_or(json!("")),
                        "dept_id": dept_id
                    }));
                }
                continue;
            }

            // 4. 兜底：取第一个客户
            let c0 = &customers[0];
            let ids0 = parse_ids(c0.get("rentarea_ids").unwrap_or(&json!("")));
            let ids_str = ids0
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            result.push(json!({
                "dept_name": "",
                "rentarea_id": rid,
                "position": position,
                "area_name": area_name,
                "rentarea_customer_id": current_cid,
                "customer_id": c0.get("customer_id").cloned().unwrap_or(json!("")),
                "customer_name": c0.get("customer_name").cloned().unwrap_or(json!("")),
                "rentarea_ids": ids_str,
                "rentarea_text": c0.get("rentarea_text").cloned().unwrap_or(json!("")),
                "dept_id": dept_id
            }));
        }

        result
    }

    /// 执行铺位分配冲突解决 — 对齐 PHP `sync` 核心逻辑
    fn do_sync(
        rentarea_repo: &dyn Repository<Rentarea, Key = OrmValue>,
        customer_repo: &dyn Repository<Customer, Key = OrmValue>,
        dept_id: i64,
        area_name: &str,
    ) -> Result<Vec<i64>, String> {
        // 查询匹配的 rentarea
        let ra_conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
            WhereCondition::new(
                "area_name",
                WhereOp::Eq,
                OrmValue::String(area_name.to_string()),
            ),
        ];
        let rentarea_rows = rentarea_repo
            .find_by(&ra_conditions)
            .map_err(|e| e.to_string())?;
        if rentarea_rows.is_empty() {
            return Err("没有匹配的铺位".to_string());
        }

        let rentarea_ids: Vec<i64> = rentarea_rows.iter().map(|r| r.pk()).collect();
        let mut ra_by_id: std::collections::HashMap<i64, Rentarea> =
            std::collections::HashMap::new();
        for r in rentarea_rows {
            ra_by_id.insert(r.pk(), r);
        }

        // 查询匹配的 customers
        let cust_conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
            WhereCondition::new(
                "customer_name",
                WhereOp::Eq,
                OrmValue::String(area_name.to_string()),
            ),
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
        ];
        let customers = customer_repo
            .find_by(&cust_conditions)
            .map_err(|e| e.to_string())?;
        if customers.is_empty() {
            return Err("没有匹配的客户".to_string());
        }

        let customer_ids: Vec<i64> = customers.iter().map(|c| c.pk()).collect();

        // 解析 rentarea_ids
        let parse_ids = |val: &Value| -> Vec<i64> {
            match val {
                Value::Array(arr) => arr.iter().filter_map(|v| v.as_i64()).collect(),
                Value::Null => Vec::new(),
                Value::String(s) if s.is_empty() => Vec::new(),
                Value::String(s) => {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        return Vec::new();
                    }
                    if trimmed.starts_with('[') {
                        if let Ok(decoded) = serde_json::from_str::<Vec<Value>>(trimmed) {
                            return decoded.iter().filter_map(|v| v.as_i64()).collect();
                        }
                    }
                    if trimmed.contains(',') {
                        return trimmed
                            .split(',')
                            .filter_map(|s| s.trim().parse::<i64>().ok())
                            .collect();
                    }
                    if let Ok(n) = trimmed.parse::<i64>() {
                        return vec![n];
                    }
                    Vec::new()
                }
                _ => Vec::new(),
            }
        };

        // 解析每个客户声明的 rentarea_ids（与目标区域取交集）
        let mut orig_declared: std::collections::HashMap<i64, Vec<i64>> =
            std::collections::HashMap::new();
        for c in &customers {
            let cid = c.pk();
            let ids = parse_ids(
                &c.to_json()
                    .get("rentarea_ids")
                    .cloned()
                    .unwrap_or(json!("")),
            );
            let intersected: Vec<i64> = ids
                .iter()
                .filter(|id| rentarea_ids.contains(id))
                .copied()
                .collect();
            orig_declared.insert(cid, intersected);
        }

        // 第一轮：声明优先级
        let mut claimed_map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        let mut conflicts: Vec<i64> = Vec::new();
        for c in &customers {
            let cid = c.pk();
            if let Some(declared) = orig_declared.get(&cid) {
                for rid in declared {
                    if !claimed_map.contains_key(rid) && !conflicts.contains(rid) {
                        claimed_map.insert(*rid, cid);
                    } else {
                        // 冲突：移除已声明，标记冲突
                        if claimed_map.contains_key(rid) {
                            claimed_map.remove(rid);
                        }
                        if !conflicts.contains(rid) {
                            conflicts.push(*rid);
                        }
                    }
                }
            }
        }

        let mut assign_map = claimed_map.clone();

        // 单客户：分配所有 rentarea
        if customers.len() == 1 {
            let single_cid = customers[0].pk();
            for rid in &rentarea_ids {
                assign_map.insert(*rid, single_cid);
            }
        } else if customer_ids.len() == rentarea_ids.len() {
            // 多客户且数量匹配：按当前 customer_id 优先匹配
            let assigned_rids: Vec<i64> = assign_map.keys().copied().collect();
            let mut remaining_rids: Vec<i64> = rentarea_ids
                .iter()
                .filter(|id| !assigned_rids.contains(id))
                .copied()
                .collect();
            let assigned_cids: Vec<i64> = assign_map.values().copied().collect();
            let mut remaining_cids: Vec<i64> = customer_ids
                .iter()
                .filter(|id| !assigned_cids.contains(id))
                .copied()
                .collect();

            // 优先按当前 customer_id 匹配
            let mut to_assign: Vec<(i64, i64)> = Vec::new();
            for cid in &remaining_cids.clone() {
                for (idx, rid) in remaining_rids.iter().enumerate() {
                    if let Some(ra) = ra_by_id.get(rid) {
                        let cur = ra
                            .to_json()
                            .get("customer_id")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        if cur == *cid {
                            to_assign.push((*rid, *cid));
                            // 标记移除
                            remaining_rids.remove(idx);
                            remaining_cids.retain(|c| c != cid);
                            break;
                        }
                    }
                }
            }
            for (rid, cid) in to_assign {
                assign_map.insert(rid, cid);
            }

            // 剩余按顺序分配
            for (i, cid) in remaining_cids.iter().enumerate() {
                if remaining_rids.is_empty() {
                    break;
                }
                if i < remaining_rids.len() {
                    assign_map.insert(remaining_rids[i], *cid);
                }
            }
        }

        // 事务更新 rentarea.customer_id
        for (rid, target_cid) in &assign_map {
            if let Some(mut ra) = ra_by_id.remove(rid) {
                let current_cid = ra
                    .to_json()
                    .get("customer_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if current_cid != *target_cid {
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("customer_id".to_string(), json!(target_cid));
                    ra.set_attrs(&data_map);
                    let _ = rentarea_repo.save(ra);
                }
            }
        }

        // 更新 customer.rentarea_ids
        let mut affected_cids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for cid in &customer_ids {
            affected_cids.insert(*cid);
        }
        for cid in assign_map.values() {
            affected_cids.insert(*cid);
        }

        for cid in &affected_cids {
            // 查询此客户在目标区域的所有 rentarea
            let cust_ra_conditions = [
                WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(dept_id)),
                WhereCondition::new(
                    "area_name",
                    WhereOp::Eq,
                    OrmValue::String(area_name.to_string()),
                ),
                WhereCondition::new("customer_id", WhereOp::Eq, OrmValue::I64(*cid)),
            ];
            if let Ok(list) = rentarea_repo.find_by(&cust_ra_conditions) {
                let mut ids: Vec<i64> = list.iter().map(|r| r.pk()).collect();
                ids.sort_unstable();
                let ids_str = if ids.is_empty() {
                    String::new()
                } else {
                    ids.iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                };

                let c_conditions = [WhereCondition::new(
                    "customer_id",
                    WhereOp::Eq,
                    OrmValue::I64(*cid),
                )];
                if let Ok(Some(mut customer)) = customer_repo.find_one_by(&c_conditions) {
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("rentarea_ids".to_string(), json!(ids_str));
                    customer.set_attrs(&data_map);
                    let _ = customer_repo.save(customer);
                }
            }
        }

        Ok(conflicts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sz_orm_core::repository::InMemoryRepository;

    fn make_rentarea(
        id: i64,
        dept_id: i64,
        area_name: &str,
        customer_id: i64,
        is_delete: i64,
        app_id: i64,
    ) -> Rentarea {
        Rentarea::new()
            .with_data("rentarea_id", json!(id))
            .with_data("dept_id", json!(dept_id))
            .with_data("area_name", json!(area_name))
            .with_data("customer_id", json!(customer_id))
            .with_data("is_delete", json!(is_delete))
            .with_data("app_id", json!(app_id))
            .with_data("position", json!("A1"))
            .with_data("rent", json!(1000))
    }

    fn make_customer(
        id: i64,
        dept_id: i64,
        name: &str,
        linkman_name: &str,
        rentarea_ids: &str,
        is_delete: i64,
        app_id: i64,
    ) -> Customer {
        Customer::new()
            .with_data("customer_id", json!(id))
            .with_data("dept_id", json!(dept_id))
            .with_data("customer_name", json!(name))
            .with_data("linkman_name", json!(linkman_name))
            .with_data("rentarea_ids", json!(rentarea_ids))
            .with_data("is_delete", json!(is_delete))
            .with_data("app_id", json!(app_id))
            .with_data("status", json!(1))
    }

    // -------------------- do_sync_rentarea 测试 --------------------

    #[test]
    fn test_do_sync_rentarea_updates_existing() {
        let repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "A1", 100, 0, 10001),
            make_rentarea(2, 34, "A2", 200, 0, 10001),
        ]);

        let incoming = json!([
            {"rentarea_id": 1, "dept_id": 34, "area_name": "A1-updated", "customer_id": 300}
        ]);
        let incoming_map: std::collections::HashMap<String, &Value> = incoming
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| {
                r.get("rentarea_id")
                    .and_then(|v| v.as_i64())
                    .map(|id| (id.to_string(), r))
            })
            .collect();

        let result = SyncController::do_sync_rentarea(&repo, &[34], &incoming_map).unwrap();
        // 1 条更新，1 条标记删除
        assert!(result.contains("已更新:1条"));
        assert!(result.contains("删除:1条"));
    }

    #[test]
    fn test_do_sync_rentarea_marks_unchanged_for_already_deleted() {
        let repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "A1", 100, 1, 10001), // 已删除
        ]);

        let incoming: Vec<Value> = vec![]; // 空 incoming
        let incoming_map: std::collections::HashMap<String, &Value> = incoming
            .iter()
            .filter_map(|r| {
                r.get("rentarea_id")
                    .and_then(|v| v.as_i64())
                    .map(|id| (id.to_string(), r))
            })
            .collect();

        let result = SyncController::do_sync_rentarea(&repo, &[34], &incoming_map).unwrap();
        assert!(result.contains("未更新:1条"));
    }

    // -------------------- do_sync_customer 测试 --------------------

    #[test]
    fn test_do_sync_customer_updates_by_linkman_name() {
        let repo = InMemoryRepository::from_vec(vec![
            make_customer(100, 34, "张三", "张三", "1,2", 0, 10001),
            make_customer(200, 34, "李四", "李四", "3", 0, 10001),
        ]);

        let incoming = json!([
            {"linkman_name": "张三", "dept_id": 34, "phone": "13800000000"}
        ]);
        let incoming_map: std::collections::HashMap<String, &Value> = incoming
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| {
                r.get("linkman_name")
                    .and_then(|v| v.as_str())
                    .map(|s| (s.to_string(), r))
            })
            .collect();

        let result = SyncController::do_sync_customer(&repo, &[34], &incoming_map).unwrap();
        // 1 条更新，1 条标记删除
        assert!(result.contains("已更新:1条"));
    }

    // -------------------- do_add_rentarea 测试 --------------------

    #[test]
    fn test_do_add_rentarea_respects_5_limit() {
        let repo: InMemoryRepository<Rentarea> = InMemoryRepository::new();
        // 7 条数据，但只应添加 5 条（PHP 硬编码上限）
        let data: Vec<Value> = (1..=7)
            .map(|i| json!({"dept_id": 34, "area_name": format!("A{}", i)}))
            .collect();

        let (num, total) = SyncController::do_add_rentarea(&repo, &data, 10001);
        assert_eq!(num, 5);
        assert_eq!(total, 7);
    }

    #[test]
    fn test_do_add_rentarea_unset_rentarea_id() {
        let repo: InMemoryRepository<Rentarea> = InMemoryRepository::new();
        let data = vec![json!({"rentarea_id": 999, "dept_id": 34, "area_name": "A1"})];

        let (num, _) = SyncController::do_add_rentarea(&repo, &data, 10001);
        assert_eq!(num, 1);
        // 验证新记录的 rentarea_id 不等于 999
        let conditions = [WhereCondition::new(
            "area_name",
            WhereOp::Eq,
            OrmValue::String("A1".to_string()),
        )];
        let saved = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_ne!(saved.pk(), 999);
    }

    // -------------------- do_add_customer 测试 --------------------

    #[test]
    fn test_do_add_customer_skips_first_2_php_bug() {
        let repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        // 5 条数据，但 PHP 跳过前 2 条，只添加后 3 条
        let data: Vec<Value> = (1..=5)
            .map(|i| json!({"customer_name": format!("客户{}", i), "dept_id": 34}))
            .collect();

        let (num, total) = SyncController::do_add_customer(&repo, &data, 10001);
        assert_eq!(num, 3); // 跳过前 2 条
        assert_eq!(total, 5);
    }

    #[test]
    fn test_do_add_customer_sets_status_1() {
        let repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        let data = vec![
            json!({"customer_name": "skip1", "dept_id": 34}),
            json!({"customer_name": "skip2", "dept_id": 34}),
            json!({"customer_name": "real", "dept_id": 34, "status": 0}),
        ];

        let (num, _) = SyncController::do_add_customer(&repo, &data, 10001);
        assert_eq!(num, 1);
        // 验证 status 被设置为 1
        let conditions = [WhereCondition::new(
            "customer_name",
            WhereOp::Eq,
            OrmValue::String("real".to_string()),
        )];
        let saved = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(saved.to_json()["status"], 1);
    }

    // -------------------- do_add_contract 测试 --------------------

    #[test]
    fn test_do_add_contract_respects_500_limit() {
        let contract_repo: InMemoryRepository<Contract> = InMemoryRepository::new();
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        // 502 条数据，但只应添加 500 条
        let data: Vec<Value> = (1..=502)
            .map(|i| json!({"dept_id": 34, "customer_name": format!("c{}", i)}))
            .collect();

        let (num, total) =
            SyncController::do_add_contract(&contract_repo, &customer_repo, &data, 10001);
        assert_eq!(num, 500);
        assert_eq!(total, 502);
    }

    #[test]
    fn test_do_add_contract_sets_customer_id_0_when_no_match() {
        let contract_repo: InMemoryRepository<Contract> = InMemoryRepository::new();
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        let data = vec![json!({
            "dept_id": 34,
            "customer_name": "不存在的客户",
            "contract_name": "合同1"
        })];

        let (num, _) =
            SyncController::do_add_contract(&contract_repo, &customer_repo, &data, 10001);
        assert_eq!(num, 1);
        // 验证 customer_id = 0
        // 注：contract_name 被拼接为 customer_name + contract_name
        let saved = contract_repo
            .find_by(&[WhereCondition::new(
                "customer_id",
                WhereOp::Eq,
                OrmValue::I64(0),
            )])
            .unwrap();
        assert_eq!(saved.len(), 1);
    }

    // -------------------- do_sync11 测试 --------------------

    #[test]
    fn test_do_sync11_updates_rentarea_customer_id() {
        let rentarea_repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "A1", 100, 0, 10001),
            make_rentarea(2, 34, "A1", 200, 0, 10001),
            make_rentarea(3, 34, "A1", 0, 0, 10001),
        ]);
        let customer_repo =
            InMemoryRepository::from_vec(vec![make_customer(300, 34, "A1", "A1", "", 0, 10001)]);

        let data = json!({"dept_id": 34, "area_name": "A1", "customer_id": 300});
        let result = SyncController::do_sync11(&rentarea_repo, &customer_repo, &data);
        assert!(result.is_ok());

        // 验证所有 A1 区域的 rentarea.customer_id 都更新为 300
        let conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(34)),
            WhereCondition::new("area_name", WhereOp::Eq, OrmValue::String("A1".to_string())),
        ];
        let list = rentarea_repo.find_by(&conditions).unwrap();
        for ra in list {
            assert_eq!(ra.to_json()["customer_id"], 300);
        }

        // 验证 customer.rentarea_ids 更新为 "1,2,3"
        let cust_conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(300),
        )];
        let customer = customer_repo
            .find_one_by(&cust_conditions)
            .unwrap()
            .unwrap();
        let customer_json = customer.to_json();
        let ids = customer_json["rentarea_ids"].as_str().unwrap();
        let id_vec: Vec<i64> = ids.split(',').filter_map(|s| s.parse().ok()).collect();
        assert!(id_vec.contains(&1));
        assert!(id_vec.contains(&2));
        assert!(id_vec.contains(&3));
    }

    // -------------------- check_rentarea_customer 测试 --------------------

    #[test]
    fn test_check_excludes_special_area_names() {
        let rentarea_repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "空置", 0, 0, 10001),
            make_rentarea(2, 34, "自产自销使用", 0, 0, 10001),
            make_rentarea(3, 34, "自产自销空置", 0, 0, 10001),
            make_rentarea(4, 34, "正常商户", 0, 0, 10001),
        ]);
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();

        let result = SyncController::check_rentarea_customer(
            &rentarea_repo,
            &customer_repo,
            &json!({"dept_id": 34}),
        );
        // 只有 "正常商户" 应被处理
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["rentarea_id"], 4);
    }

    #[test]
    fn test_check_no_matching_customer() {
        let rentarea_repo =
            InMemoryRepository::from_vec(vec![make_rentarea(1, 34, "无客户", 0, 0, 10001)]);
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();

        let result =
            SyncController::check_rentarea_customer(&rentarea_repo, &customer_repo, &json!({}));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["customer_id"], "");
        assert_eq!(result[0]["customer_name"], "");
    }

    #[test]
    fn test_check_single_customer_auto_assign() {
        let rentarea_repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "张三", 0, 0, 10001), // 当前 customer_id=0
        ]);
        let customer_repo = InMemoryRepository::from_vec(vec![make_customer(
            100, 34, "张三", "张三", "1", 0, 10001,
        )]);

        let result =
            SyncController::check_rentarea_customer(&rentarea_repo, &customer_repo, &json!({}));
        // customer_id=0 ≠ 100，应进入 result
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["customer_id"], 100);
    }

    #[test]
    fn test_check_declared_rentarea_takes_priority() {
        let rentarea_repo = InMemoryRepository::from_vec(vec![make_rentarea(
            1, 34, "张三", 200,   // 当前 customer_id=200
            0,     // is_delete=0
            10001, // app_id
        )]);
        let customer_repo = InMemoryRepository::from_vec(vec![
            make_customer(100, 34, "张三", "张三", "1", 0, 10001), // 声明 rentarea_id=1
            make_customer(200, 34, "张三", "张三", "2", 0, 10001),
        ]);

        let result =
            SyncController::check_rentarea_customer(&rentarea_repo, &customer_repo, &json!({}));
        // customer 100 声明了 rid=1，但 current_cid=200 ≠ 100，应进入 result
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["customer_id"], 100);
    }

    // -------------------- do_sync 测试 --------------------

    #[test]
    fn test_do_sync_single_customer_assigns_all_rentareas() {
        let rentarea_repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "张三", 0, 0, 10001),
            make_rentarea(2, 34, "张三", 0, 0, 10001),
            make_rentarea(3, 34, "张三", 0, 0, 10001),
        ]);
        let customer_repo = InMemoryRepository::from_vec(vec![make_customer(
            100, 34, "张三", "张三", "", 0, 10001,
        )]);

        let result = SyncController::do_sync(&rentarea_repo, &customer_repo, 34, "张三");
        assert!(result.is_ok());
        let conflicts = result.unwrap();
        assert!(conflicts.is_empty());

        // 验证所有 rentarea.customer_id = 100
        let conditions = [
            WhereCondition::new("dept_id", WhereOp::Eq, OrmValue::I64(34)),
            WhereCondition::new(
                "area_name",
                WhereOp::Eq,
                OrmValue::String("张三".to_string()),
            ),
        ];
        let list = rentarea_repo.find_by(&conditions).unwrap();
        for ra in list {
            assert_eq!(ra.to_json()["customer_id"], 100);
        }

        // 验证 customer.rentarea_ids = "1,2,3"
        let cust_conditions = [WhereCondition::new(
            "customer_id",
            WhereOp::Eq,
            OrmValue::I64(100),
        )];
        let customer = customer_repo
            .find_one_by(&cust_conditions)
            .unwrap()
            .unwrap();
        let customer_json = customer.to_json();
        let ids = customer_json["rentarea_ids"].as_str().unwrap();
        assert_eq!(ids, "1,2,3");
    }

    #[test]
    fn test_do_sync_no_matching_rentarea_returns_error() {
        let rentarea_repo: InMemoryRepository<Rentarea> = InMemoryRepository::new();
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();

        let result = SyncController::do_sync(&rentarea_repo, &customer_repo, 34, "不存在");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "没有匹配的铺位");
    }

    #[test]
    fn test_do_sync_no_matching_customer_returns_error() {
        let rentarea_repo =
            InMemoryRepository::from_vec(vec![make_rentarea(1, 34, "张三", 0, 0, 10001)]);
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();

        let result = SyncController::do_sync(&rentarea_repo, &customer_repo, 34, "张三");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "没有匹配的客户");
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_sync_add_customer_skips_first_2() {
        // R5: PHP addCustomer 硬编码跳过前 2 条（if($key >= 2)）
        let repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        let data: Vec<Value> = (1..=5)
            .map(|i| json!({"customer_name": format!("c{}", i)}))
            .collect();
        let (num, total) = SyncController::do_add_customer(&repo, &data, 10001);
        assert_eq!(num, 3);
        assert_eq!(total, 5);
    }

    #[test]
    fn test_r5_php_sync_add_contract_limit_500() {
        // R5: PHP addContract 上限 500 条
        let contract_repo: InMemoryRepository<Contract> = InMemoryRepository::new();
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        let data: Vec<Value> = (1..=600)
            .map(|i| json!({"customer_name": format!("c{}", i)}))
            .collect();
        let (num, _) =
            SyncController::do_add_contract(&contract_repo, &customer_repo, &data, 10001);
        assert_eq!(num, 500);
    }

    #[test]
    fn test_r5_php_sync_add_rentarea_limit_5() {
        // R5: PHP addRentarea 上限 5 条
        let repo: InMemoryRepository<Rentarea> = InMemoryRepository::new();
        let data: Vec<Value> = (1..=10)
            .map(|i| json!({"area_name": format!("A{}", i)}))
            .collect();
        let (num, _) = SyncController::do_add_rentarea(&repo, &data, 10001);
        assert_eq!(num, 5);
    }

    #[test]
    fn test_r5_php_sync_check_excludes_special_area_names() {
        // R5: PHP check 排除 '空置'/'自产自销使用'/'自产自销空置'
        let rentarea_repo = InMemoryRepository::from_vec(vec![
            make_rentarea(1, 34, "空置", 0, 0, 10001),
            make_rentarea(2, 34, "正常", 0, 0, 10001),
        ]);
        let customer_repo: InMemoryRepository<Customer> = InMemoryRepository::new();
        let result =
            SyncController::check_rentarea_customer(&rentarea_repo, &customer_repo, &json!({}));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["area_name"], "正常");
    }

    #[test]
    fn test_r5_php_sync_rentarea_format_message() {
        // R5: PHP syncRentarea 返回 "已更新:X条,删除:Y条,未更新:Z条" 格式
        let repo = InMemoryRepository::from_vec(vec![make_rentarea(1, 34, "A1", 0, 0, 10001)]);
        let incoming_map: std::collections::HashMap<String, &Value> =
            std::collections::HashMap::new();
        let result = SyncController::do_sync_rentarea(&repo, &[34], &incoming_map).unwrap();
        assert!(result.contains("已更新:"));
        assert!(result.contains("删除:"));
        assert!(result.contains("未更新:"));
    }
}

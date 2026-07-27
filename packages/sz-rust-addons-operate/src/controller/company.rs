//! Company 控制器 — 对齐 PHP `addons/operate/controller/admin/Company.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `index()` | [`CompanyController::index`] | 分页列表 |
//! | `export()` | [`CompanyController::export`] | 导出列表（不分页） |
//! | `add()` | [`CompanyController::add`] | 添加公司 |
//! | `edit()` | [`CompanyController::edit`] | 编辑公司 |
//! | `del()` | [`CompanyController::del`] | 软删除公司 |
//! | `detail()` | [`CompanyController::detail`] | 公司详情 |
//!
//! ## PHP 源码依据
//!
//! ```php
//! public function index(): Json {
//!     $param = $this->postData();
//!     $model = new CompanyModel();
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
use crate::model::Company;

/// Company 控制器 — 对齐 PHP `Company` 控制器
///
/// 实现 `AddonsBaseController` trait（继承 `BaseController` + `SzController`），
/// 业务方法通过 `&dyn Repository<Company, Key = Value>` 参数注入数据库访问。
pub struct CompanyController;

impl SzController for CompanyController {}
impl BaseController for CompanyController {}
impl AddonsBaseController for CompanyController {}

impl CompanyController {
    /// 分页列表 — 对齐 PHP `index()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function index(): Json {
    ///     $param = $this->postData();
    ///     $model = new CompanyModel();
    ///     $result['list'] = $model->getList($param,'list');
    ///     return $this->renderSuccess('', compact('result'));
    /// }
    /// ```
    #[tracing::instrument(skip(self, req, repo))]
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Company, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "list");
        self.render_success("", json!({"result": result}))
    }

    /// 导出列表 — 对齐 PHP `export()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function export(): Json {
    ///     $param = $this->postData();
    ///     $model = new CompanyModel();
    ///     $result['list'] = $model->getList($param,'export');
    ///     return $this->renderSuccess('', compact('result'));
    /// }
    /// ```
    #[tracing::instrument(skip(self, req, repo))]
    pub async fn export(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Company, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "export");
        self.render_success("", json!({"result": result}))
    }

    /// 添加公司 — 对齐 PHP `add()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add(): Json {
    ///     $param = $this->postData();
    ///     $model = new CompanyModel();
    ///     $data = json_decode($param['formData'], true);
    ///     if($model->add($data)){
    ///         return $this->renderSuccess('添加成功');
    ///     }
    ///     return $this->renderError($model->getError() ?: '添加失败');
    /// }
    /// ```
    #[tracing::instrument(skip(self, req, repo))]
    pub async fn add(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Company, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::add_company(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 编辑公司 — 对齐 PHP `edit()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function edit(): Json {
    ///     $param = $this->postData();
    ///     $model = CompanyModel::detail($param['company_id']);
    ///     $data = json_decode($param['formData'], true);
    ///     if($model->edit($data)){
    ///         return $this->renderSuccess("更新成功");
    ///     }
    ///     return $this->renderError($model->getError() ?:'更新失败');
    /// }
    /// ```
    #[tracing::instrument(skip(self, req, repo))]
    pub async fn edit(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Company, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let company_id = match get_i64_param(&param, "company_id") {
            Some(id) => id,
            None => return self.render_error("company_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::edit_company(repo, company_id, &data) {
            Ok(()) => self.render_success("更新成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 软删除公司 — 对齐 PHP `del()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function del(): Json {
    ///     $param = $this->postData();
    ///     $model = CompanyModel::detail($param['company_id']);
    ///     if(!$model->setDelete()){
    ///         return $this->renderError('删除失败');
    ///     }
    ///     return $this->renderSuccess("删除成功");
    /// }
    /// ```
    #[tracing::instrument(skip(self, req, repo))]
    pub async fn del(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Company, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let company_id = match get_i64_param(&param, "company_id") {
            Some(id) => id,
            None => return self.render_error("company_id 参数缺失", json!({}), 0),
        };

        match Self::set_delete(repo, company_id) {
            Ok(()) => self.render_success("删除成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 公司详情 — 对齐 PHP `detail()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function detail(): Json {
    ///     $param = $this->postData();
    ///     $detail = CompanyModel::detail($param['company_id']);
    ///     if($detail){
    ///         return $this->renderSuccess('', ['detail'=>$detail]);
    ///     }
    ///     return $this->renderError('数据不存在');
    /// }
    /// ```
    #[tracing::instrument(skip(self, req, repo))]
    pub async fn detail(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Company, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let company_id = match get_i64_param(&param, "company_id") {
            Some(id) => id,
            None => return self.render_error("company_id 参数缺失", json!({}), 0),
        };

        match Self::detail_company(repo, company_id) {
            Some(detail) => self.render_success("", json!({"detail": detail})),
            None => self.render_error("数据不存在", json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法（对齐 PHP `Company` 模型业务方法）
    // ========================================================================

    /// 查询公司详情 — 对齐 PHP `Company::detail($company_id)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public static function detail($company_id){
    ///     return self::find($company_id);
    /// }
    /// ```
    fn detail_company(
        repo: &dyn Repository<Company, Key = OrmValue>,
        company_id: i64,
    ) -> Option<Value> {
        let conditions = [WhereCondition::new(
            "company_id",
            WhereOp::Eq,
            OrmValue::I64(company_id),
        )];
        repo.find_one_by(&conditions)
            .ok()
            .flatten()
            .map(|c| c.to_json())
    }

    /// 查询公司列表 — 对齐 PHP `Company::getList($param, $type)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function getList($param,$type){
    ///     $model = $this;
    ///     foreach ($param as $key=>$val){
    ///         if(empty($val))unset($param[$key]);
    ///     }
    ///     $params = array_merge([
    ///         'keyword'=>'', 'is_limit'=>0, 'sortType'=>'all', 'list_rows'=>15
    ///     ], $param);
    ///     if(!empty($params['keyword'])){
    ///         $model = $model->where('company_linkman|company_name|company_address','like', '%'.trim($params['keyword']).'%');
    ///     }
    ///     $sort = [];
    ///     if($params['sortType']==='all') { $sort = ['sort'=>'asc']; }
    ///     $list = $model->where(['is_delete'=>0,'app_id'=>$param['app_id']])->order($sort);
    ///     if ($type == 'export') {
    ///         if($params['is_limit'] > 0){ $list = $list->limit($params['limit'])->select(); }
    ///         else { $list = $list->select(); }
    ///     } else {
    ///         $list = $list->paginate($params);
    ///     }
    ///     return ['list'=>$list];
    /// }
    /// ```
    ///
    /// # 简化说明
    ///
    /// - 事务、缓存、关联关系：NOTE(各功能模块)
    /// - LIKE 搜索：使用简单的 `contains` 匹配（对齐 PHP `LIKE '%keyword%'`）
    /// - 分页：内存分页（对齐 PHP `paginate`）
    fn get_list(
        repo: &dyn Repository<Company, Key = OrmValue>,
        param: &Value,
        list_type: &str,
    ) -> Value {
        let app_id = get_app_id(param);
        let keyword = get_str_param(param, "keyword").unwrap_or_default();
        let sort_type = get_str_param(param, "sortType").unwrap_or_else(|| "all".to_string());
        let list_rows = get_i64_param(param, "list_rows").unwrap_or(15) as usize;
        let page = get_i64_param(param, "page").unwrap_or(1) as usize;
        let is_limit = get_i64_param(param, "is_limit").unwrap_or(0);
        let limit = get_i64_param(param, "limit").unwrap_or(0) as usize;

        // 查询 is_delete=0 AND app_id=$app_id 的记录
        let conditions = [
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
        ];
        let mut items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };

        // keyword 过滤（对齐 PHP `company_linkman|company_name|company_address LIKE '%keyword%'`）
        if !keyword.is_empty() {
            let kw = keyword.trim();
            items.retain(|item| {
                let linkman = item
                    .get("company_linkman")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let name = item
                    .get("company_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let address = item
                    .get("company_address")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                linkman.contains(kw) || name.contains(kw) || address.contains(kw)
            });
        }

        // 排序（对齐 PHP `sortType='all'` 时 `sort asc`）
        if sort_type == "all" {
            items.sort_by_key(|item| item.get("sort").and_then(|v| v.as_i64()).unwrap_or(0));
        }

        // 分页
        let result_list = if list_type == "export" {
            if is_limit > 0 && limit > 0 {
                items.into_iter().take(limit).collect::<Vec<_>>()
            } else {
                items
            }
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

    /// 添加公司 — 对齐 PHP `Company::add($data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add($data): bool {
    ///     $this->startTrans();
    ///     try{
    ///         if($this->save($data)){ $this->commit(); return true; }
    ///         else { $this->error ='操作失败！！！'; return false; }
    ///     }catch(Exception $e){
    ///         $this->error = $e->getMessage();
    ///         $this->rollback();
    ///         return false;
    ///     }
    /// }
    /// ```
    ///
    /// # 简化说明
    ///
    /// - 事务：NOTE(事务模块)（InMemoryRepository 不支持事务）
    /// - `save($data)`：调用 `set_attrs` 批量赋值后 `repo.save`
    fn add_company(
        repo: &dyn Repository<Company, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        let mut model = Company::new();
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

    /// 编辑公司 — 对齐 PHP `Company::edit($data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function edit($data): bool {
    ///     $this->startTrans();
    ///     try{
    ///         if($this->save($data)){ $this->commit(); return true; }
    ///         else { $this->error ='操作失败！！！'; return false; }
    ///     }catch(Exception $e){
    ///         $this->error = $e->getMessage();
    ///         $this->rollback();
    ///         return false;
    ///     }
    /// }
    /// ```
    fn edit_company(
        repo: &dyn Repository<Company, Key = OrmValue>,
        company_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "company_id",
            WhereOp::Eq,
            OrmValue::I64(company_id),
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

    /// 软删除公司 — 对齐 PHP `Company::setDelete()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function setDelete(): bool {
    ///     return $this->save(['is_delete'=>1]);
    /// }
    /// ```
    fn set_delete(
        repo: &dyn Repository<Company, Key = OrmValue>,
        company_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "company_id",
            WhereOp::Eq,
            OrmValue::I64(company_id),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_core::repository::InMemoryRepository;

    /// 创建测试用 Company
    fn make_company(id: i64, name: &str, app_id: i64) -> Company {
        Company::new()
            .with_data("company_id", json!(id))
            .with_data("company_name", json!(name))
            .with_data("company_linkman", json!("联系人"))
            .with_data("company_address", json!("地址"))
            .with_data("sort", json!(100))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    /// 创建预填充的仓储
    fn make_repo() -> InMemoryRepository<Company> {
        InMemoryRepository::from_vec(vec![
            make_company(1, "公司A", 10001),
            make_company(2, "公司B", 10001),
            make_company(3, "公司C", 20002),
        ])
    }

    // -------------------- index 测试 --------------------

    #[tokio::test]
    async fn test_index_returns_paginated_list() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"app_id": 10001, "page": 1, "list_rows": 15}))
                    .unwrap(),
            ))
            .unwrap();

        let resp = ctrl.index(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_index_filters_by_app_id() {
        let repo = make_repo();

        // 直接测试 get_list 业务方法
        let result = CompanyController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let list = result["list"].as_array().unwrap();
        // app_id=10001 有 2 条记录
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_index_filters_by_keyword() {
        let repo = make_repo();
        // keyword="公司A" 应只匹配 company_name="公司A"
        let result = CompanyController::get_list(
            &repo,
            &json!({"app_id": 10001, "keyword": "公司A"}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["company_name"], "公司A");
    }

    #[tokio::test]
    async fn test_index_pagination() {
        let repo = make_repo();
        // list_rows=1, page=1 应只返回 1 条
        let result = CompanyController::get_list(
            &repo,
            &json!({"app_id": 10001, "page": 1, "list_rows": 1}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
    }

    // -------------------- export 测试 --------------------

    #[tokio::test]
    async fn test_export_returns_all_without_pagination() {
        let repo = make_repo();
        let result = CompanyController::get_list(&repo, &json!({"app_id": 10001}), "export");
        let list = result["list"].as_array().unwrap();
        // export 不分页，返回全部
        assert_eq!(list.len(), 2);
    }

    // -------------------- add 测试 --------------------

    #[tokio::test]
    async fn test_add_success() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let form_data = json!({"company_name": "新公司", "company_linkman": "新联系人"});
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"formData": form_data})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.add(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // 验证仓储中新增了一条记录
        assert_eq!(repo.len(), 4);
    }

    #[tokio::test]
    async fn test_add_missing_form_data() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"other": 1})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.add(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // 不应新增记录
        assert_eq!(repo.len(), 3);
    }

    // -------------------- edit 测试 --------------------

    #[tokio::test]
    async fn test_edit_success() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let form_data = json!({"company_name": "更新后的名称"});
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"company_id": 1, "formData": form_data})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.edit(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // 验证记录已更新
        let conditions = [WhereCondition::new(
            "company_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["company_name"], "更新后的名称");
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let form_data = json!({"company_name": "更新"});
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"company_id": 999, "formData": form_data})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.edit(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    // -------------------- del 测试 --------------------

    #[tokio::test]
    async fn test_del_success() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"company_id": 1})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.del(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // 验证 is_delete 已设置为 1
        let conditions = [WhereCondition::new(
            "company_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let deleted = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(deleted.to_json()["is_delete"], 1);
    }

    // -------------------- detail 测试 --------------------

    #[tokio::test]
    async fn test_detail_success() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"company_id": 1})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.detail(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_detail_not_found() {
        let ctrl = CompanyController;
        let repo = make_repo();
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(
                serde_json::to_string(&json!({"company_id": 999})).unwrap(),
            ))
            .unwrap();

        let resp = ctrl.detail(req, &repo).await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_company_get_list_returns_list_key() {
        // R5: PHP getList 返回 ['list' => $list] 格式
        let repo = make_repo();
        let result = CompanyController::get_list(&repo, &json!({"app_id": 10001}), "list");
        assert!(result.is_object());
        assert!(result["list"].is_array());
    }

    #[test]
    fn test_r5_php_company_detail_returns_json_value() {
        // R5: PHP detail 返回模型实例（toArray 后为关联数组）
        let repo = make_repo();
        let detail = CompanyController::detail_company(&repo, 1);
        assert!(detail.is_some());
        let detail = detail.unwrap();
        assert_eq!(detail["company_id"], 1);
        assert_eq!(detail["company_name"], "公司A");
    }

    #[test]
    fn test_r5_php_company_set_delete_sets_is_delete_1() {
        // R5: PHP setDelete 设置 is_delete=1（非物理删除）
        let repo = make_repo();
        CompanyController::set_delete(&repo, 1).unwrap();
        let conditions = [WhereCondition::new(
            "company_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let model = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(model.to_json()["is_delete"], 1);
    }
}

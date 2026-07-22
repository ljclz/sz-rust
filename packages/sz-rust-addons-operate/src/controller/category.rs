//! Category 控制器 — 对齐 PHP `addons/operate/controller/admin/Category.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `index()` | [`CategoryController::index`] | 分页列表 |
//! | `add()` | [`CategoryController::add`] | 添加分类 |
//! | `edit()` | [`CategoryController::edit`] | 编辑分类 |
//! | `del()` | [`CategoryController::del`] | 软删除分类 |
//!
//! ## PHP 源码依据
//!
//! ```php
//! public function index(): Json {
//!     $param = $this->postData();
//!     $model = new CategoryModel();
//!     $result['list'] = $model->getList($param);
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

use crate::controller::common::{get_app_id, get_i64_param, parse_form_data};
use crate::model::Category;

/// Category 控制器 — 对齐 PHP `Category` 控制器
pub struct CategoryController;

impl SzController for CategoryController {}
impl BaseController for CategoryController {}
impl AddonsBaseController for CategoryController {}

impl CategoryController {
    /// 分页列表 — 对齐 PHP `index()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function index(): Json {
    ///     $param = $this->postData();
    ///     $model = new CategoryModel();
    ///     $result['list'] = $model->getList($param);
    ///     return $this->renderSuccess('', compact('result'));
    /// }
    /// ```
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Category, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param);
        self.render_success("", json!({"result": result}))
    }

    /// 添加分类 — 对齐 PHP `add()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add(): Json {
    ///     $param = $this->postData();
    ///     $model = new CategoryModel();
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
        repo: &dyn Repository<Category, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::add_category(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 编辑分类 — 对齐 PHP `edit()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function edit(): Json {
    ///     $param = $this->postData();
    ///     $model = CategoryModel::detail($param['cat_id']);
    ///     $data = json_decode($param['formData'], true);
    ///     if($model->edit($data)){
    ///         return $this->renderSuccess("更新成功");
    ///     }
    ///     return $this->renderError($model->getError() ?:'更新失败');
    /// }
    /// ```
    pub async fn edit(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Category, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let cat_id = match get_i64_param(&param, "cat_id") {
            Some(id) => id,
            None => return self.render_error("cat_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::edit_category(repo, cat_id, &data) {
            Ok(()) => self.render_success("更新成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 软删除分类 — 对齐 PHP `del()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function del(): Json {
    ///     $param = $this->postData();
    ///     $model = CategoryModel::detail($param['cat_id']);
    ///     if(!$model->setDelete()){
    ///         return $this->renderError('删除失败');
    ///     }
    ///     return $this->renderSuccess("删除成功");
    /// }
    /// ```
    pub async fn del(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Category, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let cat_id = match get_i64_param(&param, "cat_id") {
            Some(id) => id,
            None => return self.render_error("cat_id 参数缺失", json!({}), 0),
        };

        match Self::set_delete(repo, cat_id) {
            Ok(()) => self.render_success("删除成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法（对齐 PHP `Category` 模型业务方法）
    // ========================================================================

    /// 查询分类列表 — 对齐 PHP `Category::getList($param)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function getList($param) {
    ///     return $this->where(['is_delete'=>0,'app_id'=>$param['app_id']])
    ///         ->order(['create_time' => 'desc'])
    ///         ->paginate($param, false, ['query' => request()->request()]);
    /// }
    /// ```
    fn get_list(repo: &dyn Repository<Category, Key = OrmValue>, param: &Value) -> Value {
        let app_id = get_app_id(param);
        let list_rows = get_i64_param(param, "list_rows").unwrap_or(15) as usize;
        let page = get_i64_param(param, "page").unwrap_or(1) as usize;

        let conditions = [
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
        ];
        let mut items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|c| c.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };

        // PHP 按 create_time desc 排序（简化：按 cat_id desc）
        items.sort_by(|a, b| {
            let a_id = a.get("cat_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("cat_id").and_then(|v| v.as_i64()).unwrap_or(0);
            b_id.cmp(&a_id)
        });

        // 分页
        let start = page.saturating_sub(1) * list_rows;
        let result_list = if start >= items.len() {
            Vec::new()
        } else {
            let end = (start + list_rows).min(items.len());
            items[start..end].to_vec()
        };

        json!({"list": result_list})
    }

    /// 添加分类 — 对齐 PHP `Category::add($data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add($data): bool {
    ///     if(empty($data['cat_name'])){ $this->error = '请输入名称'; return false; }
    ///     // ... 事务 + save
    /// }
    /// ```
    fn add_category(
        repo: &dyn Repository<Category, Key = OrmValue>,
        data: &Value,
    ) -> Result<(), String> {
        // PHP 校验：cat_name 不能为空
        let cat_name = data.get("cat_name").and_then(|v| v.as_str()).unwrap_or("");
        if cat_name.is_empty() {
            return Err("请输入名称".to_string());
        }

        let mut model = Category::new();
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

    /// 编辑分类 — 对齐 PHP `Category::edit($data)`
    fn edit_category(
        repo: &dyn Repository<Category, Key = OrmValue>,
        cat_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "cat_id",
            WhereOp::Eq,
            OrmValue::I64(cat_id),
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

    /// 软删除分类 — 对齐 PHP `Category::setDelete()`
    fn set_delete(
        repo: &dyn Repository<Category, Key = OrmValue>,
        cat_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "cat_id",
            WhereOp::Eq,
            OrmValue::I64(cat_id),
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

    fn make_category(id: i64, name: &str, app_id: i64) -> Category {
        Category::new()
            .with_data("cat_id", json!(id))
            .with_data("cat_name", json!(name))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    fn make_repo() -> InMemoryRepository<Category> {
        InMemoryRepository::from_vec(vec![
            make_category(1, "餐饮", 10001),
            make_category(2, "零售", 10001),
            make_category(3, "测试", 20002),
        ])
    }

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = CategoryController::get_list(&repo, &json!({"app_id": 10001}));
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_add_category_validates_name() {
        let repo = make_repo();
        let result = CategoryController::add_category(&repo, &json!({"cat_name": ""}));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "请输入名称");
    }

    #[test]
    fn test_add_category_success() {
        let repo = make_repo();
        let result = CategoryController::add_category(&repo, &json!({"cat_name": "新分类"}));
        assert!(result.is_ok());
        assert_eq!(repo.len(), 4);
    }

    #[test]
    fn test_edit_category_success() {
        let repo = make_repo();
        let result = CategoryController::edit_category(&repo, 1, &json!({"cat_name": "更新名称"}));
        assert!(result.is_ok());
        let conditions = [WhereCondition::new("cat_id", WhereOp::Eq, OrmValue::I64(1))];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["cat_name"], "更新名称");
    }

    #[test]
    fn test_edit_category_not_found() {
        let repo = make_repo();
        let result = CategoryController::edit_category(&repo, 999, &json!({"cat_name": "x"}));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_delete_success() {
        let repo = make_repo();
        let result = CategoryController::set_delete(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new("cat_id", WhereOp::Eq, OrmValue::I64(1))];
        let deleted = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(deleted.to_json()["is_delete"], 1);
    }

    #[test]
    fn test_r5_php_category_get_list_returns_list_key() {
        let repo = make_repo();
        let result = CategoryController::get_list(&repo, &json!({"app_id": 10001}));
        assert!(result["list"].is_array());
    }
}

//! Level 控制器 — 对齐 PHP `addons/operate/controller/admin/Level.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `index()` | [`LevelController::index`] | 分页列表 |
//! | `add()` | [`LevelController::add`] | 添加等级 |
//! | `edit()` | [`LevelController::edit`] | 编辑等级 |
//! | `delete()` | [`LevelController::delete`] | 软删除等级 |
//!
//! ## 注意
//!
//! PHP Level 控制器的方法名为 `delete`（不是 `del`），与 Company/Category 的 `del` 不同。
//! Rust 端严格对齐 PHP 方法名。

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use serde_json::{json, Value};
use sz_rust_core::controller::{AddonsBaseController, BaseController, SzController};
use sz_rust_core::model::Mutator as _;
use sz_rust_core::orm::repository::{Repository, WhereCondition, WhereOp};
use sz_rust_core::orm::ModelExt as _;
use sz_rust_core::orm::Value as OrmValue;

use crate::controller::common::{get_app_id, get_i64_param, parse_form_data};
use crate::model::Level;

/// Level 控制器 — 对齐 PHP `Level` 控制器
pub struct LevelController;

impl SzController for LevelController {}
impl BaseController for LevelController {}
impl AddonsBaseController for LevelController {}

impl LevelController {
    /// 分页列表 — 对齐 PHP `index()`
    #[tracing::instrument(skip_all)]
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Level, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param);
        self.render_success("", json!({"result": result}))
    }

    /// 添加等级 — 对齐 PHP `add()`
    #[tracing::instrument(skip_all)]
    pub async fn add(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Level, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::add_level(repo, &data) {
            Ok(()) => self.render_success("添加成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 编辑等级 — 对齐 PHP `edit()`
    #[tracing::instrument(skip_all)]
    pub async fn edit(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Level, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let level_id = match get_i64_param(&param, "level_id") {
            Some(id) => id,
            None => return self.render_error("level_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match Self::edit_level(repo, level_id, &data) {
            Ok(()) => self.render_success("更新成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    /// 软删除等级 — 对齐 PHP `delete()`
    ///
    /// **注意**：PHP Level 控制器方法名为 `delete`（不是 `del`）。
    #[tracing::instrument(skip_all)]
    pub async fn delete(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<Level, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let level_id = match get_i64_param(&param, "level_id") {
            Some(id) => id,
            None => return self.render_error("level_id 参数缺失", json!({}), 0),
        };

        match Self::set_delete(repo, level_id) {
            Ok(()) => self.render_success("删除成功", json!({})),
            Err(e) => self.render_error(&e, json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法（对齐 PHP `Level` 模型业务方法）
    // ========================================================================

    /// 查询等级列表 — 对齐 PHP `Level::getList($param)`
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
    fn get_list(repo: &dyn Repository<Level, Key = OrmValue>, param: &Value) -> Value {
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

        // PHP 按 create_time desc 排序（简化：按 level_id desc）
        items.sort_by(|a, b| {
            let a_id = a.get("level_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("level_id").and_then(|v| v.as_i64()).unwrap_or(0);
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

    /// 添加等级 — 对齐 PHP `Level::add($data)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function add($data): bool {
    ///     if(empty($data['level_name'])){ $this->error = '请输入名称'; return false; }
    ///     // ... 事务 + save
    /// }
    /// ```
    fn add_level(repo: &dyn Repository<Level, Key = OrmValue>, data: &Value) -> Result<(), String> {
        // PHP 校验：level_name 不能为空
        let level_name = data
            .get("level_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if level_name.is_empty() {
            return Err("请输入名称".to_string());
        }

        let mut model = Level::new();
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

    /// 编辑等级 — 对齐 PHP `Level::edit($data)`
    fn edit_level(
        repo: &dyn Repository<Level, Key = OrmValue>,
        level_id: i64,
        data: &Value,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "level_id",
            WhereOp::Eq,
            OrmValue::I64(level_id),
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

    /// 软删除等级 — 对齐 PHP `Level::setDelete()`
    fn set_delete(
        repo: &dyn Repository<Level, Key = OrmValue>,
        level_id: i64,
    ) -> Result<(), String> {
        let conditions = [WhereCondition::new(
            "level_id",
            WhereOp::Eq,
            OrmValue::I64(level_id),
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
    use sz_rust_core::orm::repository::InMemoryRepository;

    fn make_level(id: i64, name: &str, app_id: i64) -> Level {
        Level::new()
            .with_data("level_id", json!(id))
            .with_data("level_name", json!(name))
            .with_data("level_sort", json!(10))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
    }

    fn make_repo() -> InMemoryRepository<Level> {
        InMemoryRepository::from_vec(vec![
            make_level(1, "VIP", 10001),
            make_level(2, "普通", 10001),
            make_level(3, "测试", 20002),
        ])
    }

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = LevelController::get_list(&repo, &json!({"app_id": 10001}));
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_add_level_validates_name() {
        let repo = make_repo();
        // 空 level_name 应返回错误
        let result = LevelController::add_level(&repo, &json!({"level_name": ""}));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "请输入名称");
    }

    #[test]
    fn test_add_level_success() {
        let repo = make_repo();
        let result =
            LevelController::add_level(&repo, &json!({"level_name": "新等级", "level_sort": 5}));
        assert!(result.is_ok());
        assert_eq!(repo.len(), 4);
    }

    #[test]
    fn test_edit_level_success() {
        let repo = make_repo();
        let result = LevelController::edit_level(&repo, 1, &json!({"level_name": "更新名称"}));
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "level_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["level_name"], "更新名称");
    }

    #[test]
    fn test_edit_level_not_found() {
        let repo = make_repo();
        let result = LevelController::edit_level(&repo, 999, &json!({"level_name": "x"}));
        assert!(result.is_err());
    }

    #[test]
    fn test_set_delete_success() {
        let repo = make_repo();
        let result = LevelController::set_delete(&repo, 1);
        assert!(result.is_ok());
        let conditions = [WhereCondition::new(
            "level_id",
            WhereOp::Eq,
            OrmValue::I64(1),
        )];
        let deleted = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(deleted.to_json()["is_delete"], 1);
    }

    #[test]
    fn test_r5_php_level_get_list_returns_list_key() {
        let repo = make_repo();
        let result = LevelController::get_list(&repo, &json!({"app_id": 10001}));
        assert!(result["list"].is_array());
    }
}
